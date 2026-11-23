use std::{
    error::Error,
    fs::File,
    io::{self, Seek},
    net::SocketAddr,
    path::{Path, PathBuf},
    pin::Pin,
    sync::Arc,
    task::{Context, Poll},
    time::Duration,
};

use axum::{
    Router,
    body::Body,
    extract::{Request, State},
    http::{Method, StatusCode, header},
    response::{IntoResponse, Response},
    routing::any,
};
use http_body_util::BodyExt;
use serde::Deserialize;
use serde_json::{Value, json};
use tokio::{
    io::{AsyncRead, AsyncWriteExt, ReadBuf},
    sync::{OwnedSemaphorePermit, Semaphore},
    time::timeout,
};
use tokio_util::io::ReaderStream;

use crate::{
    http::tls,
    restic_scope::archive_identity,
    storage::{StorageSettings, checksum, readback, store, temporary},
    task_client::TaskClient,
    tokens::{hex, valid_token},
    workspace::MAX_ARCHIVE,
};

#[derive(Deserialize)]
struct Settings {
    #[serde(flatten)]
    storage: StorageSettings,
    controller_endpoint: String,
    ca_file: PathBuf,
    worker_token_file: PathBuf,
    listen: std::net::IpAddr,
    port: u16,
    tls: Value,
    #[serde(default)]
    accept_uploads: bool,
}

struct Worker {
    settings: Settings,
    client: TaskClient,
    capacity: Arc<Semaphore>,
    binaries: PathBuf,
}

#[derive(Deserialize)]
struct ReadGrant {
    snapshot: String,
    path: String,
    sha256: String,
}

#[derive(Deserialize)]
struct UploadGrant {
    id: String,
    path: String,
    sha256: String,
    size: u64,
}

fn blocked() -> io::Error {
    io::Error::other("Unexpected checkpoint identity.")
}

impl Worker {
    fn retrieve(&self, grant: ReadGrant) -> Result<(File, u64), Box<dyn Error>> {
        let mut archive = temporary()?;
        readback(
            &self.settings.storage.restic_command,
            &grant.snapshot,
            &grant.path,
            &mut archive,
            self.settings.storage.max_archive_bytes,
            Duration::from_secs(120),
        )?;
        let (actual, size) = checksum(&mut archive, self.settings.storage.max_archive_bytes)?;
        if !hex(&grant.sha256, 64) || grant.sha256 != actual {
            return Err(blocked().into());
        }
        Ok((archive, size))
    }

    fn upload(&self, grant: UploadGrant, mut archive: File) -> Result<Value, Box<dyn Error>> {
        archive.rewind()?;
        let storage = &self.settings.storage;
        let (actual, bytes) = checksum(&mut archive, storage.max_archive_bytes.min(MAX_ARCHIVE))?;
        if bytes != grant.size || actual != grant.sha256 {
            return Err(
                io::Error::other("Uploaded archive does not match its declared digest.").into(),
            );
        }
        let filename = grant.path.strip_prefix('/').ok_or_else(blocked)?;
        let command = storage
            .upload_command
            .as_ref()
            .unwrap_or(&storage.restic_command);
        Ok(store(
            command,
            &mut archive,
            filename,
            &grant.id,
            storage.max_archive_bytes,
            Duration::from_secs(65),
            &self.binaries,
        )?)
    }
}

fn error(status: u16, code: &'static str) -> Response {
    json_answer(status, json!({"code":code}))
}

fn json_answer(status: u16, body: Value) -> Response {
    (
        StatusCode::from_u16(status).unwrap_or(StatusCode::BAD_GATEWAY),
        [(header::CACHE_CONTROL, "no-store")],
        axum::Json(body),
    )
        .into_response()
}

struct ArchiveResponse {
    file: tokio::fs::File,
    _permit: OwnedSemaphorePermit,
}

impl AsyncRead for ArchiveResponse {
    fn poll_read(
        mut self: Pin<&mut Self>,
        context: &mut Context<'_>,
        buffer: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        Pin::new(&mut self.file).poll_read(context, buffer)
    }
}

async fn download(worker: Arc<Worker>, ticket: String, permit: OwnedSemaphorePermit) -> Response {
    let result = tokio::task::spawn_blocking(move || {
        let proof = worker
            .client
            .request("/v1/redeem-checkpoint", Some(&json!({"ticket":ticket})))
            .map_err(|error| {
                (
                    if error.status == Some(404) { 404 } else { 502 },
                    if error.status == Some(404) {
                        "ticket_unavailable"
                    } else {
                        "recovery_unavailable"
                    },
                )
            })?;
        let proof = serde_json::from_value(proof).map_err(|_| (502, "checkpoint_unavailable"))?;
        worker
            .retrieve(proof)
            .map_err(|_| (502, "checkpoint_unavailable"))
    })
    .await;
    match result {
        Ok(Ok((file, size))) => {
            let source = ArchiveResponse {
                file: tokio::fs::File::from_std(file),
                _permit: permit,
            };
            (
                [
                    (header::CACHE_CONTROL, "no-store".to_owned()),
                    (header::CONTENT_TYPE, "application/gzip".to_owned()),
                    (header::CONTENT_LENGTH, size.to_string()),
                ],
                Body::from_stream(ReaderStream::new(source)),
            )
                .into_response()
        }
        Ok(Err((status, code))) => {
            if code == "checkpoint_unavailable" {
                tracing::warn!("Stored checkpoint could not be independently verified");
            }
            error(status, code)
        }
        Err(_) => error(502, "checkpoint_unavailable"),
    }
}

async fn receive(mut body: Body, length: u64) -> io::Result<File> {
    let mut output = tokio::fs::File::from_std(temporary().map_err(io::Error::other)?);
    let mut total = 0_u64;
    let operation = async {
        while let Some(frame) = timeout(Duration::from_secs(8), body.frame()).await? {
            let frame = frame.map_err(io::Error::other)?;
            if let Ok(chunk) = frame.into_data() {
                total += chunk.len() as u64;
                if total > length {
                    return Err(io::Error::other(
                        "Archive upload exceeded its declared length.",
                    ));
                }
                output.write_all(&chunk).await?;
            }
        }
        if total != length {
            return Err(io::Error::other("Archive upload ended early."));
        }
        output.flush().await?;
        Ok(())
    };
    timeout(Duration::from_secs(60), operation).await??;
    Ok(output.into_std().await)
}

async fn post(worker: Arc<Worker>, ticket: String, request: Request) -> Response {
    let length = request
        .headers()
        .get(header::CONTENT_LENGTH)
        .map_or(Some(0), |value| {
            value
                .to_str()
                .ok()
                .and_then(|length| length.parse::<u64>().ok())
        });
    let Some(length) = length else {
        return error(400, "invalid_length");
    };
    let redeemed = {
        let worker = worker.clone();
        let ticket = ticket.clone();
        tokio::task::spawn_blocking(move || {
            worker
                .client
                .request("/v1/redeem-upload", Some(&json!({"ticket":ticket})))
        })
        .await
    };
    let grant = match redeemed {
        Ok(Ok(grant)) => serde_json::from_value::<UploadGrant>(grant),
        Ok(Err(e)) => return error(e.status.unwrap_or(502), "upload_grant_unavailable"),
        Err(_) => return error(502, "upload_grant_unavailable"),
    };

    let received = match grant {
        Ok(grant)
            if grant.size == length
                && (1..=worker.settings.storage.max_archive_bytes.min(MAX_ARCHIVE))
                    .contains(&length)
                && grant
                    .path
                    .strip_prefix('/')
                    .and_then(archive_identity)
                    .is_some_and(|(_, id)| id == Some(&grant.id))
                && hex(&grant.sha256, 64) =>
        {
            match receive(request.into_body(), length).await {
                Ok(archive) => {
                    let worker = worker.clone();
                    match tokio::task::spawn_blocking(move || {
                        worker.upload(grant, archive).map_err(|e| e.to_string())
                    })
                    .await
                    {
                        Ok(result) => result,
                        Err(_) => Err("Workspace upload worker ended early.".to_owned()),
                    }
                }
                Err(_) => {
                    Err("Archive upload ended early or exceeded its receive deadline.".to_owned())
                }
            }
        }
        _ => Err("Archive identity or size does not match the grant.".to_owned()),
    };

    let (verified, payload) = match received {
        Ok(mut receipt) => {
            receipt["ticket"] = json!(ticket);
            (true, receipt)
        }
        Err(reason) => (
            false,
            json!({"ticket":ticket,"error":reason.chars().take(240).collect::<String>()}),
        ),
    };
    let acknowledgment = tokio::task::spawn_blocking(move || {
        worker.client.request("/v1/complete-upload", Some(&payload))
    })
    .await;
    match acknowledgment {
        Ok(Ok(receipt)) if verified => json_answer(200, receipt),
        Ok(Ok(_)) => error(502, "checkpoint_not_acknowledged"),
        _ => error(502, "checkpoint_acknowledgment_unavailable"),
    }
}

async fn route(State(worker): State<Arc<Worker>>, request: Request) -> Response {
    if request
        .uri()
        .path_and_query()
        .is_none_or(|path| path.as_str() != "/v1/checkpoint")
        || !matches!(*request.method(), Method::GET | Method::POST)
        || *request.method() == Method::POST && !worker.settings.accept_uploads
    {
        return error(404, "unknown_route");
    }
    let ticket = request
        .headers()
        .get(header::AUTHORIZATION)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.strip_prefix("Bearer "))
        .filter(|secret| valid_token(secret, 43, 43));
    let Some(ticket) = ticket.map(str::to_owned) else {
        return error(401, "invalid_ticket");
    };
    let Ok(permit) = worker.capacity.clone().try_acquire_owned() else {
        return error(503, "worker_busy");
    };
    if *request.method() == Method::POST {
        let _permit = permit;
        post(worker, ticket, request).await
    } else {
        download(worker, ticket, permit).await
    }
}

pub async fn run(config: &Path) -> Result<(), Box<dyn Error>> {
    let settings: Settings = serde_json::from_reader(File::open(config)?)?;
    let certificate =
        tls(&settings.tls)?.ok_or_else(|| io::Error::other("Checkpoint recovery requires TLS."))?;
    let listener = std::net::TcpListener::bind(SocketAddr::new(settings.listen, settings.port))?;
    let client = TaskClient::new(
        &settings.controller_endpoint,
        settings.ca_file.clone(),
        settings.worker_token_file.clone(),
    )?;
    let binaries = std::env::current_exe()?
        .parent()
        .ok_or_else(blocked)?
        .to_path_buf();
    let worker = Arc::new(Worker {
        settings,
        client,
        capacity: Arc::new(Semaphore::new(2)),
        binaries,
    });
    let app = Router::new().fallback(any(route)).with_state(worker);
    println!("Checkpoint recovery listening with TLS");
    lab_http_transport::tls_with_deadlines(
        axum_server::from_tcp(listener)?,
        certificate,
        Duration::from_secs(8),
    )
    .serve(app.into_make_service())
    .await?;
    Ok(())
}
