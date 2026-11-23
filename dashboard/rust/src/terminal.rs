use std::{path::Path, time::Duration};

use aes::{Aes128, Aes192, Aes256};
use base64::{
    Engine as _,
    engine::general_purpose::{STANDARD, STANDARD_NO_PAD},
};
use cbc::cipher::{BlockEncryptMut, KeyIvInit, block_padding::Pkcs7};
use reqwest::{Client, Url, redirect::Policy};
use ring::{digest, hmac};
use serde::Deserialize;
use serde_json::{Map, Value, json};
use tokio::{sync::Mutex, time::Instant};

use crate::{Registry, inventory::access, unix_time, upstream::read_json};

const API: &str = "http://127.0.0.1:8095/guacamole/api/";
const TMUX: &str = "export PATH=/opt/homebrew/bin:/usr/local/bin:$PATH; exec tmux -L lab-terminal new-session -A -s browser";

#[derive(Default)]
pub(crate) struct Terminal {
    cache: Mutex<Option<Cached>>,
}

struct Cached {
    fingerprint: digest::Digest,
    token: String,
    created: Instant,
}

fn terminal_id(name: &str) -> bool {
    name.as_bytes().first().is_some_and(u8::is_ascii_lowercase)
        && name.len() <= 49
        && name
            .bytes()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == b'-')
}

fn connections(
    records: &[Value],
    scope: Option<&str>,
    pins: &str,
    key: &str,
) -> Map<String, Value> {
    let mut result = Map::new();
    for record in records {
        let spec = &record["spec"];
        if spec["lifecycle"] != "active"
            || !spec["capabilities"]
                .as_array()
                .is_some_and(|values| values.iter().any(|cap| cap == "machine"))
        {
            continue;
        }
        let config = &spec["resources"];
        let machine = match scope {
            Some(target) if record["id"] == target => {
                config["staging_review"]["access"]["terminal_id"].as_str()
            }
            _ => None,
        }
        .or_else(|| spec["host"].as_str());
        let Some(machine) = machine.filter(|name| terminal_id(name)) else {
            continue;
        };
        for route in access(record, scope).as_array().into_iter().flatten() {
            if route["id"] != "os" {
                continue;
            }
            let Some(address) = route["url"]
                .as_str()
                .and_then(|raw| Url::parse(raw).ok())
                .filter(|url| url.scheme() == "ssh" && url.password().is_none())
            else {
                continue;
            };
            let Some(hostname) = address
                .host_str()
                .map(|host| host.trim_start_matches('[').trim_end_matches(']'))
            else {
                continue;
            };
            let Some(username) = route["username"]
                .as_str()
                .filter(|username| !username.is_empty())
            else {
                continue;
            };
            let port = address.port().unwrap_or(22);
            let destination = if port == 22 {
                hostname.to_owned()
            } else {
                format!("[{hostname}]:{port}")
            };
            let look_up = route["host_key_alias"]
                .as_str()
                .filter(|alias| !alias.is_empty())
                .unwrap_or(&destination);
            let pin = pins.lines().find_map(|line| {
                let mut fields = line.split_whitespace();
                let hosts = fields.next()?;
                if !hosts.split(',').any(|name| name == look_up) {
                    return None;
                }
                Some((fields.next()?, fields.next()?))
            });
            let Some((algorithm, fingerprint)) = pin else {
                continue;
            };
            result.insert(machine.into(), json!({
                "protocol":"ssh","parameters":{
                    "hostname":hostname, "port":port.to_string(), "username":username,
                    "host-key":format!("{destination} {algorithm} {fingerprint}"), "private-key":key,
                    "font-size":"18","scrollback":"5000","server-alive-interval":"30",
                    "color-scheme":"gray-black","backspace":"127","terminal-type":"xterm-256color","command":TMUX,
                }
            }));
        }
    }
    result
}

fn signed_data(key: &[u8], value: &Value) -> Result<String, &'static str> {
    let raw = serde_json::to_vec(value).map_err(|_| "Cannot serialize terminal configuration")?;
    let signed = hmac::sign(&hmac::Key::new(hmac::HMAC_SHA256, key), &raw);
    let mut clear = signed.as_ref().to_vec();
    clear.extend_from_slice(&raw);
    let length = clear.len();
    clear.resize(length + 16, 0);
    let iv = [0; 16];
    let cipher_text = match key.len() {
        16 => cbc::Encryptor::<Aes128>::new_from_slices(key, &iv)
            .map_err(|_| "Invalid terminal encryption key")?
            .encrypt_padded_mut::<Pkcs7>(&mut clear, length),
        24 => cbc::Encryptor::<Aes192>::new_from_slices(key, &iv)
            .map_err(|_| "Invalid terminal encryption key")?
            .encrypt_padded_mut::<Pkcs7>(&mut clear, length),
        32 => cbc::Encryptor::<Aes256>::new_from_slices(key, &iv)
            .map_err(|_| "Invalid terminal encryption key")?
            .encrypt_padded_mut::<Pkcs7>(&mut clear, length),
        _ => return Err("Invalid terminal encryption key size"),
    }
    .map_err(|_| "Terminal encryption failed")?;
    Ok(STANDARD.encode(cipher_text))
}

fn hex_key(hex: &str) -> Result<Vec<u8>, &'static str> {
    let bytes = hex.trim().as_bytes();
    if !matches!(bytes.len(), 32 | 48 | 64) {
        return Err("Invalid Guacamole encryption key size");
    }
    bytes
        .chunks_exact(2)
        .map(|pair| {
            let hex = std::str::from_utf8(pair).map_err(|_| "Invalid Guacamole encryption key")?;
            u8::from_str_radix(hex, 16).map_err(|_| "Invalid Guacamole encryption key")
        })
        .collect()
}

impl Terminal {
    pub async fn launch(
        &self,
        private: &Path,
        scope: Option<&str>,
        machine: &str,
    ) -> Result<String, &'static str> {
        if !terminal_id(machine) {
            return Err("Invalid terminal ID");
        }
        let registry = Registry::from_config(&private.join("reader.json")).await?;
        let pins = tokio::fs::read_to_string(private.join("guacamole_known_hosts"))
            .await
            .map_err(|_| "Host pins unavailable")?;
        let key = tokio::fs::read_to_string(private.join("terminal_key"))
            .await
            .map_err(|_| "Terminal key unavailable")?;
        let records = registry.records().await?;
        let available = connections(&records, scope, &pins, &key);
        if !available.contains_key(machine) {
            return Err("Terminal is outside the available scope");
        }
        let encoded =
            serde_json::to_vec(&available).map_err(|_| "Cannot serialize terminal connections")?;
        let fingerprint = digest::digest(&digest::SHA256, &encoded);
        let client = Client::builder()
            .no_proxy()
            .redirect(Policy::none())
            .timeout(Duration::from_secs(15))
            .build()
            .map_err(|_| "Cannot initialize terminal client")?;
        let mut cache = self.cache.lock().await;
        let mut active = None;
        if let Some(entry) = cache.as_ref()
            && entry.created.elapsed() < Duration::from_secs(2700)
            && entry.fingerprint.as_ref() == fingerprint.as_ref()
        {
            let session = client
                .head(format!("{API}session"))
                .header("Guacamole-Token", &entry.token)
                .timeout(Duration::from_secs(10))
                .send()
                .await;
            if session.is_ok_and(|response| response.status().is_success()) {
                active = Some(entry.token.clone());
            }
        }
        let token = match active {
            Some(token) => token,
            None => {
                let secret = tokio::fs::read_to_string(private.join("guacamole-key"))
                    .await
                    .map_err(|_| "Issuer key unavailable")?;
                let login: crate::Login = read_json(&private.join("login.json")).await?;
                let payload = json!({"username":login.username,"expires":((unix_time()+3600.0)*1000.0) as u64,"connections":available});
                let encrypted = signed_data(&hex_key(&secret)?, &payload)?;
                let body: String = form_urlencoded::Serializer::new(String::new())
                    .append_pair("data", &encrypted)
                    .finish();
                let response = client
                    .post(format!("{API}tokens"))
                    .header("Content-Type", "application/x-www-form-urlencoded")
                    .body(body)
                    .send()
                    .await
                    .map_err(|_| "Terminal issuer request failed")?;
                if !response.status().is_success() {
                    return Err("Terminal issuer rejected authentication");
                }
                #[derive(Deserialize)]
                struct Session {
                    #[serde(rename = "authToken")]
                    token: String,
                }
                let body = response
                    .bytes()
                    .await
                    .map_err(|_| "Terminal response interrupted")?;
                if body.len() > 32_768 {
                    return Err("Terminal response exceeded limit");
                }
                let session: Session =
                    serde_json::from_slice(&body).map_err(|_| "Invalid terminal response")?;
                if session.token.is_empty() {
                    return Err("Empty terminal token");
                }
                *cache = Some(Cached {
                    fingerprint,
                    token: session.token.clone(),
                    created: Instant::now(),
                });
                session.token
            }
        };
        let client_id = STANDARD_NO_PAD.encode(format!("{machine}\0c\0json"));
        let client_id: String = form_urlencoded::byte_serialize(client_id.as_bytes()).collect();
        let token: String = form_urlencoded::byte_serialize(token.as_bytes()).collect();
        Ok(format!("/guacamole/#/client/{client_id}?token={token}"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rediscovery_uses_current_ssh_address_with_a_pinned_alias() {
        let mut record = json!({"id":"machine-test","spec":{
            "lifecycle":"active","capabilities":["machine"],"host":"test","resources":{
                "management_access":[{"id":"os","url":"ssh://192.0.2.11:2222","username":"tester","host_key_alias":"machine-test"}],
                "staging_review":{"access":{"terminal_id":"disposable","management_access":[
                    {"id":"os","url":"ssh://192.0.2.11:2222","username":"tester","host_key_alias":"machine-test"}
                ]}}
            }
        }});
        let pins = "machine-test ssh-ed25519 AAAAtest\n";
        let unscoped = connections(&[record.clone()], None, pins, "fixture-key");
        assert_eq!(unscoped["test"]["parameters"]["hostname"], "192.0.2.11");
        assert_eq!(
            unscoped["test"]["parameters"]["host-key"],
            "[192.0.2.11]:2222 ssh-ed25519 AAAAtest"
        );
        let staged = connections(&[record.clone()], Some("machine-test"), pins, "fixture-key");
        assert!(staged.get("test").is_none());
        assert!(staged.get("disposable").is_some());
        record["spec"]["resources"]["management_access"][0]["host_key_alias"] = json!("wrong-pin");
        assert!(connections(&[record], None, pins, "fixture-key").is_empty());
    }
}

#[cfg(test)]
#[test]
fn signed_terminal_payload_matches_the_existing_issuer_protocol() {
    let key: Vec<u8> = (0..16).collect();
    assert_eq!(
        signed_data(&key, &json!({"aa":1,"bb":"test"})).unwrap(),
        "zuWh4EtyJpt0qFPpdA9QVc86p4F4O+Rt3XILTKDNKunNAfwvg+aiqfOkwV1o6VaYXvnZAxNal6QwS9MMIdIviw=="
    );
}
