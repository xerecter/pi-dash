//! A second Karma runner can activate inside the existing daemon.

use std::process::Stdio;
use std::time::Duration;

use karma_team::config::{
    file,
    schema::{Config, RunnerConfig},
};
use karma_team::ipc::{
    client::Client,
    protocol::{Request, Response},
};
use karma_team::util::paths::Paths;

#[tokio::test]
async fn karma_runner_activation_adds_second_project_without_restarting_the_daemon() {
    let root = tempfile::tempdir().unwrap();
    let paths = Paths::resolve(
        Some(root.path().join("config")),
        Some(root.path().join("data")),
    )
    .unwrap();
    paths.ensure().unwrap();
    let make_runner = |name: &str| -> RunnerConfig {
        serde_json::from_value(serde_json::json!({
            "name": name, "runner_id": uuid::Uuid::new_v4(),
            "project_slug": name, "workspace_slug": "test",
            "workspace": {"working_dir": root.path().join(name)},
            "agent": {"kind": "karma"},
            "karma": {"binary": env!("CARGO_BIN_EXE_karma-team")},
        }))
        .unwrap()
    };
    let first = make_runner("FIRST");
    let second = make_runner("SECOND");
    let config: Config = serde_json::from_value(serde_json::json!({
        "version": 2, "daemon": {"cloud_url": "http://127.0.0.1:1", "auto_update": false},
        "cli": {"token": "mt_test_token", "workspace_slug": "test"},
        "runner": [first],
    }))
    .unwrap();
    file::write_config(&paths, &config).unwrap();
    let mut daemon = tokio::process::Command::new(env!("CARGO_BIN_EXE_karma-team"))
        .env("KARMA_TEAM_CONFIG_DIR", &paths.config_dir)
        .env("KARMA_TEAM_DATA_DIR", &paths.data_dir)
        .args(["__run", "--offline"])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .kill_on_drop(true)
        .spawn()
        .unwrap();
    let pid = daemon.id();
    let mut ipc = tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            if let Ok(client) = Client::connect(paths.ipc_socket_path()).await {
                break client;
            }
            assert!(
                daemon.try_wait().unwrap().is_none(),
                "daemon exited before IPC became ready"
            );
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .unwrap();
    let Response::Status(before) = ipc.call(Request::StatusGet).await.unwrap() else {
        panic!("status expected")
    };
    assert_eq!(before.runners.len(), 1);
    file::mutate_config(&paths, |config| {
        config.runners.push(second.clone());
        Ok(())
    })
    .unwrap();

    for _ in 0..2 {
        assert!(matches!(
            ipc.call(Request::RunnerActivateLocal {
                runner: "SECOND".into()
            })
            .await
            .unwrap(),
            Response::Ack
        ));
        assert!(daemon.try_wait().unwrap().is_none());
        assert_eq!(daemon.id(), pid);
        let Response::Status(after) = ipc.call(Request::StatusGet).await.unwrap() else {
            panic!("status expected")
        };
        assert_eq!(after.runners.len(), 2);
        assert!(
            after
                .runners
                .iter()
                .any(|runner| runner.runner_id == before.runners[0].runner_id)
        );
        assert!(
            after
                .runners
                .iter()
                .any(|runner| runner.runner_id == second.runner_id)
        );
        assert!(matches!(
            ipc.call(Request::ApprovalsList {
                runner: Some("SECOND".into())
            })
            .await
            .unwrap(),
            Response::Approvals(_)
        ));
    }
    assert!(matches!(
        ipc.call(Request::RunnerActivateLocal {
            runner: "MISSING".into()
        })
        .await
        .unwrap(),
        Response::Error(_)
    ));
    daemon.kill().await.unwrap();
    daemon.wait().await.unwrap();
}
