//! Regression tests for the user-scoped shell uninstaller.
#![cfg(unix)]

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::Output;
use std::time::Duration;

use tempfile::TempDir;
use tokio::process::Command;

struct Fixture {
    root: TempDir,
    home: PathBuf,
    bin: PathBuf,
    config: PathBuf,
    data: PathBuf,
    cache: PathBuf,
    log: PathBuf,
}

fn executable(path: &Path, body: &str) {
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, body).unwrap();
    fs::set_permissions(path, fs::Permissions::from_mode(0o755)).unwrap();
}

impl Fixture {
    fn new() -> Self {
        let root = tempfile::tempdir().unwrap();
        let home = root.path().join("home");
        let bin = home.join(".local/bin");
        let config = home.join(".config/karma-team");
        let data = home.join(".local/share/karma-team");
        let cache = home.join(".cache/karma-team");
        let log = root.path().join("commands.log");
        fs::create_dir_all(&bin).unwrap();
        fs::create_dir_all(&config).unwrap();
        fs::create_dir_all(&data).unwrap();
        executable(&bin.join("karma-team"), "#!/bin/sh\nexit 0\n");
        executable(
            &root.path().join("tools/systemctl"),
            "#!/bin/sh\nprintf 'systemctl %s\\n' \"$*\" >> \"$TEST_LOG\"\ncase \"$1 $2 $3\" in\n  '--user is-active karma-team.service') exit 3 ;;\n  *) exit 0 ;;\nesac\n",
        );
        executable(&root.path().join("tools/uname"), "#!/bin/sh\necho Linux\n");
        Self {
            root,
            home,
            bin,
            config,
            data,
            cache,
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
            .arg(self.root.path().join("runner/uninstall.sh"))
            .current_dir(self.root.path())
            .env("HOME", &self.home)
            .env("PATH", path)
            .env("TEST_LOG", &self.log)
            .env("KARMA_TEAM_INSTALL_DIR", &self.bin)
            .env("KARMA_TEAM_CONFIG_DIR", &self.config)
            .env("KARMA_TEAM_DATA_DIR", &self.data)
            .env_remove("ZDOTDIR")
            .kill_on_drop(true);
        command
    }

    fn copy_script(&self) {
        fs::create_dir_all(self.root.path().join("runner")).unwrap();
        fs::write(
            self.root.path().join("runner/uninstall.sh"),
            include_str!("../uninstall.sh"),
        )
        .unwrap();
    }
}

async fn run(command: &mut Command) -> Output {
    tokio::time::timeout(Duration::from_secs(10), command.output())
        .await
        .expect("uninstaller timed out")
        .expect("launch uninstaller")
}

#[tokio::test]
async fn dry_run_is_non_destructive_and_does_not_call_cli_or_service() {
    let fixture = Fixture::new();
    fixture.copy_script();
    fs::write(fixture.config.join("config.toml"), "secret").unwrap();
    fs::write(fixture.data.join("marker"), "keep").unwrap();
    let output = run(&mut fixture.command().args(["--dry-run", "--yes"])).await;
    assert!(output.status.success(), "{output:?}");
    assert!(fixture.bin.join("karma-team").exists());
    assert!(fixture.config.join("config.toml").exists());
    assert!(fixture.data.join("marker").exists());
    assert!(!fixture.log.exists(), "dry-run must not invoke commands");
}

#[tokio::test]
async fn local_only_removes_owned_files_and_profile_marker_but_keeps_other_data() {
    let fixture = Fixture::new();
    fixture.copy_script();
    fs::write(fixture.config.join("config.toml"), "secret").unwrap();
    fs::write(fixture.config.join("credentials.toml"), "secret").unwrap();
    fs::create_dir_all(fixture.data.join("logs")).unwrap();
    fs::create_dir_all(&fixture.cache).unwrap();
    fs::write(fixture.cache.join("cache.db"), "cache").unwrap();
    fs::write(fixture.data.join("logs/runner.log"), "log").unwrap();
    fs::create_dir_all(fixture.data.join("workspaces/project/.git")).unwrap();
    fs::write(fixture.data.join("workspaces/project/README"), "keep").unwrap();
    fs::write(
        fixture.home.join(".profile"),
        "# user\n\n# karma-team PATH\ncase ':${PATH:-}:' in *:'/tmp/bin':*) ;; *) export PATH='/tmp/bin'${PATH:+:\"$PATH\"} ;; esac\n\n# tail\n",
    )
    .unwrap();
    let output = run(&mut fixture.command().args(["--local-only", "--yes"])).await;
    assert!(
        output.status.success(),
        "stdout={} stderr={}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(!fixture.bin.join("karma-team").exists());
    assert!(!fixture.config.join("config.toml").exists());
    assert!(!fixture.data.join("logs").exists());
    assert!(!fixture.cache.exists());
    assert!(fixture.data.join("workspaces/project/.git").exists());
    let profile = fs::read_to_string(fixture.home.join(".profile")).unwrap();
    assert!(!profile.contains("# karma-team PATH"));
    assert!(profile.contains("# user") && profile.contains("# tail"));
}

#[tokio::test]
async fn headless_without_yes_refuses_without_mutating_anything() {
    let fixture = Fixture::new();
    fixture.copy_script();
    let mut command = fixture.command();
    command.stdin(std::process::Stdio::null());
    let output = run(&mut command).await;
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("--yes"));
    assert!(fixture.bin.join("karma-team").exists());
    assert!(!fixture.log.exists());
}

#[tokio::test]
async fn inherited_root_home_and_runtime_are_not_used_by_uninstall() {
    let fixture = Fixture::new();
    fixture.copy_script();
    let stale_home = fixture.root.path().join("stale-root-home");
    let stale_runtime = fixture.root.path().join("run-user-0");
    fs::create_dir_all(&stale_home).unwrap();
    fs::create_dir_all(&stale_runtime).unwrap();
    fs::write(stale_runtime.join("must-stay"), "root runtime").unwrap();
    executable(
        &fixture.root.path().join("tools/id"),
        "#!/bin/sh\n[ \"$1\" = -u ] && { echo 424242; exit 0; }\nexit 1\n",
    );
    executable(
        &fixture.root.path().join("tools/getent"),
        &format!(
            "#!/bin/sh\nprintf '%s\\n' 'laiye:x:424242:424242::{}:/bin/sh'\n",
            fixture.home.display()
        ),
    );
    executable(
        &fixture.root.path().join("tools/stat"),
        &format!(
            "#!/bin/sh\nlast=''\nfor arg in \"$@\"; do last=$arg; done\ncase \"$last\" in\n  '{}') echo 424242 ;;\n  *) echo 0 ;;\nesac\n",
            fixture.home.display()
        ),
    );
    executable(
        &fixture.bin.join("karma-team"),
        "#!/bin/sh\nprintf 'HOME=%s\\nXDG=%s\\nDBUS=%s\\n' \"$HOME\" \"${XDG_RUNTIME_DIR:-}\" \"${DBUS_SESSION_BUS_ADDRESS:-}\" >> \"$TEST_LOG\"\nexit 0\n",
    );

    let output = run(fixture
        .command()
        .args(["--local-only", "--yes"])
        .env("HOME", &stale_home)
        .env("XDG_RUNTIME_DIR", &stale_runtime)
        .env(
            "DBUS_SESSION_BUS_ADDRESS",
            format!("unix:path={}/bus", stale_runtime.display()),
        ))
    .await;
    assert!(
        output.status.success(),
        "stdout={} stderr={}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let log = fs::read_to_string(&fixture.log).unwrap();
    assert!(log.contains(&format!("HOME={}", fixture.home.display())));
    assert!(log.contains("XDG=\n"), "{log}");
    assert!(log.contains("DBUS=\n"), "{log}");
    assert!(stale_runtime.join("must-stay").is_file());
    assert!(String::from_utf8_lossy(&output.stderr).contains("ignoring XDG_RUNTIME_DIR="));
}

#[tokio::test]
async fn nonstandard_runtime_base_never_deletes_an_unrelated_karma_team_directory() {
    let fixture = Fixture::new();
    fixture.copy_script();
    let unrelated = fixture.home.join("karma-team");
    fs::create_dir_all(&unrelated).unwrap();
    fs::write(unrelated.join("must-stay"), "user checkout").unwrap();

    let output = run(fixture
        .command()
        .args(["--local-only", "--yes"])
        .env_remove("KARMA_TEAM_DATA_DIR")
        .env("XDG_RUNTIME_DIR", &fixture.home))
    .await;

    assert!(
        output.status.success(),
        "stdout={} stderr={}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(unrelated.join("must-stay").is_file());
    assert!(
        String::from_utf8_lossy(&output.stderr).contains("Skipping non-standard runtime cleanup")
    );
}

#[tokio::test]
async fn sudo_root_uninstall_for_an_ordinary_user_is_rejected_before_writing() {
    let fixture = Fixture::new();
    fixture.copy_script();
    executable(
        &fixture.root.path().join("tools/id"),
        "#!/bin/sh\n[ \"$1\" = -u ] && { echo 0; exit 0; }\nexit 1\n",
    );
    let output = run(fixture
        .command()
        .args(["--local-only", "--yes"])
        .env("SUDO_USER", "laiye"))
    .await;
    assert!(!output.status.success());
    assert!(
        String::from_utf8_lossy(&output.stderr)
            .contains("Refusing to uninstall a per-user karma-team CLI as root")
    );
    assert!(fixture.bin.join("karma-team").is_file());
    assert!(!fixture.log.exists());
}

#[tokio::test]
async fn dangerous_state_or_cache_roots_are_rejected_before_any_command() {
    let fixture = Fixture::new();
    fixture.copy_script();
    for (variable, value) in [
        ("KARMA_TEAM_DATA_DIR", "/"),
        ("KARMA_TEAM_CACHE_DIR", "/tmp"),
    ] {
        let output = run(fixture
            .command()
            .args(["--yes", "--local-only"])
            .env(variable, value))
        .await;
        assert!(!output.status.success());
        assert!(String::from_utf8_lossy(&output.stderr).contains("unsafe"));
        assert!(fixture.bin.join("karma-team").exists());
        assert!(!fixture.log.exists());
    }
    for variable in [
        "KARMA_TEAM_CONFIG_DIR",
        "KARMA_TEAM_DATA_DIR",
        "KARMA_TEAM_CACHE_DIR",
    ] {
        let output = run(fixture
            .command()
            .args(["--yes", "--local-only"])
            .env(variable, ""))
        .await;
        assert!(!output.status.success());
        assert!(String::from_utf8_lossy(&output.stderr).contains("unsafe"));
        assert!(!fixture.log.exists());
    }
}

#[tokio::test]
async fn configured_paths_are_not_evaluated_as_shell_code() {
    let fixture = Fixture::new();
    fixture.copy_script();
    let marker = fixture.root.path().join("must-not-exist");
    let malicious = format!(
        "{}/$(touch {})",
        fixture.root.path().display(),
        marker.display()
    );
    let output = run(fixture
        .command()
        .args(["--yes", "--local-only"])
        .env("KARMA_TEAM_DATA_DIR", malicious))
    .await;
    assert!(!output.status.success());
    assert!(!marker.exists());
    assert!(String::from_utf8_lossy(&output.stderr).contains("unsafe"));
}

#[tokio::test]
async fn service_manager_verification_failure_preserves_local_state() {
    let fixture = Fixture::new();
    fixture.copy_script();
    let unit = fixture.home.join(".config/systemd/user/karma-team.service");
    fs::create_dir_all(unit.parent().unwrap()).unwrap();
    fs::write(&unit, "[Service]\n").unwrap();
    fs::write(fixture.config.join("config.toml"), "keep\n").unwrap();
    let output = run(&mut fixture.command().args(["--local-only", "--yes"])).await;
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("verify"));
    assert!(fixture.bin.join("karma-team").exists());
    assert!(fixture.config.join("config.toml").exists());
    assert!(unit.exists());
}

#[tokio::test]
async fn foreground_daemon_preserves_state_until_stopped() {
    let fixture = Fixture::new();
    fixture.copy_script();
    executable(
        &fixture.root.path().join("tools/ps"),
        "#!/bin/sh\nprintf '999 %s __run\\n' \"$KARMA_TEAM_INSTALL_DIR/karma-team\"\n",
    );
    fs::write(fixture.config.join("config.toml"), "keep\n").unwrap();
    let output = run(&mut fixture.command().args(["--local-only", "--yes"])).await;
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("foreground"));
    assert!(fixture.bin.join("karma-team").exists());
    assert!(fixture.config.join("config.toml").exists());
}

#[tokio::test]
async fn cloud_failure_is_nonzero_and_restores_config_before_aborting() {
    let fixture = Fixture::new();
    fixture.copy_script();
    fs::write(fixture.config.join("config.toml"), "token = 'secret'\n").unwrap();
    executable(
        &fixture.bin.join("karma-team"),
        "#!/bin/sh\nprintf 'remove --all\\n' >> \"$TEST_LOG\"\nprintf 'Some cloud cleanup failed\\n'\necho removed >&2\nexit 0\n",
    );
    let output = run(&mut fixture.command().args(["--yes"])).await;
    assert!(!output.status.success());
    assert!(fixture.config.join("config.toml").exists());
    assert!(fixture.bin.join("karma-team").exists());
    assert!(String::from_utf8_lossy(&output.stderr).contains("cloud cleanup"));
}
