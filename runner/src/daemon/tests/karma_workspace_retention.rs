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

#[cfg(target_os = "linux")]
async fn cancel_at_phase(phase: &str) {
    let temp = tempfile::tempdir().unwrap();
    let paths = paths_for(temp.path());
    paths.ensure().unwrap();
    let project_root = temp.path().join("project");
    std::fs::create_dir(&project_root).unwrap();
    let ready = temp.path().join("ready.pid");
    let descendant = temp.path().join("child.pid");
    let binary = temp.path().join("fake-karma");
    let repo_url = if phase == "push" {
        let (remote, operation_dir) = seed_explicit_branch_workspace(
            temp.path(),
            &project_root,
            &temp.path().join("project-created"),
        );
        let hook = operation_dir.join(".git/hooks/pre-push");
        std::fs::write(
            &hook,
            format!(
                "#!/bin/sh\ntrap '' INT TERM\nsh -c 'trap \"\" INT TERM; echo $$ > \"$1\"; exec sleep 300' child '{}' &\necho $$ > '{}'\nwait\n",
                descendant.display(), ready.display()
            ),
        ).unwrap();
        std::fs::set_permissions(&hook, std::fs::Permissions::from_mode(0o755)).unwrap();
        Some(remote.to_string_lossy().into_owned())
    } else {
        None
    };
    let script = format!(
        r#"#!/bin/sh
set -eu
block() {{
  trap '' INT TERM
  sh -c 'trap "" INT TERM; echo $$ > "$1"; exec sleep 300' child '{}' &
  echo $$ > '{}'
  wait
}}
if [ "$1" = project ]; then
  if [ '{}' = setup ]; then block; fi
  if [ "$2" = list ]; then
    printf '📁 %s\n   ID: demo\n' '{}'
  else
    printf '📁 %s\nPath: %s\n' '{}' '{}'
  fi
  exit 0
fi
IFS= read -r input
if [ '{phase}' = push ]; then
  printf '%s\n' '{{"type":"system","subtype":"init","session_id":"abort-test"}}'
  printf '%s\n' '{{"type":"result","subtype":"success","session_id":"abort-test","result":"done"}}'
  exit 0
fi
if [ '{phase}' = running ]; then
  printf '%s\n' '{{"type":"system","subtype":"init","session_id":"abort-test"}}'
fi
block
"#,
        descendant.display(),
        ready.display(),
        phase,
        project_name(&project_root),
        project_name(&project_root),
        project_root.display()
    );
    std::fs::write(&binary, script).unwrap();
    std::fs::set_permissions(&binary, std::fs::Permissions::from_mode(0o755)).unwrap();
    let mut config = runner_config("main", "DEMO", temp.path().join("unused"));
    config.project_slug = None;
    config.agent.kind = AgentKind::Karma;
    config.karma.binary = binary.to_string_lossy().into_owned();
    let runner_paths = paths.for_runner(config.runner_id);
    runner_paths.ensure().unwrap();
    let state = StateHandle::new(Config {
        version: 2,
        daemon: Default::default(),
        runners: vec![config.clone()],
        cli: None,
    });
    let cancel = tokio_util::sync::CancellationToken::new();
    let (tx, mut rx) = mpsc::channel(64);
    let mut worker = AssignWorker {
        runner_paths,
        daemon_paths: paths,
        runner_config: config.clone(),
        state: state.clone(),
        approvals: ApprovalRouter::new(),
        out: RunnerOut::new(config.runner_id, tx),
        cancel: cancel.clone(),
    };
    let name = project_name(&project_root);
    let task = tokio::spawn(async move {
        worker
            .run(AssignedRun {
                run_id: uuid::Uuid::new_v4(),
                issue_identifier: Some("DEMO-10".into()),
                project_name: Some(name),
                prompt: "test cancellation".into(),
                repo_ref: repo_url.as_ref().map(|_| "main".into()),
                git_work_branch: repo_url.as_ref().map(|_| EXPLICIT_WORK_BRANCH.into()),
                repo_url,
                expected_codex_model: None,
            })
            .await
    });
    let pids: Vec<u32> = tokio::time::timeout(Duration::from_secs(60), async {
        loop {
            let read_pid = |path: &Path| {
                std::fs::read_to_string(path)
                    .ok()?
                    .trim()
                    .parse::<u32>()
                    .ok()
            };
            if let (Some(parent), Some(child)) = (read_pid(&ready), read_pid(&descendant)) {
                break vec![parent, child];
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("fake Karma did not reach cancellation phase");
    let mut messages = Vec::new();
    if phase == "running" {
        tokio::time::timeout(Duration::from_secs(5), async {
            while let Some(envelope) = rx.recv().await {
                let started = matches!(envelope.body, ClientMsg::RunStarted { .. });
                messages.push(envelope.body);
                if started {
                    break;
                }
            }
        })
        .await
        .unwrap();
    }
    cancel.cancel();
    tokio::time::timeout(Duration::from_secs(10), task)
        .await
        .expect("abort did not stop the assignment promptly")
        .unwrap()
        .unwrap();
    while let Ok(envelope) = rx.try_recv() {
        messages.push(envelope.body);
    }
    assert_eq!(
        messages
            .iter()
            .filter(|msg| matches!(msg, ClientMsg::RunCancelled { .. }))
            .count(),
        1
    );
    assert!(!messages.iter().any(|msg| matches!(
        msg,
        ClientMsg::RunCompleted { .. } | ClientMsg::RunFailed { .. }
    )));
    assert!(state.runner_snapshot().await.current_run.is_none());
    tokio::time::timeout(Duration::from_secs(2), async {
        loop {
            let any_running = pids.iter().any(|pid| {
                std::fs::read_to_string(format!("/proc/{pid}/stat"))
                    .ok()
                    .and_then(|stat| {
                        stat.rsplit_once(") ")
                            .map(|(_, rest)| !rest.starts_with('Z'))
                    })
                    .unwrap_or(false)
            });
            if !any_running {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("Karma or its descendant survived abort");
}

#[cfg(target_os = "linux")]
#[tokio::test]
async fn abort_interrupts_workspace_preparation() {
    cancel_at_phase("setup").await;
}

#[cfg(target_os = "linux")]
#[tokio::test]
async fn abort_interrupts_agent_initialization() {
    cancel_at_phase("startup").await;
}

#[cfg(target_os = "linux")]
#[tokio::test]
async fn abort_stops_running_agent_and_descendants_before_acknowledging() {
    cancel_at_phase("running").await;
}

#[cfg(target_os = "linux")]
#[tokio::test]
async fn abort_interrupts_git_push_after_agent_completion() {
    cancel_at_phase("push").await;
}

fn project_name(project_root: &Path) -> String {
    // Project discovery takes a per-user/name lock, even across processes.
    // Independent fixtures must not serialize behind the same project name.
    format!("Demo Project {}", project_root.display())
}

fn fake_karma_script(
    project_root: &Path,
    state_file: &Path,
    ending: Ending,
    expected_branch: Option<&str>,
) -> String {
    let result_event = match ending {
        Ending::Failed => {
            r#"{"type":"result","subtype":"error_during_execution","session_id":"sess_retention","is_error":true,"result":"intentional failure"}"#
        }
        Ending::Cancelled => "",
        Ending::Completed => {
            r#"{"type":"result","subtype":"success","session_id":"sess_retention","result":"done","usage":{"input_tokens":1,"output_tokens":1}}"#
        }
    };
    let branch_setup = match expected_branch {
        Some(branch) => format!(
            "case \" $* \" in\n  *\" --worktree \"*) exit 41 ;;\nesac\n\
             test \"$(git branch --show-current)\" = '{branch}'\n\
             printf 'user-visible result' > result.txt\n\
             git add -- result.txt\n\
             git -c user.name='Karma Test' -c user.email='karma@example.invalid' commit -m 'agent result' >/dev/null"
        ),
        None => "case \" $* \" in\n  *\" --worktree \"*) ;;\n  *) exit 1 ;;\nesac\n\
                 printf 'user-visible result' > result.txt"
            .to_string(),
    };
    format!(
        r#"#!/bin/sh
set -eu
project_root='{}'
state_file='{}'
project_name='{}'
if [ "$1" = project ] && [ "$2" = list ]; then
  if [ -f "$state_file" ]; then
    printf 'Fetching projects...\n\n📁 %s\n   ID: demo-id\n   Path: %s\n' "$project_name" "$project_root"
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
  printf '📁 %s\nPath: %s\n' "$project_name" "$project_root"
  exit 0
fi
git rev-parse --verify HEAD >/dev/null 2>&1
{}
(cat >/dev/null) &
printf '%s\n' '{{"type":"system","subtype":"init","session_id":"sess_retention","model":"claude-sonnet-4-6"}}'
printf '%s\n' '{}'
sleep {}
"#,
        project_root.display(),
        state_file.display(),
        project_name(project_root),
        branch_setup,
        result_event,
        if matches!(ending, Ending::Cancelled) {
            "300"
        } else {
            "0.2"
        },
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
        fake_karma_script(&project_root, &state_file, ending, None),
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
        cancel: cancel.clone(),
    };
    let name = project_name(&project_root);
    let worker_task = tokio::spawn(async move {
        worker
            .run(AssignedRun {
                run_id: uuid::Uuid::new_v4(),
                issue_identifier: Some("DEMO-10".into()),
                project_name: Some(name),
                prompt: "Create a user-visible result".into(),
                repo_url,
                repo_ref: None,
                git_work_branch: None,
                expected_codex_model: None,
            })
            .await
    });
    let messages = tokio::time::timeout(Duration::from_secs(90), async {
        let mut messages = Vec::new();
        while let Some(envelope) = rx.recv().await {
            if matches!(ending, Ending::Cancelled)
                && matches!(envelope.body, ClientMsg::RunStarted { .. })
            {
                cancel.cancel();
            }
            messages.push(envelope.body);
        }
        messages
    })
    .await
    .expect("Karma assignment timed out");
    worker_task
        .await
        .unwrap()
        .expect("Karma assignment failed internally");
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

const EXPLICIT_WORK_BRANCH: &str = "feature/team-work";

struct ExplicitBranchRun {
    _temp: tempfile::TempDir,
    remote: PathBuf,
    operation_dir: PathBuf,
    messages: Vec<ClientMsg>,
    completed_tips_at_delivery: Option<(Option<String>, String)>,
    completed_base_tip_at_delivery: Option<String>,
}

fn git_output(cwd: &Path, args: &[&str]) -> std::process::Output {
    std::process::Command::new("git")
        .current_dir(cwd)
        .args(args)
        .output()
        .unwrap_or_else(|error| panic!("failed to run git {args:?}: {error}"))
}

fn git(cwd: &Path, args: &[&str]) -> String {
    let output = git_output(cwd, args);
    assert!(
        output.status.success(),
        "git {args:?} failed ({}): {}",
        output.status,
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8_lossy(&output.stdout).trim().to_string()
}

fn remote_branch_tip(root: &Path, remote: &Path, branch: &str) -> Option<String> {
    let remote = remote.to_string_lossy();
    let reference = format!("refs/heads/{branch}");
    let output = git_output(
        root,
        &[
            "--git-dir",
            remote.as_ref(),
            "rev-parse",
            "--verify",
            &reference,
        ],
    );
    output
        .status
        .success()
        .then(|| String::from_utf8_lossy(&output.stdout).trim().to_string())
}

fn seed_explicit_branch_workspace(
    root: &Path,
    project_root: &Path,
    state_file: &Path,
) -> (PathBuf, PathBuf) {
    let remote = root.join("remote.git");
    let seed = root.join("seed");
    let remote_arg = remote.to_string_lossy();
    let seed_arg = seed.to_string_lossy();
    git(root, &["init", "--bare", remote_arg.as_ref()]);
    git(root, &["init", seed_arg.as_ref()]);
    git(&seed, &["checkout", "-b", "main"]);
    std::fs::write(seed.join("README.md"), "base\n").expect("write seed file");
    git(&seed, &["add", "README.md"]);
    git(
        &seed,
        &[
            "-c",
            "user.name=Karma Test",
            "-c",
            "user.email=karma@example.invalid",
            "commit",
            "-m",
            "base",
        ],
    );
    git(&seed, &["remote", "add", "origin", remote_arg.as_ref()]);
    git(&seed, &["push", "-u", "origin", "main"]);
    git(
        root,
        &[
            "--git-dir",
            remote_arg.as_ref(),
            "symbolic-ref",
            "HEAD",
            "refs/heads/main",
        ],
    );

    let operation_dir = project_root.join("main").join("DEMO-10");
    std::fs::create_dir_all(operation_dir.parent().expect("operation parent"))
        .expect("create operation parent");
    let operation_arg = operation_dir.to_string_lossy();
    git(
        root,
        &["clone", remote_arg.as_ref(), operation_arg.as_ref()],
    );
    std::fs::write(state_file, "created\n").expect("mark Karma project as existing");
    (remote, operation_dir)
}

fn reject_work_branch_pushes(remote: &Path) {
    let hook = remote.join("hooks").join("pre-receive");
    std::fs::write(
        &hook,
        format!(
            "#!/bin/sh\nwhile read old new ref; do\n  if [ \"$ref\" = \"refs/heads/{EXPLICIT_WORK_BRANCH}\" ]; then\n    exit 1\n  fi\ndone\nexit 0\n"
        ),
    )
    .expect("write rejecting pre-receive hook");
    std::fs::set_permissions(&hook, std::fs::Permissions::from_mode(0o755))
        .expect("make pre-receive hook executable");
}

async fn run_explicit_branch_case(
    ending: Ending,
    reject_push: bool,
    base_branch: &'static str,
) -> ExplicitBranchRun {
    let temp = tempfile::tempdir().expect("temp dir");
    let root = temp.path().to_path_buf();
    let paths = paths_for(&root);
    paths.ensure().expect("runner paths");

    let project_root = root.join("karma-project");
    std::fs::create_dir_all(&project_root).expect("create Karma project root");
    let state_file = root.join("project-created");
    let (remote, operation_dir) = seed_explicit_branch_workspace(&root, &project_root, &state_file);
    if reject_push {
        reject_work_branch_pushes(&remote);
    }

    let binary = root.join("fake-karma");
    std::fs::write(
        &binary,
        fake_karma_script(
            &project_root,
            &state_file,
            ending,
            Some(EXPLICIT_WORK_BRANCH),
        ),
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
    // Capacity one plus live consumption makes a lifecycle send observable at
    // the instant it occurs. In particular, a RunCompleted cannot hide a
    // later push attempt behind a fully buffered worker execution.
    let (tx, mut rx) = mpsc::channel(1);
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
        cancel: cancel.clone(),
    };
    let remote_url = remote.to_string_lossy().into_owned();
    let project_name = project_name(&project_root);
    let worker_task = tokio::spawn(async move {
        worker
            .run(AssignedRun {
                run_id: uuid::Uuid::new_v4(),
                issue_identifier: Some("DEMO-10".into()),
                project_name: Some(project_name),
                prompt: "Create and commit a user-visible result".into(),
                repo_url: Some(remote_url),
                repo_ref: Some(base_branch.into()),
                git_work_branch: Some(EXPLICIT_WORK_BRANCH.into()),
                expected_codex_model: None,
            })
            .await
    });

    let mut completed_tips_at_delivery = None;
    let mut completed_base_tip_at_delivery = None;
    let messages = tokio::time::timeout(Duration::from_secs(90), async {
        let mut messages = Vec::new();
        while let Some(envelope) = rx.recv().await {
            let message = envelope.body;
            if matches!(ending, Ending::Cancelled)
                && matches!(message, ClientMsg::RunStarted { .. })
            {
                cancel.cancel();
            }
            if let ClientMsg::Accept {
                workspace_state, ..
            } = &message
            {
                assert_eq!(
                    workspace_state.branch.as_deref(),
                    Some(EXPLICIT_WORK_BRANCH),
                    "Accept must describe the branch already prepared for the agent"
                );
                if base_branch != "main" {
                    assert_eq!(remote_branch_tip(&root, &remote, base_branch), None);
                    assert_eq!(git(&operation_dir, &["ls-tree", "-r", base_branch]), "");
                }
            }
            if matches!(&message, ClientMsg::RunCompleted { .. }) {
                completed_base_tip_at_delivery = remote_branch_tip(&root, &remote, base_branch);
                completed_tips_at_delivery = Some((
                    remote_branch_tip(&root, &remote, EXPLICIT_WORK_BRANCH),
                    git(&operation_dir, &["rev-parse", "HEAD"]),
                ));
            }
            messages.push(message);
        }
        messages
    })
    .await
    .expect("explicit branch assignment timed out");
    worker_task
        .await
        .expect("explicit branch worker task panicked")
        .expect("explicit branch assignment failed internally");

    ExplicitBranchRun {
        _temp: temp,
        remote,
        operation_dir,
        messages,
        completed_tips_at_delivery,
        completed_base_tip_at_delivery,
    }
}

#[tokio::test]
async fn explicit_work_branch_runs_without_nested_worktree_and_pushes_before_completion() {
    let run = run_explicit_branch_case(Ending::Completed, false, "main").await;
    let completed_count = run
        .messages
        .iter()
        .filter(|message| matches!(message, ClientMsg::RunCompleted { .. }))
        .count();
    assert_eq!(completed_count, 1, "expected one RunCompleted message");
    assert!(
        !run.messages
            .iter()
            .any(|message| matches!(message, ClientMsg::RunFailed { .. })),
        "successful push must not also report RunFailed"
    );
    let (remote_tip, local_tip) = run
        .completed_tips_at_delivery
        .expect("RunCompleted was not observed live");
    assert_eq!(
        remote_tip.as_deref(),
        Some(local_tip.as_str()),
        "work branch must already be on origin when RunCompleted is sent"
    );
    assert_eq!(
        git(&run.operation_dir, &["branch", "--show-current"]),
        EXPLICIT_WORK_BRANCH
    );
}

#[tokio::test]
async fn rejected_work_branch_push_reports_only_run_failed_and_keeps_local_commit() {
    let run = run_explicit_branch_case(Ending::Completed, true, "main").await;
    assert!(
        run.messages
            .iter()
            .any(|message| matches!(message, ClientMsg::RunFailed { detail: Some(detail), .. } if detail.contains("pushing git work branch"))),
        "a rejected push must be surfaced as RunFailed: {:?}",
        run.messages
    );
    assert!(
        !run.messages
            .iter()
            .any(|message| matches!(message, ClientMsg::RunCompleted { .. })),
        "RunCompleted must not be sent before a rejected push"
    );
    assert_eq!(
        remote_branch_tip(run._temp.path(), &run.remote, EXPLICIT_WORK_BRANCH),
        None
    );
    assert!(
        run.operation_dir.join("result.txt").is_file(),
        "push failure must preserve the agent's local work"
    );
    assert_eq!(
        git(&run.operation_dir, &["branch", "--show-current"]),
        EXPLICIT_WORK_BRANCH
    );
}

#[tokio::test]
async fn failed_and_cancelled_runs_do_not_push_explicit_work_branch() {
    for ending in [Ending::Failed, Ending::Cancelled] {
        let run = run_explicit_branch_case(ending, false, "main").await;
        let expected_terminal = match ending {
            Ending::Failed => run
                .messages
                .iter()
                .any(|message| matches!(message, ClientMsg::RunFailed { .. })),
            Ending::Cancelled => run
                .messages
                .iter()
                .any(|message| matches!(message, ClientMsg::RunCancelled { .. })),
            Ending::Completed => unreachable!(),
        };
        assert!(
            expected_terminal,
            "missing terminal message for {ending:?}: {:?}",
            run.messages
        );
        assert!(
            !run.messages
                .iter()
                .any(|message| matches!(message, ClientMsg::RunCompleted { .. })),
            "{ending:?} run must not report completion"
        );
        assert_eq!(
            remote_branch_tip(run._temp.path(), &run.remote, EXPLICIT_WORK_BRANCH),
            None,
            "{ending:?} run must not publish unfinished work"
        );
        assert!(
            run.operation_dir.join("result.txt").is_file(),
            "the fake agent must commit before its {ending:?} terminal event"
        );
    }
}

#[tokio::test]
async fn missing_base_and_work_branch_are_both_published_before_run_completed() {
    let run = run_explicit_branch_case(Ending::Completed, false, "empty-base").await;
    let base = run
        .completed_base_tip_at_delivery
        .expect("base must be published before completion");
    let (remote_tip, local_tip) = run.completed_tips_at_delivery.expect("run must complete");
    assert_eq!(remote_tip.as_deref(), Some(local_tip.as_str()));
    assert_eq!(git(&run.operation_dir, &["rev-parse", "HEAD^"]), base);
    assert_eq!(
        git(&run.operation_dir, &["ls-tree", "-r", "empty-base"]),
        ""
    );
    assert!(!run.operation_dir.join("README.md").exists());
}

#[tokio::test]
async fn rejected_atomic_push_keeps_both_new_branches_local_and_reports_failure() {
    let run = run_explicit_branch_case(Ending::Completed, true, "empty-base").await;
    assert!(run.messages.iter().any(|message| matches!(message, ClientMsg::RunFailed { detail: Some(detail), .. } if detail.contains("pushing git work branch"))));
    assert!(
        !run.messages
            .iter()
            .any(|message| matches!(message, ClientMsg::RunCompleted { .. }))
    );
    for branch in ["empty-base", EXPLICIT_WORK_BRANCH] {
        assert!(remote_branch_tip(run._temp.path(), &run.remote, branch).is_none());
        assert!(!git(&run.operation_dir, &["rev-parse", branch]).is_empty());
    }
    assert!(run.operation_dir.join("result.txt").is_file());
}

#[tokio::test]
async fn unsuccessful_tasks_never_publish_the_empty_base() {
    for ending in [Ending::Failed, Ending::Cancelled] {
        let run = run_explicit_branch_case(ending, false, "empty-base").await;
        assert!(
            !run.messages
                .iter()
                .any(|message| matches!(message, ClientMsg::RunCompleted { .. }))
        );
        for branch in ["empty-base", EXPLICIT_WORK_BRANCH] {
            assert!(remote_branch_tip(run._temp.path(), &run.remote, branch).is_none());
        }
        assert!(run.operation_dir.join("result.txt").is_file());
    }
}
