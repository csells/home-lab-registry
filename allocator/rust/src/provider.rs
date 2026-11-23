use std::{
    collections::BTreeMap,
    io::{Read, Write},
    path::PathBuf,
    process::{Command, Stdio},
    sync::Arc,
};

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::{Rejected, native_provider::NativeProvider};

#[derive(Clone)]
pub struct Provider(Backend);

#[derive(Clone)]
enum Backend {
    Native(Arc<NativeProvider>),
    Legacy { adapter: PathBuf, settings: PathBuf },
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum ProviderKind {
    Fixed,
    Dynamic,
    Tart,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct Slot {
    pub identity: String,
    pub recipe: String,
    pub user: String,
    pub host_key: Option<String>,
    pub name: Option<String>,
    pub node: String,
    pub guest_id: String,
    pub kind: ProviderKind,
}

#[derive(Deserialize)]
#[serde(untagged)]
enum AdapterReply {
    Success { result: Value },
    Failed { error: AdapterProblem },
}

#[derive(Deserialize)]
struct AdapterProblem {
    code: String,
    reason: String,
    status: u16,
}

pub(crate) fn unavailable() -> Rejected {
    Rejected::new(
        "provider_unavailable",
        "Check the recorded lease before retrying.",
        502,
    )
}

impl Provider {
    pub fn new(adapter: PathBuf, settings: PathBuf) -> Self {
        Self(Backend::Legacy { adapter, settings })
    }

    pub fn native(settings: &Value) -> Result<Self, Rejected> {
        Ok(Self(Backend::Native(Arc::new(NativeProvider::open(
            settings,
        )?))))
    }

    pub fn call(&self, request: Value) -> Result<Value, Rejected> {
        let Backend::Legacy { adapter, settings } = &self.0 else {
            let Backend::Native(provider) = &self.0 else {
                unreachable!()
            };
            return provider.call(request);
        };
        const OUTPUT_LIMIT: u64 = 256 * 1024;
        let request = serde_json::to_vec(&request).map_err(|_| unavailable())?;
        if request.len() > 128 * 1024 {
            return Err(unavailable());
        }
        let mut child = Command::new("python3")
            .arg(adapter)
            .arg(settings)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .spawn()
            .map_err(|_| unavailable())?;
        let result = (|| {
            let mut input = child.stdin.take().ok_or_else(unavailable)?;
            input.write_all(&request).map_err(|_| unavailable())?;
            drop(input);
            let mut response = Vec::new();
            child
                .stdout
                .take()
                .ok_or_else(unavailable)?
                .take(OUTPUT_LIMIT + 1)
                .read_to_end(&mut response)
                .map_err(|_| unavailable())?;
            if response.len() as u64 > OUTPUT_LIMIT {
                return Err(unavailable());
            }
            Ok(response)
        })();
        if result.is_err() {
            let _ = child.kill();
        }
        let status = child.wait().map_err(|_| unavailable())?;
        let response = result?;
        if !status.success() {
            return Err(unavailable());
        }
        match serde_json::from_slice(&response).map_err(|_| unavailable())? {
            AdapterReply::Success { result } => Ok(result),
            AdapterReply::Failed { error } => {
                Err(Rejected::new(error.code, error.reason, error.status))
            }
        }
    }

    pub fn generation(&self) -> Result<String, Rejected> {
        self.call(json!({"action":"controller-generation"}))?
            .as_str()
            .map(str::to_owned)
            .ok_or_else(unavailable)
    }

    pub fn slots(&self) -> Result<BTreeMap<String, Slot>, Rejected> {
        serde_json::from_value(self.call(json!({"action":"slots"}))?).map_err(|_| unavailable())
    }
}
