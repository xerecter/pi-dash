use anyhow::{Context, Result};
use fs2::FileExt;
use std::fs::{File, OpenOptions};
use std::path::Path;
use std::process::Stdio;
use std::time::Duration;
use tokio::process::Command;
use uuid::Uuid;

use crate::cloud::protocol::WorkspaceState;

pub async fn clone(url: &str, target: &Path) -> Result<()> {
    if let Some(parent) = target.parent() {
        tokio::fs::create_dir_all(parent)
            .await
            .with_context(|| format!("creating parent dir {parent:?}"))?;
    }
    // `--` separator is required: a hostile `url` like `--upload-pack=...` would
    // otherwise be interpreted as a flag and execute arbitrary commands.
    let use_first_use_trust = should_accept_new_host(url).await;
    let out = crate::util::process::command_output(clone_command(url, target, use_first_use_trust))
        .await
        .context("spawning git clone")?;
    if !out.status.success() {
        anyhow::bail!(
            "git clone failed ({}): {}",
            out.status,
            String::from_utf8_lossy(&out.stderr).trim()
        );
    }
    Ok(())
}

const SSH_CLONE_COMMAND: &str = "ssh -o StrictHostKeyChecking=accept-new -o BatchMode=yes";

async fn git_setting_exists(key: &str) -> bool {
    let status = Command::new("git")
        .args(["config", "--get", key])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .await;
    // Exit 1 means no setting. On other errors leave Git's SSH configuration
    // alone rather than overriding it with a less strict policy.
    !matches!(status, Ok(status) if status.code() == Some(1))
}

async fn git_conditional_includes_exist() -> bool {
    let status = Command::new("git")
        .args(["config", "--get-regexp", r"^includeif\..*\.path$"])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .await;
    // includeIf gitdir: for the clone target only takes effect after Git
    // initializes that directory, so it cannot be fully inspected here.
    !matches!(status, Ok(status) if status.code() == Some(1))
}

async fn should_accept_new_host(url: &str) -> bool {
    if !is_ssh_clone_url(url)
        || std::env::var_os("GIT_SSH_COMMAND").is_some()
        || std::env::var_os("GIT_SSH").is_some()
        || std::env::var_os("GIT_SSH_VARIANT").is_some()
        || git_setting_exists("core.sshCommand").await
        || git_setting_exists("ssh.variant").await
        || git_conditional_includes_exist().await
    {
        return false;
    }

    let Some((host, user, port)) = ssh_target(url) else {
        return false;
    };
    let mut command = Command::new("ssh");
    command.arg("-G").arg("-o").arg("BatchMode=yes");
    if !user.is_empty() {
        command.arg("-l").arg(user);
    }
    if let Some(port) = port {
        command.arg("-p").arg(port.to_string());
    }
    command.arg("--").arg(host);
    command
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .kill_on_drop(true);
    // Respect explicit SSH config (including a required known_hosts pin).
    // A missing SSH client or unreadable config leaves Git's defaults intact.
    matches!(
        tokio::time::timeout(Duration::from_secs(5), command.output()).await,
        Ok(Ok(output)) if output.status.success() && ssh_policy_is_ask(&output.stdout)
    )
}

fn ssh_policy_is_ask(config: &[u8]) -> bool {
    String::from_utf8_lossy(config).lines().any(|line| {
        let mut fields = line.split_whitespace();
        fields.next() == Some("stricthostkeychecking") && fields.next() == Some("ask")
    })
}

fn ssh_target(url: &str) -> Option<(String, String, Option<u16>)> {
    if url.get(..6)?.eq_ignore_ascii_case("ssh://") {
        let parsed = reqwest::Url::parse(url).ok()?;
        return Some((
            parsed.host_str()?.to_string(),
            parsed.username().to_string(),
            parsed.port(),
        ));
    }
    let (user_host, _) = url.split_once(':')?;
    let (user, host) = user_host.rsplit_once('@')?;
    Some((host.to_string(), user.to_string(), None))
}

fn clone_command(url: &str, target: &Path, use_first_use_trust: bool) -> Command {
    let mut command = Command::new("git");
    command
        .arg("clone")
        .arg("--")
        .arg(url)
        .arg(target)
        // The daemon cannot answer an interactive credential prompt. Force a
        // prompt-free failure so workspace resolution can fall back instead of
        // leaving the assignment blocked indefinitely.
        .env("GIT_TERMINAL_PROMPT", "0")
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);

    // A daemon cannot approve OpenSSH's first-connection prompt. Trust the
    // first key for SSH clones, but continue to reject changed host keys.
    // Preserve an operator-supplied SSH command or strict host-key policy.
    configure_ssh_host_key_policy(&mut command, url, use_first_use_trust);
    command
}

fn configure_ssh_host_key_policy(command: &mut Command, url: &str, use_first_use_trust: bool) {
    if is_ssh_clone_url(url) && use_first_use_trust {
        command.env("GIT_SSH_COMMAND", SSH_CLONE_COMMAND);
    }
}

fn is_ssh_clone_url(url: &str) -> bool {
    url.get(..6)
        .is_some_and(|scheme| scheme.eq_ignore_ascii_case("ssh://"))
        || (!url.contains("://")
            && url
                .split_once(':')
                .is_some_and(|(user_host, _)| user_host.contains('@')))
}

pub fn is_git_repo(path: &Path) -> bool {
    path.join(".git").exists()
}

#[derive(Debug)]
pub struct PreparedWorkBranch {
    pub name: String,
    base: Option<PreparedBaseBranch>,
}

#[derive(Debug)]
struct PreparedBaseBranch {
    name: String,
    tip: String,
}

/// Prepare `branch` as the checked-out work branch for an assigned work item.
///
/// If the branch exists on `origin`, its remote-tracking ref is refreshed and
/// the local branch is created, preserved, or fast-forwarded as appropriate.
/// A local branch that is ahead of the remote is deliberately preserved; a
/// diverged branch fails rather than discarding either side. If the remote
/// branch does not exist, an existing local branch is reused (important after
/// a previous push failure), or a new branch is created from `repo_ref` / the
/// remote's symbolic default branch. A missing explicit base is initialized
/// with an empty root commit, retained locally until successful completion.
pub async fn prepare_work_branch(
    path: &Path,
    branch: &str,
    repo_ref: Option<&str>,
) -> Result<PreparedWorkBranch> {
    validate_branch_name(branch)
        .await
        .with_context(|| format!("validating work branch {branch:?}"))?;
    if let Some(repo_ref) = repo_ref {
        validate_branch_name(repo_ref)
            .await
            .with_context(|| format!("validating repository base branch {repo_ref:?}"))?;
    }

    // Prepare the base even when the work branch already exists: a retry may
    // need to publish a base whose previous push never completed.
    let base = if let Some(name) = repo_ref {
        if remote_branch_exists(path, name).await?
            || (name == branch && local_branch_exists(path, branch).await?)
        {
            None
        } else {
            Some(PreparedBaseBranch {
                name: name.to_string(),
                tip: initialize_empty_base_branch(path, name).await?,
            })
        }
    } else {
        None
    };
    let prepared = PreparedWorkBranch {
        name: branch.to_string(),
        base,
    };

    if remote_branch_exists(path, branch).await? {
        fetch_remote_branch(path, branch).await?;
        prepare_existing_remote_branch(path, branch).await?;
        return Ok(prepared);
    }

    if local_branch_exists(path, branch).await? {
        checkout_preserved_branch(path, branch)
            .await
            .with_context(|| format!("checking out preserved local work branch {branch:?}"))?;
        return Ok(prepared);
    }

    let base_branch = match repo_ref {
        Some(base_branch) => base_branch.to_string(),
        None => remote_default_branch(path).await?,
    };
    let base_ref = if let Some(base) = &prepared.base {
        base.tip.clone()
    } else {
        fetch_remote_branch(path, &base_branch).await?;
        remote_tracking_ref(&base_branch)
    };
    git_output(path, &["checkout", "--no-track", "-b", branch, &base_ref])
        .await
        .with_context(|| {
            format!("creating local work branch {branch:?} from base branch {base_branch:?}")
        })?;
    Ok(prepared)
}

async fn initialize_empty_base_branch(path: &Path, branch: &str) -> Result<String> {
    let empty_tree = git_output(path, &["mktree"]).await?;
    let local_ref = local_branch_ref(branch);
    if !local_branch_exists(path, branch).await? {
        // Stable metadata gives independent runners the same root commit for
        // this base. No parent or index is used, even in a populated checkout.
        let message = format!("Initialize empty project base branch {branch}");
        let commit = Command::new("git")
            .current_dir(path)
            .env("GIT_AUTHOR_NAME", "karma-team runner")
            .env("GIT_AUTHOR_EMAIL", "karma-team-runner@localhost")
            .env("GIT_COMMITTER_NAME", "karma-team runner")
            .env("GIT_COMMITTER_EMAIL", "karma-team-runner@localhost")
            .env("GIT_AUTHOR_DATE", "2000-01-01T00:00:00 +0000")
            .env("GIT_COMMITTER_DATE", "2000-01-01T00:00:00 +0000")
            .args(["commit-tree", "--no-gpg-sign", &empty_tree, "-m", &message])
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true)
            .output()
            .await
            .context("creating empty project base commit")?;
        if !commit.status.success() {
            anyhow::bail!(
                "creating empty project base commit failed: {}",
                String::from_utf8_lossy(&commit.stderr).trim()
            );
        }
        let tip = String::from_utf8_lossy(&commit.stdout).trim().to_string();
        if let Err(error) = git_output(path, &["update-ref", &local_ref, &tip, ""]).await {
            // Another initializer may have won the compare-and-swap. Validate
            // its branch below rather than replacing an existing local ref.
            if !local_branch_exists(path, branch).await? {
                return Err(error).context("creating local project base branch");
            }
        }
    }
    let tip = git_output(path, &["rev-parse", &local_ref]).await?;
    let tree = git_output(path, &["rev-parse", &format!("{tip}^{{tree}}")]).await?;
    let parents = git_output(path, &["rev-list", "--parents", "-n", "1", &tip]).await?;
    if tree != empty_tree || parents.split_whitespace().count() != 1 {
        anyhow::bail!(
            "remote base branch {branch:?} is missing, but its local branch contains existing history; refusing to replace it with an empty base"
        );
    }
    Ok(tip)
}

/// Push the checked-out work branch without rewriting remote history.
///
/// The explicit `HEAD:refs/heads/...` refspec prevents a stale local ref or
/// upstream configuration from sending commits to a different branch. A run
/// with uncommitted user work fails instead of silently reporting only the
/// older HEAD as completed; runner-owned `.karma-team/` metadata is excluded.
pub async fn push_work_branch(path: &Path, branch: &str) -> Result<()> {
    push_prepared_work_branch(
        path,
        &PreparedWorkBranch {
            name: branch.to_string(),
            base: None,
        },
    )
    .await
}

/// Publish an initialized base and the completed work together. The base uses
/// its captured empty tip, never the agent's HEAD or an unrelated local ref.
pub async fn push_prepared_work_branch(path: &Path, prepared: &PreparedWorkBranch) -> Result<()> {
    let branch = prepared.name.as_str();
    validate_branch_name(branch)
        .await
        .with_context(|| format!("validating work branch {branch:?}"))?;
    let current = git_output(path, &["symbolic-ref", "--quiet", "--short", "HEAD"])
        .await
        .with_context(|| {
            format!("checking the current branch before pushing work branch {branch:?}")
        })?;
    if current != branch {
        anyhow::bail!(
            "refusing to push work branch {branch:?}: the workspace is currently on {current:?}"
        );
    }
    let pending = git_output(
        path,
        &[
            "status",
            "--porcelain",
            "--untracked-files=all",
            "--",
            ".",
            ":(exclude).karma-team",
            ":(exclude).karma-team/**",
        ],
    )
    .await
    .context("checking for uncommitted work before push")?;
    if !pending.is_empty() {
        anyhow::bail!(
            "refusing to push work branch {branch:?}: uncommitted tracked or untracked changes remain outside runner-owned .karma-team/:\n{pending}"
        );
    }

    // An agent may already have pushed this commit, or another contributor may
    // have advanced the remote from it. In either case all local work is on the
    // remote, and pushing the older/equal local tip is unnecessary (and would
    // be rejected in the remote-ahead case).
    let base = prepared.base.as_ref().filter(|base| base.name != branch);
    let mut refspecs = Vec::new();
    if let Some(base) = base
        && !remote_branch_contains_tip(path, &base.name, &base.tip).await?
    {
        refspecs.push(format!("{}:refs/heads/{}", base.tip, base.name));
    }
    if !remote_branch_contains_tip(path, branch, "HEAD").await? {
        refspecs.push(format!("HEAD:refs/heads/{branch}"));
    }
    if refspecs.is_empty() {
        set_branch_upstream(path, branch).await?;
        return Ok(());
    }

    let mut args = vec!["push", "--set-upstream"];
    if refspecs.len() > 1 {
        args.push("--atomic");
    }
    args.push("origin");
    args.extend(refspecs.iter().map(String::as_str));
    let push_error = match git_output(path, &args).await {
        Ok(_) => {
            set_branch_upstream(path, branch).await?;
            return Ok(());
        }
        Err(error) => error,
    };

    // Close the race between the containment check and push: another process
    // can publish this exact commit immediately before our push is rejected.
    // Preserve the original push error if the follow-up check itself fails.
    let base_published = match base {
        Some(base) => remote_branch_contains_tip(path, &base.name, &base.tip)
            .await
            .unwrap_or(false),
        None => true,
    };
    if base_published
        && remote_branch_contains_tip(path, branch, "HEAD")
            .await
            .unwrap_or(false)
    {
        set_branch_upstream(path, branch).await?;
        return Ok(());
    }
    Err(push_error).with_context(|| {
        format!(
            "pushing work branch {branch:?} and initialized base {:?} to origin",
            base.map(|base| base.name.as_str())
        )
    })
}

async fn prepare_existing_remote_branch(path: &Path, branch: &str) -> Result<()> {
    let local_ref = local_branch_ref(branch);
    let remote_ref = remote_tracking_ref(branch);
    if !local_branch_exists(path, branch).await? {
        git_output(path, &["checkout", "--track", "-b", branch, &remote_ref])
            .await
            .with_context(|| format!("creating local branch {branch:?} from {remote_ref:?}"))?;
        return Ok(());
    }

    let local_tip = git_output(path, &["rev-parse", &local_ref]).await?;
    let remote_tip = git_output(path, &["rev-parse", &remote_ref]).await?;
    let should_fast_forward = if local_tip == remote_tip {
        false
    } else if is_ancestor(path, &remote_ref, &local_ref).await? {
        // Local-only commits are recoverable work from this runner. Keep them.
        false
    } else if is_ancestor(path, &local_ref, &remote_ref).await? {
        true
    } else {
        anyhow::bail!(
            "local work branch {branch:?} has diverged from origin/{branch}; refusing to reset, rebase, or overwrite either history (local {local_tip}, remote {remote_tip})"
        );
    };

    checkout_preserved_branch(path, branch)
        .await
        .with_context(|| format!("checking out local work branch {branch:?}"))?;
    if should_fast_forward {
        git_output(path, &["merge", "--ff-only", "--no-edit", &remote_ref])
            .await
            .with_context(|| format!("fast-forwarding work branch {branch:?} to {remote_ref:?}"))?;
    }
    // Existing local branches may have been created while the remote branch
    // was absent, or may carry stale upstream configuration.
    set_branch_upstream(path, branch).await?;
    Ok(())
}

async fn checkout_preserved_branch(path: &Path, branch: &str) -> Result<String> {
    // Older Karma versions created persistent linked worktrees and may have
    // left this branch checked out in one of them. The runner serializes work
    // for a workspace and explicit work-branch runs no longer create a nested
    // worktree, so it is safe to bypass only Git's occupancy guard. We do not
    // prune/delete the historical worktree or alter its files/index.
    git_output(path, &["checkout", "--ignore-other-worktrees", branch]).await
}

async fn set_branch_upstream(path: &Path, branch: &str) -> Result<()> {
    let remote_ref = remote_tracking_ref(branch);
    git_output(path, &["branch", "--set-upstream-to", &remote_ref, branch])
        .await
        .with_context(|| format!("setting upstream for work branch {branch:?}"))?;
    Ok(())
}

async fn remote_branch_contains_tip(path: &Path, branch: &str, tip: &str) -> Result<bool> {
    if !remote_branch_exists(path, branch).await? {
        return Ok(false);
    }
    fetch_remote_branch(path, branch).await?;
    is_ancestor(path, tip, &remote_tracking_ref(branch)).await
}

async fn remote_branch_exists(path: &Path, branch: &str) -> Result<bool> {
    let remote_ref = format!("refs/heads/{branch}");
    let args = [
        "ls-remote",
        "--exit-code",
        "--heads",
        "origin",
        remote_ref.as_str(),
    ];
    let output = git_command_output(path, &args).await?;
    match output.status.code() {
        Some(0) => Ok(true),
        // `git ls-remote --exit-code` reserves status 2 for a reachable remote
        // with no matching refs. Authentication/network/config errors use a
        // different status and must not be mistaken for a missing branch.
        Some(2) => Ok(false),
        _ => anyhow::bail!(
            "git {:?} failed while checking remote branch {branch:?} ({}): {}",
            args,
            output.status,
            String::from_utf8_lossy(&output.stderr).trim()
        ),
    }
}

async fn fetch_remote_branch(path: &Path, branch: &str) -> Result<()> {
    let source = format!("refs/heads/{branch}");
    let target = format!("refs/remotes/origin/{branch}");
    // Force only the disposable remote-tracking ref so it truthfully mirrors
    // the remote even after an intentional remote rewind. No local branch is
    // reset or rewritten here.
    let refspec = format!("+{source}:{target}");
    git_output(path, &["fetch", "--no-tags", "origin", &refspec])
        .await
        .with_context(|| format!("fetching remote branch {branch:?}"))?;
    Ok(())
}

async fn remote_default_branch(path: &Path) -> Result<String> {
    let args = ["ls-remote", "--symref", "origin", "HEAD"];
    let output = git_command_output(path, &args).await?;
    if !output.status.success() {
        anyhow::bail!(
            "git {:?} failed while resolving the remote default branch ({}): {}",
            args,
            output.status,
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }
    let stdout = String::from_utf8_lossy(&output.stdout);
    let branch = stdout.lines().find_map(|line| {
        let (value, target) = line.split_once('\t')?;
        if target != "HEAD" {
            return None;
        }
        value.strip_prefix("ref: refs/heads/")
    });
    let Some(branch) = branch else {
        anyhow::bail!(
            "origin HEAD is not a symbolic branch; configure a project base branch explicitly"
        );
    };
    validate_branch_name(branch)
        .await
        .context("validating the remote default branch")?;
    Ok(branch.to_string())
}

async fn local_branch_exists(path: &Path, branch: &str) -> Result<bool> {
    let local_ref = local_branch_ref(branch);
    let args = ["show-ref", "--verify", "--quiet", local_ref.as_str()];
    let output = git_command_output(path, &args).await?;
    match output.status.code() {
        Some(0) => Ok(true),
        Some(1) => Ok(false),
        _ => anyhow::bail!(
            "git {:?} failed while checking local branch {branch:?} ({}): {}",
            args,
            output.status,
            String::from_utf8_lossy(&output.stderr).trim()
        ),
    }
}

async fn is_ancestor(path: &Path, ancestor: &str, descendant: &str) -> Result<bool> {
    let args = ["merge-base", "--is-ancestor", ancestor, descendant];
    let output = git_command_output(path, &args).await?;
    match output.status.code() {
        Some(0) => Ok(true),
        Some(1) => Ok(false),
        _ => anyhow::bail!(
            "git {:?} failed while comparing branch histories ({}): {}",
            args,
            output.status,
            String::from_utf8_lossy(&output.stderr).trim()
        ),
    }
}

fn local_branch_ref(branch: &str) -> String {
    format!("refs/heads/{branch}")
}

fn remote_tracking_ref(branch: &str) -> String {
    format!("refs/remotes/origin/{branch}")
}

/// Validate untrusted branch input before interpolating it into refspecs.
async fn validate_branch_name(branch: &str) -> Result<()> {
    if branch.is_empty()
        || branch == "@"
        || branch.starts_with('-')
        || branch.contains("@{")
        || branch
            .chars()
            .any(|character| character == ' ' || character.is_control())
    {
        anyhow::bail!("invalid Git branch name: {branch:?}");
    }
    let output = Command::new("git")
        .args(["check-ref-format", "--branch", branch])
        .env("GIT_TERMINAL_PROMPT", "0")
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true)
        .output()
        .await
        .context("spawning git check-ref-format")?;
    if !output.status.success() {
        anyhow::bail!(
            "invalid Git branch name {branch:?}: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }
    Ok(())
}

async fn acquire_worktree_lock(path: &Path) -> Result<File> {
    let data_override = std::env::var_os("KARMA_TEAM_DATA_DIR").map(Into::into);
    let data_dir = crate::util::paths::Paths::resolve(None, data_override)?.data_dir;
    let lock_dir = data_dir.join("worktree-locks");
    tokio::fs::create_dir_all(&lock_dir)
        .await
        .with_context(|| format!("creating worktree lock directory {lock_dir:?}"))?;
    let canonical = tokio::fs::canonicalize(path)
        .await
        .with_context(|| format!("resolving worktree path {path:?}"))?;
    let key = Uuid::new_v5(&Uuid::NAMESPACE_URL, canonical.to_string_lossy().as_bytes());
    let lock_path = lock_dir.join(format!("{key}.lock"));
    let mut options = OpenOptions::new();
    options.create(true).truncate(false).read(true).write(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let file = options
        .open(&lock_path)
        .with_context(|| format!("opening worktree lock {lock_path:?}"))?;
    tokio::time::timeout(Duration::from_secs(180), async {
        loop {
            match file.try_lock_exclusive() {
                Ok(()) => return Ok(file),
                Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                    tokio::time::sleep(Duration::from_millis(50)).await;
                }
                Err(error) => return Err(error).context("locking Git worktree initialization"),
            }
        }
    })
    .await
    .context("timed out waiting for Git worktree initialization lock")?
}

/// Prepare a directory for tools that require a Git worktree (currently the
/// Karma CLI). Existing files are deliberately left untracked: the only
/// commit created here is an empty bootstrap commit, so runner setup never
/// silently commits user work.
pub async fn ensure_worktree_base(path: &Path) -> Result<()> {
    tokio::fs::create_dir_all(path)
        .await
        .with_context(|| format!("creating git workspace {path:?}"))?;
    // Serialize git init as well as HEAD creation across runner processes.
    // A second caller rechecks the repository after acquiring the lock.
    let _lock = acquire_worktree_lock(path).await?;

    // `git rev-parse` walks up to parent repositories. Check for metadata in
    // this directory first so a runner workspace nested under another repo
    // gets its own worktree base instead of mutating the parent's HEAD.
    if !path.join(".git").exists() {
        let out = Command::new("git")
            .arg("init")
            .arg(path)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .output()
            .await
            .context("initializing git workspace")?;
        if !out.status.success() {
            anyhow::bail!(
                "git init failed ({}): {}",
                out.status,
                String::from_utf8_lossy(&out.stderr).trim()
            );
        }
    }

    let head = Command::new("git")
        .current_dir(path)
        .args(["rev-parse", "--verify", "HEAD"])
        .output()
        .await
        .context("checking git workspace HEAD")?;
    if head.status.success() {
        return Ok(());
    }

    // Build the first commit from an empty tree, not the caller's index:
    // `git commit --allow-empty` would also commit any files already staged.
    let tree = Command::new("git")
        .current_dir(path)
        .args(["mktree"])
        .stdin(Stdio::null())
        .output()
        .await
        .context("creating empty git tree")?;
    if !tree.status.success() {
        anyhow::bail!(
            "git mktree failed ({}): {}",
            tree.status,
            String::from_utf8_lossy(&tree.stderr).trim()
        );
    }
    let tree_id = String::from_utf8_lossy(&tree.stdout).trim().to_string();
    let commit = Command::new("git")
        .current_dir(path)
        .env("GIT_AUTHOR_NAME", "karma-team runner")
        .env("GIT_AUTHOR_EMAIL", "karma-team-runner@localhost")
        .env("GIT_COMMITTER_NAME", "karma-team runner")
        .env("GIT_COMMITTER_EMAIL", "karma-team-runner@localhost")
        .args(["commit-tree", &tree_id, "-m", "Initialize runner workspace"])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .await
        .context("creating empty git workspace commit")?;
    if !commit.status.success() {
        anyhow::bail!(
            "git commit-tree failed ({}): {}",
            commit.status,
            String::from_utf8_lossy(&commit.stderr).trim()
        );
    }
    let commit_id = String::from_utf8_lossy(&commit.stdout).trim().to_string();
    let update = Command::new("git")
        .current_dir(path)
        .args(["update-ref", "HEAD", &commit_id, ""])
        .output()
        .await
        .context("updating git workspace HEAD")?;
    if !update.status.success() {
        // Another runner may have initialized this same branch concurrently.
        if git_output(path, &["rev-parse", "--verify", "HEAD"])
            .await
            .is_ok()
        {
            return Ok(());
        }
        anyhow::bail!(
            "git update-ref failed ({}): {}",
            update.status,
            String::from_utf8_lossy(&update.stderr).trim()
        );
    }
    Ok(())
}

/// Read-only snapshot of the working directory reported to the cloud on
/// `Accept`. Runs no mutation — only `rev-parse` and `status --porcelain`.
pub async fn workspace_state(path: &Path) -> Result<WorkspaceState> {
    let branch = git_output(path, &["rev-parse", "--abbrev-ref", "HEAD"])
        .await
        .ok();
    let head = git_output(path, &["rev-parse", "HEAD"]).await.ok();
    let status = git_output(path, &["status", "--porcelain"]).await?;
    let dirty = !status.trim().is_empty();
    Ok(WorkspaceState {
        branch,
        head,
        dirty,
    })
}

async fn git_output(path: &Path, args: &[&str]) -> Result<String> {
    let out = git_command_output(path, args).await?;
    if !out.status.success() {
        anyhow::bail!(
            "git {:?} failed ({}): {}",
            args,
            out.status,
            String::from_utf8_lossy(&out.stderr).trim()
        );
    }
    Ok(String::from_utf8_lossy(&out.stdout).trim().to_string())
}

async fn git_command_output(path: &Path, args: &[&str]) -> Result<std::process::Output> {
    let mut command = Command::new("git");
    command
        .current_dir(path)
        .args(args)
        // Runner commands execute without a terminal. Fail promptly when
        // credentials are unavailable instead of waiting for an interactive
        // prompt that can never be answered.
        .env("GIT_TERMINAL_PROMPT", "0")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    crate::util::process::command_output(command)
        .await
        .with_context(|| format!("spawning git {args:?}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[cfg(unix)]
    use std::os::unix::fs::PermissionsExt;
    use std::path::PathBuf;
    #[cfg(unix)]
    use std::process::Command as StdCommand;
    use tempfile::{TempDir, tempdir};

    struct TestRepositories {
        _temp: TempDir,
        remote: PathBuf,
        publisher: PathBuf,
        workspace: PathBuf,
    }

    impl TestRepositories {
        async fn new() -> Self {
            let temp = tempdir().unwrap();
            let remote = temp.path().join("origin.git");
            let publisher = temp.path().join("publisher");
            let workspace = temp.path().join("workspace");
            let remote_arg = remote.to_string_lossy().into_owned();
            let publisher_arg = publisher.to_string_lossy().into_owned();
            let workspace_arg = workspace.to_string_lossy().into_owned();

            git_output(temp.path(), &["init", "--bare", &remote_arg])
                .await
                .unwrap();
            git_output(temp.path(), &["init", "-b", "trunk", &publisher_arg])
                .await
                .unwrap();
            configure_test_identity(&publisher).await;
            std::fs::write(publisher.join("base.txt"), "base\n").unwrap();
            git_output(&publisher, &["add", "base.txt"]).await.unwrap();
            git_output(&publisher, &["commit", "-m", "base"])
                .await
                .unwrap();
            git_output(&publisher, &["remote", "add", "origin", &remote_arg])
                .await
                .unwrap();
            git_output(&publisher, &["push", "--set-upstream", "origin", "trunk"])
                .await
                .unwrap();
            git_output(&remote, &["symbolic-ref", "HEAD", "refs/heads/trunk"])
                .await
                .unwrap();
            git_output(temp.path(), &["clone", &remote_arg, &workspace_arg])
                .await
                .unwrap();
            configure_test_identity(&workspace).await;

            Self {
                _temp: temp,
                remote,
                publisher,
                workspace,
            }
        }

        async fn create_remote_branch(&self, branch: &str) -> String {
            git_output(&self.publisher, &["checkout", "trunk"])
                .await
                .unwrap();
            git_output(&self.publisher, &["checkout", "-b", branch])
                .await
                .unwrap();
            self.commit_in(&self.publisher, "remote.txt", "remote branch\n")
                .await;
            git_output(
                &self.publisher,
                &["push", "--set-upstream", "origin", branch],
            )
            .await
            .unwrap();
            git_output(&self.publisher, &["rev-parse", "HEAD"])
                .await
                .unwrap()
        }

        async fn commit_in(&self, repository: &Path, file: &str, contents: &str) -> String {
            std::fs::write(repository.join(file), contents).unwrap();
            git_output(repository, &["add", file]).await.unwrap();
            git_output(repository, &["commit", "-m", contents.trim()])
                .await
                .unwrap();
            git_output(repository, &["rev-parse", "HEAD"])
                .await
                .unwrap()
        }

        async fn remote_ref(&self, branch: &str) -> Option<String> {
            git_output(
                &self.remote,
                &["rev-parse", "--verify", &format!("refs/heads/{branch}")],
            )
            .await
            .ok()
        }
    }

    async fn configure_test_identity(path: &Path) {
        git_output(path, &["config", "user.name", "Karma Team Test"])
            .await
            .unwrap();
        git_output(path, &["config", "user.email", "runner-test@localhost"])
            .await
            .unwrap();
    }

    #[test]
    fn ssh_clone_accepts_only_new_host_keys() {
        for url in [
            "git@gitlab.example.com:group/repo.git",
            "ssh://git@gitlab.example.com:2222/group/repo.git",
            "SSH://git@gitlab.example.com:2222/group/repo.git",
        ] {
            assert!(is_ssh_clone_url(url));
            let mut command = Command::new("git");
            configure_ssh_host_key_policy(&mut command, url, true);
            assert_eq!(
                command
                    .as_std()
                    .get_envs()
                    .find(|(key, _)| *key == "GIT_SSH_COMMAND")
                    .and_then(|(_, value)| value),
                Some(SSH_CLONE_COMMAND.as_ref())
            );
        }
    }

    #[test]
    fn explicit_ssh_environment_is_preserved() {
        let mut command = Command::new("git");
        configure_ssh_host_key_policy(&mut command, "git@gitlab.example.com:group/repo.git", false);
        assert!(
            command
                .as_std()
                .get_envs()
                .all(|(key, _)| key != "GIT_SSH_COMMAND")
        );
    }

    #[test]
    fn ssh_target_handles_scp_and_explicit_ports() {
        assert_eq!(
            ssh_target("git@gitlab.example.com:team/repo.git"),
            Some(("gitlab.example.com".into(), "git".into(), None))
        );
        assert_eq!(
            ssh_target("ssh://deploy@gitlab.example.com:2222/team/repo.git"),
            Some(("gitlab.example.com".into(), "deploy".into(), Some(2222)))
        );
        assert!(ssh_target("ssh://bad host/team/repo.git").is_none());
    }

    #[test]
    fn ssh_policy_uses_tofu_only_for_default_ask() {
        assert!(ssh_policy_is_ask(
            b"hostname gitlab.example.com\nstricthostkeychecking ask\n"
        ));
        for policy in ["yes", "no", "off", "accept-new"] {
            assert!(!ssh_policy_is_ask(
                format!("stricthostkeychecking {policy}\n").as_bytes()
            ));
        }
        assert!(!ssh_policy_is_ask(b""));
    }

    #[cfg(unix)]
    #[test]
    fn configured_git_and_ssh_settings_are_preserved() {
        let tmp = tempdir().unwrap();
        // The child gets a private PATH and Git configuration; no process-wide
        // environment changes can race other tests in this test binary.
        let ssh = tmp.path().join("ssh");
        std::fs::write(
            &ssh,
            "#!/bin/sh\nprintf 'stricthostkeychecking %s\\n' \"$KARMA_TEAM_TEST_SSH_POLICY\"\n",
        )
        .unwrap();
        std::fs::set_permissions(&ssh, std::fs::Permissions::from_mode(0o700)).unwrap();

        for (case, expected, ssh_policy) in [
            ("default", true, "ask"),
            ("strict_ssh_config", false, "yes"),
            ("already_accept_new", false, "accept-new"),
            ("git_ssh_command", false, "ask"),
            ("git_ssh", false, "ask"),
            ("git_config_ssh_variant", false, "ask"),
            ("git_core_ssh_command", false, "ask"),
            ("git_ssh_variant", false, "ask"),
            ("git_conditional_include", false, "ask"),
        ] {
            let mut child = StdCommand::new(std::env::current_exe().unwrap());
            child
                .args([
                    "--exact",
                    "workspace::git::tests::isolated_ssh_settings_child",
                ])
                .current_dir(tmp.path())
                .env(
                    "PATH",
                    format!(
                        "{}:{}",
                        tmp.path().display(),
                        std::env::var("PATH").unwrap()
                    ),
                )
                .env("KARMA_TEAM_TEST_SSH_CASE", case)
                .env("KARMA_TEAM_TEST_SSH_POLICY", ssh_policy)
                .env("KARMA_TEAM_TEST_SSH_EXPECTED", expected.to_string())
                .env("GIT_CONFIG_NOSYSTEM", "1")
                .env("GIT_CONFIG_GLOBAL", "/dev/null")
                .env_remove("GIT_CONFIG_COUNT")
                .env_remove("GIT_CONFIG_PARAMETERS")
                .env_remove("GIT_SSH_COMMAND")
                .env_remove("GIT_SSH")
                .env_remove("GIT_SSH_VARIANT");
            match case {
                "git_ssh_command" => {
                    child.env("GIT_SSH_COMMAND", "ssh -o StrictHostKeyChecking=yes");
                }
                "git_ssh" => {
                    child.env("GIT_SSH", "/custom/ssh");
                }
                "git_ssh_variant" => {
                    child.env("GIT_SSH_VARIANT", "plink");
                }
                "git_core_ssh_command" | "git_config_ssh_variant" | "git_conditional_include" => {
                    child
                        .env("GIT_CONFIG_COUNT", "1")
                        .env(
                            "GIT_CONFIG_KEY_0",
                            match case {
                                "git_core_ssh_command" => "core.sshCommand",
                                "git_config_ssh_variant" => "ssh.variant",
                                _ => "includeIf.gitdir:/tmp/other-repository/.git.path",
                            },
                        )
                        .env(
                            "GIT_CONFIG_VALUE_0",
                            if case == "git_conditional_include" {
                                "/dev/null"
                            } else {
                                "ssh -o StrictHostKeyChecking=yes"
                            },
                        );
                }
                _ => {}
            }
            let output = child.output().unwrap();
            assert!(
                output.status.success()
                    && String::from_utf8_lossy(&output.stdout).contains("1 passed"),
                "{case} failed: stdout={} stderr={}",
                String::from_utf8_lossy(&output.stdout),
                String::from_utf8_lossy(&output.stderr)
            );
        }
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn isolated_ssh_settings_child() {
        let Some(expected) = std::env::var_os("KARMA_TEAM_TEST_SSH_EXPECTED") else {
            return;
        };
        assert_eq!(
            should_accept_new_host("git@karma-team-hostkey-test.invalid:group/repo.git").await,
            expected == "true",
            "case: {:?}",
            std::env::var_os("KARMA_TEAM_TEST_SSH_CASE")
        );
    }

    #[test]
    fn non_ssh_clone_keeps_git_defaults() {
        for url in [
            "https://gitlab.example.com/group/repo.git",
            "http://gitlab.example.com/group/repo.git",
            "git://gitlab.example.com/group/repo.git",
        ] {
            assert!(!is_ssh_clone_url(url));
            let command = clone_command(url, Path::new("/tmp/target"), true);
            assert!(
                command
                    .as_std()
                    .get_envs()
                    .all(|(key, _)| key != "GIT_SSH_COMMAND")
            );
        }
    }

    #[test]
    fn every_clone_command_disables_terminal_prompts_without_changing_ssh_policy() {
        for (url, use_first_use_trust, expected_ssh_command) in [
            ("https://gitlab.example.com/group/repo.git", false, None),
            (
                "git@gitlab.example.com:group/repo.git",
                true,
                Some(SSH_CLONE_COMMAND),
            ),
            ("git@gitlab.example.com:group/repo.git", false, None),
        ] {
            let command = clone_command(url, Path::new("/tmp/target"), use_first_use_trust);
            let envs: std::collections::HashMap<_, _> = command.as_std().get_envs().collect();
            assert_eq!(
                envs.get(std::ffi::OsStr::new("GIT_TERMINAL_PROMPT"))
                    .and_then(|value| *value),
                Some(std::ffi::OsStr::new("0"))
            );
            assert_eq!(
                envs.get(std::ffi::OsStr::new("GIT_SSH_COMMAND"))
                    .and_then(|value| *value),
                expected_ssh_command.map(std::ffi::OsStr::new)
            );
        }
    }

    #[tokio::test]
    async fn prepares_an_existing_remote_branch_with_tracking() {
        let repositories = TestRepositories::new().await;
        let expected_tip = repositories.create_remote_branch("feature/existing").await;

        prepare_work_branch(&repositories.workspace, "feature/existing", Some("trunk"))
            .await
            .unwrap();

        assert_eq!(
            git_output(
                &repositories.workspace,
                &["symbolic-ref", "--short", "HEAD"]
            )
            .await
            .unwrap(),
            "feature/existing"
        );
        assert_eq!(
            git_output(&repositories.workspace, &["rev-parse", "HEAD"])
                .await
                .unwrap(),
            expected_tip
        );
        assert_eq!(
            git_output(
                &repositories.workspace,
                &[
                    "rev-parse",
                    "--abbrev-ref",
                    "--symbolic-full-name",
                    "@{upstream}",
                ],
            )
            .await
            .unwrap(),
            "origin/feature/existing"
        );
    }

    #[tokio::test]
    async fn fast_forwards_a_local_branch_that_is_behind_origin() {
        let repositories = TestRepositories::new().await;
        repositories.create_remote_branch("feature/behind").await;
        prepare_work_branch(&repositories.workspace, "feature/behind", Some("trunk"))
            .await
            .unwrap();

        let remote_tip = repositories
            .commit_in(&repositories.publisher, "remote.txt", "remote update\n")
            .await;
        git_output(
            &repositories.publisher,
            &["push", "origin", "feature/behind"],
        )
        .await
        .unwrap();

        prepare_work_branch(&repositories.workspace, "feature/behind", Some("trunk"))
            .await
            .unwrap();
        assert_eq!(
            git_output(&repositories.workspace, &["rev-parse", "HEAD"])
                .await
                .unwrap(),
            remote_tip
        );
    }

    #[tokio::test]
    async fn preserves_a_local_branch_that_is_ahead_of_origin() {
        let repositories = TestRepositories::new().await;
        repositories.create_remote_branch("feature/ahead").await;
        prepare_work_branch(&repositories.workspace, "feature/ahead", Some("trunk"))
            .await
            .unwrap();
        let local_tip = repositories
            .commit_in(&repositories.workspace, "local.txt", "local only\n")
            .await;

        prepare_work_branch(&repositories.workspace, "feature/ahead", Some("trunk"))
            .await
            .unwrap();

        assert_eq!(
            git_output(&repositories.workspace, &["rev-parse", "HEAD"])
                .await
                .unwrap(),
            local_tip
        );
    }

    #[tokio::test]
    async fn checks_out_a_branch_held_by_a_historical_worktree_without_altering_it() {
        let repositories = TestRepositories::new().await;
        repositories
            .create_remote_branch("feature/historical-worktree")
            .await;
        prepare_work_branch(
            &repositories.workspace,
            "feature/historical-worktree",
            Some("trunk"),
        )
        .await
        .unwrap();
        git_output(&repositories.workspace, &["checkout", "trunk"])
            .await
            .unwrap();
        let historical_worktree = repositories.remote.with_file_name("old-karma-worktree");
        let historical_arg = historical_worktree.to_string_lossy().into_owned();
        git_output(
            &repositories.workspace,
            &[
                "worktree",
                "add",
                &historical_arg,
                "feature/historical-worktree",
            ],
        )
        .await
        .unwrap();
        std::fs::write(
            historical_worktree.join("uncommitted.txt"),
            "preserve historical work\n",
        )
        .unwrap();

        prepare_work_branch(
            &repositories.workspace,
            "feature/historical-worktree",
            Some("trunk"),
        )
        .await
        .unwrap();

        assert_eq!(
            git_output(
                &repositories.workspace,
                &["symbolic-ref", "--short", "HEAD"]
            )
            .await
            .unwrap(),
            "feature/historical-worktree"
        );
        assert_eq!(
            std::fs::read_to_string(historical_worktree.join("uncommitted.txt")).unwrap(),
            "preserve historical work\n"
        );
        assert_eq!(
            git_output(&historical_worktree, &["status", "--porcelain"])
                .await
                .unwrap(),
            "?? uncommitted.txt"
        );
    }

    #[tokio::test]
    async fn refuses_to_rewrite_diverged_local_and_remote_branches() {
        let repositories = TestRepositories::new().await;
        repositories.create_remote_branch("feature/diverged").await;
        prepare_work_branch(&repositories.workspace, "feature/diverged", Some("trunk"))
            .await
            .unwrap();
        let local_tip = repositories
            .commit_in(&repositories.workspace, "local.txt", "local update\n")
            .await;
        let remote_tip = repositories
            .commit_in(&repositories.publisher, "remote.txt", "remote update\n")
            .await;
        git_output(
            &repositories.publisher,
            &["push", "origin", "feature/diverged"],
        )
        .await
        .unwrap();

        let error = prepare_work_branch(&repositories.workspace, "feature/diverged", Some("trunk"))
            .await
            .unwrap_err();

        assert!(error.to_string().contains("has diverged"), "{error:#}");
        assert_eq!(
            git_output(&repositories.workspace, &["rev-parse", "HEAD"])
                .await
                .unwrap(),
            local_tip
        );
        assert_eq!(
            repositories.remote_ref("feature/diverged").await.unwrap(),
            remote_tip
        );
    }

    #[tokio::test]
    async fn creates_a_missing_work_branch_from_the_explicit_base() {
        let repositories = TestRepositories::new().await;
        let base_tip = repositories.remote_ref("trunk").await.unwrap();

        prepare_work_branch(&repositories.workspace, "feature/new", Some("trunk"))
            .await
            .unwrap();

        assert_eq!(
            git_output(&repositories.workspace, &["rev-parse", "HEAD"])
                .await
                .unwrap(),
            base_tip
        );
        assert_eq!(
            git_output(
                &repositories.workspace,
                &["symbolic-ref", "--short", "HEAD"]
            )
            .await
            .unwrap(),
            "feature/new"
        );
        assert!(repositories.remote_ref("feature/new").await.is_none());
    }

    #[tokio::test]
    async fn explicit_base_wins_when_it_differs_from_remote_default() {
        let repositories = TestRepositories::new().await;
        let default_tip = repositories.remote_ref("trunk").await.unwrap();
        let explicit_base_tip = repositories.create_remote_branch("release/base").await;
        assert_ne!(explicit_base_tip, default_tip);

        prepare_work_branch(
            &repositories.workspace,
            "feature/from-release",
            Some("release/base"),
        )
        .await
        .unwrap();

        assert_eq!(
            git_output(&repositories.workspace, &["rev-parse", "HEAD"])
                .await
                .unwrap(),
            explicit_base_tip
        );
        assert_ne!(
            git_output(&repositories.workspace, &["rev-parse", "HEAD"])
                .await
                .unwrap(),
            default_tip
        );
        assert_eq!(
            git_output(
                &repositories.workspace,
                &["symbolic-ref", "--short", "HEAD"]
            )
            .await
            .unwrap(),
            "feature/from-release"
        );
    }

    #[tokio::test]
    async fn creates_a_missing_work_branch_from_the_remote_default() {
        let repositories = TestRepositories::new().await;
        let base_tip = repositories.remote_ref("trunk").await.unwrap();

        prepare_work_branch(&repositories.workspace, "feature/default-base", None)
            .await
            .unwrap();

        assert_eq!(
            git_output(&repositories.workspace, &["rev-parse", "HEAD"])
                .await
                .unwrap(),
            base_tip
        );
        assert_eq!(
            git_output(
                &repositories.workspace,
                &["symbolic-ref", "--short", "HEAD"]
            )
            .await
            .unwrap(),
            "feature/default-base"
        );
    }

    #[tokio::test]
    async fn reuses_a_missing_remote_branch_with_unpushed_local_commits() {
        let repositories = TestRepositories::new().await;
        prepare_work_branch(&repositories.workspace, "feature/recovery", Some("trunk"))
            .await
            .unwrap();
        let local_tip = repositories
            .commit_in(&repositories.workspace, "recovery.txt", "recover me\n")
            .await;

        prepare_work_branch(&repositories.workspace, "feature/recovery", Some("trunk"))
            .await
            .unwrap();

        assert_eq!(
            git_output(&repositories.workspace, &["rev-parse", "HEAD"])
                .await
                .unwrap(),
            local_tip
        );
        assert!(repositories.remote_ref("feature/recovery").await.is_none());
    }

    #[tokio::test]
    async fn rejects_invalid_work_and_base_branch_names() {
        for invalid in [
            "",
            "-rf",
            "a..b",
            "a//b",
            "a.lock",
            "trailing.",
            "with space",
            "with\ncontrol",
            "HEAD",
            "@",
            "@{-1}",
        ] {
            assert!(
                validate_branch_name(invalid).await.is_err(),
                "accepted invalid branch {invalid:?}"
            );
        }
        for valid in ["main", "feature/work-item-42", "release/1.2.3"] {
            validate_branch_name(valid).await.unwrap();
        }

        let repositories = TestRepositories::new().await;
        let error = prepare_work_branch(
            &repositories.workspace,
            "feature/valid",
            Some("invalid..base"),
        )
        .await
        .unwrap_err();
        assert!(
            error
                .to_string()
                .contains("validating repository base branch"),
            "{error:#}"
        );
    }

    #[tokio::test]
    async fn initializes_an_empty_base_and_publishes_both_branches_on_completion() {
        let repositories = TestRepositories::new().await;
        let original = repositories.remote_ref("trunk").await.unwrap();

        let prepared =
            prepare_work_branch(&repositories.workspace, "feature/new", Some("missing-base"))
                .await
                .unwrap();
        let base = prepared.base.as_ref().unwrap();
        assert_eq!(
            git_output(&repositories.workspace, &["rev-parse", "HEAD"])
                .await
                .unwrap(),
            base.tip
        );
        assert_eq!(
            git_output(&repositories.workspace, &["ls-tree", "-r", "HEAD"])
                .await
                .unwrap(),
            ""
        );
        assert_eq!(
            git_output(&repositories.workspace, &["rev-list", "--count", "HEAD"])
                .await
                .unwrap(),
            "1"
        );
        assert!(!repositories.workspace.join("base.txt").exists());
        assert!(repositories.remote_ref("missing-base").await.is_none());
        assert!(repositories.remote_ref("feature/new").await.is_none());

        let work_tip = repositories
            .commit_in(&repositories.workspace, "result.txt", "task result\n")
            .await;
        push_prepared_work_branch(&repositories.workspace, &prepared)
            .await
            .unwrap();
        assert_eq!(
            repositories.remote_ref("missing-base").await.unwrap(),
            base.tip
        );
        assert_eq!(
            repositories.remote_ref("feature/new").await.unwrap(),
            work_tip
        );
        assert_eq!(repositories.remote_ref("trunk").await.unwrap(), original);
        assert_eq!(
            git_output(&repositories.remote, &["rev-parse", "feature/new^"])
                .await
                .unwrap(),
            base.tip
        );
        assert_eq!(
            git_output(&repositories.remote, &["ls-tree", "-r", "missing-base"])
                .await
                .unwrap(),
            ""
        );
    }

    #[tokio::test]
    async fn initializes_branches_in_an_empty_remote_repository() {
        let temp = tempdir().unwrap();
        let remote = temp.path().join("empty.git");
        let workspace = temp.path().join("workspace");
        git_output(
            temp.path(),
            &[
                "init",
                "--bare",
                "--initial-branch=main",
                remote.to_str().unwrap(),
            ],
        )
        .await
        .unwrap();
        clone(remote.to_str().unwrap(), &workspace).await.unwrap();
        let prepared = prepare_work_branch(&workspace, "feature/first", Some("main"))
            .await
            .unwrap();
        assert_eq!(git_output(&workspace, &["ls-files"]).await.unwrap(), "");
        push_prepared_work_branch(&workspace, &prepared)
            .await
            .unwrap();
        assert_eq!(
            git_output(&remote, &["rev-parse", "main"]).await.unwrap(),
            git_output(&remote, &["rev-parse", "feature/first"])
                .await
                .unwrap()
        );
    }

    #[tokio::test]
    async fn retries_reuse_the_empty_base_without_losing_work_commits() {
        let repositories = TestRepositories::new().await;
        let first = prepare_work_branch(&repositories.workspace, "feature/retry", Some("new-base"))
            .await
            .unwrap();
        let tip = repositories
            .commit_in(&repositories.workspace, "result.txt", "retain work\n")
            .await;
        let retry = prepare_work_branch(&repositories.workspace, "feature/retry", Some("new-base"))
            .await
            .unwrap();
        assert_eq!(first.base.unwrap().tip, retry.base.as_ref().unwrap().tip);
        assert_eq!(
            git_output(&repositories.workspace, &["rev-parse", "HEAD"])
                .await
                .unwrap(),
            tip
        );
        push_prepared_work_branch(&repositories.workspace, &retry)
            .await
            .unwrap();
        assert_eq!(repositories.remote_ref("feature/retry").await.unwrap(), tip);
    }

    #[tokio::test]
    async fn independent_workspaces_share_the_same_empty_base_commit() {
        let repositories = TestRepositories::new().await;
        let other = repositories.remote.with_file_name("other");
        clone(repositories.remote.to_str().unwrap(), &other)
            .await
            .unwrap();
        configure_test_identity(&other).await;
        let (first, second) = tokio::join!(
            prepare_work_branch(&repositories.workspace, "feature/one", Some("new-base")),
            prepare_work_branch(&other, "feature/two", Some("new-base")),
        );
        let first = first.unwrap();
        let second = second.unwrap();
        assert_eq!(
            first.base.as_ref().unwrap().tip,
            second.base.as_ref().unwrap().tip
        );
        repositories
            .commit_in(&repositories.workspace, "one.txt", "one\n")
            .await;
        repositories.commit_in(&other, "two.txt", "two\n").await;
        push_prepared_work_branch(&repositories.workspace, &first)
            .await
            .unwrap();
        push_prepared_work_branch(&other, &second).await.unwrap();
        assert_eq!(
            git_output(
                &repositories.remote,
                &["merge-base", "feature/one", "feature/two"]
            )
            .await
            .unwrap(),
            repositories.remote_ref("new-base").await.unwrap()
        );
    }

    #[tokio::test]
    async fn preserves_an_existing_remote_work_branch_when_initializing_its_base() {
        let repositories = TestRepositories::new().await;
        let tip = repositories.create_remote_branch("feature/existing").await;
        let prepared = prepare_work_branch(
            &repositories.workspace,
            "feature/existing",
            Some("new-base"),
        )
        .await
        .unwrap();
        assert_eq!(
            git_output(&repositories.workspace, &["rev-parse", "HEAD"])
                .await
                .unwrap(),
            tip
        );
        push_prepared_work_branch(&repositories.workspace, &prepared)
            .await
            .unwrap();
        assert_eq!(
            repositories.remote_ref("feature/existing").await.unwrap(),
            tip
        );
        assert_eq!(
            repositories.remote_ref("new-base").await.unwrap(),
            prepared.base.unwrap().tip
        );
    }

    #[tokio::test]
    async fn base_and_work_may_be_the_same_branch_even_on_retry() {
        let repositories = TestRepositories::new().await;
        prepare_work_branch(&repositories.workspace, "new-base", Some("new-base"))
            .await
            .unwrap();
        let tip = repositories
            .commit_in(&repositories.workspace, "result.txt", "same branch\n")
            .await;
        let retry = prepare_work_branch(&repositories.workspace, "new-base", Some("new-base"))
            .await
            .unwrap();
        push_prepared_work_branch(&repositories.workspace, &retry)
            .await
            .unwrap();
        assert_eq!(repositories.remote_ref("new-base").await.unwrap(), tip);
    }

    #[tokio::test]
    async fn missing_base_never_replaces_a_local_branch_with_existing_history() {
        let repositories = TestRepositories::new().await;
        git_output(&repositories.workspace, &["branch", "local-base"])
            .await
            .unwrap();
        let original = git_output(&repositories.workspace, &["rev-parse", "local-base"])
            .await
            .unwrap();
        let error = prepare_work_branch(&repositories.workspace, "feature/new", Some("local-base"))
            .await
            .unwrap_err();
        assert!(error.to_string().contains("existing history"), "{error:#}");
        assert_eq!(
            git_output(&repositories.workspace, &["rev-parse", "local-base"])
                .await
                .unwrap(),
            original
        );
        assert!(repositories.remote_ref("feature/new").await.is_none());
    }

    #[tokio::test]
    async fn empty_base_setup_does_not_discard_staged_changes() {
        let repositories = TestRepositories::new().await;
        std::fs::write(
            repositories.workspace.join("base.txt"),
            "staged user work\n",
        )
        .unwrap();
        git_output(&repositories.workspace, &["add", "base.txt"])
            .await
            .unwrap();
        assert!(
            prepare_work_branch(&repositories.workspace, "feature/new", Some("new-base"))
                .await
                .is_err()
        );
        assert_eq!(
            git_output(&repositories.workspace, &["show", ":base.txt"])
                .await
                .unwrap(),
            "staged user work"
        );
        assert_eq!(
            std::fs::read_to_string(repositories.workspace.join("base.txt")).unwrap(),
            "staged user work\n"
        );
        assert_eq!(
            git_output(&repositories.workspace, &["ls-tree", "-r", "new-base"])
                .await
                .unwrap(),
            ""
        );
        assert!(repositories.remote_ref("new-base").await.is_none());
    }

    #[tokio::test]
    async fn conflicting_remote_base_rejects_both_pushes_without_losing_local_work() {
        let repositories = TestRepositories::new().await;
        let prepared =
            prepare_work_branch(&repositories.workspace, "feature/new", Some("new-base"))
                .await
                .unwrap();
        let tip = repositories
            .commit_in(&repositories.workspace, "result.txt", "local work\n")
            .await;
        let other_tip = repositories.create_remote_branch("new-base").await;
        assert!(
            push_prepared_work_branch(&repositories.workspace, &prepared)
                .await
                .is_err()
        );
        assert_eq!(
            repositories.remote_ref("new-base").await.unwrap(),
            other_tip
        );
        assert!(repositories.remote_ref("feature/new").await.is_none());
        assert_eq!(
            git_output(&repositories.workspace, &["rev-parse", "HEAD"])
                .await
                .unwrap(),
            tip
        );
    }

    #[tokio::test]
    async fn dirty_work_is_not_published_with_its_new_base() {
        let repositories = TestRepositories::new().await;
        let prepared =
            prepare_work_branch(&repositories.workspace, "feature/new", Some("new-base"))
                .await
                .unwrap();
        std::fs::write(repositories.workspace.join("unfinished.txt"), "keep me\n").unwrap();
        assert!(
            push_prepared_work_branch(&repositories.workspace, &prepared)
                .await
                .is_err()
        );
        assert!(repositories.remote_ref("new-base").await.is_none());
        assert!(repositories.remote_ref("feature/new").await.is_none());
        assert_eq!(
            std::fs::read_to_string(repositories.workspace.join("unfinished.txt")).unwrap(),
            "keep me\n"
        );
    }

    #[tokio::test]
    async fn does_not_treat_an_unreachable_remote_as_a_missing_branch() {
        let repositories = TestRepositories::new().await;
        let missing_remote = repositories.remote.with_file_name("unreachable.git");
        git_output(
            &repositories.workspace,
            &[
                "remote",
                "set-url",
                "origin",
                &missing_remote.to_string_lossy(),
            ],
        )
        .await
        .unwrap();

        let error = prepare_work_branch(
            &repositories.workspace,
            "feature/not-created",
            Some("trunk"),
        )
        .await
        .unwrap_err();

        assert!(
            error.to_string().contains("checking remote branch"),
            "{error:#}"
        );
        assert!(
            !local_branch_exists(&repositories.workspace, "feature/not-created")
                .await
                .unwrap()
        );
    }

    #[tokio::test]
    async fn pushes_the_checked_out_work_branch_and_sets_upstream() {
        let repositories = TestRepositories::new().await;
        let base_tip = repositories.remote_ref("trunk").await.unwrap();
        prepare_work_branch(&repositories.workspace, "feature/push", Some("trunk"))
            .await
            .unwrap();
        let local_tip = repositories
            .commit_in(&repositories.workspace, "pushed.txt", "push me\n")
            .await;

        push_work_branch(&repositories.workspace, "feature/push")
            .await
            .unwrap();

        assert_eq!(
            repositories.remote_ref("feature/push").await.unwrap(),
            local_tip
        );
        assert_eq!(repositories.remote_ref("trunk").await.unwrap(), base_tip);
        assert_eq!(
            git_output(
                &repositories.workspace,
                &[
                    "rev-parse",
                    "--abbrev-ref",
                    "--symbolic-full-name",
                    "@{upstream}",
                ],
            )
            .await
            .unwrap(),
            "origin/feature/push"
        );
    }

    #[tokio::test]
    async fn refuses_to_push_when_the_checked_out_branch_differs() {
        let repositories = TestRepositories::new().await;
        let current_branch = git_output(
            &repositories.workspace,
            &["symbolic-ref", "--short", "HEAD"],
        )
        .await
        .unwrap();
        assert_eq!(current_branch, "trunk");

        let error = push_work_branch(&repositories.workspace, "feature/not-checked-out")
            .await
            .unwrap_err();

        assert!(
            error.to_string().contains("currently on \"trunk\""),
            "{error:#}"
        );
        assert!(
            repositories
                .remote_ref("feature/not-checked-out")
                .await
                .is_none()
        );
    }

    #[tokio::test]
    async fn refuses_to_report_a_push_while_uncommitted_user_work_remains() {
        let repositories = TestRepositories::new().await;
        prepare_work_branch(&repositories.workspace, "feature/dirty", Some("trunk"))
            .await
            .unwrap();
        std::fs::write(
            repositories.workspace.join("unfinished.txt"),
            "not committed\n",
        )
        .unwrap();

        let error = push_work_branch(&repositories.workspace, "feature/dirty")
            .await
            .unwrap_err();

        assert!(
            error
                .to_string()
                .contains("uncommitted tracked or untracked"),
            "{error:#}"
        );
        assert!(repositories.remote_ref("feature/dirty").await.is_none());
    }

    #[tokio::test]
    async fn ignores_runner_owned_context_when_checking_for_uncommitted_work() {
        let repositories = TestRepositories::new().await;
        prepare_work_branch(
            &repositories.workspace,
            "feature/runner-context",
            Some("trunk"),
        )
        .await
        .unwrap();
        std::fs::create_dir_all(repositories.workspace.join(".karma-team")).unwrap();
        std::fs::write(
            repositories.workspace.join(".karma-team/context.md"),
            "runner-generated context\n",
        )
        .unwrap();

        push_work_branch(&repositories.workspace, "feature/runner-context")
            .await
            .unwrap();

        assert!(
            repositories
                .remote_ref("feature/runner-context")
                .await
                .is_some()
        );
        assert_eq!(
            std::fs::read_to_string(repositories.workspace.join(".karma-team/context.md")).unwrap(),
            "runner-generated context\n"
        );
    }

    #[tokio::test]
    async fn treats_an_unuploaded_workpad_as_uncommitted_user_work() {
        let repositories = TestRepositories::new().await;
        prepare_work_branch(&repositories.workspace, "feature/workpad", Some("trunk"))
            .await
            .unwrap();
        std::fs::write(
            repositories.workspace.join(".karma-team-workpad.md"),
            "not uploaded\n",
        )
        .unwrap();

        let error = push_work_branch(&repositories.workspace, "feature/workpad")
            .await
            .unwrap_err();

        assert!(error.to_string().contains(".karma-team-workpad.md"));
        assert!(repositories.remote_ref("feature/workpad").await.is_none());
    }

    #[tokio::test]
    async fn treats_a_remote_that_already_contains_local_head_as_pushed() {
        let repositories = TestRepositories::new().await;
        let local_tip = repositories
            .create_remote_branch("feature/already-pushed")
            .await;
        prepare_work_branch(
            &repositories.workspace,
            "feature/already-pushed",
            Some("trunk"),
        )
        .await
        .unwrap();
        let remote_tip = repositories
            .commit_in(&repositories.publisher, "remote.txt", "remote advanced\n")
            .await;
        git_output(
            &repositories.publisher,
            &["push", "origin", "feature/already-pushed"],
        )
        .await
        .unwrap();

        push_work_branch(&repositories.workspace, "feature/already-pushed")
            .await
            .unwrap();

        assert_eq!(
            git_output(&repositories.workspace, &["rev-parse", "HEAD"])
                .await
                .unwrap(),
            local_tip
        );
        assert_eq!(
            repositories
                .remote_ref("feature/already-pushed")
                .await
                .unwrap(),
            remote_tip
        );
        assert_eq!(
            git_output(
                &repositories.workspace,
                &[
                    "rev-parse",
                    "--abbrev-ref",
                    "--symbolic-full-name",
                    "@{upstream}",
                ],
            )
            .await
            .unwrap(),
            "origin/feature/already-pushed"
        );
    }

    #[tokio::test]
    async fn refuses_a_non_fast_forward_push_without_losing_either_tip() {
        let repositories = TestRepositories::new().await;
        prepare_work_branch(&repositories.workspace, "feature/push-race", Some("trunk"))
            .await
            .unwrap();
        let local_tip = repositories
            .commit_in(&repositories.workspace, "local.txt", "local race commit\n")
            .await;
        let remote_tip = repositories.create_remote_branch("feature/push-race").await;

        let error = push_work_branch(&repositories.workspace, "feature/push-race")
            .await
            .unwrap_err();

        assert!(
            error.to_string().contains("pushing work branch"),
            "{error:#}"
        );
        assert_eq!(
            git_output(&repositories.workspace, &["rev-parse", "HEAD"])
                .await
                .unwrap(),
            local_tip
        );
        assert_eq!(
            repositories.remote_ref("feature/push-race").await.unwrap(),
            remote_tip
        );
    }

    #[tokio::test]
    async fn worktree_base_initializes_without_committing_existing_files() {
        let tmp = tempdir().unwrap();
        let note = tmp.path().join("keep.txt");
        std::fs::write(&note, "leave me untracked").unwrap();

        ensure_worktree_base(tmp.path()).await.unwrap();

        assert!(is_git_repo(tmp.path()));
        assert!(
            Command::new("git")
                .current_dir(tmp.path())
                .args(["rev-parse", "--verify", "HEAD"])
                .output()
                .await
                .unwrap()
                .status
                .success()
        );
        let status = Command::new("git")
            .current_dir(tmp.path())
            .args(["status", "--porcelain"])
            .output()
            .await
            .unwrap();
        assert_eq!(
            String::from_utf8_lossy(&status.stdout).trim(),
            "?? keep.txt"
        );
        assert_eq!(std::fs::read_to_string(note).unwrap(), "leave me untracked");
    }

    #[tokio::test]
    async fn worktree_base_does_not_bootstrap_a_parent_repository() {
        let tmp = tempdir().unwrap();
        let parent = tmp.path().join("parent");
        let child = parent.join("runner-workspace");
        std::fs::create_dir_all(&child).unwrap();
        let init = Command::new("git")
            .arg("init")
            .arg(&parent)
            .output()
            .await
            .unwrap();
        assert!(init.status.success());

        ensure_worktree_base(&child).await.unwrap();

        assert!(child.join(".git").exists());
        let parent_head = Command::new("git")
            .current_dir(&parent)
            .args(["rev-parse", "--verify", "HEAD"])
            .output()
            .await
            .unwrap();
        assert!(!parent_head.status.success(), "parent repo was committed");
    }

    #[tokio::test]
    async fn worktree_base_bootstraps_unborn_head_without_committing_staged_files() {
        let tmp = tempdir().unwrap();
        assert!(
            Command::new("git")
                .arg("init")
                .arg(tmp.path())
                .output()
                .await
                .unwrap()
                .status
                .success()
        );
        std::fs::write(tmp.path().join("staged.txt"), "keep staged").unwrap();
        assert!(
            Command::new("git")
                .current_dir(tmp.path())
                .args(["add", "staged.txt"])
                .output()
                .await
                .unwrap()
                .status
                .success()
        );

        ensure_worktree_base(tmp.path()).await.unwrap();
        let first_head = git_output(tmp.path(), &["rev-parse", "HEAD"])
            .await
            .unwrap();
        assert_eq!(
            git_output(tmp.path(), &["ls-tree", "--name-only", "HEAD"])
                .await
                .unwrap(),
            ""
        );
        assert_eq!(
            git_output(tmp.path(), &["diff", "--cached", "--name-only"])
                .await
                .unwrap(),
            "staged.txt"
        );
        ensure_worktree_base(tmp.path()).await.unwrap();
        assert_eq!(
            git_output(tmp.path(), &["rev-parse", "HEAD"])
                .await
                .unwrap(),
            first_head
        );
    }

    #[tokio::test]
    async fn concurrent_worktree_initialization_creates_one_empty_commit() {
        let tmp = tempdir().unwrap();
        std::fs::write(tmp.path().join("existing.txt"), "preserved").unwrap();
        let mut tasks = Vec::new();
        for _ in 0..12 {
            let path = tmp.path().to_path_buf();
            tasks.push(tokio::spawn(
                async move { ensure_worktree_base(&path).await },
            ));
        }
        for task in tasks {
            task.await.unwrap().unwrap();
        }
        assert_eq!(
            git_output(tmp.path(), &["rev-list", "--count", "HEAD"])
                .await
                .unwrap(),
            "1"
        );
        assert_eq!(
            git_output(tmp.path(), &["ls-tree", "--name-only", "HEAD"])
                .await
                .unwrap(),
            ""
        );
        assert_eq!(
            std::fs::read_to_string(tmp.path().join("existing.txt")).unwrap(),
            "preserved"
        );
    }
}
