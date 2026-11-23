use std::{collections::BTreeMap, sync::Arc};

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::{Rejected, native_provider::NativeProvider};

#[derive(Clone)]
pub struct Provider(Arc<NativeProvider>);

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

pub(crate) fn unavailable() -> Rejected {
    Rejected::new(
        "provider_unavailable",
        "Check the recorded lease before retrying.",
        502,
    )
}

impl Provider {
    pub fn native(settings: &Value) -> Result<Self, Rejected> {
        Ok(Self(Arc::new(NativeProvider::open(settings)?)))
    }

    pub fn call(&self, request: Value) -> Result<Value, Rejected> {
        self.0.call(request)
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
