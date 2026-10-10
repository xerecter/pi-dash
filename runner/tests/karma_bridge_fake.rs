//! End-to-end Karma agent test.
//!
//! Karma wraps Claude Code and exposes the identical headless stream-JSON CLI,
//! so the Karma agent is driven by the Claude Code bridge with `karma` as the
//! executable. This test proves the wiring: a fake `karma` binary receives the
//! Claude Code flags and the run reaches completion through
//! `AgentBridge::spawn_from_config` with `agent.kind = "karma"`.

#![cfg(unix)]

use std::os::unix::fs::PermissionsExt;
use std::path::Path;
use std::time::Duration;

use karma_team::agent::{AgentBridge, BridgeEvent, RunPayload};
use karma_team::config::schema::RunnerConfig;
use uuid::Uuid;

/// Fake `karma`: records its argv (to prove the Claude Code contract is used),
/// drains stdin, then emits canned stream-JSON events ending in `result`.
const FAKE_KARMA: &str = r#"#!/bin/sh
set -e
printf '%s\n' "$@" > argv.txt
(cat >/dev/null) &
printf '%s\n' '{"type":"system","subtype":"init","session_id":"sess_karma_001","model":"claude-opus-5"}'
printf '%s\n' '{"type":"assistant","message":{"role":"assistant","content":[{"type":"text","text":"hello from karma"}]},"session_id":"sess_karma_001"}'
printf '%s\n' '{"type":"result","subtype":"success","session_id":"sess_karma_001","result":"done","total_cost_usd":0.0,"usage":{"input_tokens":1,"output_tokens":1}}'
sleep 0.2
"#;

fn write_fake_karma(dir: &Path) -> std::path::PathBuf {
    let path = dir.join("karma");
    std::fs::write(&path, FAKE_KARMA).unwrap();
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o700)).unwrap();
    path
}

fn karma_runner(binary: &Path, working_dir: &Path) -> RunnerConfig {
    serde_json::from_value(serde_json::json!({
        "name": "karma-runner",
        "runner_id": Uuid::new_v4(),
        "workspace_slug": "test",
        "project_slug": "TASKS",
        "agent": { "kind": "karma" },
        "karma": { "binary": binary.to_str().unwrap() },
        "workspace": { "working_dir": working_dir.to_str().unwrap() },
    }))
    .unwrap()
}

#[tokio::test]
async fn karma_agent_drives_the_claude_contract_to_completion() {
    let root = tempfile::tempdir().unwrap();
    let cwd = root.path().join("work");
    std::fs::create_dir_all(&cwd).unwrap();
    let binary = write_fake_karma(root.path());
    let runner = karma_runner(&binary, &cwd);

    // The Karma agent must select the configured `karma` binary.
    assert_eq!(runner.agent.kind.default_binary(), "karma");
    assert_eq!(runner.karma.binary, binary.to_str().unwrap());

    let mut bridge = AgentBridge::spawn_from_config(&runner, &cwd, None)
        .await
        .expect("spawn karma via the Claude Code bridge");

    let payload = RunPayload {
        run_id: Uuid::new_v4(),
        prompt: "say hi".into(),
        model: None,
    };
    // Generous timeout: the agent is spawned through a login shell
    // (`bash -ilc`), whose startup sources the host's rc files and can take
    // several seconds (conda/nvm/etc.), so a tight bound is flaky.
    let mut cursor =
        tokio::time::timeout(Duration::from_secs(30), bridge.run_one_shot(&payload, &cwd))
            .await
            .expect("karma run setup should not time out")
            .expect("karma run setup");

    // Pin the durable agent-kind label the cloud records in agent_metadata.
    assert_eq!(cursor.agent_kind(), "karma");

    let mut completed = false;
    let deadline = tokio::time::Instant::now() + Duration::from_secs(30);
    while tokio::time::Instant::now() < deadline && !completed {
        let Some(events) =
            tokio::time::timeout(Duration::from_millis(500), bridge.next_events(&mut cursor))
                .await
                .ok()
                .flatten()
        else {
            break;
        };
        for ev in events {
            match ev {
                BridgeEvent::Completed { done_payload, .. } => {
                    completed = true;
                    assert_eq!(
                        done_payload.get("conclusion").and_then(|v| v.as_str()),
                        Some("success"),
                    );
                }
                BridgeEvent::Failed { detail, .. } => {
                    panic!("unexpected Failed event: {detail:?}");
                }
                _ => {}
            }
        }
    }
    assert!(completed, "expected a Completed event from fake karma");

    bridge.shutdown(Duration::from_secs(1)).await.unwrap();

    // The fake recorded the argv it was launched with: it must match the
    // Claude Code headless contract the Karma agent reuses.
    let argv = std::fs::read_to_string(cwd.join("argv.txt")).unwrap();
    for expected in [
        "--worktree",
        "--print",
        "--input-format",
        "stream-json",
        "--output-format",
        "--include-partial-messages",
    ] {
        assert!(
            argv.split_whitespace().any(|a| a == expected),
            "expected {expected:?} in karma argv:\n{argv}"
        );
    }
}
