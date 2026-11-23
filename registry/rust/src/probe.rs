use std::{
    collections::{BTreeMap, HashMap},
    time::Duration,
};

use futures_util::stream::{self, StreamExt};
use reqwest::{Client, Response, redirect::Policy};
use serde::Deserialize;
use serde_json::{Value, json};
use tokio::{net::TcpStream, sync::watch};

use crate::{Catalog, Problem, RegistryHttp, http::blocking};

#[derive(Deserialize)]
pub(crate) struct Credential {
    url: String,
    token: String,
}

fn ready(value: &Value, excluded: &Value) -> bool {
    value.is_object()
        && excluded.as_object().is_some_and(|rules| {
            rules.iter().all(|(key, excluded)| {
                value[key].as_str().is_some_and(|value| {
                    excluded
                        .as_array()
                        .is_some_and(|items| !items.iter().any(|item| item == value))
                })
            })
        })
}

async fn readiness(mut response: Response, rules: &Value) -> Option<bool> {
    let mut body = Vec::new();
    while let Some(chunk) = response.chunk().await.ok()? {
        if chunk.len() > (1 << 20) - body.len() {
            return None;
        }
        body.extend_from_slice(&chunk);
    }
    serde_json::from_slice::<Value>(&body)
        .ok()
        .map(|value| ready(&value, rules))
}

async fn check(
    client: &Client,
    spec: &Value,
    credentials: &HashMap<String, Credential>,
) -> (bool, String) {
    let probe = &spec["probe"];
    if let Some(host) = probe["tcp_host"].as_str() {
        let Some(port) = probe["tcp_port"]
            .as_u64()
            .and_then(|value| u16::try_from(value).ok())
        else {
            return (false, "Probe failed: Invalid TCP port".into());
        };
        let connect =
            tokio::time::timeout(Duration::from_secs(3), TcpStream::connect((host, port))).await;
        return match connect {
            Ok(Ok(_)) => (true, format!("TCP {host}:{port} connected")),
            Ok(Err(_)) => (false, format!("TCP {host}:{port} connection failed")),
            Err(_) => (false, format!("TCP {host}:{port} timed out")),
        };
    }
    let Some(url) = probe["url"].as_str() else {
        return (false, "Probe failed: KeyError".into());
    };
    let mut request = client.get(url);
    if let Some(reference) = probe["bearer_ref"].as_str() {
        let credential = credentials
            .get(reference)
            .filter(|credential| credential.url == url);
        let Some(credential) = credential else {
            return (false, "Probe failed: ValueError".into());
        };
        request = request.bearer_auth(&credential.token);
    }
    let Ok(response) = request.send().await else {
        return (false, "Probe failed: URLError".into());
    };
    let code = response.status().as_u16();
    let mut healthy = u64::from(code) == probe["expected_status"].as_u64().unwrap_or(200);
    let mut evidence = format!("HTTP {code}");
    if let Some(rules) = probe.get("json_not_in") {
        match readiness(response, rules).await {
            Some(value) => {
                healthy &= value;
                evidence.push_str(if value {
                    "; application state available"
                } else {
                    "; application state unavailable or missing"
                });
            }
            None => {
                healthy = false;
                evidence.push_str("; invalid readiness JSON");
            }
        }
    }
    (healthy, evidence)
}

pub(crate) async fn monitor(
    state: RegistryHttp,
    catalog: Catalog,
    credentials: HashMap<String, Credential>,
    mut stop: watch::Receiver<bool>,
) {
    let Ok(client) = Client::builder()
        .tls_backend_native()
        .no_proxy()
        .redirect(Policy::none())
        .timeout(Duration::from_secs(3))
        .build()
    else {
        tracing::error!("Registry could not initialize health probe client");
        return;
    };
    while !*stop.borrow() {
        let run = cycle(&state, &catalog, &client, &credentials);
        tokio::select! {
            _ = stop.wait_for(|stopping| *stopping) => return,
            result = run => {
                if result.is_err() { tracing::warn!("Registry health reconciliation failed"); }
            }
        }
        tokio::select! {
            _ = stop.wait_for(|stopping| *stopping) => return,
            _ = tokio::time::sleep(Duration::from_secs(600)) => {}
        }
    }
}

async fn cycle(
    state: &RegistryHttp,
    catalog: &Catalog,
    client: &Client,
    credentials: &HashMap<String, Credential>,
) -> Result<(), Problem> {
    let records = blocking({
        let catalog = catalog.clone();
        move || catalog.listing(&BTreeMap::new())
    })
    .await?;
    let candidates = records.into_iter().filter(|record| {
        let spec = &record["spec"];
        spec["lifecycle"] == "active"
            && spec["probe"].is_object()
            && spec["probe"]["vantage"].as_str().unwrap_or("registry") == "registry"
    });
    let jobs = stream::iter(candidates).map(|record| async move {
        let (healthy,evidence) = check(client,&record["spec"],credentials).await;
        let (Some(id),Some(revision)) = (record["id"].as_str().map(str::to_owned),record["revision"].as_i64()) else { return };
        let target = id.clone();
        let catalog = catalog.clone();
        let result = blocking(move || catalog.observe(&id,json!({
            "status":if healthy {"healthy"} else {"unhealthy"},"evidence":evidence,"ttl_seconds":1800,
        }),Some(revision))).await;
        match result {
            Ok(_) => state.committed(Some(target)),
            Err(error) if error.status() == 409 => {},
            Err(_) => tracing::warn!("Health observation could not be saved"),
        }
    }).buffer_unordered(4);
    jobs.for_each(|_| async {}).await;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn readiness_requires_each_declared_property_to_be_present_and_outside_exclusions() {
        let excluded = json!({"status":["starting","error"],"database":["disconnected"]});
        assert!(ready(
            &json!({"status":"ready","database":"connected"}),
            &excluded
        ));
        assert!(!ready(&json!({"status":"ready"}), &excluded));
        assert!(!ready(
            &json!({"status":"ready","database":true}),
            &excluded
        ));
        assert!(!ready(
            &json!({"status":"starting","database":"connected"}),
            &excluded
        ));
    }
}
