//! Exercise the self-hosted shell updater against a loopback HTTP server.
#![cfg(unix)]

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::Output;
use std::time::Duration;

use tempfile::TempDir;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;
use tokio::process::Command;

const OLD_BINARY: &str = r#"#!/bin/sh
case "$1" in
  --version) echo 'karma-team 0.1.23' ;;
  status) printf '%s\n' "$TEST_STATUS_JSON" ;;
  restart) echo restart >> "$TEST_LOG" ;;
esac
"#;
const NEW_BINARY: &str = r#"#!/bin/sh
case "$1" in
  --version) echo 'karma-team 0.1.24' ;;
  status) printf '%s\n' "${TEST_UPDATED_STATUS_JSON:-$TEST_STATUS_JSON}" ;;
  restart) echo restart >> "$TEST_LOG" ;;
esac
"#;
const IDLE_STATUS: &str = r#"{"service":"active","daemon":{"runners":[{"status":"idle","current_run":null,"approvals_pending":0}]}}"#;

struct Installation {
    root: TempDir,
    repo: PathBuf,
    home: PathBuf,
    destination: PathBuf,
    log: PathBuf,
}

fn executable(path: &Path, body: &str) {
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, body).unwrap();
    fs::set_permissions(path, fs::Permissions::from_mode(0o755)).unwrap();
}

impl Installation {
    fn new() -> Self {
        let root = tempfile::tempdir().unwrap();
        let repo = root.path().join("source tree");
        let home = root.path().join("home");
        let destination = root.path().join("install bin");
        let log = root.path().join("commands.log");
        fs::create_dir_all(repo.join("runner")).unwrap();
        fs::create_dir_all(&home).unwrap();
        fs::create_dir_all(&destination).unwrap();
        fs::create_dir_all(root.path().join("downloads")).unwrap();
        fs::write(repo.join("runner/update.sh"), include_str!("../update.sh")).unwrap();
        executable(&destination.join("karma-team"), OLD_BINARY);
        executable(
            &root.path().join("tools/uname"),
            "#!/bin/sh\ncase \"$1\" in -s) echo Linux ;; -m) echo x86_64 ;; esac\n",
        );
        executable(
            &root.path().join("tools/ldd"),
            "#!/bin/sh\necho 'GNU libc'\n",
        );
        Self {
            root,
            repo,
            home,
            destination,
            log,
        }
    }

    fn command(&self) -> Command {
        let mut command = Command::new("sh");
        let path = format!(
            "{}:{}",
            self.root.path().join("tools").display(),
            std::env::var("PATH").unwrap()
        );
        command
            .arg(self.repo.join("runner/update.sh"))
            .current_dir(self.root.path())
            .env("PATH", path)
            .env("HOME", &self.home)
            .env("KARMA_TEAM_INSTALL_DIR", &self.destination)
            .env("KARMA_TEAM_API_URL", "http://127.0.0.1:0")
            .env("TMPDIR", self.root.path().join("downloads"))
            .env("TEST_LOG", &self.log)
            .env("TEST_STATUS_JSON", IDLE_STATUS)
            .env_remove("TEST_UPDATED_STATUS_JSON")
            .env("NO_PROXY", "127.0.0.1")
            .env("no_proxy", "127.0.0.1")
            .env_remove("KARMA_TEAM_BIN_URL")
            .env_remove("SUDO_USER")
            .env_remove("DOAS_USER")
            .kill_on_drop(true);
        command
    }
}

async fn run(command: &mut Command) -> Output {
    tokio::time::timeout(Duration::from_secs(15), command.output())
        .await
        .expect("updater timed out")
        .expect("launch updater")
}

fn assert_success(output: &Output) {
    assert!(
        output.status.success(),
        "stdout: {}\nstderr: {}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

async fn serve_path(status: &str, body: &str, path: &str) -> (String, tokio::task::JoinHandle<()>) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}{path}", listener.local_addr().unwrap());
    let request_prefix = format!("GET {path}");
    let response = format!(
        "HTTP/1.1 {status}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    );
    let task = tokio::spawn(async move {
        let (mut socket, _) = tokio::time::timeout(Duration::from_secs(10), listener.accept())
            .await
            .expect("updater never requested the binary")
            .unwrap();
        let mut request = Vec::new();
        while !request.ends_with(b"\r\n\r\n") {
            let byte = socket.read_u8().await.unwrap();
            request.push(byte);
            assert!(request.len() < 8192);
        }
        assert!(request.starts_with(request_prefix.as_bytes()));
        socket.write_all(response.as_bytes()).await.unwrap();
    });
    (url, task)
}

#[tokio::test]
async fn update_replaces_binary_and_restarts_active_service() {
    let install = Installation::new();
    let path = "/api/cli/bin/x86_64-unknown-linux-gnu/";
    let (url, server) = serve_path("200 OK", NEW_BINARY, path).await;
    let api_url = url.strip_suffix(path).unwrap();
    let output = run(install.command().env("KARMA_TEAM_API_URL", api_url)).await;
    server.await.unwrap();
    assert_success(&output);
    assert_eq!(
        fs::read_to_string(install.destination.join("karma-team")).unwrap(),
        NEW_BINARY
    );
    assert_eq!(fs::read_to_string(install.log).unwrap(), "restart\n");
}

#[tokio::test]
async fn check_and_no_restart_preserve_expected_state() {
    let install = Installation::new();
    let path = "/api/cli/bin/x86_64-unknown-linux-gnu/";
    let (url, server) = serve_path("200 OK", NEW_BINARY, path).await;
    let api_url = url.strip_suffix(path).unwrap();
    let output = run(install
        .command()
        .args(["--check"])
        .env("KARMA_TEAM_API_URL", api_url))
    .await;
    server.await.unwrap();
    assert_success(&output);
    assert_eq!(
        fs::read_to_string(install.destination.join("karma-team")).unwrap(),
        OLD_BINARY
    );
    assert!(!install.log.exists());

    let (url, server) = serve_path("200 OK", NEW_BINARY, path).await;
    let api_url = url.strip_suffix(path).unwrap();
    let output = run(install
        .command()
        .args(["--no-restart"])
        .env("KARMA_TEAM_API_URL", api_url))
    .await;
    server.await.unwrap();
    assert_success(&output);
    assert_eq!(
        fs::read_to_string(install.destination.join("karma-team")).unwrap(),
        NEW_BINARY
    );
    assert!(!install.log.exists());
}

#[tokio::test]
async fn busy_or_unknown_daemon_is_updated_without_restart() {
    for status in [
        r#"{"service":"active","daemon":{"runners":[{"status":"busy","current_run":null,"approvals_pending":0}]}}"#,
        r#"{"service":"active","daemon":{"runners":[{"status":"idle","current_run":{"run_id":"running"},"approvals_pending":0}]}}"#,
        r#"{"service":"active","daemon":{"runners":[{"status":"idle","current_run":null,"approvals_pending":1}]}}"#,
        r#"{"service":"active","daemon":{"runners":[{"status":"idle","current_run":null,"approvals_pending":0},{"status":"busy","current_run":null,"approvals_pending":0}]}}"#,
        r#"{"service":"active","daemon":{"runners":[{"status":"idle"}]}}"#,
        r#"{"service":"active","daemon":null}"#,
        r#"{"service":"unknown","daemon":{"runners":[]}}"#,
        "invalid JSON",
    ] {
        let install = Installation::new();
        let path = "/api/cli/bin/x86_64-unknown-linux-gnu/";
        let (url, server) = serve_path("200 OK", NEW_BINARY, path).await;
        let output = run(install
            .command()
            .env("KARMA_TEAM_API_URL", url.strip_suffix(path).unwrap())
            .env("TEST_STATUS_JSON", status))
        .await;
        server.await.unwrap();
        assert_success(&output);
        assert_eq!(
            fs::read_to_string(install.destination.join("karma-team")).unwrap(),
            NEW_BINARY
        );
        assert!(!install.log.exists(), "must not restart for {status}");
        assert!(String::from_utf8_lossy(&output.stdout).contains("restart deferred"));
    }
}

#[tokio::test]
async fn work_arriving_during_update_defers_restart() {
    let install = Installation::new();
    let path = "/api/cli/bin/x86_64-unknown-linux-gnu/";
    let (url, server) = serve_path("200 OK", NEW_BINARY, path).await;
    let output = run(install
        .command()
        .env("KARMA_TEAM_API_URL", url.strip_suffix(path).unwrap())
        .env(
            "TEST_UPDATED_STATUS_JSON",
            IDLE_STATUS.replace("idle", "busy"),
        ))
    .await;
    server.await.unwrap();
    assert_success(&output);
    assert!(!install.log.exists());
    assert!(String::from_utf8_lossy(&output.stdout).contains("restart deferred"));
}

#[tokio::test]
async fn identical_binary_skips_restart_unless_forced() {
    let install = Installation::new();
    let path = "/api/cli/bin/x86_64-unknown-linux-gnu/";
    for force in [false, true] {
        let (url, server) = serve_path("200 OK", OLD_BINARY, path).await;
        let mut command = install.command();
        command.env("KARMA_TEAM_API_URL", url.strip_suffix(path).unwrap());
        if force {
            command.arg("--force");
        }
        let output = run(&mut command).await;
        server.await.unwrap();
        assert_success(&output);
        assert_eq!(install.log.exists(), force);
    }
}

#[tokio::test]
async fn failed_download_preserves_old_binary() {
    let install = Installation::new();
    let path = "/api/cli/bin/x86_64-unknown-linux-gnu/";
    let (url, server) = serve_path("404 Not Found", "missing", path).await;
    let api_url = url.strip_suffix(path).unwrap();
    let output = run(install.command().env("KARMA_TEAM_API_URL", api_url)).await;
    server.await.unwrap();
    assert!(!output.status.success());
    assert_eq!(
        fs::read_to_string(install.destination.join("karma-team")).unwrap(),
        OLD_BINARY
    );
}

#[tokio::test]
async fn invalid_download_preserves_old_binary() {
    let install = Installation::new();
    let path = "/api/cli/bin/x86_64-unknown-linux-gnu/";
    let (url, server) = serve_path("200 OK", "not an executable", path).await;
    let api_url = url.strip_suffix(path).unwrap();
    let output = run(install.command().env("KARMA_TEAM_API_URL", api_url)).await;
    server.await.unwrap();
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("not a valid karma-team executable"));
    assert_eq!(
        fs::read_to_string(install.destination.join("karma-team")).unwrap(),
        OLD_BINARY
    );
    assert!(!install.log.exists());
}

#[tokio::test]
async fn root_cannot_update_another_users_installation() {
    let install = Installation::new();
    executable(
        &install.root.path().join("tools/id"),
        "#!/bin/sh\nif [ \"$1\" = \"-u\" ]; then echo 0; else exec /usr/bin/id \"$@\"; fi\n",
    );
    let output = run(install.command().env("SUDO_USER", "laiye")).await;
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("Refusing to update"));
    assert_eq!(
        fs::read_to_string(install.destination.join("karma-team")).unwrap(),
        OLD_BINARY
    );
}
