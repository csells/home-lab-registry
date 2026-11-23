use std::{
    net::{Ipv4Addr, SocketAddr},
    path::{Path, PathBuf},
    process::Stdio,
    sync::Arc,
    time::Duration,
};

use axum::{
    Json, Router,
    extract::State,
    http::{HeaderMap, StatusCode, header},
    response::{IntoResponse, Response},
    routing::{get, post},
};
use clap::Parser;
use serde_json::{Value, json};
use tokio::sync::Mutex;

const PAGE: &str = include_str!("../../../review/index.html");

#[derive(Parser)]
#[command(about = "Serve redacted backup qualification results on loopback.")]
struct Options {
    #[arg(long)]
    report: PathBuf,
    #[arg(long, default_value_t = 9084)]
    port: u16,
    #[arg(long)]
    check: Option<PathBuf>,
    #[arg(long, default_value = "")]
    origin: String,
    #[arg(long)]
    script: Option<PathBuf>,
}

struct Review {
    options: Options,
    status: Mutex<Value>,
    script: PathBuf,
}

fn answer(content_type: &'static str, body: impl IntoResponse) -> Response {
    (
        [(header::CONTENT_TYPE,content_type),
        (header::CACHE_CONTROL,"no-store"),
        (header::X_CONTENT_TYPE_OPTIONS,"nosniff"),
        (header::CONTENT_SECURITY_POLICY,"default-src 'none'; style-src 'unsafe-inline'; script-src 'self'; connect-src 'self'")],
        body,
    ).into_response()
}

fn escape(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&#39;")
}

fn report_text(value: &Value) -> String {
    escape(value.as_str().unwrap_or_default())
}

fn escape_list(value: &Value) -> String {
    value
        .as_array()
        .into_iter()
        .flatten()
        .map(|line| format!("<li>{}</li>", report_text(line)))
        .collect()
}

fn render(report: &Value) -> String {
    let rows = report["sources"]
        .as_array()
        .into_iter()
        .flatten()
        .map(|row| {
            format!(
                "<tr><td>{}</td><td>{}</td><td>{}</td></tr>",
                report_text(&row["source"]),
                report_text(&row["result"]),
                report_text(&row["recovery"]),
            )
        })
        .collect::<String>();
    // Substitute each placeholder once, so report text cannot start a second substitution.
    let mut page = String::new();
    let mut remaining = PAGE;
    for (key, content) in [
        ("{{UPDATED}}", report_text(&report["checked_at"])),
        ("{{ROWS}}", rows),
        ("{{CHECKS}}", escape_list(&report["checks"])),
        ("{{REMAINING}}", escape_list(&report["remaining"])),
    ] {
        if let Some((prefix, suffix)) = remaining.split_once(key) {
            page.push_str(prefix);
            page.push_str(&content);
            remaining = suffix;
        }
    }
    page.push_str(remaining);
    page
}

async fn data(review: &Review) -> Result<Value, StatusCode> {
    let raw = tokio::fs::read(&review.options.report)
        .await
        .map_err(|_| StatusCode::SERVICE_UNAVAILABLE)?;
    let mut value: Value =
        serde_json::from_slice(&raw).map_err(|_| StatusCode::SERVICE_UNAVAILABLE)?;
    let report = value
        .as_object_mut()
        .ok_or(StatusCode::SERVICE_UNAVAILABLE)?;
    report.insert("last_run".to_owned(), review.status.lock().await.clone());
    Ok(value)
}

async fn html(State(review): State<Arc<Review>>) -> Result<Response, StatusCode> {
    let report = data(&review).await?;
    Ok(answer("text/html; charset=utf-8", render(&report)))
}

async fn json_report(State(review): State<Arc<Review>>) -> Result<Response, StatusCode> {
    Ok(answer("application/json", Json(data(&review).await?)))
}

async fn javascript(State(review): State<Arc<Review>>) -> Result<Response, StatusCode> {
    let bytes = tokio::fs::read(&review.script)
        .await
        .map_err(|_| StatusCode::NOT_FOUND)?;
    Ok(answer("text/javascript; charset=utf-8", bytes))
}

async fn trigger(State(review): State<Arc<Review>>, headers: HeaderMap) -> StatusCode {
    let Some(command) = review.options.check.clone() else {
        return StatusCode::NOT_FOUND;
    };
    let allowed = format!("http://127.0.0.1:{}", review.options.port);
    if headers
        .get(header::ORIGIN)
        .and_then(|origin| origin.to_str().ok())
        .is_none_or(|origin| {
            origin != allowed
                && (review.options.origin.is_empty() || origin != review.options.origin)
        })
    {
        return StatusCode::FORBIDDEN;
    }
    {
        let mut state = review.status.lock().await;
        if state["state"] == "running" {
            return StatusCode::CONFLICT;
        }
        *state = json!({"state":"running"});
    }
    tokio::spawn(async move {
        let mut job = tokio::process::Command::new(command);
        job.stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .kill_on_drop(true);
        let state = match tokio::time::timeout(Duration::from_secs(300), job.status()).await {
            Ok(Ok(status)) if status.success() => "passed",
            Ok(_) => "failed",
            Err(_) => "timed-out",
        };
        let checked = time::OffsetDateTime::now_utc()
            .format(&time::format_description::well_known::Rfc3339)
            .ok();
        *review.status.lock().await = json!({"state":state,"checked_at":checked});
    });
    StatusCode::ACCEPTED
}

#[tokio::main(worker_threads = 2)]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let options = Options::parse();
    let script = options.script.clone().unwrap_or(
        std::env::current_exe()?
            .parent()
            .unwrap_or(Path::new("."))
            .join("backup-review.js"),
    );
    let listener =
        tokio::net::TcpListener::bind(SocketAddr::new(Ipv4Addr::LOCALHOST.into(), options.port))
            .await?;
    let review = Arc::new(Review {
        options,
        status: Mutex::new(json!({"state":"not-requested"})),
        script,
    });
    let router = Router::new()
        .route("/", get(html))
        .route("/report.json", get(json_report))
        .route("/healthz", get(|| async { answer("text/plain", "ok") }))
        .route("/checks.js", get(javascript))
        .route("/checks", post(trigger))
        .with_state(review);
    axum::serve(listener, router).await?;
    Ok(())
}
