use std::{
    path::{Path, PathBuf},
    time::Duration,
};

use serde::Deserialize;
use serde_json::{Value, json};
use tokio::{
    sync::{mpsc, watch},
    time::{Instant, sleep, timeout, timeout_at},
};

use crate::{
    metrics::Frames,
    upstream::{SecureClient, read_json},
};

const READ_TIMEOUT: Duration = Duration::from_secs(15);
const FEED_TIMEOUT: Duration = Duration::from_secs(30);
const CATALOG_LIMIT: usize = 32 * 1024 * 1024;
const HEALTH_LIMIT: usize = 256 * 1024;

#[derive(Debug, PartialEq, Eq)]
pub(crate) enum Change {
    Catalog,
    Health(Vec<String>),
    Connection(bool),
}

#[derive(Default)]
struct Cursor(Option<(Option<String>, u64)>);

impl Cursor {
    fn resume(&self) -> Option<String> {
        let (Some(epoch), version) = self.0.as_ref()? else {
            return None;
        };
        Some(format!("{epoch}:{version}"))
    }

    fn observe(&mut self, frame: &Value) -> Result<Option<Change>, &'static str> {
        if *frame == json!({"heartbeat":true}) {
            return Ok(None);
        }
        let version = frame["version"].as_u64().ok_or("Invalid catalog version")?;
        let epoch = if let Some(epoch) = frame.get("epoch") {
            let name = epoch
                .as_str()
                .filter(|name| {
                    name.len() == 32 && name.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'))
                })
                .ok_or("Invalid catalog epoch")?;
            Some(name.to_owned())
        } else {
            None
        };
        let change = if let Some(health) = frame.get("health") {
            let health = health
                .as_array()
                .filter(|ids| (1..=128).contains(&ids.len()))
                .ok_or("Invalid catalog health notification")?;
            let ids = health
                .iter()
                .map(|id| {
                    id.as_str()
                        .filter(|id| valid_id(id))
                        .map(str::to_owned)
                        .ok_or("Invalid catalog health ID")
                })
                .collect::<Result<Vec<_>, _>>()?;
            Change::Health(ids)
        } else {
            Change::Catalog
        };
        let cursor = Some((epoch, version));
        if self.0 == cursor {
            return Ok(None);
        }
        self.0 = cursor;
        Ok(Some(change))
    }
}

fn valid_id(id: &str) -> bool {
    id.as_bytes().first().is_some_and(u8::is_ascii_lowercase)
        && id.len() <= 64
        && id
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-')
}

#[derive(Deserialize)]
struct Settings {
    url: String,
    ca_file: PathBuf,
    reader_token: String,
    #[serde(default = "enabled")]
    catalog_change_stream: bool,
    catalog_snapshot_at: Option<f64>,
}

fn enabled() -> bool {
    true
}

#[derive(Clone)]
pub struct Registry {
    client: SecureClient,
    feed: SecureClient,
    pub changes_enabled: bool,
    pub snapshot_at: Option<f64>,
}

impl Registry {
    pub async fn from_config(path: &Path) -> Result<Self, &'static str> {
        let settings: Settings = read_json(path).await?;
        let ca = if let Ok(relative) = settings.ca_file.strip_prefix("~") {
            PathBuf::from(std::env::var_os("HOME").ok_or("Registry CA path cannot be resolved")?)
                .join(relative)
        } else {
            settings.ca_file
        };
        let client = SecureClient::new(
            &settings.url,
            &ca,
            settings.reader_token.clone(),
            READ_TIMEOUT,
            true,
        )
        .await?;
        let feed = SecureClient::new(
            &settings.url,
            &ca,
            settings.reader_token,
            FEED_TIMEOUT,
            true,
        )
        .await?;
        Ok(Self {
            client,
            feed,
            changes_enabled: settings.catalog_change_stream,
            snapshot_at: settings.catalog_snapshot_at,
        })
    }

    pub async fn records(&self) -> Result<Vec<Value>, &'static str> {
        let response = timeout(
            READ_TIMEOUT,
            self.client.json("/v1/services", &[], CATALOG_LIMIT),
        )
        .await
        .map_err(|_| "Registry catalog read timed out")??;
        response["services"]
            .as_array()
            .filter(|records| {
                records.iter().all(|record| {
                    record["id"].as_str().is_some_and(valid_id) && record["spec"].is_object()
                })
            })
            .cloned()
            .ok_or("Invalid registry catalog")
    }

    pub async fn health(
        &self,
        ids: &[String],
    ) -> Result<serde_json::Map<String, Value>, &'static str> {
        let query: Vec<_> = ids.iter().map(|id| ("id", id.as_str())).collect();
        let response = timeout(
            READ_TIMEOUT,
            self.client.json("/v1/health", &query, HEALTH_LIMIT),
        )
        .await
        .map_err(|_| "Registry health read timed out")??;
        response["health"]
            .as_object()
            .cloned()
            .ok_or("Invalid registry health response")
    }

    pub(crate) async fn watch(
        self,
        updates: mpsc::Sender<Change>,
        mut stop: watch::Receiver<bool>,
    ) {
        let mut cursor = Cursor::default();
        let mut retry = Duration::from_secs(2);
        let mut connected = false;
        while !*stop.borrow() {
            let result = tokio::select! {
                _ = stop.wait_for(|stop| *stop) => return,
                result = self.events(&mut cursor, &updates, &mut connected, &mut retry) => result,
            };
            if let Err(reason) = result {
                tracing::warn!(reason, "Catalog stream disconnected");
            }
            if connected {
                connected = false;
                if updates.send(Change::Connection(false)).await.is_err() {
                    return;
                }
            }
            tokio::select! {
                _ = stop.wait_for(|stop| *stop) => return,
                _ = sleep(retry) => {}
            }
            retry = (retry * 2).min(Duration::from_secs(60));
        }
    }

    async fn events(
        &self,
        cursor: &mut Cursor,
        updates: &mpsc::Sender<Change>,
        connected: &mut bool,
        retry: &mut Duration,
    ) -> Result<(), &'static str> {
        let resume = cursor.resume();
        let mut response = self
            .feed
            .open(
                "/v1/catalog/changes",
                &[("health", "1")],
                "application/x-ndjson",
                resume.as_deref(),
            )
            .await?;
        if response
            .headers()
            .get("content-type")
            .and_then(|value| value.to_str().ok())
            .and_then(|value| value.split(';').next())
            .is_none_or(|value| value != "application/x-ndjson")
        {
            return Err("Unexpected catalog stream response type");
        }
        let mut frames = Frames::bounded(9_999);
        let mut deadline = Instant::now() + FEED_TIMEOUT;
        loop {
            let chunk = timeout_at(deadline, response.chunk())
                .await
                .map_err(|_| "Catalog heartbeat timed out")?
                .map_err(|_| "Catalog stream interrupted")?
                .ok_or("Catalog stream closed")?;
            let mut changes = Vec::new();
            let mut received = false;
            frames.receive(&chunk, |frame| {
                if let Some(change) = cursor.observe(&frame)? {
                    changes.push(change);
                }
                received = true;
                Ok(())
            })?;
            if received {
                *retry = Duration::from_secs(2);
                deadline = Instant::now() + FEED_TIMEOUT;
                if !*connected {
                    updates
                        .send(Change::Connection(true))
                        .await
                        .map_err(|_| "Catalog subscriber stopped")?;
                    *connected = true;
                }
            }
            for change in changes {
                updates
                    .send(change)
                    .await
                    .map_err(|_| "Catalog subscriber stopped")?;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn catalog_cursor_handles_reconnect_and_scoped_health_without_full_reload() {
        let mut feed = Cursor::default();
        let epoch = "4da2c435bef648398d4cbabe890b55f1";
        assert_eq!(feed.observe(&json!({"heartbeat":true})).unwrap(), None);
        assert_eq!(
            feed.observe(&json!({"epoch":epoch,"version":1})).unwrap(),
            Some(Change::Catalog)
        );
        assert_eq!(
            feed.resume().as_deref(),
            Some("4da2c435bef648398d4cbabe890b55f1:1")
        );
        assert_eq!(
            feed.observe(&json!({"epoch":epoch,"version":1})).unwrap(),
            None
        );
        assert_eq!(
            feed.observe(&json!({"epoch":epoch,"version":2,"health":["service-test"]}))
                .unwrap(),
            Some(Change::Health(vec!["service-test".into()])),
        );
        assert!(
            feed.observe(&json!({"epoch":epoch,"version":3,"health":["invalid/name"]}))
                .is_err()
        );
        assert_eq!(feed.resume().unwrap(), format!("{epoch}:2"));
    }

    #[test]
    fn legacy_unversioned_epochs_do_not_send_an_unsafe_resume_header() {
        let mut feed = Cursor::default();
        assert_eq!(
            feed.observe(&json!({"version":20})).unwrap(),
            Some(Change::Catalog)
        );
        assert_eq!(feed.resume(), None);
        assert!(feed.observe(&json!({"version":-1})).is_err());
        assert!(
            feed.observe(&json!({"version":0,"epoch":"invalid"}))
                .is_err()
        );
        assert!(feed.observe(&json!({"version":true})).is_err());
    }
}
