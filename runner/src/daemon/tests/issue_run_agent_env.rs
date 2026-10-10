//! End-to-end issue-run tests for the agent environment and the
//! unanswered-question guard.
//!
//! A shell script stands in for `codex app-server`: it completes the
//! JSON-RPC handshake, then emits `agentMessage` items that carry a
//! `questions` block — the shape Codex produced when an unattended run
//! looped for hours re-asking the same question. The script echoes
//! `$KARMA_TEAM_ISSUE_IDENTIFIER` into each question so the tests can also
//! prove the issue key reached the agent process.

use super::tests::{paths_for, runner_config};
use super::*;
use crate::daemon::question_guard::MAX_UNANSWERED_QUESTIONS;
use std::time::Duration;

/// How the fake agent ends after asking its questions.
enum Ending {
    /// Keep the turn open forever, like the real looping agent.
    Hang,
    /// Close the turn cleanly.
    Complete,
}

fn fake_codex_script(questions: u32, ending: Ending) -> String {
    let ending = match ending {
        Ending::Hang => "sleep 60",
        Ending::Complete => {
            r#"printf '%s\n' '{"jsonrpc":"2.0","method":"turn/completed","params":{"conclusion":"success"}}'
sleep 1"#
        }
    };
    format!(
        r#"#!/bin/sh
read _
printf '%s\n' '{{"jsonrpc":"2.0","id":1,"result":{{}}}}'
read _
read _
printf '%s\n' '{{"jsonrpc":"2.0","id":2,"result":{{"threadId":"th_question_loop"}}}}'
read _
i=0
while [ "$i" -lt {questions} ]; do
  i=$((i+1))
  item='{{"id":"q'"$i"'","type":"agentMessage","phase":"final_answer","delivery":"async","text":"Confirm for '"${{KARMA_TEAM_ISSUE_IDENTIFIER:-unset}}"'","questions":[{{"title":"Confirm","options":["stop","implement"]}}]}}'
  # Codex sends the item on both started and completed; only the latter
  # may count, or every question would be counted twice.
  printf '{{"jsonrpc":"2.0","method":"item/started","params":{{"item":%s}}}}\n' "$item"
  printf '{{"jsonrpc":"2.0","method":"item/completed","params":{{"item":%s}}}}\n' "$item"
done
{ending}
"#
    )
}

/// Run one assignment against the fake agent and return the terminal
/// message the worker sent to the cloud.
async fn run_assignment(script: String, issue_identifier: Option<&str>) -> ClientMsg {
    let tmp = tempfile::tempdir().unwrap();
    let paths = paths_for(tmp.path());
    paths.ensure().unwrap();
    let wd = tmp.path().join("task-folder");
    std::fs::create_dir_all(&wd).unwrap();

    let binary = tmp.path().join("fake-codex");
    std::fs::write(&binary, script).unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&binary, std::fs::Permissions::from_mode(0o755)).unwrap();
    }

    let mut config = runner_config("main", "PIDAS", wd);
    config.project_slug = None; // no cloud context lookup
    config.agent.kind = AgentKind::Codex;
    config.codex.binary = binary.to_string_lossy().into_owned();
    let runner_paths = paths.for_runner(config.runner_id);
    runner_paths.ensure().unwrap();

    let (tx, mut rx) = mpsc::channel(256);
    let mut worker = AssignWorker {
        runner_paths,
        daemon_paths: paths,
        runner_config: config.clone(),
        state: StateHandle::new(Config {
            version: 2,
            daemon: Default::default(),
            runners: vec![config.clone()],
            cli: None,
        }),
        approvals: ApprovalRouter::new(),
        out: RunnerOut::new(config.runner_id, tx),
        cancel: tokio_util::sync::CancellationToken::new(),
    };
    tokio::time::timeout(
        Duration::from_secs(30),
        worker.run(
            uuid::Uuid::new_v4(),
            issue_identifier.map(str::to_string),
            "Build the logistics portal".into(),
            None,
            None,
        ),
    )
    .await
    .expect("run must end on its own instead of looping")
    .unwrap();

    // `run` has returned, so everything it sent is already queued.
    let mut terminal = None;
    while let Ok(env) = rx.try_recv() {
        if matches!(
            env.body,
            ClientMsg::RunFailed { .. } | ClientMsg::RunCompleted { .. }
        ) {
            terminal = Some(env.body);
        }
    }
    terminal.expect("worker sent no terminal message")
}

#[cfg(not(target_os = "windows"))]
#[tokio::test]
async fn legacy_codex_issue_run_is_rejected_before_question_processing() {
    let msg = run_assignment(fake_codex_script(10, Ending::Hang), Some("PIDAS-10")).await;
    match msg {
        ClientMsg::RunFailed { reason, detail, .. } => {
            assert_eq!(reason, FailureReason::CodexCrash);
            let detail = detail.unwrap_or_default();
            assert!(
                detail.contains("only the karma agent is supported"),
                "{detail}"
            );
        }
        other => panic!("expected RunFailed, got {other:?}"),
    }
}

#[cfg(not(target_os = "windows"))]
#[tokio::test]
async fn legacy_codex_issue_run_never_launches_its_binary() {
    let msg = run_assignment(
        fake_codex_script(MAX_UNANSWERED_QUESTIONS - 1, Ending::Complete),
        Some("PIDAS-10"),
    )
    .await;
    match msg {
        ClientMsg::RunFailed { detail, .. } => {
            assert!(
                detail
                    .unwrap_or_default()
                    .contains("only the karma agent is supported")
            );
        }
        other => panic!("expected policy failure, got {other:?}"),
    }
}

#[cfg(not(target_os = "windows"))]
#[tokio::test]
async fn legacy_codex_issue_run_is_rejected_even_without_identifier() {
    let msg = run_assignment(fake_codex_script(10, Ending::Hang), None).await;
    match msg {
        ClientMsg::RunFailed { detail, .. } => {
            assert!(
                detail
                    .unwrap_or_default()
                    .contains("only the karma agent is supported")
            );
        }
        other => panic!("expected RunFailed, got {other:?}"),
    }
}
