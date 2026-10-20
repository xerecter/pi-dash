//! End-to-end retention tests for Karma issue-run workspaces.

use super::tests::{paths_for, runner_config};
use super::*;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::time::Duration;

#[derive(Clone, Copy, Debug)]
enum Ending {
    Completed,
    Failed,
    Cancelled,
}

fn fake_karma_script(project_root: &Path, state_file: &Path, ending: Ending) -> String {
    let result_event = match ending {
        Ending::Failed => {
            r#"{"type":"result","subtype":"error_during_execution","session_id":"sess_retention","is_error":true,"result":"intentional failure"}"#
        }
        Ending::Completed | Ending::Cancelled => {
            r#"{"type":"result","subtype":"success","session_id":"sess_retention","result":"done","usage":{"input_tokens":1,"output_tokens":1}}"#
        }
    };
    format!(
        r#"#!/bin/sh
set -eu
project_root='{}'
state_file='{}'
if [ "$1" = project ] && [ "$2" = list ]; then
  if [ -f "$state_file" ]; then
    printf 'Fetching projects...\n\n📁 Demo Project\n   ID: demo-id\n   Path: %s\n' "$project_root"
  else
    printf 'Fetching projects...\nNo projects found.\n'
  fi
  exit 0
fi
if [ "$1" = project ] && [ "$2" = create ]; then
  test "$#" -eq 5
  test "$4" = --no-session
  test "$5" = --no-git
  mkdir -p "$project_root"
  : > "$state_file"
  printf 'Project ID: demo-id\n'
  exit 0
fi
if [ "$1" = project ] && [ "$2" = info ]; then
  printf '📁 Demo Project\nPath: %s\n' "$project_root"
  exit 0
fi
git rev-parse --verify HEAD >/dev/null 2>&1
case " $* " in
  *" --worktree "*) ;;
  *) exit 1 ;;
esac
printf 'user-visible result' > result.txt
(cat >/dev/null) &
printf '%s\n' '{{"type":"system","subtype":"init","session_id":"sess_retention","model":"claude-sonnet-4-6"}}'
printf '%s\n' '{}'
sleep 0.2
"#,
        project_root.display(),
        state_file.display(),
        result_event,
    )
}

async fn run_case_with_repo_url(
    ending: Ending,
    repo_url: Option<String>,
) -> (tempfile::TempDir, PathBuf, ClientMsg) {
    let temp = tempfile::tempdir().expect("temp dir");
    let root = temp.path().to_path_buf();
    let paths = paths_for(&root);
    paths.ensure().expect("runner paths");

    let project_root = root.join("karma-project");
    let state_file = root.join("project-created");
    let binary = root.join("fake-karma");
    std::fs::write(
        &binary,
        fake_karma_script(&project_root, &state_file, ending),
    )
    .expect("write fake karma");
    std::fs::set_permissions(&binary, std::fs::Permissions::from_mode(0o755))
        .expect("make fake karma executable");

    let mut config = runner_config("main", "DEMO", root.join("unused-runner-workspace"));
    config.project_slug = None;
    config.agent.kind = AgentKind::Karma;
    config.karma.binary = binary.to_string_lossy().into_owned();
    let runner_paths = paths.for_runner(config.runner_id);
    runner_paths.ensure().expect("runner data paths");

    let cancel = tokio_util::sync::CancellationToken::new();
    if matches!(ending, Ending::Cancelled) {
        cancel.cancel();
    }
    let (tx, mut rx) = mpsc::channel(32);
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
        cancel,
    };
    tokio::time::timeout(
        // Karma invokes several CLI commands through the login shell before
        // the assignment starts. This can exceed 30 seconds on machines with
        // slow shell initialization (e.g. conda hooks), so leave room for the
        // actual workspace/clone path instead of timing out during setup.
        Duration::from_secs(90),
        worker.run(
            uuid::Uuid::new_v4(),
            Some("DEMO-10".into()),
            Some("Demo Project".into()),
            "Create a user-visible result".into(),
            repo_url,
            None,
        ),
    )
    .await
    .expect("Karma assignment timed out")
    .expect("Karma assignment failed internally");

    let messages: Vec<_> = std::iter::from_fn(|| rx.try_recv().ok())
        .map(|envelope| envelope.body)
        .collect();
    assert!(
        messages
            .iter()
            .any(|message| matches!(message, ClientMsg::Accept { .. })),
        "Karma assignment was not accepted before execution"
    );
    let terminal = messages
        .into_iter()
        .find(|message| {
            matches!(
                message,
                ClientMsg::RunCompleted { .. }
                    | ClientMsg::RunFailed { .. }
                    | ClientMsg::RunCancelled { .. }
            )
        })
        .expect("worker sent no terminal message");
    (temp, project_root, terminal)
}

async fn run_case(ending: Ending) -> (tempfile::TempDir, PathBuf, ClientMsg) {
    run_case_with_repo_url(ending, None).await
}

#[tokio::test]
async fn karma_project_and_work_item_files_survive_every_terminal_outcome() {
    for ending in [Ending::Completed, Ending::Failed, Ending::Cancelled] {
        let (_temp, project_root, terminal) = run_case(ending).await;
        match (ending, terminal) {
            (Ending::Completed, ClientMsg::RunCompleted { .. })
            | (Ending::Failed, ClientMsg::RunFailed { .. })
            | (Ending::Cancelled, ClientMsg::RunCancelled { .. }) => {}
            (expected, actual) => panic!("expected {expected:?} terminal message, got {actual:?}"),
        }

        let operation_dir = project_root.join("main").join("DEMO-10");
        assert!(project_root.is_dir(), "Karma project must be retained");
        assert!(
            operation_dir.is_dir(),
            "work item directory must be retained"
        );
        assert!(
            operation_dir.join(".git").exists(),
            "Karma work item directory needs a Git base for --worktree"
        );
        assert!(
            std::process::Command::new("git")
                .current_dir(&operation_dir)
                .args(["rev-parse", "--verify", "HEAD"])
                .output()
                .expect("inspect Karma workspace HEAD")
                .status
                .success(),
            "Karma worktree base must have an initial commit"
        );
        assert_eq!(
            std::fs::read_to_string(operation_dir.join("result.txt"))
                .expect("work item result must be retained"),
            "user-visible result"
        );
    }
}

#[tokio::test]
async fn clone_failure_does_not_block_a_karma_agent_assignment() {
    let (_temp, project_root, terminal) = run_case_with_repo_url(
        Ending::Completed,
        Some("https://127.0.0.1:1/unreachable.git".into()),
    )
    .await;

    assert!(
        matches!(terminal, ClientMsg::RunCompleted { .. }),
        "clone failure must reach the Karma agent, got {terminal:?}"
    );
    let operation_dir = project_root.join("main").join("DEMO-10");
    assert_eq!(
        std::fs::read_to_string(operation_dir.join("result.txt"))
            .expect("Karma agent result must survive"),
        "user-visible result"
    );
}
