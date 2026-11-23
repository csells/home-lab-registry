use std::{
    io::{Write, stdout},
    net::Ipv4Addr,
    path::PathBuf,
    sync::Arc,
};

use axum::{
    Router,
    extract::State,
    http::{StatusCode, Uri},
    response::sse::{Event, Sse},
    routing::get,
};
use lab_dashboard::{Dashboard, Snapshot};
use serde_json::Value;
use tokio::{net::TcpListener, sync::watch};

#[tokio::main(worker_threads = 2)]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let assets = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("..");
    let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).await?;
    let (_stop, listening) = watch::channel(false);
    let dashboard = Dashboard::new(
        assets,
        "Basic Zml4dHVyZTpmaXh0dXJl".parse()?,
        [],
        true,
        Snapshot::default(),
        listening,
    );
    let settings = std::env::args_os().nth(1);
    let stream_rows: Value = match settings {
        Some(path) => serde_json::from_reader(std::fs::File::open(path)?)?,
        None => Value::Null,
    };
    let fixtures = Router::new().route("/__fixtures/metrics", get(
        |State(rows): State<Arc<Value>>, uri: Uri| async move {
            let (_, key) = form_urlencoded::parse(uri.query().unwrap_or_default().as_bytes())
                .find(|(key, _)| key == "machine").ok_or(StatusCode::NOT_FOUND)?;
            let value = rows.get(key.as_ref()).ok_or(StatusCode::NOT_FOUND)?.to_string();
            let events = async_stream::stream! {
                yield Ok::<_,std::convert::Infallible>(Event::default().event("metrics").data(value));
                std::future::pending::<()>().await;
            };
            Ok::<_,StatusCode>(Sse::new(events))
        },
    )).with_state(Arc::new(stream_rows));
    let addr = listener.local_addr()?;
    println!("http://{addr}");
    stdout().flush()?;
    axum::serve(listener, dashboard.router().merge(fixtures)).await?;
    Ok(())
}
