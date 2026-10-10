use anyhow::{Context, Result};
use std::path::Path;
use std::process::Stdio;
use tokio::process::Command;

use crate::cloud::protocol::WorkspaceState;

pub async fn clone(url: &str, target: &Path) -> Result<()> {
    if let Some(parent) = target.parent() {
        tokio::fs::create_dir_all(parent)
            .await
            .with_context(|| format!("creating parent dir {parent:?}"))?;
    }
    // `--` separator is required: a hostile `url` like `--upload-pack=...` would
    // otherwise be interpreted as a flag and execute arbitrary commands.
    let out = Command::new("git")
        .arg("clone")
        .arg("--")
        .arg(url)
        .arg(target)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
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

pub fn is_git_repo(path: &Path) -> bool {
    path.join(".git").exists()
}

pub fn is_empty_dir(path: &Path) -> bool {
    match std::fs::read_dir(path) {
        Ok(mut rd) => rd.next().is_none(),
        Err(_) => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

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
}

/// Prepare a directory for tools that require a Git worktree (currently the
/// Karma CLI). Existing files are deliberately left untracked: the only
/// commit created here is an empty bootstrap commit, so runner setup never
/// silently commits user work.
pub async fn ensure_worktree_base(path: &Path) -> Result<()> {
    tokio::fs::create_dir_all(path)
        .await
        .with_context(|| format!("creating git workspace {path:?}"))?;

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

    let out = Command::new("git")
        .current_dir(path)
        .env("GIT_AUTHOR_NAME", "karma-team runner")
        .env("GIT_AUTHOR_EMAIL", "karma-team-runner@localhost")
        .env("GIT_COMMITTER_NAME", "karma-team runner")
        .env("GIT_COMMITTER_EMAIL", "karma-team-runner@localhost")
        .args([
            "commit",
            "--allow-empty",
            "--no-verify",
            "-m",
            "Initialize runner workspace",
        ])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .await
        .context("creating empty git workspace commit")?;
    if !out.status.success() {
        anyhow::bail!(
            "git commit failed ({}): {}",
            out.status,
            String::from_utf8_lossy(&out.stderr).trim()
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
    let out = Command::new("git")
        .current_dir(path)
        .args(args)
        .output()
        .await?;
    if !out.status.success() {
        anyhow::bail!(
            "git {:?} failed: {}",
            args,
            String::from_utf8_lossy(&out.stderr).trim()
        );
    }
    Ok(String::from_utf8_lossy(&out.stdout).trim().to_string())
}
