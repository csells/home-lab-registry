use std::fmt::Write;

use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use ring::{
    digest,
    rand::{SecureRandom, SystemRandom},
};

use crate::Rejected;

pub(crate) fn token() -> Result<String, Rejected> {
    let mut entropy = [0u8; 32];
    SystemRandom::new().fill(&mut entropy).map_err(|_| {
        Rejected::new(
            "credential_unavailable",
            "Cannot mint a secure scoped credential.",
            503,
        )
    })?;
    Ok(URL_SAFE_NO_PAD.encode(entropy))
}

pub(crate) fn valid_token(token: &str, minimum: usize, maximum: usize) -> bool {
    (minimum..=maximum).contains(&token.len())
        && token
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
}

pub(crate) fn hash(text: &str) -> String {
    let value = digest::digest(&digest::SHA256, text.as_bytes());
    let mut buffer = String::with_capacity(64);
    for byte in value.as_ref() {
        write!(buffer, "{byte:02x}").expect("write to String");
    }
    buffer
}

pub(crate) fn valid_hash(text: &str) -> bool {
    hex(text, 64)
}

pub(crate) fn hex(text: &str, len: usize) -> bool {
    text.len() == len
        && text
            .bytes()
            .all(|byte| matches!(byte,b'0'..=b'9'|b'a'..=b'f'))
}
