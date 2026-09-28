//! Exercise the shell installer against local files and a loopback HTTP server.
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

const LOCAL_BINARY: &str = "#!/bin/sh\n# local fixture\nexit 0\n";
const REMOTE_BINARY: &str = "#!/bin/sh\n# downloaded fixture\nexit 0\n";

struct Installation {
    root: TempDir,
    repo: PathBuf,
    destination: PathBuf,
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
        let destination = root.path().join("install bin");
        fs::create_dir_all(repo.join("runner")).unwrap();
        fs::create_dir_all(root.path().join("downloads")).unwrap();
        fs::write(
            repo.join("runner/install.sh"),
            include_str!("../install.sh"),
        )
        .unwrap();
        // Unexpected fallback must fail without compiling anything or contacting
        // a registry. Every installation lives under this test's temporary root.
        executable(&root.path().join("tools/cargo"), "#!/bin/sh\nexit 99\n");
        Self {
            root,
            repo,
            destination,
        }
    }

    fn command(&self) -> Command {
        let mut paths = vec![self.root.path().join("tools")];
        paths.extend(std::env::split_paths(&std::env::var_os("PATH").unwrap()));
        let mut command = Command::new("sh");
        command
            .arg(self.repo.join("runner/install.sh"))
            .current_dir(self.root.path())
            .env("PATH", std::env::join_paths(paths).unwrap())
            .env("KARMA_TEAM_INSTALL_DIR", &self.destination)
            .env("CARGO_TARGET_DIR", "target")
            .env("TMPDIR", self.root.path().join("downloads"))
            .env("NO_PROXY", "127.0.0.1")
            .env("no_proxy", "127.0.0.1")
            .env_remove("KARMA_TEAM_LOCAL_BIN")
            .env_remove("KARMA_TEAM_BIN_URL")
            .env_remove("WSL_DISTRO_NAME")
            .kill_on_drop(true);
        command
    }

    fn assert_installed(&self, name: &str, body: &str) {
        let path = self.destination.join(name);
        assert_eq!(fs::read_to_string(&path).unwrap(), body);
        assert_ne!(fs::metadata(path).unwrap().permissions().mode() & 0o111, 0);
        self.assert_clean();
    }

    fn assert_clean(&self) {
        assert_eq!(
            fs::read_dir(self.root.path().join("downloads"))
                .unwrap()
                .count(),
            0,
            "download staging files must be removed"
        );
        if self.destination.exists() {
            for file in fs::read_dir(&self.destination).unwrap() {
                assert!(
                    !file
                        .unwrap()
                        .file_name()
                        .to_string_lossy()
                        .contains(".tmp.")
                );
            }
        }
    }
}

async fn run(command: &mut Command) -> Output {
    tokio::time::timeout(Duration::from_secs(15), command.output())
        .await
        .expect("installer timed out")
        .expect("launch installer")
}

fn assert_success(output: &Output) {
    assert!(
        output.status.success(),
        "stdout: {}\nstderr: {}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

async fn serve(status: &str, body: &str) -> (String, tokio::task::JoinHandle<()>) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}/karma-team", listener.local_addr().unwrap());
    let response = format!(
        "HTTP/1.1 {status}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    );
    let task = tokio::spawn(async move {
        let (mut socket, _) = listener.accept().await.unwrap();
        let mut request = Vec::new();
        while !request.ends_with(b"\r\n\r\n") {
            let byte = socket.read_u8().await.unwrap();
            request.push(byte);
            assert!(request.len() < 8192);
        }
        assert!(request.starts_with(b"GET /karma-team"));
        socket.write_all(response.as_bytes()).await.unwrap();
    });
    (url, task)
}

#[tokio::test]
async fn default_install_uses_the_local_release_binary() {
    let install = Installation::new();
    executable(
        &install.repo.join("target/release/karma-team"),
        LOCAL_BINARY,
    );
    let output = run(&mut install.command()).await;
    assert_success(&output);
    install.assert_installed("karma-team", LOCAL_BINARY);
}

#[tokio::test]
async fn explicit_local_path_is_resolved_from_the_repository_and_takes_precedence() {
    let install = Installation::new();
    executable(&install.repo.join("custom bin/karma-team"), LOCAL_BINARY);
    executable(
        &install.repo.join("target/release/karma-team"),
        REMOTE_BINARY,
    );
    let output = run(install
        .command()
        .env("KARMA_TEAM_LOCAL_BIN", "custom bin/karma-team"))
    .await;
    assert_success(&output);
    install.assert_installed("karma-team", LOCAL_BINARY);
}

#[tokio::test]
async fn invalid_explicit_local_path_does_not_fall_back_or_replace_an_installation() {
    let install = Installation::new();
    executable(&install.destination.join("karma-team"), LOCAL_BINARY);
    let output = run(install
        .command()
        .env("KARMA_TEAM_LOCAL_BIN", "missing-file"))
    .await;
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("KARMA_TEAM_LOCAL_BIN"));
    install.assert_installed("karma-team", LOCAL_BINARY);
}

#[tokio::test]
async fn remote_binary_overrides_local_candidates_and_replaces_the_old_installation() {
    let install = Installation::new();
    executable(
        &install.repo.join("target/release/karma-team"),
        LOCAL_BINARY,
    );
    executable(&install.destination.join("karma-team"), LOCAL_BINARY);
    let (url, server) = serve("200 OK", REMOTE_BINARY).await;
    let output = run(install.command().env("KARMA_TEAM_BIN_URL", url)).await;
    server.await.unwrap();
    assert_success(&output);
    install.assert_installed("karma-team", REMOTE_BINARY);
}

#[tokio::test]
async fn remote_exe_url_with_a_query_preserves_the_executable_extension() {
    let install = Installation::new();
    let (url, server) = serve("200 OK", REMOTE_BINARY).await;
    let output = run(install
        .command()
        .env("KARMA_TEAM_BIN_URL", format!("{url}.exe?download=1")))
    .await;
    server.await.unwrap();
    assert_success(&output);
    install.assert_installed("karma-team.exe", REMOTE_BINARY);
}

#[tokio::test]
async fn failed_or_empty_downloads_preserve_the_installed_binary() {
    for (status, body) in [("404 Not Found", "missing"), ("200 OK", "")] {
        let install = Installation::new();
        executable(&install.destination.join("karma-team"), LOCAL_BINARY);
        let (url, server) = serve(status, body).await;
        let output = run(install.command().env("KARMA_TEAM_BIN_URL", url)).await;
        server.await.unwrap();
        assert!(!output.status.success());
        install.assert_installed("karma-team", LOCAL_BINARY);
    }
}

#[tokio::test]
async fn conflicting_sources_and_non_http_urls_fail_before_installing() {
    let install = Installation::new();
    let local = install.repo.join("custom/karma-team");
    executable(&local, LOCAL_BINARY);
    let output = run(install
        .command()
        .env("KARMA_TEAM_LOCAL_BIN", &local)
        .env("KARMA_TEAM_BIN_URL", "http://127.0.0.1:1/karma-team"))
    .await;
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("Set only one"));

    let output = run(install
        .command()
        .env("KARMA_TEAM_BIN_URL", "file:///tmp/karma-team"))
    .await;
    assert!(!output.status.success());
    assert!(!install.destination.exists());
    install.assert_clean();
}
