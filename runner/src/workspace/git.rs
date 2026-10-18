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
    let out = clone_command(url, target, use_first_use_trust)
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

pub fn is_empty_dir(path: &Path) -> bool {
    match std::fs::read_dir(path) {
        Ok(mut rd) => rd.next().is_none(),
        Err(_) => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[cfg(unix)]
    use std::os::unix::fs::PermissionsExt;
    #[cfg(unix)]
    use std::process::Command as StdCommand;
    use tempfile::tempdir;

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
