use std::{
    error::Error,
    io::{Cursor, Read},
    net::IpAddr,
    path::{Path, PathBuf},
    process::Command,
    sync::Arc,
    time::Duration,
};

use curl::easy::{Auth, Easy, List};
use reqwest::{
    blocking::{Client, Response},
    header,
    redirect::Policy,
};
use rustls::{
    CertificateError, DigitallySignedStruct, Error as TlsError, SignatureScheme,
    client::danger::{HandshakeSignatureValid, ServerCertVerified, ServerCertVerifier},
    crypto::{WebPkiSupportedAlgorithms, verify_tls12_signature, verify_tls13_signature},
    pki_types::{CertificateDer, ServerName, UnixTime, pem::PemObject},
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use subtle::ConstantTimeEq;
use url::Url;
use zip::ZipArchive;

use super::{child, error, source_path, string};

const ARCHIVE_LIMIT: u64 = 20 * 1024 * 1024;

#[derive(Deserialize, Serialize)]
pub(super) struct Details {
    pub id: String,
    #[serde(rename = "type")]
    kind: String,
    #[serde(default)]
    pub url: String,
    ca_file: Option<PathBuf>,
    mac: Option<String>,
    pub via_machine: Option<String>,
}

#[derive(Debug)]
struct AppliancePin {
    certificate: Vec<u8>,
    signatures: WebPkiSupportedAlgorithms,
}

impl ServerCertVerifier for AppliancePin {
    fn verify_server_cert(
        &self,
        end: &CertificateDer<'_>,
        _: &[CertificateDer<'_>],
        _: &ServerName<'_>,
        _: &[u8],
        _: UnixTime,
    ) -> Result<ServerCertVerified, TlsError> {
        if bool::from(self.certificate.as_slice().ct_eq(end.as_ref())) {
            Ok(ServerCertVerified::assertion())
        } else {
            Err(TlsError::InvalidCertificate(
                CertificateError::ApplicationVerificationFailure,
            ))
        }
    }
    fn verify_tls12_signature(
        &self,
        data: &[u8],
        certificate: &CertificateDer<'_>,
        signature: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, TlsError> {
        verify_tls12_signature(data, certificate, signature, &self.signatures)
    }
    fn verify_tls13_signature(
        &self,
        data: &[u8],
        certificate: &CertificateDer<'_>,
        signature: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, TlsError> {
        verify_tls13_signature(data, certificate, signature, &self.signatures)
    }
    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
        self.signatures.supported_schemes()
    }
}

fn client(certificate: Option<&Path>) -> Result<Client, Box<dyn Error>> {
    let builder = Client::builder()
        .no_proxy()
        .redirect(Policy::none())
        .timeout(Duration::from_secs(15));
    let Some(certificate) = certificate else {
        return Ok(builder.tls_backend_native().build()?);
    };
    let expected = CertificateDer::from_pem_file(certificate)?;
    let crypto = Arc::new(rustls::crypto::ring::default_provider());
    let pin = AppliancePin {
        certificate: expected.as_ref().to_vec(),
        signatures: crypto.signature_verification_algorithms,
    };
    let tls = rustls::ClientConfig::builder_with_provider(crypto)
        .with_safe_default_protocol_versions()?
        .dangerous()
        .with_custom_certificate_verifier(Arc::new(pin))
        .with_no_client_auth();
    Ok(builder.tls_backend_preconfigured(tls).build()?)
}

fn reported_network(data: &Value) -> Result<Vec<Value>, Box<dyn Error>> {
    if data["ok"] != true || data["result"]["success"] != true {
        return Err(error("GLKVM rejected network read.").into());
    }
    let details = &data["result"]["config"];
    let mode = details["is_dhcp"]
        .as_bool()
        .ok_or_else(|| error("Missing assignment mode."))?;
    Ok(vec![json!({"address":string(details,"ip_address")?,
        "mac":string(details,"mac_address")?.to_ascii_lowercase(),
        "assignment":if mode {"dhcp"} else {"static"},"source":"Authenticated GLKVM network settings"})])
}

fn bounded_body(response: Response, limit: u64) -> Result<Vec<u8>, Box<dyn Error>> {
    let response = response.error_for_status()?;
    let mut body = Vec::new();
    response.take(limit + 1).read_to_end(&mut body)?;
    if body.len() as u64 > limit {
        return Err(error("Appliance response exceeded its read limit.").into());
    }
    Ok(body)
}

fn login_cookies(response: Response) -> Result<String, Box<dyn Error>> {
    let response = response.error_for_status()?;
    let cookie = response
        .headers()
        .get_all(header::SET_COOKIE)
        .iter()
        .filter_map(|value| value.to_str().ok())
        .filter_map(|value| value.split(';').next())
        .collect::<Vec<_>>()
        .join("; ");
    bounded_body(response, 64 * 1024)?;
    Ok(cookie)
}

fn diagnostics(response: Response, mac: &str, host: IpAddr) -> Result<Vec<Value>, Box<dyn Error>> {
    let bytes = bounded_body(response, ARCHIVE_LIMIT)?;
    let mut zip = ZipArchive::new(Cursor::new(bytes))?;
    let mut file = |name: &str, limit: u64| -> Result<Vec<u8>, Box<dyn Error>> {
        let mut data = Vec::new();
        let entry = zip.by_name(name)?;
        if entry.size() > limit {
            return Err(error("Appliance archive member too large.").into());
        }
        entry.take(limit + 1).read_to_end(&mut data)?;
        if data.len() as u64 > limit {
            return Err(error("Appliance archive member too large.").into());
        }
        Ok(data)
    };
    let config: Value = serde_json::from_slice(&file("config.json", 1024 * 1024)?)?;
    let report = file("system-diagnostics.txt", ARCHIVE_LIMIT)?;
    let mac = mac.to_ascii_lowercase();
    if mac.is_empty()
        || !String::from_utf8_lossy(&report)
            .to_ascii_lowercase()
            .contains(&mac)
    {
        return Err(
            error("JetKVM diagnostics could not verify the expected interface MAC.").into(),
        );
    }
    let method = config["network_config"]["ipv4_mode"]
        .as_str()
        .filter(|mode| matches!(*mode, "dhcp" | "static"))
        .ok_or_else(|| error("Unknown JetKVM assignment mode."))?;
    Ok(vec![json!({"address":host,"mac":mac,"assignment":method,
        "source":"Authenticated JetKVM diagnostics and saved network configuration"})])
}

fn is_lan(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(ip) => ip.is_private() || ip.is_link_local() || ip.is_loopback(),
        IpAddr::V6(ip) => ip.is_unique_local() || ip.is_unicast_link_local() || ip.is_loopback(),
    }
}

fn base(details: &Details) -> Result<(String, IpAddr), Box<dyn Error>> {
    let url = Url::parse(&details.url)?;
    let host = match url.host() {
        Some(url::Host::Ipv4(ip)) => IpAddr::V4(ip),
        Some(url::Host::Ipv6(ip)) => IpAddr::V6(ip),
        _ => return Err(error("Require an appliance IP address.").into()),
    };
    if !is_lan(host)
        || !matches!(url.scheme(), "http" | "https")
        || !url.username().is_empty()
        || url.password().is_some()
        || url.query().is_some()
        || !matches!(url.path(), "" | "/")
    {
        return Err(error("Appliance endpoint must be local and credential-free.").into());
    }
    Ok((url.origin().ascii_serialization(), host))
}

const ENUMERATION: &str = "http://schemas.xmlsoap.org/ws/2004/09/enumeration";
const RESOURCE: &str = "http://intel.com/wbem/wscim/1/amt-schema/1/AMT_EthernetPortSettings";

fn xml_text(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

fn amt(base: &str, credentials: &Value) -> Result<Vec<Value>, Box<dyn Error>> {
    let username = credentials["username"].as_str().unwrap_or("admin");
    let password = string(credentials, "password")?;
    let url = format!("{base}/wsman");
    let call = |action: &str, body: &str| -> Result<String, Box<dyn Error>> {
        let id = uuid::Uuid::new_v4();
        let xml = format!(
            r#"<s:Envelope xmlns:s="http://www.w3.org/2003/05/soap-envelope" xmlns:a="http://schemas.xmlsoap.org/ws/2004/08/addressing" xmlns:w="http://schemas.dmtf.org/wbem/wsman/1/wsman.xsd" xmlns:e="{ENUMERATION}"><s:Header><a:To>http://127.0.0.1:16992/wsman</a:To><a:ReplyTo><a:Address>http://schemas.xmlsoap.org/ws/2004/08/addressing/role/anonymous</a:Address></a:ReplyTo><a:Action s:mustUnderstand="true">{ENUMERATION}/{action}</a:Action><w:ResourceURI s:mustUnderstand="true">{RESOURCE}</w:ResourceURI><a:MessageID>uuid:{id}</a:MessageID><w:OperationTimeout>PT60S</w:OperationTimeout></s:Header><s:Body>{body}</s:Body></s:Envelope>"#
        );
        let mut client = Easy::new();
        client.url(&url)?;
        client.noproxy("*")?;
        client.follow_location(false)?;
        client.timeout(Duration::from_secs(20))?;
        client.max_filesize(1024 * 1024)?;
        client.username(username)?;
        client.password(password)?;
        client.http_auth(Auth::new().digest(true))?;
        let mut headers = List::new();
        headers.append("Content-Type: application/soap+xml;charset=UTF-8")?;
        client.http_headers(headers)?;
        client.post_fields_copy(xml.as_bytes())?;
        let mut reply = Vec::new();
        {
            let mut transfer = client.transfer();
            transfer.write_function(|chunk| {
                if reply.len() + chunk.len() > 1024 * 1024 {
                    return Ok(0);
                }
                reply.extend_from_slice(chunk);
                Ok(chunk.len())
            })?;
            transfer.perform()?;
        }
        if client.response_code()? != 200 {
            return Err(error("Authenticated AMT read failed.").into());
        }
        Ok(String::from_utf8(reply)?)
    };
    let mut reply = call(
        "Enumerate",
        "<e:Enumerate><w:OptimizeEnumeration/><w:MaxElements>20</w:MaxElements></e:Enumerate>",
    )?;
    let mut rows = Vec::new();
    for _ in 0..5 {
        let document = roxmltree::Document::parse(&reply)?;
        let nodes = || document.descendants().filter(|node| node.is_element());
        if nodes().any(|node| node.tag_name().name() == "Fault") {
            return Err(error("AMT SOAP fault.").into());
        }
        for item in nodes().filter(|node| node.tag_name().name() == "AMT_EthernetPortSettings") {
            let field = |name: &str| {
                item.children()
                    .find(|node| node.is_element() && node.tag_name().name() == name)
                    .and_then(|node| node.text())
            };
            let mac = field("MACAddress").ok_or_else(|| error("Missing AMT interface MAC."))?;
            if mac == "00-00-00-00-00-00" {
                continue;
            }
            let assignment = match field("DHCPEnabled") {
                Some("true") => "dhcp",
                Some("false") => "static",
                _ => return Err(error("Unknown AMT assignment mode.").into()),
            };
            let ip = field("IPAddress").ok_or_else(|| error("Missing AMT address evidence."))?;
            rows.push(
                json!({"address":ip,"mac":mac.replace('-',":").to_ascii_lowercase(),
                "assignment":assignment,"source":"Authenticated AMT EthernetPortSettings"}),
            );
        }
        let context = nodes().find(|node| node.tag_name().name() == "EnumerationContext");
        if nodes().any(|node| node.tag_name().name() == "EndOfSequence") || context.is_none() {
            if rows.is_empty() {
                return Err(error("No AMT interfaces returned.").into());
            }
            return Ok(rows);
        }
        let context = xml_text(context.and_then(|node| node.text()).unwrap_or_default());
        reply = call(
            "Pull",
            &format!(
                "<e:Pull><e:EnumerationContext>{context}</e:EnumerationContext><e:MaxElements>20</e:MaxElements></e:Pull>"
            ),
        )?;
    }
    Err(error("AMT enumeration did not finish.").into())
}

fn collect(
    details: &Details,
    private: &Path,
    credentials: &Value,
) -> Result<Vec<Value>, Box<dyn Error>> {
    let (base, host) = base(details)?;
    let mut rows = if details.kind == "amt" {
        amt(&base, credentials)?
    } else {
        let certificate = details
            .ca_file
            .as_deref()
            .map(|file| source_path(private, file))
            .transpose()?;
        if certificate.is_some() && !base.starts_with("https://") {
            return Err(error("Pinned appliance certificate requires HTTPS.").into());
        }
        let client = client(certificate.as_deref())?;
        let password = string(credentials, "password")?;
        match details.kind.as_str() {
            "glinet" => {
                let payload = url::form_urlencoded::Serializer::new(String::new())
                    .append_pair("user", "admin")
                    .append_pair("passwd", password)
                    .finish();
                let cookies = login_cookies(
                    client
                        .post(format!("{base}/api/auth/login"))
                        .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
                        .body(payload)
                        .send()?,
                )?;
                let data = bounded_body(
                    client
                        .get(format!("{base}/api/system/get_network_config"))
                        .header(header::COOKIE, cookies)
                        .send()?,
                    1024 * 1024,
                )?;
                reported_network(&serde_json::from_slice(&data)?)?
            }
            "jetkvm" => {
                let cookie = login_cookies(
                    client
                        .post(format!("{base}/auth/login-local"))
                        .header(header::CONTENT_TYPE, "application/json")
                        .body(json!({"password":password}).to_string())
                        .send()?,
                )?;
                let response = client
                    .get(format!("{base}/diagnostics"))
                    .header(header::COOKIE, cookie)
                    .timeout(Duration::from_secs(30))
                    .send()?;
                diagnostics(
                    response,
                    details
                        .mac
                        .as_deref()
                        .ok_or_else(|| error("Missing enrolled JetKVM MAC."))?,
                    host,
                )?
            }
            _ => return Err(error("Unknown appliance type.").into()),
        }
    };
    let expiry = details
        .ca_file
        .as_deref()
        .map(|file| source_path(private, file))
        .transpose()?;
    let expired = if let Some(file) = expiry {
        let report = child(
            Command::new("openssl")
                .args(["x509", "-checkend", "0", "-noout", "-in"])
                .arg(file),
            None,
            8,
            4096,
        )?;
        !report.status.success()
            && String::from_utf8_lossy(&report.stdout).contains("Certificate will expire")
    } else {
        false
    };
    for row in &mut rows {
        row["machine"] = json!(details.id);
        if expired {
            row["warning"] = json!(
                "Appliance TLS certificate expired; identity verified by exact certificate pin. Renew its certificate."
            );
        }
    }
    Ok(rows)
}

pub(super) fn report(details: &Details, private: &Path, credentials: &Value) -> Vec<Value> {
    collect(details,private,credentials).unwrap_or_else(|_| vec![json!({"machine":details.id,"status":"unverified","reason":"Appliance settings read failed"})])
}

pub(super) fn remote(input: impl Read) -> Result<Vec<Value>, Box<dyn Error>> {
    let data: Value = serde_json::from_reader(input.take(16 * 1024))?;
    let details: Details = serde_json::from_value(data["target"].clone())?;
    if details.kind != "amt" || details.ca_file.is_some() {
        return Err(error("SSH appliance reader supports only scoped AMT reads.").into());
    }
    Ok(report(&details, Path::new("."), &data["credentials"]))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_different_appliance_certificate_is_rejected_before_http() {
        let pin = AppliancePin {
            certificate: b"enrolled device certificate".to_vec(),
            signatures: rustls::crypto::ring::default_provider().signature_verification_algorithms,
        };
        let name: ServerName<'_> = "kvm.example".try_into().unwrap();
        let stamp = UnixTime::now();
        assert!(
            pin.verify_server_cert(
                &CertificateDer::from(b"enrolled device certificate".as_slice()),
                &[],
                &name,
                &[],
                stamp
            )
            .is_ok()
        );
        assert!(
            pin.verify_server_cert(
                &CertificateDer::from(b"different certificate".as_slice()),
                &[],
                &name,
                &[],
                stamp
            )
            .is_err()
        );
    }

    #[test]
    fn glinet_explicit_false_means_static_and_missing_mode_is_unverified() {
        let mut data = json!({"ok":true,"result":{"success":true,"config":{
            "ip_address":"192.0.2.5","mac_address":"AA:BB:CC:DD:EE:FF","is_dhcp":false,
        }}});
        let rows = reported_network(&data).unwrap();
        assert_eq!(rows[0]["assignment"], "static");
        assert_eq!(rows[0]["mac"], "aa:bb:cc:dd:ee:ff");
        data["result"]["config"]["is_dhcp"] = Value::Null;
        assert!(reported_network(&data).is_err());
    }
}
