use std::{
    io::{BufRead, BufReader, Read, Write},
    net::TcpListener,
    path::Path,
    process::{Command, Stdio},
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
    thread,
    time::Duration,
};

use lab_registry::client::{Method, RegistryClient};
use rustls::{
    ServerConfig, ServerConnection, StreamOwned,
    pki_types::{CertificateDer, PrivateKeyDer, pem::PemObject},
};
use serde_json::{Value, json};

fn certificate(root: &Path) {
    let output = Command::new("openssl")
        .args(["req", "-x509", "-newkey", "rsa:2048", "-nodes", "-keyout"])
        .arg(root.join("key.pem"))
        .arg("-out")
        .arg(root.join("cert.pem"))
        .args([
            "-days",
            "1",
            "-subj",
            "/CN=localhost",
            "-addext",
            "subjectAltName=DNS:localhost",
        ])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
}

enum Offer {
    Complete(usize),
    Truncated,
    Invalid,
    Denied,
    Redirect,
}

fn fixture(root: &Path, offers: Vec<Offer>) -> (String, Arc<AtomicUsize>) {
    let listener = TcpListener::bind(("127.0.0.1", 0)).unwrap();
    let url = format!(
        "https://localhost:{}",
        listener.local_addr().unwrap().port()
    );
    let certs = CertificateDer::pem_file_iter(root.join("cert.pem"))
        .unwrap()
        .collect::<Result<Vec<_>, _>>()
        .unwrap();
    let key = PrivateKeyDer::from_pem_file(root.join("key.pem")).unwrap();
    let tls = Arc::new(
        ServerConfig::builder_with_provider(Arc::new(rustls::crypto::ring::default_provider()))
            .with_safe_default_protocol_versions()
            .unwrap()
            .with_no_client_auth()
            .with_single_cert(certs, key)
            .unwrap(),
    );
    let hits = Arc::new(AtomicUsize::new(0));
    let count = hits.clone();
    thread::spawn(move || {
        for offer in offers {
            let (tcp, _) = listener.accept().unwrap();
            tcp.set_read_timeout(Some(Duration::from_secs(3))).unwrap();
            let conn = ServerConnection::new(tls.clone()).unwrap();
            let mut stream = BufReader::new(StreamOwned::new(conn, tcp));
            let mut request = String::new();
            let mut line = String::new();
            loop {
                if stream.read_line(&mut line).unwrap_or(0) == 0 {
                    break;
                }
                if line == "\r\n" {
                    break;
                }
                request.push_str(&line);
                line.clear();
            }
            if request.is_empty() {
                continue;
            }
            count.fetch_add(1, Ordering::Relaxed);
            if request.starts_with("GET /v1/services") {
                assert!(request.starts_with(
                    "GET /v1/services?host=node-1&status=healthy&include_retired=true "
                ));
            }

            assert!(
                request.contains(if request.starts_with("PUT ") {
                    "Authorization: Bearer writer-test-only"
                } else {
                    "Authorization: Bearer reader-test-only"
                }),
                "{request}"
            );
            if request.starts_with("PUT /write ") {
                assert!(request.contains("If-Match: 7"));
                let bytes: usize = request
                    .lines()
                    .find_map(|line| line.strip_prefix("Content-Length: "))
                    .unwrap()
                    .parse()
                    .unwrap();
                let mut body = vec![0; bytes];
                stream.read_exact(&mut body).unwrap();
                assert_eq!(
                    serde_json::from_slice::<Value>(&body).unwrap(),
                    json!({"note":"quotes \" slash \\ newline\nUnicode λ"})
                );
            }
            let (status, body, extra, declared) = match offer {
                Offer::Complete(n) => (
                    200,
                    json!({"ok":true,"payload":"x".repeat(n)}).to_string(),
                    "",
                    None,
                ),
                Offer::Truncated => (200, "{\"ok\":true}".to_owned(), "", Some(250_000)),
                Offer::Invalid => (200, "{broken".to_owned(), "", None),
                Offer::Denied => (403, "reader-test-only".to_owned(), "", None),
                Offer::Redirect => (
                    302,
                    String::new(),
                    "Location: https://localhost:1/wrong\r\n",
                    None,
                ),
            };
            let io = stream.get_mut();
            write!(io,"HTTP/1.1 {status} status\r\n{extra}Content-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", declared.unwrap_or(body.len())).unwrap();
            io.conn.send_close_notify();
            io.flush().unwrap();
        }
    });
    (url, hits)
}

fn client(root: &Path, url: &str) -> RegistryClient {
    RegistryClient::from_value(json!({
        "url":url,"ca_file":root.join("cert.pem"),
        "reader_token":"reader-test-only","writer_token":"writer-test-only",
    }))
    .unwrap()
}

#[test]
fn accepts_complete_large_json_and_retries_only_interrupted_reads() {
    let root = tempfile::tempdir().unwrap();
    certificate(root.path());
    let (url, hits) = fixture(
        root.path(),
        vec![Offer::Truncated, Offer::Complete(250_000)],
    );
    let response = client(root.path(), &url)
        .request(Method::Get, "/retry", None, None)
        .unwrap();
    assert_eq!(response["payload"].as_str().unwrap().len(), 250_000);
    assert_eq!(hits.load(Ordering::Relaxed), 2);

    let (url, hits) = fixture(
        root.path(),
        vec![Offer::Truncated, Offer::Truncated, Offer::Truncated],
    );
    assert!(
        client(root.path(), &url)
            .request(Method::Get, "/truncated", None, None)
            .is_err()
    );
    assert_eq!(hits.load(Ordering::Relaxed), 3);
}

#[test]
fn writes_keep_json_and_revision_and_are_never_replayed() {
    let root = tempfile::tempdir().unwrap();
    certificate(root.path());
    let (url, hits) = fixture(root.path(), vec![Offer::Complete(0), Offer::Truncated]);
    let registry = client(root.path(), &url);
    let payload = json!({"note":"quotes \" slash \\ newline\nUnicode λ"});
    assert!(
        registry
            .request(Method::Put, "/write", Some(&payload), Some(7))
            .is_ok()
    );
    assert!(
        registry
            .request(Method::Put, "/write", Some(&payload), Some(7))
            .is_err()
    );
    assert_eq!(hits.load(Ordering::Relaxed), 2);
}

#[test]
fn redirects_denials_bad_json_and_untrusted_roots_never_supply_a_result() {
    let root = tempfile::tempdir().unwrap();
    certificate(root.path());
    let (url, hits) = fixture(
        root.path(),
        vec![Offer::Redirect, Offer::Denied, Offer::Invalid],
    );
    let registry = client(root.path(), &url);
    for (route, reason) in [
        ("/redirect", "HTTP 302"),
        ("/deny", "HTTP 403"),
        ("/invalid", "valid JSON"),
    ] {
        let problem = registry
            .request(Method::Get, route, None, None)
            .unwrap_err()
            .to_string();
        assert!(problem.contains(reason), "{problem}");
        assert!(!problem.contains("reader-test-only"));
    }
    assert_eq!(hits.load(Ordering::Relaxed), 3);
    assert!(
        RegistryClient::from_value(
            json!({"url":"http://localhost","ca_file":root.path().join("cert.pem")})
        )
        .is_err()
    );

    let wrong = tempfile::tempdir().unwrap();
    certificate(wrong.path());
    let (url, hits) = fixture(root.path(), vec![Offer::Complete(0)]);
    assert!(
        client(wrong.path(), &url)
            .request(Method::Get, "/tls", None, None)
            .is_err()
    );
    assert_eq!(hits.load(Ordering::Relaxed), 0);
}

#[test]
fn the_cli_uses_private_configuration_and_existing_filters() {
    use std::os::unix::fs::PermissionsExt;
    let root = tempfile::tempdir().unwrap();
    certificate(root.path());
    let (url, hits) = fixture(root.path(), vec![Offer::Complete(0)]);
    let config = root.path().join("client.json");
    std::fs::write(
        &config,
        serde_json::to_vec(&json!({"url":url,"ca_file":root.path().join("cert.pem"),
        "reader_token":"reader-test-only"}))
        .unwrap(),
    )
    .unwrap();
    std::fs::set_permissions(&config, std::fs::Permissions::from_mode(0o644)).unwrap();
    let mut call = Command::new(env!("CARGO_BIN_EXE_lab-registry-client"));
    call.arg("--config")
        .arg(&config)
        .args([
            "list",
            "--include-retired",
            "--host",
            "node-1",
            "--status",
            "healthy",
        ])
        .stdin(Stdio::null());
    let denied = call.output().unwrap();
    assert!(!denied.status.success());
    assert!(String::from_utf8_lossy(&denied.stderr).contains("mode 0600"));
    assert_eq!(hits.load(Ordering::Relaxed), 0);
    std::fs::set_permissions(&config, std::fs::Permissions::from_mode(0o600)).unwrap();
    let response = call.output().unwrap();
    assert!(
        response.status.success(),
        "{}",
        String::from_utf8_lossy(&response.stderr)
    );
    assert_eq!(
        serde_json::from_slice::<Value>(&response.stdout).unwrap()["ok"],
        true
    );
    assert_eq!(hits.load(Ordering::Relaxed), 1);
}

#[test]
fn server_accepts_hostname_bind_and_standard_cli_options() {
    let help = Command::new(env!("CARGO_BIN_EXE_lab-registry"))
        .arg("--help")
        .output()
        .unwrap();
    assert!(help.status.success());
    assert!(String::from_utf8_lossy(&help.stdout).contains("--config"));

    let root = tempfile::tempdir().unwrap();
    certificate(root.path());
    let port = std::net::TcpListener::bind(("localhost", 0))
        .unwrap()
        .local_addr()
        .unwrap()
        .port();
    let config = root.path().join("server.json");
    std::fs::write(
        &config,
        json!({
            "bind": "localhost", "port": port, "database": root.path().join("catalog.sqlite"),
            "cert": root.path().join("cert.pem"), "key": root.path().join("key.pem"),
            "reader_token": "test-reader", "writer_token": "test-writer",
            "health_probes_enabled": false
        })
        .to_string(),
    )
    .unwrap();
    let mut registry = Command::new(env!("CARGO_BIN_EXE_lab-registry"))
        .arg(format!("--config={}", config.display()))
        .spawn()
        .unwrap();
    let connected = (0..100).any(|_| {
        if let Ok(_connection) = std::net::TcpStream::connect(("localhost", port)) {
            return true;
        }
        if registry.try_wait().unwrap().is_some() {
            return false;
        }
        std::thread::sleep(std::time::Duration::from_millis(20));
        false
    });
    let version = Command::new("curl").arg("--version").output().unwrap();
    let http2 = String::from_utf8_lossy(&version.stdout).contains("HTTP2");
    let probe = Command::new("curl")
        .args(["--silent", "--show-error", "--fail", "--max-time", "5"])
        .arg(if http2 { "--http2" } else { "--http1.1" })
        .arg("--cacert")
        .arg(root.path().join("cert.pem"))
        .args([
            "--output",
            if cfg!(windows) { "NUL" } else { "/dev/null" },
            "--write-out",
            "%{http_version}",
        ])
        .arg(format!("https://localhost:{port}/healthz"))
        .output()
        .unwrap();
    let _ = registry.kill();
    registry.wait().unwrap();
    assert!(connected, "registry did not listen on localhost");
    assert!(
        probe.status.success(),
        "{}",
        String::from_utf8_lossy(&probe.stderr)
    );
    assert_eq!(
        probe.stdout,
        if http2 {
            b"2".as_slice()
        } else {
            b"1.1".as_slice()
        }
    );
}
