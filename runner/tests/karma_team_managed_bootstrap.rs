use std::io::Write;
use std::process::{Command, Stdio};

#[test]
fn bootstrap_preserves_server_machine_identity_and_workspace() {
    let root = tempfile::tempdir().unwrap();
    let config = root.path().join("config");
    let data = root.path().join("data");
    let machine_id = uuid::Uuid::new_v4();
    let paths =
        karma_team::util::paths::Paths::resolve(Some(config.clone()), Some(data.clone())).unwrap();
    let existing_runner_id = uuid::Uuid::new_v4();
    let existing_config: karma_team::config::schema::Config =
        serde_json::from_value(serde_json::json!({
            "version": 2,
            "daemon": {
                "cloud_url": "http://localhost:18002",
                "dev_machine_id": machine_id,
                "auto_update": false
            },
            "cli": {
                "token": "mt-existing-token",
                "workspace_slug": "desktop-e2e"
            },
            "runner": [{
                "name": "existing-project-runner",
                "runner_id": existing_runner_id,
                "workspace_slug": "desktop-e2e",
                "project_slug": "EXISTING",
                "workspace": {"working_dir": data.join("existing-worktree")}
            }]
        }))
        .unwrap();
    karma_team::config::file::write_config(&paths, &existing_config).unwrap();
    let mut child = Command::new(env!("CARGO_BIN_EXE_karma-team"))
        .env("KARMA_TEAM_CONFIG_DIR", &config)
        .env("KARMA_TEAM_DATA_DIR", &data)
        .args([
            "__managed",
            "bootstrap",
            "--cloud-url",
            "http://localhost:18002",
            "--workspace",
            "desktop-e2e",
            "--dev-machine-id",
            &machine_id.to_string(),
        ])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(b"mt-local-test-token\n")
        .unwrap();
    let result = child.wait_with_output().unwrap();
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    let cfg = karma_team::config::file::load_config(&paths).unwrap();
    assert_eq!(cfg.daemon.dev_machine_id, Some(machine_id));
    let cli = cfg.cli.unwrap();
    assert_eq!(cli.workspace_slug.as_deref(), Some("desktop-e2e"));
    assert_eq!(cli.token.as_deref(), Some("mt-local-test-token"));
    assert!(
        cfg.runners
            .iter()
            .any(|runner| runner.runner_id == existing_runner_id)
    );
    assert!(!String::from_utf8_lossy(&result.stdout).contains("mt-local-test-token"));
}
