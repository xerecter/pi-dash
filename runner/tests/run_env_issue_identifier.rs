//! `KARMA_TEAM_ISSUE_IDENTIFIER` plumbing: the `assign` wire field decodes (and
//! stays optional for older clouds), and a run scope exports it onto every
//! agent subprocess.

use karma_team::cloud::protocol::{FailureReason, ServerMsg};
use karma_team::util::shell::{
    ISSUE_IDENTIFIER_ENV, RUN_ENV, RUN_ID_ENV, RunEnv, login_shell_command,
};
use std::ffi::OsString;
use tokio::process::Command;
use uuid::Uuid;

fn env_value(cmd: &Command, key: &str) -> Option<OsString> {
    cmd.as_std()
        .get_envs()
        .find(|(k, _)| *k == key)
        .and_then(|(_, v)| v.map(|v| v.to_os_string()))
}

async fn command_in_run_scope(issue_identifier: Option<&str>) -> (Uuid, Command) {
    let run_id = Uuid::new_v4();
    let env = RunEnv {
        run_id,
        issue_identifier: issue_identifier.map(str::to_string),
    };
    let cmd = RUN_ENV
        .scope(env, async {
            login_shell_command("codex", &["app-server"], None)
        })
        .await;
    (run_id, cmd)
}

#[tokio::test]
async fn run_scope_exports_issue_identifier_alongside_run_id() {
    let (run_id, cmd) = command_in_run_scope(Some("PIDAS-10")).await;
    assert_eq!(env_value(&cmd, RUN_ID_ENV), Some(run_id.to_string().into()));
    assert_eq!(
        env_value(&cmd, ISSUE_IDENTIFIER_ENV),
        Some("PIDAS-10".into())
    );
}

#[tokio::test]
async fn run_scope_without_identifier_does_not_export_it() {
    let (_, cmd) = command_in_run_scope(None).await;
    assert!(env_value(&cmd, RUN_ID_ENV).is_some());
    assert_eq!(env_value(&cmd, ISSUE_IDENTIFIER_ENV), None);
}

#[tokio::test]
async fn blank_identifier_is_not_exported() {
    let (_, cmd) = command_in_run_scope(Some("  ")).await;
    assert_eq!(env_value(&cmd, ISSUE_IDENTIFIER_ENV), None);
}

#[test]
fn no_run_scope_exports_neither_variable() {
    // Chat sessions never enter a run scope.
    let cmd = login_shell_command("codex", &["app-server"], None);
    assert_eq!(env_value(&cmd, RUN_ID_ENV), None);
    assert_eq!(env_value(&cmd, ISSUE_IDENTIFIER_ENV), None);
}

fn assign_json(extra: serde_json::Value) -> serde_json::Value {
    let mut body = serde_json::json!({
        "v": 1,
        "type": "assign",
        "run_id": Uuid::new_v4(),
        "work_item_id": Uuid::new_v4(),
        "prompt": "p",
        "repo_url": null,
        "repo_ref": null,
        "expected_codex_model": null,
        "approval_policy_overrides": null,
        "deadline": null,
    });
    body.as_object_mut()
        .unwrap()
        .extend(extra.as_object().unwrap().clone());
    body
}

#[test]
fn assign_decodes_work_item_identifier() {
    let raw = assign_json(serde_json::json!({"work_item_identifier": "PIDAS-10"}));
    match serde_json::from_value::<ServerMsg>(raw).unwrap() {
        ServerMsg::Assign {
            work_item_identifier,
            ..
        } => {
            assert_eq!(work_item_identifier.as_deref(), Some("PIDAS-10"));
        }
        other => panic!("expected Assign, got {other:?}"),
    }
}

#[test]
fn assign_from_older_cloud_without_identifier_still_decodes() {
    match serde_json::from_value::<ServerMsg>(assign_json(serde_json::json!({}))).unwrap() {
        ServerMsg::Assign {
            work_item_identifier,
            ..
        } => assert_eq!(work_item_identifier, None),
        other => panic!("expected Assign, got {other:?}"),
    }
}

/// The cloud tags the run by matching this literal; it is a cross-language
/// contract (`AWAITING_HUMAN_INPUT_ERROR_CODE` in `runner/models.py`).
#[test]
fn awaiting_human_input_wire_name_is_stable() {
    let json = serde_json::to_value(FailureReason::AwaitingHumanInput).unwrap();
    assert_eq!(json, serde_json::json!("awaiting_human_input"));
}
