use std::{fs::File, io, process::Command, time::Duration};

use serde_json::{Value, json};

use crate::{process::bounded, tokens::hex};

pub fn deliver_alert(value: &Value) -> io::Result<Value> {
    if !value["id"].as_str().is_some_and(|id| hex(id, 32))
        || !value["lease_id"].as_str().is_some_and(|id| hex(id, 32))
        || value["owner"].as_str().is_none_or(str::is_empty)
        || value["failed_at"]
            .as_f64()
            .is_none_or(|stamp| !stamp.is_finite())
    {
        return Err(io::Error::other("Invalid export alert identity."));
    }
    Ok(
        json!({"id":value["id"], "owner":value["owner"], "lease_id":value["lease_id"],
        "failed_at":value["failed_at"], "kind":"lease-export-failed"}),
    )
}

pub fn run_alert_recipient(
    command: &mut Command,
    input: &File,
    timeout: Duration,
) -> io::Result<()> {
    let output = bounded(command, Some(input), timeout, 1024 * 1024)
        .map_err(|_| io::Error::other("Alert delivery did not finish."))?;
    if !output.status.success() {
        return Err(io::Error::other("Alert recipient did not accept delivery."));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_the_safe_alert_envelope_leaves_the_worker() {
        let alert = json!({"id":"a".repeat(32),"lease_id":"b".repeat(32),"owner":"owner",
            "failed_at":302.5,"reason":"sensitive diagnostic","challenge":"secret","environment":{}});
        let envelope = deliver_alert(&alert).unwrap();
        assert_eq!(envelope.as_object().unwrap().len(), 5);
        assert_eq!(envelope["kind"], "lease-export-failed");
        for key in ["id", "lease_id", "owner", "failed_at"] {
            assert_eq!(envelope[key], alert[key]);
        }
    }
}
