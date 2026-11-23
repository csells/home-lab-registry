use std::collections::VecDeque;

use serde_json::{Value, json};

const MAX_FRAME: usize = 262_144;
const MAX_HISTORY: usize = 120;

pub struct Frames {
    pending: Vec<u8>,
    max: usize,
}

impl Default for Frames {
    fn default() -> Self {
        Self::bounded(MAX_FRAME)
    }
}

impl Frames {
    pub fn bounded(max: usize) -> Self {
        Self {
            pending: Vec::new(),
            max,
        }
    }

    pub fn receive(
        &mut self,
        mut input: &[u8],
        mut apply: impl FnMut(Value) -> Result<(), &'static str>,
    ) -> Result<(), &'static str> {
        while !input.is_empty() {
            let end = input.iter().position(|byte| *byte == b'\n');
            let length = end.unwrap_or(input.len());
            if length > self.max - self.pending.len() {
                return Err("Collector frame too large");
            }
            if let Some(end) = end {
                let value = if self.pending.is_empty() {
                    serde_json::from_slice(&input[..end])
                } else {
                    self.pending.extend_from_slice(&input[..end]);
                    let parsed = serde_json::from_slice(&self.pending);
                    self.pending.clear();
                    parsed
                }
                .map_err(|_| "Invalid collector JSON")?;
                apply(value)?;
                input = &input[end + 1..];
            } else {
                self.pending.extend_from_slice(input);
                break;
            }
        }
        Ok(())
    }
}

#[derive(Default)]
pub struct History {
    samples: VecDeque<(f64, Value)>,
    error: Option<&'static str>,
}

impl History {
    pub fn disconnected(&mut self) {
        self.error = Some("Collector stream unavailable");
    }

    pub fn apply(&mut self, frame: Value) -> Result<bool, &'static str> {
        let replace = match frame["type"].as_str() {
            Some("heartbeat") => return Ok(false),
            Some("history") => true,
            Some("samples") => false,
            _ => return Err("Invalid collector frame"),
        };
        let field = if replace { "history" } else { "samples" };
        let Value::Array(incoming) = frame
            .get(field)
            .cloned()
            .ok_or("Missing collector samples")?
        else {
            return Err("Invalid collector samples");
        };
        if incoming.len() > MAX_HISTORY {
            return Err("Too many collector samples");
        }
        let mut validated = Vec::with_capacity(incoming.len());
        for sample in incoming {
            let stamp = sample["checked"]
                .as_f64()
                .ok_or("Invalid collector sample time")?;
            if !matches!(sample["status"].as_str(), Some("available" | "unavailable")) {
                return Err("Invalid collector sample status");
            }
            validated.push((stamp, sample));
        }
        if replace {
            self.samples.clear();
        }
        for (stamp, sample) in validated {
            if self.samples.back().is_none_or(|(last, _)| stamp > *last) {
                if self.samples.len() == MAX_HISTORY {
                    self.samples.pop_front();
                }
                self.samples.push_back((stamp, sample));
            }
        }
        self.error = None;
        Ok(true)
    }

    pub fn reading(&self, now: f64) -> Value {
        let mut latest = self
            .samples
            .back()
            .map_or_else(|| json!({"status":"waiting"}), |(_, sample)| sample.clone());
        let expired =
            self.samples.back().is_some() && latest["expires"].as_f64().unwrap_or(0.0) < now;
        if let Some(error) = self
            .error
            .or_else(|| expired.then_some("Collector data expired"))
        {
            latest["status"] = json!("unavailable");
            latest["error"] = json!(error);
        }
        latest["history"] = Value::Array(
            self.samples
                .iter()
                .map(|(_, sample)| sample.clone())
                .collect(),
        );
        latest["interval_seconds"] = json!(5);
        latest
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn history_and_delta_frames_accept_only_valid_ordered_samples_and_keep_peaks() {
        let mut history = History::default();
        let frame = json!({"type":"history","history":[
            {"checked":12,"expires":20,"status":"available","gpu":{"status":"available","percent":0}},
            {"checked":11,"expires":20,"status":"available"},
            {"checked":14,"expires":20,"status":"available","cpu_percent":90},
        ]});
        assert!(history.apply(frame).unwrap());
        assert!(!history.apply(json!({"type":"heartbeat"})).unwrap());
        assert!(
            history
                .apply(json!({"type":"samples","samples":[
                    {"checked":14,"expires":20,"status":"unavailable"},
                    {"checked":16,"expires":22,"status":"available","cpu_percent":2}
                ]}))
                .unwrap()
        );
        let data = history.reading(19.0);
        assert_eq!(data["status"], "available");
        assert_eq!(data["history"].as_array().unwrap().len(), 3);
        assert_eq!(data["history"][0]["gpu"]["percent"], 0);
        assert_eq!(data["history"][1]["cpu_percent"], 90);
        assert_eq!(data["cpu_percent"], 2);
        assert_eq!(history.reading(23.0)["status"], "unavailable");
        assert!(
            history
                .apply(json!({"type":"samples","samples":[{"checked":21,"status":"oops"}]}))
                .is_err()
        );
        assert_eq!(history.reading(19.0), data);
    }

    #[test]
    fn framed_transport_rejects_an_oversized_partial_frame_without_discarding_its_limit() {
        let mut buffer = Frames::default();
        let mut frames = vec![];
        let mut apply = |value| {
            frames.push(value);
            Ok(())
        };
        buffer.receive(b"{\"type\":", &mut apply).unwrap();
        buffer.receive(b"\"heartbeat\"}\n{}\n", &mut apply).unwrap();
        assert_eq!(frames, vec![json!({"type":"heartbeat"}), json!({})]);
        buffer
            .receive(&vec![b' '; MAX_FRAME], |_| unreachable!())
            .unwrap();
        assert!(buffer.receive(b" ", |_| unreachable!()).is_err());
        assert_eq!(buffer.pending.len(), MAX_FRAME);
    }
}

#[cfg(test)]
mod target_tests {
    use crate::collector::Target;

    #[test]
    fn collector_identity_is_separate_from_its_dial_address() {
        let target = Target::parse(
            "https://192.0.2.12:9911/stream",
            Some("collector-node.example.invalid"),
        )
        .unwrap();
        assert_eq!(target.dial_host, "192.0.2.12");
        assert_eq!(target.port, 9911);
        assert_eq!(
            target.https.host_str(),
            Some("collector-node.example.invalid")
        );
        assert_eq!(target.https.path(), "/stream");
        for invalid in [
            "http://192.0.2.12:9911/stream",
            "https://192.0.2.12/stream",
            "https://user@192.0.2.12:9911/stream",
            "https://192.0.2.12:9911/stream?x=1",
            "https://192.0.2.12:9911/stream#something",
            "https://192.0.2.12:9911/path",
        ] {
            assert!(Target::parse(invalid, None).is_err(), "{invalid}");
        }
        assert!(Target::parse("https://192.0.2.12:9911/stream", Some("a/b")).is_err());
        for certificate_ip in ["192.0.2.13", "2001:db8::1"] {
            let target =
                Target::parse("https://192.0.2.12:9911/stream", Some(certificate_ip)).unwrap();
            assert_eq!(target.dial_host, "192.0.2.12");
        }
        assert!(Target::parse("https://192.0.2.12:9911/stream", Some("0x7f000001")).is_err());
    }
}
