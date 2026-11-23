use std::{
    collections::{HashMap, HashSet},
    fmt::Write as _,
    net::{Ipv4Addr, SocketAddr},
    path::PathBuf,
    time::Duration,
};

use axum::http::HeaderValue;
use ipnet::IpNet;
use reqwest::{Certificate, Client, Identity, redirect::Policy, tls::Version};
use ring::digest;
use serde::Deserialize;
use serde_json::Value;
use subtle::ConstantTimeEq;

use crate::{
    Problem,
    http::{same_auth, token_header},
    validation::valid_id,
};

#[derive(Default)]
pub(crate) struct Enrolled {
    machines: HashMap<String, Machine>,
    publishers: HashMap<String, Publisher>,
    verify: Option<PresenceVerifier>,
}

#[derive(Deserialize)]
struct Machine {
    token_sha256: String,
    allowed_networks: Vec<IpNet>,
    tls_name: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RuntimeConfig {
    token: String,
    binding: Value,
}

struct Publisher {
    auth: HeaderValue,
    binding: Value,
}

#[derive(Clone, Deserialize)]
pub(crate) struct PresenceVerifier {
    ca_file: PathBuf,
    cert_file: PathBuf,
    key_file: PathBuf,
}

fn sha256(token: &str) -> String {
    let digest = digest::digest(&digest::SHA256, token.as_bytes());
    let mut hex = String::with_capacity(64);
    for byte in digest.as_ref() {
        write!(hex, "{byte:02x}").expect("write to String");
    }
    hex
}

fn binding_valid(value: &Value) -> bool {
    matches!(value["kind"].as_str(), Some("native" | "external"))
        && value["provider"]
            .as_str()
            .is_some_and(|name| valid_id(name) && name.len() <= 32)
        && value["instance"].as_str().is_some_and(|name| {
            (1..=192).contains(&name.len()) && name.bytes().all(|byte| matches!(byte, b' '..=b'~'))
        })
}

impl Enrolled {
    pub(crate) fn new(
        publishers: &Value,
        machines: &Value,
        reader: &str,
        writer: &str,
        verify: Option<PresenceVerifier>,
    ) -> Result<Self, Problem> {
        let machine_config: HashMap<String, Machine> = serde_json::from_value(machines.clone())
            .map_err(|_| Problem::new(500, "Invalid machine enrollments"))?;
        let publisher_config: HashMap<String, RuntimeConfig> =
            serde_json::from_value(publishers.clone())
                .map_err(|_| Problem::new(500, "Invalid runtime publisher enrollments"))?;
        let mut tokens = HashSet::from([reader.to_owned(), writer.to_owned()]);
        let mut configured = HashMap::new();
        for (id, publisher) in publisher_config {
            let token = publisher.token;
            let allowed = valid_id(&id)
                && binding_valid(&publisher.binding)
                && (32..=128).contains(&token.len())
                && token
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
                && tokens.insert(token.clone())
                && !machine_config
                    .values()
                    .any(|machine| machine.token_sha256 == sha256(&token));
            if !allowed {
                return Err(Problem::new(
                    500,
                    "Each runtime publisher needs an exact binding and unique token",
                ));
            }
            configured.insert(
                id,
                Publisher {
                    auth: token_header(&token)?,
                    binding: publisher.binding,
                },
            );
        }
        Ok(Self {
            machines: machine_config,
            publishers: configured,
            verify,
        })
    }

    pub(crate) fn runtime(&self, id: &str, provided: Option<&HeaderValue>) -> Option<Value> {
        let entry = self.publishers.get(id)?;
        same_auth(provided, &entry.auth).then(|| entry.binding.clone())
    }

    pub(crate) async fn announcement(
        &self,
        catalog: crate::Catalog,
        id: &str,
        provided: Option<&HeaderValue>,
        report: impl Future<Output = Result<Value, Problem>>,
    ) -> Result<(Value, bool), Problem> {
        let enrollment = self
            .machines
            .get(id)
            .filter(|enrollment| {
                provided
                    .and_then(|auth| auth.to_str().ok())
                    .and_then(|auth| auth.strip_prefix("Bearer "))
                    .filter(|token| !token.is_empty())
                    .is_some_and(|token| {
                        bool::from(
                            sha256(token)
                                .as_bytes()
                                .ct_eq(enrollment.token_sha256.as_bytes()),
                        )
                    })
            })
            .ok_or_else(|| Problem::new(401, "Machine credential required"))?;
        let record = id.to_owned();
        let before = crate::http::blocking({
            let catalog = catalog.clone();
            let id = record.clone();
            move || catalog.get(&id)
        })
        .await?;
        if before["spec"]["lifecycle"] != "active"
            || !before["spec"]["capabilities"]
                .as_array()
                .is_some_and(|caps| caps.iter().any(|cap| cap == "machine"))
        {
            return Err(Problem::new(403, "Active enrolled machine required"));
        }
        let report = report.await?;
        let (address, interface) = enrollment.address(&report)?;
        let address_text = address.to_string();
        let current = crate::http::blocking({
            let catalog = catalog.clone();
            let id = record.clone();
            move || catalog.last_enrolled_address(&id)
        })
        .await?;
        if current.as_deref() != Some(address_text.as_str()) {
            let verifier = self.verify.as_ref().ok_or_else(|| {
                Problem::new(409, "Collector identity not verified at announced address")
            })?;
            verifier.verify(address, &enrollment.tls_name).await?;
        }
        let interface = interface.to_owned();
        crate::http::blocking(move || catalog.announce_verified(&record, &address_text, &interface))
            .await
    }
}

impl Machine {
    fn address<'a>(&self, value: &'a Value) -> Result<(Ipv4Addr, &'a str), Problem> {
        let request_error = || Problem::new(400, "Invalid enrolled address announcement");
        if !value.as_object().is_some_and(|keys| keys.len() == 2) {
            return Err(request_error());
        }
        let ip = value["address"]
            .as_str()
            .and_then(|value| value.parse::<Ipv4Addr>().ok())
            .filter(|ip| {
                !ip.is_loopback()
                    && !ip.is_link_local()
                    && !ip.is_multicast()
                    && !ip.is_unspecified()
                    && self
                        .allowed_networks
                        .iter()
                        .any(|net| net.contains(&std::net::IpAddr::V4(*ip)))
            })
            .ok_or_else(request_error)?;
        let interface = value["interface"]
            .as_str()
            .filter(|name| {
                (1..=64).contains(&name.chars().count())
                    && name
                        .chars()
                        .all(|ch| ch.is_alphanumeric() || matches!(ch, '-' | '_' | '.'))
            })
            .ok_or_else(request_error)?;
        Ok((ip, interface))
    }
}

impl PresenceVerifier {
    async fn verify(&self, address: Ipv4Addr, name: &str) -> Result<(), Problem> {
        let invalid = || Problem::new(409, "Collector identity not verified at announced address");
        let checked = async {
            if name.is_empty()
                || name.parse::<std::net::IpAddr>().is_ok()
                || !name
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'.'))
            {
                return Err(invalid());
            }
            let _ = rustls::crypto::ring::default_provider().install_default();
            let (ca, mut cert, key) = tokio::try_join!(
                tokio::fs::read(&self.ca_file),
                tokio::fs::read(&self.cert_file),
                tokio::fs::read(&self.key_file),
            )
            .map_err(|_| invalid())?;
            let roots = Certificate::from_pem_bundle(&ca).map_err(|_| invalid())?;
            if roots.is_empty() {
                return Err(invalid());
            }
            cert.push(b'\n');
            cert.extend_from_slice(&key);
            let credential = Identity::from_pem(&cert).map_err(|_| invalid())?;
            let client = Client::builder()
                .tls_backend_rustls()
                .tls_version_min(Version::TLS_1_3)
                .no_proxy()
                .redirect(Policy::none())
                .tls_certs_only(roots)
                .identity(credential)
                .resolve(name, SocketAddr::from((address, 9911)))
                .timeout(Duration::from_secs(5))
                .build()
                .map_err(|_| invalid())?;
            let response = client
                .get(format!("https://{name}:9911/stream"))
                .send()
                .await
                .map_err(|_| invalid())?;
            if response.status() != reqwest::StatusCode::OK {
                return Err(invalid());
            }
            Ok(())
        };
        tokio::time::timeout(Duration::from_secs(6), checked)
            .await
            .map_err(|_| invalid())?
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn enrollment_requires_distinct_scoped_credentials_and_a_bound_identity() {
        let token = "runtime_test_fixture_token_0123456";
        let values = json!({"service-test":{"token":token,"binding":{"kind":"native","provider":"test","instance":"test-service"}}});
        assert!(Enrolled::new(&values, &json!({}), "reader", "writer", None).is_ok());
        assert!(Enrolled::new(&values, &json!({}), "reader", token, None).is_err());
        let shared_hash = sha256(token);
        let machines = json!({"machine-test":{"token_sha256":shared_hash,"allowed_networks":["192.0.2.0/24"],"tls_name":"machine-test"}});
        assert!(Enrolled::new(&values, &machines, "reader", "writer", None).is_err());
    }

    #[test]
    fn enrolled_addresses_stay_within_their_authorized_ipv4_network() {
        let rule = serde_json::from_value::<Machine>(json!({"token_sha256":sha256("fixture"),
            "allowed_networks":["192.0.2.0/24","127.0.0.0/8","169.254.0.0/16"],"tls_name":"machine-test"})).unwrap();
        assert!(
            rule.address(&json!({"address":"192.0.2.90","interface":"lab-eth0"}))
                .is_ok()
        );
        for bad in ["127.0.0.1", "169.254.1.1", "198.51.100.1", "::1"] {
            assert!(
                rule.address(&json!({"address":bad,"interface":"lab-eth0"}))
                    .is_err()
            );
        }
    }
}
