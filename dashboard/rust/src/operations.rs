use std::{sync::Mutex, time::Duration};

use axum::{
    Json,
    http::{HeaderValue, StatusCode, header},
    response::{IntoResponse, Response},
};
use serde::Serialize;
use tokio::{sync::watch, time::Instant};

use crate::{Dashboard, InventoryPayload, unix_time};

const COMPARISON_TIMEOUT: Duration = Duration::from_secs(45);
const COMPARISON_INTERVAL: Duration = Duration::from_secs(10);
const REFRESH_TIMEOUT: Duration = Duration::from_secs(12);

#[derive(Clone, Default)]
pub struct UpdateRequest {
    pub catalog_generation: u64,
    pub comparison_id: Option<String>,
}

pub(crate) struct Operations {
    requested: watch::Sender<UpdateRequest>,
    refreshed: watch::Sender<u64>,
    comparison: Mutex<Option<Comparison>>,
}

struct Comparison {
    receipt: Receipt,
    started: Instant,
}

#[derive(Clone, Serialize)]
struct Receipt {
    id: String,
    state: &'static str,
}

impl Default for Operations {
    fn default() -> Self {
        Self {
            requested: watch::Sender::new(UpdateRequest::default()),
            refreshed: watch::Sender::new(0),
            comparison: Mutex::new(None),
        }
    }
}

impl Dashboard {
    pub fn requested_updates(&self) -> watch::Receiver<UpdateRequest> {
        self.shared.operations.requested.subscribe()
    }

    pub fn finish_refresh(&self, generation: u64) {
        let operations = &self.shared.operations;
        if generation <= operations.requested.borrow().catalog_generation {
            operations.refreshed.send_if_modified(|completed| {
                if generation <= *completed {
                    return false;
                }
                *completed = generation;
                true
            });
        }
    }

    pub fn finish_comparison(&self, id: &str, success: bool) -> bool {
        let operations = &self.shared.operations;
        let mut comparison = operations.comparison.lock().expect("comparison lock");
        let Some(comparison) = comparison.as_mut() else {
            return false;
        };
        if comparison.receipt.id != id || comparison.receipt.state != "pending" {
            return false;
        }
        comparison.receipt.state = if success { "complete" } else { "failed" };
        let receipt = serde_json::to_value(&comparison.receipt).expect("serializable receipt");
        self.shared.latest.send_modify(|snapshot| {
            std::sync::Arc::make_mut(snapshot).comparison = Some(receipt);
        });
        operations
            .requested
            .send_modify(|request| request.comparison_id = None);
        true
    }

    pub(crate) fn reconcile(&self) -> Response {
        let operations = &self.shared.operations;
        let mut comparison = operations.comparison.lock().expect("comparison lock");
        let now = Instant::now();
        if comparison.as_ref().is_some_and(|running| {
            running.receipt.state == "pending" || now - running.started < COMPARISON_INTERVAL
        }) {
            return StatusCode::TOO_MANY_REQUESTS.into_response();
        }
        let receipt = Receipt {
            id: uuid::Uuid::new_v4().simple().to_string(),
            state: "pending",
        };
        *comparison = Some(Comparison {
            receipt: receipt.clone(),
            started: now,
        });
        let value = serde_json::to_value(&receipt).expect("serializable receipt");
        self.shared.latest.send_modify(|snapshot| {
            std::sync::Arc::make_mut(snapshot).comparison = Some(value);
        });
        operations
            .requested
            .send_modify(|request| request.comparison_id = Some(receipt.id.clone()));
        drop(comparison);

        let dashboard = self.clone();
        let id = receipt.id.clone();
        let mut updates = self.requested_updates();
        let mut shutdown = self.shared.shutdown.clone();
        tokio::spawn(async move {
            let wait =
                updates.wait_for(|request| request.comparison_id.as_deref() != Some(id.as_str()));
            let deadline = tokio::time::sleep_until(now + COMPARISON_TIMEOUT);
            tokio::select! {
                _ = shutdown.wait_for(|stopping| *stopping) => {},
                _ = wait => {},
                _ = deadline => { dashboard.finish_comparison(&id, false); },
            }
        });
        action_result(StatusCode::ACCEPTED, Json(receipt))
    }

    pub(crate) async fn refresh_catalog(&self) -> Response {
        let operations = &self.shared.operations;
        let mut completed = operations.refreshed.subscribe();
        let mut stopping = self.shared.shutdown.clone();
        let mut accepted = 0;
        operations.requested.send_modify(|request| {
            request.catalog_generation += 1;
            accepted = request.catalog_generation;
        });
        let finished = tokio::select! {
            result = tokio::time::timeout(REFRESH_TIMEOUT, completed.wait_for(|done| *done >= accepted)) => {
                matches!(result, Ok(Ok(_)))
            }
            _ = stopping.wait_for(|stopping| *stopping) => false,
        };
        if !finished {
            return StatusCode::SERVICE_UNAVAILABLE.into_response();
        }
        let snapshot = self.shared.latest.borrow().clone();
        if snapshot.error.is_some() {
            return StatusCode::SERVICE_UNAVAILABLE.into_response();
        }
        action_result(
            StatusCode::OK,
            Json(InventoryPayload {
                snapshot: &snapshot,
                server_time: unix_time(),
                lease_review_enabled: Some(self.shared.lease_review_enabled),
            }),
        )
    }

    pub(crate) fn publish_with_comparison(&self, mut snapshot: crate::Snapshot) {
        let comparison = self
            .shared
            .operations
            .comparison
            .lock()
            .expect("comparison lock");
        if let Some(current) = comparison.as_ref() {
            snapshot.comparison =
                Some(serde_json::to_value(&current.receipt).expect("serializable receipt"));
        }
        self.shared
            .latest
            .send_replace(std::sync::Arc::new(snapshot));
    }
}

fn action_result(status: StatusCode, body: impl IntoResponse) -> Response {
    let mut response = (status, body).into_response();
    response
        .headers_mut()
        .insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    response
}
