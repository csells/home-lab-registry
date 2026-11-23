use base64::{Engine, engine::general_purpose::STANDARD};
use serde_json::Value;

use crate::Rejected;

pub(crate) const LINUX: &str = "debian13-amd64";
pub(crate) const APPLE: &str = "macos-xcode-arm64";

pub(crate) fn public_key(value: &Value) -> Result<String, Rejected> {
    let approved = || {
        Rejected::new(
            "invalid_ssh_key",
            "Provide one OpenSSH Ed25519 public key without a comment.",
            400,
        )
    };
    let value = value
        .as_str()
        .filter(|value| value.chars().count() <= 200)
        .ok_or_else(approved)?;
    let parts: Vec<_> = value.split_whitespace().collect();
    if let ["ssh-ed25519", body] = parts.as_slice() {
        let public = STANDARD.decode(body).ok().filter(|decoded| {
            const SIGNATURE: &[u8] = b"\0\0\0\x0bssh-ed25519\0\0\0 ";
            decoded.len() == SIGNATURE.len() + 32 && decoded.starts_with(SIGNATURE)
        });
        if let Some(public) = public {
            return Ok(format!("ssh-ed25519 {}", STANDARD.encode(public)));
        }
    }
    Err(approved())
}
