use std::{net::Ipv4Addr, path::PathBuf, time::Duration};

use lab_dashboard::{Dashboard, Registry};
use tokio::{net::TcpListener, signal, sync::watch};

#[tokio::main(worker_threads = 2)]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    tracing_subscriber::fmt()
        .without_time()
        .with_target(false)
        .init();
    let mut args = std::env::args_os().skip(1);
    let assets = args
        .next()
        .map(PathBuf::from)
        .unwrap_or(std::env::current_dir()?);
    let port: u16 = args
        .next()
        .map(|value| value.into_string().map_err(|_| "Invalid dashboard port"))
        .transpose()?
        .map(|value| value.parse())
        .transpose()?
        .unwrap_or(8089);
    if args.next().is_some() || port == 0 {
        return Err("Usage: lab-dashboard [dashboard-directory] [loopback-port]".into());
    }
    let private = assets.join("private");
    let scope = std::env::var("LAB_DASHBOARD_ACCESS_TARGET").ok();
    let (shutdown, stopped) = watch::channel(false);
    let registry = Registry::from_config(&private.join("reader.json")).await?;
    let dashboard = Dashboard::from_directory(assets, private, scope, stopped.clone()).await?;
    let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, port)).await?;
    let app = dashboard.router();
    let mut runtime = tokio::spawn(async move { dashboard.maintain(registry).await });
    let mut closed = stopped.clone();
    let mut server = std::pin::pin!(
        axum::serve(listener, app)
            .with_graceful_shutdown(async move {
                let _ = closed.wait_for(|value| *value).await;
            })
            .into_future()
    );
    let mut terminate = signal::unix::signal(signal::unix::SignalKind::terminate())?;
    let stop = async {
        tokio::select! {
            _ = signal::ctrl_c() => {}
            _ = terminate.recv() => {}
        }
    };
    tokio::select! {
        result = &mut server => result?,
        result = &mut runtime => {
            shutdown.send_replace(true);
            result??;
            return Err("Dashboard inventory worker exited".into());
        }
        _ = stop => {
            shutdown.send_replace(true);
            match tokio::time::timeout(Duration::from_secs(5), &mut server).await {
                Ok(result) => result?,
                Err(_) => tracing::warn!("Dashboard connections did not close before shutdown deadline"),
            }
        }
    }
    runtime.abort();
    Ok(())
}
