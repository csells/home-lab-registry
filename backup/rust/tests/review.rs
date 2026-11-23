use std::{
    fs,
    io::{BufRead, BufReader},
    process::{Child, Command, Stdio},
    time::Duration,
};

struct Running(Child);
impl Drop for Running {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

#[test]
fn missing_browser_bundle_fails_startup_instead_of_serving_an_inert_page() {
    let root = tempfile::tempdir().unwrap();
    let mut review = Running(
        Command::new(env!("CARGO_BIN_EXE_lab-backup-review"))
            .arg("--report")
            .arg(root.path().join("report.json"))
            .arg("--script")
            .arg(root.path().join("missing.js"))
            .args(["--port", "0"])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .unwrap(),
    );
    let status = (0..100).find_map(|_| {
        std::thread::sleep(Duration::from_millis(10));
        review.0.try_wait().unwrap()
    });
    assert!(status.is_some_and(|status| !status.success()));
}

#[test]
fn the_browser_can_run_a_readable_qualification_with_an_explicit_interpreter() {
    let root = tempfile::tempdir().unwrap();
    let check = root.path().join("qualified");
    let report = root.path().join("report.json");
    let script = root.path().join("browser.js");
    fs::write(&check, "exit 0\n").unwrap();
    fs::write(&report, "{}").unwrap();
    fs::write(&script, "document.title = 'qualified';").unwrap();
    let mut review = Running(
        Command::new(env!("CARGO_BIN_EXE_lab-backup-review"))
            .arg("--report")
            .arg(report)
            .arg("--script")
            .arg(script)
            .arg("--check")
            .arg(check)
            .args(["--check-interpreter", "/bin/sh", "--port", "0"])
            .stdout(Stdio::piped())
            .spawn()
            .unwrap(),
    );
    let mut announced = String::new();
    BufReader::new(review.0.stdout.take().unwrap())
        .read_line(&mut announced)
        .unwrap();
    let origin = announced
        .trim()
        .strip_prefix("Backup review listening at ")
        .expect("bound review address");
    let request = |method, path| {
        Command::new("curl")
            .args([
                "--silent",
                "--show-error",
                "--max-time",
                "1",
                "--request",
                method,
                "--header",
                &format!("Origin: {origin}"),
                &format!("{origin}{path}"),
            ])
            .stderr(Stdio::null())
            .output()
            .unwrap()
    };
    let ready = (0..60).any(|_| {
        if request("GET", "/healthz").status.success() {
            return true;
        }
        if review.0.try_wait().unwrap().is_some() {
            return false;
        }
        std::thread::sleep(Duration::from_millis(20));
        false
    });
    assert!(ready, "Review process did not become healthy");
    assert_eq!(
        request("GET", "/checks.js").stdout,
        b"document.title = 'qualified';"
    );
    assert!(request("POST", "/checks").status.success());
    let passed = (0..60).any(|_| {
        let body = request("GET", "/report.json").stdout;
        if String::from_utf8_lossy(&body).contains("\"passed\"") {
            return true;
        }
        std::thread::sleep(Duration::from_millis(20));
        false
    });
    assert!(passed, "Qualification result did not reach the review");
}
