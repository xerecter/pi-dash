//! Lifecycle commands must reject legacy non-Karma configs before touching
//! the service manager or writing a unit.
#![cfg(target_os = "linux")]

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::Command;

use tempfile::TempDir;
use uuid::Uuid;

struct Fixture {
    _root: TempDir,
    config_dir: PathBuf,
    data_dir: PathBuf,
    home: PathBuf,
    fake_bin: PathBuf,
    service_log: PathBuf,
}

impl Fixture {
    fn new() -> Self {
        let root = tempfile::tempdir().unwrap();
        let config_dir = root.path().join("config");
        let data_dir = root.path().join("data");
        let home = root.path().join("home");
        let fake_bin = root.path().join("bin");
        let service_log = root.path().join("systemctl.log");
        fs::create_dir_all(&config_dir).unwrap();
        fs::create_dir_all(&data_dir).unwrap();
        fs::create_dir_all(&home).unwrap();
        fs::create_dir_all(&fake_bin).unwrap();
        write_executable(
            &fake_bin.join("systemctl"),
            "#!/bin/sh\nprintf '%s\\n' \"$*\" >> \"$KARMA_TEST_SERVICE_LOG\"\nexit 0\n",
        );
        let config = format!(
            "version = 2\n\n[daemon]\ncloud_url = 'http://example.invalid'\n\n[[runner]]\nname = 'legacy'\nrunner_id = '{}'\nproject_slug = 'TEST'\n\n[runner.workspace]\nworking_dir = '/tmp/legacy'\n\n[runner.agent]\nkind = 'codex'\n",
            Uuid::new_v4()
        );
        fs::write(config_dir.join("config.toml"), config).unwrap();
        Self {
            _root: root,
            config_dir,
            data_dir,
            home,
            fake_bin,
            service_log,
        }
    }

    fn command(&self, subcommand: &str) -> Command {
        let original_path = std::env::var_os("PATH").unwrap();
        let path = std::env::join_paths(
            std::iter::once(self.fake_bin.clone()).chain(std::env::split_paths(&original_path)),
        )
        .unwrap();
        let mut command = Command::new(env!("CARGO_BIN_EXE_karma-team"));
        command
            .env("HOME", &self.home)
            .env("KARMA_TEAM_CONFIG_DIR", &self.config_dir)
            .env("KARMA_TEAM_DATA_DIR", &self.data_dir)
            .env("KARMA_TEST_SERVICE_LOG", &self.service_log)
            .env("PATH", path)
            .args([subcommand]);
        command
    }

    fn assert_unsupported_before_service_side_effect(&self, subcommand: &str) {
        let output = self.command(subcommand).output().unwrap();
        assert!(
            !output.status.success(),
            "{subcommand} unexpectedly succeeded"
        );
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(
            stderr.contains("unsupported agent 'codex'"),
            "{subcommand}: {stderr}"
        );
        assert!(
            stderr.contains("only the karma agent is supported"),
            "{subcommand}: {stderr}"
        );
        assert!(
            !self.service_log.exists(),
            "{subcommand} invoked the service manager before rejecting the config"
        );
        assert!(
            !self
                .home
                .join(".config/systemd/user/karma-team.service")
                .exists(),
            "{subcommand} wrote a service unit before rejecting the config"
        );
    }
}

fn write_executable(path: &Path, body: &str) {
    fs::write(path, body).unwrap();
    fs::set_permissions(path, fs::Permissions::from_mode(0o755)).unwrap();
}

#[test]
fn start_rejects_non_karma_before_starting_service() {
    Fixture::new().assert_unsupported_before_service_side_effect("start");
}

#[test]
fn restart_rejects_non_karma_before_rewriting_or_starting_service() {
    Fixture::new().assert_unsupported_before_service_side_effect("restart");
}

#[test]
fn install_rejects_non_karma_before_writing_or_starting_service() {
    Fixture::new().assert_unsupported_before_service_side_effect("install");
}
