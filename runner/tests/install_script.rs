//! Exercise the shell installer against local files and a loopback HTTP server.
#![cfg(unix)]

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Output, Stdio};
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
        fs::create_dir_all(root.path().join("home")).unwrap();
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
        let mut command = self.shell_command();
        command.arg(self.repo.join("runner/install.sh"));
        command
    }

    fn shell_command(&self) -> Command {
        let mut paths = vec![self.root.path().join("tools")];
        paths.extend(std::env::split_paths(&std::env::var_os("PATH").unwrap()));
        let mut command = Command::new("sh");
        command
            .current_dir(self.root.path())
            .env("PATH", std::env::join_paths(paths).unwrap())
            .env("HOME", self.root.path().join("home"))
            .env("SHELL", "/bin/sh")
            .env("KARMA_TEAM_INSTALL_DIR", &self.destination)
            .env("CARGO_TARGET_DIR", "target")
            .env("TMPDIR", self.root.path().join("downloads"))
            .env("NO_PROXY", "127.0.0.1")
            .env("no_proxy", "127.0.0.1")
            .env_remove("KARMA_TEAM_LOCAL_BIN")
            .env_remove("KARMA_TEAM_BIN_URL")
            .env_remove("KARMA_TEAM_API_URL")
            .env_remove("WSL_DISTRO_NAME")
            .env_remove("ZDOTDIR")
            .kill_on_drop(true);
        command
    }

    fn platform(&self, os: &str, arch: &str, libc: &str) {
        executable(
            &self.root.path().join("tools/uname"),
            &format!("#!/bin/sh\ncase \"$1\" in -s) echo '{os}' ;; -m) echo '{arch}' ;; esac\n"),
        );
        executable(
            &self.root.path().join("tools/ldd"),
            &format!("#!/bin/sh\necho '{libc}'\n"),
        );
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
    serve_path(status, body, "/karma-team").await
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
            .expect("installer never requested the binary")
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
async fn user_install_is_discoverable_after_loading_the_profile_and_is_idempotent() {
    let install = Installation::new();
    let user_home = install.root.path().join("home");
    executable(
        &install.repo.join("target/release/karma-team"),
        LOCAL_BINARY,
    );
    for _ in 0..2 {
        let output = run(install.command().env_remove("KARMA_TEAM_INSTALL_DIR")).await;
        assert_success(&output);
        assert!(String::from_utf8_lossy(&output.stdout).contains("current shell"));
    }
    let profile = user_home.join(".profile");
    assert!(
        profile.is_file(),
        "installer must configure the user's PATH"
    );
    let original = fs::read_to_string(&profile).unwrap();
    let output = run(
        Command::new("sh")
            .args(["-c", ". \"$HOME/.profile\"; . \"$HOME/.profile\"; command -v karma-team; printf '%s' \"$PATH\""])
            .env("HOME", &user_home)
            .env("PATH", "/usr/bin:/bin"),
    ).await;
    assert_success(&output);
    let bin_dir = user_home.join(".local/bin");
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.starts_with(&format!("{}/karma-team\n", bin_dir.display())));
    assert_eq!(
        stdout.lines().last().unwrap(),
        format!("{}:/usr/bin:/bin", bin_dir.display())
    );
    assert_eq!(
        stdout
            .lines()
            .last()
            .unwrap()
            .split(':')
            .filter(|entry| *entry == bin_dir.to_str().unwrap())
            .count(),
        1
    );
    let output = run(install.command().env_remove("KARMA_TEAM_INSTALL_DIR")).await;
    assert_success(&output);
    assert_eq!(fs::read_to_string(profile).unwrap(), original);
}

#[tokio::test]
async fn bash_install_preserves_startup_files_and_quotes_custom_paths() {
    let install = Installation::new();
    let user_home = install.root.path().join("home");
    let bin_dir = install.root.path().join("user's $(touch injected) bin");
    executable(
        &install.repo.join("target/release/karma-team"),
        LOCAL_BINARY,
    );
    for file in [".profile", ".bashrc", ".bash_profile", ".bash_login"] {
        fs::write(user_home.join(file), "# existing user settings").unwrap();
    }
    let output = run(install
        .command()
        .env("SHELL", "/bin/bash")
        .env("KARMA_TEAM_INSTALL_DIR", &bin_dir))
    .await;
    assert_success(&output);
    for file in [".profile", ".bashrc", ".bash_profile", ".bash_login"] {
        assert!(
            fs::read_to_string(user_home.join(file))
                .unwrap()
                .starts_with("# existing user settings\n")
        );
        let output = run(Command::new("bash")
            .args([
                "--noprofile",
                "--norc",
                "-c",
                ". \"$1\"; command -v karma-team",
                "bash",
            ])
            .arg(user_home.join(file))
            .current_dir(install.root.path())
            .env("HOME", &user_home)
            .env("PATH", "/usr/bin:/bin"))
        .await;
        assert_success(&output);
        assert_eq!(
            String::from_utf8_lossy(&output.stdout).trim(),
            bin_dir.join("karma-team").to_str().unwrap()
        );
    }
    assert!(!install.root.path().join("injected").exists());
}

#[tokio::test]
async fn invalid_install_directory_fails_before_requesting_a_download() {
    let install = Installation::new();
    let not_a_directory = install.root.path().join("existing-file");
    fs::write(&not_a_directory, "keep this file").unwrap();
    let output = run(install
        .command()
        .env("KARMA_TEAM_INSTALL_DIR", not_a_directory.join("bin"))
        .env("KARMA_TEAM_BIN_URL", "http://127.0.0.1:1/karma-team"))
    .await;
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("KARMA_TEAM_INSTALL_DIR"));
    assert!(!String::from_utf8_lossy(&output.stdout).contains("Downloading"));
    assert_eq!(
        fs::read_to_string(not_a_directory).unwrap(),
        "keep this file"
    );
}

#[tokio::test]
async fn zsh_install_uses_zdotdir_and_preserves_existing_settings() {
    let install = Installation::new();
    let dot_dir = install.root.path().join("zsh settings");
    fs::create_dir_all(&dot_dir).unwrap();
    fs::write(dot_dir.join(".zshrc"), "# existing zsh settings\n").unwrap();
    executable(
        &install.repo.join("target/release/karma-team"),
        LOCAL_BINARY,
    );
    for _ in 0..2 {
        let output = run(install
            .command()
            .env("SHELL", "/bin/zsh")
            .env("ZDOTDIR", &dot_dir))
        .await;
        assert_success(&output);
    }
    for file in [".zprofile", ".zshrc"] {
        let profile = dot_dir.join(file);
        let content = fs::read_to_string(&profile).expect("configure zsh startup files");
        assert_eq!(content.matches("# karma-team PATH").count(), 1);
        // The PATH snippet is POSIX shell code, including when added to zsh.
        let output = run(Command::new("sh")
            .args(["-c", ". \"$1\"; command -v karma-team", "sh"])
            .arg(&profile)
            .env("PATH", "/usr/bin:/bin"))
        .await;
        assert_success(&output);
        assert_eq!(
            String::from_utf8_lossy(&output.stdout).trim(),
            install.destination.join("karma-team").to_str().unwrap()
        );
    }
    assert!(
        fs::read_to_string(dot_dir.join(".zshrc"))
            .unwrap()
            .starts_with("# existing zsh settings\n")
    );
    assert!(!install.root.path().join("home/.zshrc").exists());
}

#[tokio::test]
async fn unavailable_startup_file_warns_without_failing_the_installation() {
    let install = Installation::new();
    // A directory blocks appending even when these tests run as root.
    let profile = install.root.path().join("home/.profile");
    fs::create_dir(&profile).unwrap();
    executable(
        &install.repo.join("target/release/karma-team"),
        LOCAL_BINARY,
    );
    let output = run(&mut install.command()).await;
    assert_success(&output);
    install.assert_installed("karma-team", LOCAL_BINARY);
    assert!(profile.is_dir());
    assert!(String::from_utf8_lossy(&output.stderr).contains("Could not update"));
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("current shell"));
    let activation = stdout
        .lines()
        .find(|line| line.trim_start().starts_with("export PATH="))
        .expect("print a manual PATH command");
    let output = run(Command::new("sh")
        .args(["-c", &format!("{activation}\ncommand -v karma-team")])
        .env("PATH", "/usr/bin:/bin"))
    .await;
    assert_success(&output);
    assert_eq!(
        String::from_utf8_lossy(&output.stdout).trim(),
        install.destination.join("karma-team").to_str().unwrap()
    );
}

#[tokio::test]
async fn relative_install_directory_is_persisted_as_an_absolute_path() {
    let install = Installation::new();
    executable(
        &install.repo.join("target/release/karma-team"),
        LOCAL_BINARY,
    );
    let output = run(install
        .command()
        .env("KARMA_TEAM_INSTALL_DIR", "relative bin"))
    .await;
    assert_success(&output);
    let output = run(Command::new("sh")
        .args(["-c", ". \"$HOME/.profile\"; command -v karma-team"])
        .current_dir("/")
        .env("HOME", install.root.path().join("home"))
        .env("PATH", "/usr/bin:/bin"))
    .await;
    assert_success(&output);
    assert_eq!(
        String::from_utf8_lossy(&output.stdout).trim(),
        install
            .root
            .path()
            .join("relative bin/karma-team")
            .to_str()
            .unwrap()
    );
}

#[tokio::test]
async fn unrepresentable_path_directories_are_rejected_before_downloading() {
    for directory in ["bad:path", "bad\npath"] {
        let install = Installation::new();
        let output = run(install
            .command()
            .env(
                "KARMA_TEAM_INSTALL_DIR",
                install.root.path().join(directory),
            )
            .env("KARMA_TEAM_BIN_URL", "http://127.0.0.1:1/karma-team"))
        .await;
        assert!(!output.status.success());
        assert!(String::from_utf8_lossy(&output.stderr).contains("KARMA_TEAM_INSTALL_DIR"));
        assert!(!String::from_utf8_lossy(&output.stdout).contains("Downloading"));
    }
}

#[tokio::test]
async fn installation_works_when_shell_is_not_exported() {
    let install = Installation::new();
    executable(
        &install.repo.join("target/release/karma-team"),
        LOCAL_BINARY,
    );
    let output = run(install.command().env_remove("SHELL")).await;
    assert_success(&output);
    install.assert_installed("karma-team", LOCAL_BINARY);
    assert!(install.root.path().join("home/.profile").is_file());
    assert!(!install.root.path().join("home/.bash_profile").exists());
}

#[tokio::test]
async fn inherited_root_home_and_runtime_are_repaired_for_effective_user() {
    let install = Installation::new();
    let target_home = install.root.path().join("target user home");
    let stale_home = install.root.path().join("stale root home");
    let environment_log = install.root.path().join("environment.log");
    fs::create_dir_all(&target_home).unwrap();
    fs::create_dir_all(&stale_home).unwrap();
    executable(
        &install.repo.join("target/release/karma-team"),
        LOCAL_BINARY,
    );
    executable(
        &install.root.path().join("tools/id"),
        "#!/bin/sh\n[ \"$1\" = -u ] && { echo 424242; exit 0; }\nexit 1\n",
    );
    executable(
        &install.root.path().join("tools/getent"),
        &format!(
            "#!/bin/sh\nprintf '%s\\n' 'laiye:x:424242:424242::{}:/bin/sh'\n",
            target_home.display()
        ),
    );
    executable(
        &install.root.path().join("tools/stat"),
        &format!(
            "#!/bin/sh\nlast=''\nfor arg in \"$@\"; do last=$arg; done\ncase \"$last\" in\n  '{}') echo 424242 ;;\n  *) echo 0 ;;\nesac\n",
            target_home.display()
        ),
    );
    executable(
        &install.root.path().join("tools/mkdir"),
        "#!/bin/sh\nprintf 'XDG=%s DBUS=%s\\n' \"${XDG_RUNTIME_DIR:-}\" \"${DBUS_SESSION_BUS_ADDRESS:-}\" >> \"$TEST_LOG\"\nexec /bin/mkdir \"$@\"\n",
    );

    let output = run(install
        .command()
        .env("HOME", &stale_home)
        .env("XDG_RUNTIME_DIR", "/run/user/0")
        .env("DBUS_SESSION_BUS_ADDRESS", "unix:path=/run/user/0/bus")
        .env("TEST_LOG", &environment_log))
    .await;
    assert_success(&output);
    assert!(target_home.join(".profile").is_file());
    assert!(!stale_home.join(".profile").exists());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("replacing inherited HOME="), "{stderr}");
    assert!(
        stderr.contains("ignoring XDG_RUNTIME_DIR=/run/user/0"),
        "{stderr}"
    );
    let environment = fs::read_to_string(environment_log).unwrap();
    assert!(!environment.contains("/run/user/0"), "{environment}");
    assert!(environment.lines().all(|line| line == "XDG= DBUS="));
}

#[tokio::test]
async fn sudo_root_install_for_an_ordinary_user_is_rejected_before_writing() {
    let install = Installation::new();
    executable(
        &install.repo.join("target/release/karma-team"),
        LOCAL_BINARY,
    );
    executable(
        &install.root.path().join("tools/id"),
        "#!/bin/sh\n[ \"$1\" = -u ] && { echo 0; exit 0; }\nexit 1\n",
    );
    let output = run(install.command().env("SUDO_USER", "laiye")).await;
    assert!(!output.status.success());
    assert!(
        String::from_utf8_lossy(&output.stderr)
            .contains("Refusing to install a per-user karma-team CLI as root")
    );
    assert!(!install.destination.exists());
}

#[tokio::test]
async fn relative_install_rejects_unrepresentable_parent_paths_before_downloading() {
    for directory in ["bad:parent", "bad\nparent"] {
        let install = Installation::new();
        let working_dir = install.root.path().join(directory);
        fs::create_dir(&working_dir).unwrap();
        let output = run(install
            .command()
            .current_dir(&working_dir)
            .env("KARMA_TEAM_INSTALL_DIR", "bin")
            .env("KARMA_TEAM_BIN_URL", "http://127.0.0.1:1/karma-team"))
        .await;
        assert!(!output.status.success());
        assert!(String::from_utf8_lossy(&output.stderr).contains("KARMA_TEAM_INSTALL_DIR"));
        assert!(!String::from_utf8_lossy(&output.stdout).contains("Downloading"));
        assert!(!working_dir.join("bin").exists());
    }
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

#[tokio::test]
async fn deployment_download_detects_supported_platforms_and_replaces_local_candidates() {
    for (os, arch, libc, target, name) in [
        (
            "Linux",
            "x86_64",
            "GNU libc",
            "x86_64-unknown-linux-gnu",
            "karma-team",
        ),
        (
            "Linux",
            "aarch64",
            "GNU libc",
            "aarch64-unknown-linux-gnu",
            "karma-team",
        ),
        (
            "Linux",
            "x86_64",
            "musl libc",
            "x86_64-unknown-linux-musl",
            "karma-team",
        ),
        (
            "Linux",
            "aarch64",
            "musl libc",
            "aarch64-unknown-linux-musl",
            "karma-team",
        ),
        ("Darwin", "x86_64", "", "x86_64-apple-darwin", "karma-team"),
        ("Darwin", "arm64", "", "aarch64-apple-darwin", "karma-team"),
        (
            "MINGW64_NT",
            "x86_64",
            "",
            "x86_64-pc-windows-msvc",
            "karma-team.exe",
        ),
    ] {
        let install = Installation::new();
        install.platform(os, arch, libc);
        executable(
            &install.repo.join("target/release").join(name),
            LOCAL_BINARY,
        );
        executable(&install.destination.join(name), LOCAL_BINARY);
        let path = format!("/api/cli/bin/{target}/");
        let (url, server) = serve_path("200 OK", REMOTE_BINARY, &path).await;
        let api_url = url.strip_suffix(&path).unwrap();
        let output = run(install
            .command()
            .env("KARMA_TEAM_API_URL", format!("{api_url}/")))
        .await;
        server.await.unwrap();
        assert_success(&output);
        install.assert_installed(name, REMOTE_BINARY);
        assert!(String::from_utf8_lossy(&output.stdout).contains(&format!("--url '{api_url}'")));
    }
}

#[tokio::test]
async fn explicit_sources_override_deployment_downloads() {
    let install = Installation::new();
    executable(&install.repo.join("custom/karma-team"), LOCAL_BINARY);
    let output = run(install
        .command()
        .env("KARMA_TEAM_API_URL", "http://127.0.0.1:1")
        .env("KARMA_TEAM_LOCAL_BIN", "custom/karma-team"))
    .await;
    assert_success(&output);
    install.assert_installed("karma-team", LOCAL_BINARY);

    let (url, server) = serve("200 OK", REMOTE_BINARY).await;
    let output = run(install
        .command()
        .env("KARMA_TEAM_API_URL", "http://127.0.0.1:1")
        .env("KARMA_TEAM_BIN_URL", url))
    .await;
    server.await.unwrap();
    assert_success(&output);
    install.assert_installed("karma-team", REMOTE_BINARY);
}

#[tokio::test]
async fn deployment_installer_works_from_stdin_without_a_checkout() {
    let install = Installation::new();
    install.platform("Linux", "x86_64", "GNU libc");
    fs::remove_dir_all(&install.repo).unwrap();
    let path = "/api/cli/bin/x86_64-unknown-linux-gnu/";
    let (url, server) = serve_path("200 OK", REMOTE_BINARY, path).await;
    let mut child = install
        .shell_command()
        .env("KARMA_TEAM_API_URL", url.strip_suffix(path).unwrap())
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(include_bytes!("../install.sh"))
        .await
        .unwrap();
    let output = tokio::time::timeout(Duration::from_secs(15), child.wait_with_output())
        .await
        .unwrap()
        .unwrap();
    server.await.unwrap();
    assert_success(&output);
    install.assert_installed("karma-team", REMOTE_BINARY);
}

#[tokio::test]
async fn missing_deployment_artifact_preserves_the_old_binary() {
    let install = Installation::new();
    install.platform("Darwin", "arm64", "");
    executable(&install.destination.join("karma-team"), LOCAL_BINARY);
    let path = "/api/cli/bin/aarch64-apple-darwin/";
    let (url, server) = serve_path("404 Not Found", "artifact not published", path).await;
    let output = run(install
        .command()
        .env("KARMA_TEAM_API_URL", url.strip_suffix(path).unwrap()))
    .await;
    server.await.unwrap();
    assert!(!output.status.success());
    install.assert_installed("karma-team", LOCAL_BINARY);
}

#[tokio::test]
async fn unsupported_platform_or_invalid_deployment_url_fails_before_installing() {
    let install = Installation::new();
    install.platform("Linux", "riscv64", "GNU libc");
    let output = run(install
        .command()
        .env("KARMA_TEAM_API_URL", "http://127.0.0.1:1"))
    .await;
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("Unsupported architecture"));
    let output = run(install.command().env("KARMA_TEAM_API_URL", "file:///tmp")).await;
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("KARMA_TEAM_API_URL"));
    assert!(!install.destination.exists());
    install.assert_clean();
}
