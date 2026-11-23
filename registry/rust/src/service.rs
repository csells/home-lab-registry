use std::{
    collections::{BTreeSet, HashMap},
    net::SocketAddr,
    path::PathBuf,
    time::Duration,
};

use axum_server::{
    Handle,
    tls_rustls::{RustlsConfig, bind_rustls},
};
use serde::Deserialize;
use serde_json::{Value, json};
use tokio::{signal, sync::watch};

use crate::{
    Catalog, RegistryHttp,
    enrollment::{Enrolled, PresenceVerifier},
    http::blocking,
    probe::{Credential, monitor},
};

#[derive(Deserialize)]
struct Settings {
    database: PathBuf,
    reader_token: String,
    writer_token: String,
    cert: PathBuf,
    key: PathBuf,
    #[serde(default = "bind_default")]
    bind: std::net::IpAddr,
    #[serde(default = "port_default")]
    port: u16,
    #[serde(default = "empty")]
    machine_enrollments: Value,
    #[serde(default = "empty")]
    runtime_publishers: Value,
    presence_tls: Option<PresenceVerifier>,
    #[serde(default = "enabled")]
    health_probes_enabled: bool,
    #[serde(default)]
    probe_credentials: HashMap<String, Credential>,
    #[serde(default)]
    local_checks: BTreeSet<String>,
}

fn bind_default() -> std::net::IpAddr {
    std::net::Ipv4Addr::UNSPECIFIED.into()
}
fn port_default() -> u16 {
    8443
}
fn empty() -> Value {
    json!({})
}
fn enabled() -> bool {
    true
}

pub async fn serve(config: PathBuf) -> Result<(), Box<dyn std::error::Error>> {
    type Failure = Box<dyn std::error::Error>;
    let _ = rustls::crypto::ring::default_provider().install_default();
    let settings: Settings = serde_json::from_slice(&tokio::fs::read(&config).await?)
        .map_err(|_| "Invalid registry private configuration")?;
    let (shutdown, stopped) = watch::channel(false);
    let has_machines = settings
        .machine_enrollments
        .as_object()
        .is_some_and(|object| !object.is_empty());
    if has_machines && settings.presence_tls.is_none() {
        return Err("Machine enrollments require collector identity settings".into());
    }
    let enrolled = Enrolled::new(
        &settings.runtime_publishers,
        &settings.machine_enrollments,
        &settings.reader_token,
        &settings.writer_token,
        settings.presence_tls,
    )?;
    let catalog = blocking(move || {
        Ok(Catalog::open(&settings.database)?.with_local_checks(settings.local_checks))
    })
    .await?;
    let state = RegistryHttp::new(
        catalog.clone(),
        &settings.reader_token,
        &settings.writer_token,
        stopped.clone(),
    )?
    .with_enrollment(enrolled);
    let tls = RustlsConfig::from_pem_file(&settings.cert, &settings.key).await?;
    let address = SocketAddr::from((settings.bind, settings.port));
    let handle = Handle::new();
    let app = state.router();
    let probing = settings
        .health_probes_enabled
        .then(|| tokio::spawn(monitor(state, catalog, settings.probe_credentials, stopped)));
    let mut server = std::pin::pin!(
        bind_rustls(address, tls)
            .handle(handle.clone())
            .serve(app.into_make_service())
    );
    let mut sigterm = signal::unix::signal(signal::unix::SignalKind::terminate())?;
    let signal = async {
        tokio::select! {
            _ = signal::ctrl_c() => {},
            _ = sigterm.recv() => {},
        }
    };
    let result: Result<(), Failure> = tokio::select! {
        response = &mut server => response.map_err(Into::into),
        _ = signal => {
            shutdown.send_replace(true);
            handle.graceful_shutdown(Some(Duration::from_secs(5)));
            server.await.map_err(Into::into)
        }
    };
    shutdown.send_replace(true);
    if let Some(task) = probing {
        task.abort();
    }
    result
}
