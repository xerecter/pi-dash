use anyhow::{Context, Result};
use directories::ProjectDirs;
use std::fs;
use std::io::Write;
use std::path::Component;
use std::path::{Path, PathBuf};
use uuid::Uuid;

#[cfg(unix)]
use nix::unistd::{AccessFlags, Uid, access};
#[cfg(unix)]
use std::os::unix::fs::MetadataExt;

const QUALIFIER: &str = "so";
const ORG: &str = "karma-team";
const APP: &str = "karma-team";
const RUNTIME_PIN_FILE: &str = ".runtime-dir";

#[derive(Debug, Clone)]
pub struct Paths {
    pub config_dir: PathBuf,
    pub data_dir: PathBuf,
    pub runtime_dir: PathBuf,
}

/// Per-instance filesystem paths under `data_dir/runners/<runner_id>/`. Each
/// runner instance owns its own history, logs, and identity file; this newtype
/// keeps a runner_id baked in so call sites can't accidentally write to
/// another runner's tree.
#[derive(Debug, Clone)]
pub struct RunnerPaths {
    pub runner_id: Uuid,
    base_dir: PathBuf,
}

impl Paths {
    pub fn resolve(
        config_override: Option<PathBuf>,
        data_override: Option<PathBuf>,
    ) -> Result<Self> {
        Self::resolve_with_runtime(config_override, data_override, None)
    }

    pub(crate) fn resolve_with_runtime(
        config_override: Option<PathBuf>,
        data_override: Option<PathBuf>,
        runtime_override: Option<PathBuf>,
    ) -> Result<Self> {
        let dirs = ProjectDirs::from(QUALIFIER, ORG, APP)
            .context("unable to resolve XDG project directories")?;
        let config_dir = config_override.unwrap_or_else(|| dirs.config_dir().to_path_buf());
        // A genuinely custom data directory must also get an isolated PID and
        // IPC socket. Service definitions persist the already-resolved default
        // data directory in KARMA_TEAM_DATA_DIR, though; treating that value as
        // custom would split the daemon onto <data>/runtime while an ordinary
        // CLI keeps using XDG_RUNTIME_DIR. Compare with the platform default so
        // both sides resolve the same socket after service installation.
        let isolated = data_override_is_isolated(data_override.as_deref(), dirs.data_dir());
        let data_dir = data_override.unwrap_or_else(|| dirs.data_dir().to_path_buf());
        let runtime_dir = if let Some(runtime_override) = runtime_override {
            validate_runtime_override(runtime_override, &data_dir)?
        } else if isolated {
            data_dir.join("runtime")
        } else if let Some(pinned) = read_runtime_pin(&config_dir, &data_dir) {
            pinned
        } else {
            resolve_runtime_dir(&dirs, &data_dir)
        };
        Ok(Self {
            config_dir,
            data_dir,
            runtime_dir,
        })
    }

    pub fn config_path(&self) -> PathBuf {
        self.config_dir.join("config.toml")
    }

    pub fn credentials_path(&self) -> PathBuf {
        self.config_dir.join("credentials.toml")
    }

    pub fn runtime_pin_path(&self) -> PathBuf {
        self.config_dir.join(RUNTIME_PIN_FILE)
    }

    /// Persist the exact daemon IPC directory selected during service
    /// installation. A later login shell may have a different (or missing)
    /// XDG_RUNTIME_DIR; reading this pin keeps ordinary CLI commands and the
    /// service on the same socket across those environment changes.
    pub fn persist_runtime_dir(&self) -> Result<()> {
        self.ensure_config()?;
        let runtime = validate_runtime_override(self.runtime_dir.clone(), &self.data_dir)?;
        let destination = self.runtime_pin_path();
        let temporary = self
            .config_dir
            .join(format!(".{RUNTIME_PIN_FILE}.{}.tmp", Uuid::new_v4()));
        let result = (|| -> Result<()> {
            let mut options = fs::OpenOptions::new();
            options.create_new(true).write(true);
            #[cfg(unix)]
            {
                use std::os::unix::fs::OpenOptionsExt;
                options.mode(0o600);
            }
            let mut file = options
                .open(&temporary)
                .with_context(|| format!("creating runtime pin {temporary:?}"))?;
            writeln!(file, "{}", runtime.display())
                .with_context(|| format!("writing runtime pin {temporary:?}"))?;
            file.sync_all()
                .with_context(|| format!("syncing runtime pin {temporary:?}"))?;
            fs::rename(&temporary, &destination).with_context(|| {
                format!("replacing runtime pin {destination:?} from {temporary:?}")
            })?;
            Ok(())
        })();
        if result.is_err() {
            let _ = fs::remove_file(&temporary);
        }
        result
    }

    /// Daemon-level logs directory. Used only for service-supervisor
    /// stdout/stderr (launchd / systemd unit redirection); per-runner
    /// logs live under `RunnerPaths::logs_dir()`.
    pub fn logs_dir(&self) -> PathBuf {
        self.data_dir.join("logs")
    }

    pub fn pid_path(&self) -> PathBuf {
        self.runtime_dir.join("pid")
    }

    /// Crash-safe journal of `RunFailed` signals the daemon could not deliver
    /// while shutting down.
    pub fn drain_journal_path(&self) -> PathBuf {
        self.data_dir.join("pending_run_failures.json")
    }

    pub fn ipc_socket_path(&self) -> PathBuf {
        self.runtime_dir.join("karma-team.sock")
    }

    pub fn default_working_dir(&self) -> PathBuf {
        let base = std::env::var("TMPDIR")
            .map(PathBuf::from)
            .unwrap_or_else(|_| std::env::temp_dir());
        base.join(".karma-team")
    }

    /// Per-runner data directory: `data_dir/runners/<runner_id>/`.
    pub fn runner_dir(&self, runner_id: Uuid) -> PathBuf {
        self.data_dir.join("runners").join(runner_id.to_string())
    }

    /// Build a `RunnerPaths` rooted at this runner's data directory.
    pub fn for_runner(&self, runner_id: Uuid) -> RunnerPaths {
        RunnerPaths {
            runner_id,
            base_dir: self.runner_dir(runner_id),
        }
    }

    /// Remove runner-owned runtime state while preserving the configured
    /// working directory if a legacy config placed it inside the runtime
    /// tree. Modern defaults live under `data/workspaces`, but older
    /// `connect`/TUI flows used `data/runners/<id>/workspace`; blindly
    /// deleting the runner directory would delete the user's checkout.
    pub fn remove_runner_runtime_dir(
        &self,
        runner_id: Uuid,
        working_dir: Option<&Path>,
    ) -> Result<()> {
        let runner_dir = self.runner_dir(runner_id);
        let protected = working_dir.map(absolute_for_compare);
        let runner_root = absolute_for_compare(&runner_dir);
        let protected =
            protected.filter(|path| path == &runner_root || path.starts_with(&runner_root));

        if let Some(protected) = protected {
            remove_tree_preserving(&runner_dir, &runner_root, &protected)
        } else {
            match std::fs::remove_dir_all(&runner_dir) {
                Ok(()) => Ok(()),
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
                Err(error) => Err(error)
                    .with_context(|| format!("removing runner data directory {runner_dir:?}")),
            }
        }
    }

    /// Create only the directory needed for config and credential writes.
    /// Authentication must remain usable even when a stale runtime path or
    /// an unrelated data/log directory is temporarily inaccessible.
    pub fn ensure_config(&self) -> Result<()> {
        std::fs::create_dir_all(&self.config_dir)
            .with_context(|| format!("creating {:?}", self.config_dir))
    }

    /// Create every daemon-level directory. Daemon startup intentionally
    /// keeps this stronger check so PID, IPC, logs, and runner data failures
    /// surface before the service begins processing work.
    pub fn ensure(&self) -> Result<()> {
        self.ensure_config()?;
        std::fs::create_dir_all(&self.data_dir)
            .with_context(|| format!("creating {:?}", self.data_dir))?;
        ensure_private_runtime_dir(&self.runtime_dir)?;
        let logs_dir = self.logs_dir();
        std::fs::create_dir_all(&logs_dir).with_context(|| format!("creating {logs_dir:?}"))?;
        Ok(())
    }
}

fn read_runtime_pin(config_dir: &Path, data_dir: &Path) -> Option<PathBuf> {
    let path = config_dir.join(RUNTIME_PIN_FILE);
    let contents = match fs::read_to_string(&path) {
        Ok(contents) => contents,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return None,
        Err(error) => {
            tracing::warn!(?path, %error, "ignoring unreadable runtime directory pin");
            return None;
        }
    };
    let value = contents.trim();
    if value.is_empty() || value.lines().count() != 1 {
        tracing::warn!(?path, "ignoring malformed runtime directory pin");
        return None;
    }
    match validate_runtime_override(PathBuf::from(value), data_dir) {
        Ok(runtime) => Some(runtime),
        Err(error) => {
            tracing::warn!(?path, %error, "ignoring invalid runtime directory pin");
            None
        }
    }
}

fn validate_runtime_override(path: PathBuf, data_dir: &Path) -> Result<PathBuf> {
    if !path.is_absolute()
        || path
            .components()
            .any(|component| matches!(component, Component::CurDir | Component::ParentDir))
    {
        anyhow::bail!("runtime directory override must be an absolute normalized path: {path:?}");
    }

    let is_xdg_app_dir = path.file_name().is_some_and(|name| name == APP);
    let is_isolated_data_runtime = path == data_dir.join("runtime");
    if !is_xdg_app_dir && !is_isolated_data_runtime {
        anyhow::bail!(
            "runtime directory override must be a karma-team runtime directory: {path:?}"
        );
    }
    Ok(path)
}

fn data_override_is_isolated(data_override: Option<&Path>, default_data_dir: &Path) -> bool {
    data_override.is_some_and(|path| path != default_data_dir)
}

fn ensure_private_runtime_dir(path: &Path) -> Result<()> {
    std::fs::create_dir_all(path).with_context(|| format!("creating {path:?}"))?;

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;

        let metadata = std::fs::symlink_metadata(path)
            .with_context(|| format!("inspecting runtime directory {path:?}"))?;
        let euid = Uid::effective().as_raw();
        if !metadata.is_dir() || metadata.uid() != euid {
            anyhow::bail!(
                "runtime directory {path:?} must be a real directory owned by uid {euid}"
            );
        }

        let mut permissions = metadata.permissions();
        permissions.set_mode(0o700);
        std::fs::set_permissions(path, permissions).with_context(|| {
            format!("setting private permissions on runtime directory {path:?}")
        })?;

        if !owned_directory_is_accessible(path, euid, true) {
            anyhow::bail!("runtime directory {path:?} is not private and accessible by uid {euid}");
        }
    }

    Ok(())
}

#[cfg(unix)]
fn resolve_runtime_dir(_dirs: &ProjectDirs, data_dir: &Path) -> PathBuf {
    let euid = Uid::effective().as_raw();
    let env_base = std::env::var_os("XDG_RUNTIME_DIR")
        .filter(|value| !value.is_empty())
        .map(PathBuf::from);
    let standard_base = PathBuf::from("/run/user").join(euid.to_string());

    choose_runtime_dir(env_base.as_deref(), &standard_base, data_dir, |candidate| {
        runtime_location_is_usable(candidate, euid)
    })
}

#[cfg(not(unix))]
fn resolve_runtime_dir(dirs: &ProjectDirs, data_dir: &Path) -> PathBuf {
    dirs.runtime_dir()
        .map(Path::to_path_buf)
        .unwrap_or_else(|| data_dir.join("runtime"))
}

/// Pick a per-user runtime directory without trusting a stale inherited
/// `XDG_RUNTIME_DIR`. This commonly happens after `su`/`sudo`: the effective
/// user changes but the environment still points at `/run/user/0`, making
/// even unrelated config writes fail when [`Paths::ensure`] creates the
/// runtime tree.
///
/// `fallback_data_dir` is always user-scoped (`ProjectDirs::data_dir`) and is
/// therefore the safe last resort when neither runtime base can be used.
#[cfg(unix)]
fn choose_runtime_dir<F>(
    env_base: Option<&Path>,
    standard_base: &Path,
    fallback_data_dir: &Path,
    mut is_usable: F,
) -> PathBuf
where
    F: FnMut(&Path) -> bool,
{
    if let Some(base) = env_base.filter(|base| is_usable(base)) {
        return base.join(APP);
    }

    if env_base != Some(standard_base) && is_usable(standard_base) {
        return standard_base.join(APP);
    }

    fallback_data_dir.join("runtime")
}

#[cfg(unix)]
fn runtime_location_is_usable(base: &Path, euid: u32) -> bool {
    if !owned_directory_is_accessible(base, euid, true) {
        return false;
    }

    let app_dir = base.join(APP);
    match std::fs::symlink_metadata(&app_dir) {
        // The private XDG base already prevents other users from traversing
        // into this directory. Accept an owner-accessible legacy 0755 app
        // directory so daemon startup can normalize it to 0700 instead of
        // abandoning the canonical socket for a fallback path.
        Ok(_) => owned_directory_is_accessible(&app_dir, euid, false),
        Err(error) => error.kind() == std::io::ErrorKind::NotFound,
    }
}

#[cfg(unix)]
fn owned_directory_is_accessible(path: &Path, euid: u32, require_private: bool) -> bool {
    // Do not follow a user-controlled runtime-directory symlink. Both the XDG
    // base and an existing application subdirectory must be real directories
    // owned by the effective user; callers decide whether the directory itself
    // must also exclude group/other access.
    let Ok(metadata) = std::fs::symlink_metadata(path) else {
        return false;
    };

    metadata.is_dir()
        && metadata.uid() == euid
        // The owner must be able to create files and traverse the directory.
        // Checking the mode explicitly keeps this correct for uid 0, for
        // which access(2) otherwise bypasses normal read/write permission
        // checks.
        && metadata.mode() & 0o700 == 0o700
        // The XDG base (and the final normalized app directory) must be private
        // to the user. A legacy app directory can be accepted temporarily when
        // it is already protected by a private base and will be chmod'd next.
        && (!require_private || metadata.mode() & 0o077 == 0)
        && access(
            path,
            AccessFlags::R_OK | AccessFlags::W_OK | AccessFlags::X_OK,
        )
        .is_ok()
}

fn absolute_for_compare(path: &Path) -> PathBuf {
    std::fs::canonicalize(path)
        .or_else(|_| std::path::absolute(path))
        .unwrap_or_else(|_| path.to_path_buf())
}

/// Recursively remove all children except the single ancestor chain that
/// leads to `protected`. The protected directory and its contents are never
/// traversed, so this also works when the whole runner tree is the checkout.
fn remove_tree_preserving(root: &Path, absolute_root: &Path, protected: &Path) -> Result<()> {
    if absolute_root == protected {
        return Ok(());
    }

    let entries = match std::fs::read_dir(root) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => {
            return Err(error).with_context(|| format!("reading runner data directory {root:?}"));
        }
    };
    for entry in entries {
        let entry = entry?;
        let path = entry.path();
        let absolute_path = absolute_root.join(entry.file_name());
        let contains_protected = protected.starts_with(&absolute_path);
        if contains_protected && entry.file_type()?.is_dir() {
            remove_tree_preserving(&path, &absolute_path, protected)?;
            continue;
        }
        if entry.file_type()?.is_dir() {
            std::fs::remove_dir_all(&path)
                .with_context(|| format!("removing runner runtime directory {path:?}"))?;
        } else {
            std::fs::remove_file(&path)
                .with_context(|| format!("removing runner runtime file {path:?}"))?;
        }
    }
    Ok(())
}

impl RunnerPaths {
    /// Root of this runner's per-instance tree.
    pub fn base_dir(&self) -> &Path {
        &self.base_dir
    }

    /// `<base>/history/`.
    pub fn history_dir(&self) -> PathBuf {
        self.base_dir.join("history")
    }

    /// `<base>/history/runs/` — one file per run.
    pub fn runs_dir(&self) -> PathBuf {
        self.history_dir().join("runs")
    }

    /// `<base>/history/runs_index.json`.
    pub fn runs_index_path(&self) -> PathBuf {
        self.history_dir().join("runs_index.json")
    }

    /// `<base>/logs/`.
    pub fn logs_dir(&self) -> PathBuf {
        self.base_dir.join("logs")
    }

    /// `<base>/identity.toml` — runner_id, name, registered_at, workspace_slug.
    pub fn identity_path(&self) -> PathBuf {
        self.base_dir.join("identity.toml")
    }

    /// `<base>/credentials.toml` — legacy per-runner refresh-token state.
    pub fn credentials_path(&self) -> PathBuf {
        self.base_dir.join("credentials.toml")
    }

    /// Create the runner's directory tree on disk.
    pub fn ensure(&self) -> Result<()> {
        for dir in [&self.base_dir, &self.runs_dir(), &self.logs_dir()] {
            std::fs::create_dir_all(dir).with_context(|| format!("creating {dir:?}"))?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(unix)]
    use std::os::unix::fs::PermissionsExt;

    #[test]
    fn data_override_isolates_daemon_socket_and_pid() {
        let tmp = tempfile::tempdir().unwrap();
        let first = Paths::resolve(None, Some(tmp.path().join("first"))).unwrap();
        let second = Paths::resolve(None, Some(tmp.path().join("second"))).unwrap();
        assert_eq!(first.runtime_dir, tmp.path().join("first/runtime"));
        assert_ne!(first.ipc_socket_path(), second.ipc_socket_path());
        assert_ne!(first.pid_path(), second.pid_path());
    }

    #[test]
    fn persisted_default_data_dir_keeps_service_and_cli_on_the_same_runtime() {
        let default_data = Path::new("/home/user/.local/share/karma-team");

        // Normal CLI: no override. Installed service: the same resolved
        // default is persisted in KARMA_TEAM_DATA_DIR. Neither is isolated.
        assert!(!data_override_is_isolated(None, default_data));
        assert!(!data_override_is_isolated(Some(default_data), default_data));

        assert!(data_override_is_isolated(
            Some(Path::new("/srv/isolated-karma-team")),
            default_data
        ));
    }

    #[test]
    fn service_runtime_override_survives_a_different_startup_data_environment() {
        let tmp = tempfile::tempdir().unwrap();
        let install_time_data = tmp.path().join("custom-xdg-data/karma-team");
        let startup_default_data = tmp.path().join("default-data/karma-team");
        let runtime = tmp.path().join("run/user/1000/karma-team");

        // The service persists both the data directory and the exact runtime
        // selected by the installing CLI. Even if ProjectDirs would produce a
        // different default at service startup (for example XDG_DATA_HOME was
        // not imported), the explicit runtime keeps IPC on the same socket.
        let paths = Paths::resolve_with_runtime(
            None,
            Some(install_time_data.clone()),
            Some(runtime.clone()),
        )
        .unwrap();
        assert_eq!(paths.data_dir, install_time_data);
        assert_eq!(paths.runtime_dir, runtime);
        assert_ne!(paths.data_dir, startup_default_data);
    }

    #[test]
    fn service_runtime_override_rejects_relative_broad_or_wrong_leaf_paths() {
        let data = PathBuf::from("/home/user/.local/share/karma-team");
        for runtime in [
            PathBuf::from("relative/runtime"),
            PathBuf::from("/tmp"),
            PathBuf::from("/run/user/1000/not-karma-team"),
            PathBuf::from("/run/user/1000/../1000/karma-team"),
        ] {
            assert!(Paths::resolve_with_runtime(None, Some(data.clone()), Some(runtime)).is_err());
        }
    }

    #[test]
    fn service_runtime_override_accepts_standard_xdg_and_isolated_data_runtime() {
        let data = PathBuf::from("/home/user/.local/share/karma-team");
        for runtime in [
            PathBuf::from("/run/user/1000/karma-team"),
            data.join("runtime"),
        ] {
            let paths =
                Paths::resolve_with_runtime(None, Some(data.clone()), Some(runtime.clone()))
                    .unwrap();
            assert_eq!(paths.runtime_dir, runtime);
        }
    }

    #[test]
    fn persisted_runtime_pin_keeps_later_cli_resolution_on_the_same_socket() {
        let tmp = tempfile::tempdir().unwrap();
        let config = tmp.path().join("config");
        let data = tmp.path().join("data");
        let runtime = tmp.path().join("custom-session/karma-team");
        let installed = Paths {
            config_dir: config.clone(),
            data_dir: data.clone(),
            runtime_dir: runtime.clone(),
        };
        installed.persist_runtime_dir().unwrap();

        assert_eq!(read_runtime_pin(&config, &data), Some(runtime.clone()));
        assert_eq!(
            fs::read_to_string(installed.runtime_pin_path())
                .unwrap()
                .trim(),
            runtime.to_string_lossy()
        );
    }

    #[test]
    fn malformed_runtime_pin_is_ignored() {
        let tmp = tempfile::tempdir().unwrap();
        let config = tmp.path().join("config");
        let data = tmp.path().join("data");
        fs::create_dir_all(&config).unwrap();
        fs::write(config.join(RUNTIME_PIN_FILE), "/tmp\n/other\n").unwrap();

        assert_eq!(read_runtime_pin(&config, &data), None);
    }

    #[cfg(unix)]
    #[test]
    fn stale_xdg_runtime_dir_uses_current_users_standard_runtime_dir() {
        let tmp = tempfile::tempdir().unwrap();
        let inherited_root_runtime = tmp.path().join("run/user/0");
        let current_user_runtime = tmp.path().join("run/user/1000");
        let fallback = tmp.path().join("data");

        let selected = choose_runtime_dir(
            Some(&inherited_root_runtime),
            &current_user_runtime,
            &fallback,
            |candidate| candidate == current_user_runtime,
        );

        assert_eq!(selected, current_user_runtime.join(APP));
    }

    #[cfg(unix)]
    #[test]
    fn valid_xdg_runtime_dir_is_preferred() {
        let tmp = tempfile::tempdir().unwrap();
        let xdg_runtime = tmp.path().join("custom-runtime");
        let standard_runtime = tmp.path().join("run/user/1000");
        let fallback = tmp.path().join("data");

        let selected = choose_runtime_dir(
            Some(&xdg_runtime),
            &standard_runtime,
            &fallback,
            |candidate| candidate == xdg_runtime || candidate == standard_runtime,
        );

        assert_eq!(selected, xdg_runtime.join(APP));
    }

    #[cfg(unix)]
    #[test]
    fn ensure_creates_private_runtime_and_keeps_runtime_selection_stable() {
        let tmp = tempfile::tempdir().unwrap();
        let runtime_base = tmp.path().join("runtime-base");
        let standard_runtime = tmp.path().join("missing-standard-runtime");
        let data_dir = tmp.path().join("data");
        std::fs::create_dir(&runtime_base).unwrap();
        std::fs::set_permissions(&runtime_base, std::fs::Permissions::from_mode(0o700)).unwrap();
        let euid = Uid::effective().as_raw();

        let first = choose_runtime_dir(
            Some(&runtime_base),
            &standard_runtime,
            &data_dir,
            |candidate| runtime_location_is_usable(candidate, euid),
        );
        let paths = Paths {
            config_dir: tmp.path().join("config"),
            data_dir: data_dir.clone(),
            runtime_dir: first.clone(),
        };
        paths.ensure().unwrap();

        assert_eq!(
            std::fs::symlink_metadata(&first).unwrap().mode() & 0o777,
            0o700
        );
        assert!(runtime_location_is_usable(&runtime_base, euid));

        let second = choose_runtime_dir(
            Some(&runtime_base),
            &standard_runtime,
            &data_dir,
            |candidate| runtime_location_is_usable(candidate, euid),
        );
        assert_eq!(second, first);

        // Existing app/fallback runtime directories are normalized as well.
        std::fs::set_permissions(&first, std::fs::Permissions::from_mode(0o755)).unwrap();
        paths.ensure().unwrap();
        assert_eq!(
            std::fs::symlink_metadata(&first).unwrap().mode() & 0o777,
            0o700
        );
    }

    #[cfg(unix)]
    #[test]
    fn unusable_runtime_candidates_fall_back_and_ensure_is_idempotent() {
        let tmp = tempfile::tempdir().unwrap();
        let fallback = tmp.path().join("data");
        let runtime_dir = choose_runtime_dir(
            Some(Path::new("/wrong-user-runtime")),
            Path::new("/missing-current-user-runtime"),
            &fallback,
            |_| false,
        );
        let paths = Paths {
            config_dir: tmp.path().join("config"),
            data_dir: fallback.clone(),
            runtime_dir,
        };

        paths.ensure().unwrap();
        paths.ensure().unwrap();

        assert_eq!(paths.runtime_dir, fallback.join("runtime"));
        assert!(paths.runtime_dir.is_dir());
    }

    #[cfg(unix)]
    #[test]
    fn runtime_location_rejects_wrong_owner_or_insecure_permissions() {
        let tmp = tempfile::tempdir().unwrap();
        let base = tmp.path().join("runtime-base");
        std::fs::create_dir(&base).unwrap();
        std::fs::set_permissions(&base, std::fs::Permissions::from_mode(0o700)).unwrap();
        let euid = Uid::effective().as_raw();

        assert!(runtime_location_is_usable(&base, euid));
        assert!(!runtime_location_is_usable(&base, euid.wrapping_add(1)));

        std::fs::set_permissions(&base, std::fs::Permissions::from_mode(0o500)).unwrap();
        assert!(!runtime_location_is_usable(&base, euid));

        std::fs::set_permissions(&base, std::fs::Permissions::from_mode(0o755)).unwrap();
        assert!(!runtime_location_is_usable(&base, euid));
        std::fs::set_permissions(&base, std::fs::Permissions::from_mode(0o700)).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn runtime_location_rejects_inaccessible_existing_app_directory() {
        let tmp = tempfile::tempdir().unwrap();
        let base = tmp.path().join("runtime-base");
        let app_dir = base.join(APP);
        std::fs::create_dir_all(&app_dir).unwrap();
        std::fs::set_permissions(&base, std::fs::Permissions::from_mode(0o700)).unwrap();
        std::fs::set_permissions(&app_dir, std::fs::Permissions::from_mode(0o500)).unwrap();

        assert!(!runtime_location_is_usable(
            &base,
            Uid::effective().as_raw()
        ));

        std::fs::set_permissions(&app_dir, std::fs::Permissions::from_mode(0o755)).unwrap();
        assert!(runtime_location_is_usable(&base, Uid::effective().as_raw()));

        std::fs::set_permissions(&app_dir, std::fs::Permissions::from_mode(0o700)).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn runtime_location_rejects_symlinked_base() {
        let tmp = tempfile::tempdir().unwrap();
        let actual = tmp.path().join("actual-runtime");
        let link = tmp.path().join("runtime-link");
        std::fs::create_dir(&actual).unwrap();
        std::os::unix::fs::symlink(&actual, &link).unwrap();

        assert!(!runtime_location_is_usable(
            &link,
            Uid::effective().as_raw()
        ));
    }

    fn fixed_id() -> Uuid {
        Uuid::parse_str("12345678-1234-5678-1234-567812345678").unwrap()
    }

    fn paths_at(base: &Path) -> Paths {
        Paths {
            config_dir: base.join("config"),
            data_dir: base.join("data"),
            runtime_dir: base.join("runtime"),
        }
    }

    #[test]
    fn runner_dir_is_under_data_runners_runner_id() {
        let tmp = tempfile::tempdir().unwrap();
        let paths = paths_at(tmp.path());
        let id = fixed_id();
        let dir = paths.runner_dir(id);
        assert_eq!(
            dir,
            tmp.path().join("data").join("runners").join(id.to_string())
        );
    }

    #[test]
    fn for_runner_bakes_in_runner_id_and_base_dir() {
        let tmp = tempfile::tempdir().unwrap();
        let paths = paths_at(tmp.path());
        let id = fixed_id();
        let rp = paths.for_runner(id);
        assert_eq!(rp.runner_id, id);
        assert_eq!(rp.base_dir(), paths.runner_dir(id));
    }

    #[test]
    fn runner_paths_compose_history_runs_logs_identity() {
        let tmp = tempfile::tempdir().unwrap();
        let paths = paths_at(tmp.path());
        let rp = paths.for_runner(fixed_id());
        let base = rp.base_dir().to_path_buf();
        assert_eq!(rp.history_dir(), base.join("history"));
        assert_eq!(rp.runs_dir(), base.join("history").join("runs"));
        assert_eq!(
            rp.runs_index_path(),
            base.join("history").join("runs_index.json")
        );
        assert_eq!(rp.logs_dir(), base.join("logs"));
        assert_eq!(rp.identity_path(), base.join("identity.toml"));
    }

    #[test]
    fn removing_legacy_runner_tree_preserves_nested_user_checkout() {
        let tmp = tempfile::tempdir().unwrap();
        let paths = paths_at(tmp.path());
        let id = fixed_id();
        let runner_dir = paths.runner_dir(id);
        let checkout = runner_dir.join("workspace");
        std::fs::create_dir_all(checkout.join("src")).unwrap();
        std::fs::write(checkout.join("src/main.rs"), "keep").unwrap();
        std::fs::write(runner_dir.join("credentials.toml"), "remove").unwrap();

        paths
            .remove_runner_runtime_dir(id, Some(&checkout))
            .unwrap();

        assert!(checkout.join("src/main.rs").exists());
        assert!(!runner_dir.join("credentials.toml").exists());
        assert!(runner_dir.exists());
    }

    #[test]
    fn removing_runner_tree_is_idempotent_without_checkout() {
        let tmp = tempfile::tempdir().unwrap();
        let paths = paths_at(tmp.path());
        let id = fixed_id();
        paths.for_runner(id).ensure().unwrap();
        paths.remove_runner_runtime_dir(id, None).unwrap();
        paths.remove_runner_runtime_dir(id, None).unwrap();
        assert!(!paths.runner_dir(id).exists());
    }

    #[test]
    fn runner_paths_ensure_creates_history_runs_and_logs() {
        let tmp = tempfile::tempdir().unwrap();
        let paths = paths_at(tmp.path());
        let rp = paths.for_runner(fixed_id());
        rp.ensure().unwrap();
        assert!(rp.runs_dir().is_dir(), "runs dir should be created");
        assert!(rp.logs_dir().is_dir(), "logs dir should be created");
        assert!(rp.base_dir().is_dir(), "base dir should be created");
    }

    #[test]
    fn two_runners_get_disjoint_trees() {
        let tmp = tempfile::tempdir().unwrap();
        let paths = paths_at(tmp.path());
        let a = paths.for_runner(Uuid::new_v4());
        let b = paths.for_runner(Uuid::new_v4());
        assert_ne!(a.base_dir(), b.base_dir());
        assert_ne!(a.runs_dir(), b.runs_dir());
        assert_ne!(a.logs_dir(), b.logs_dir());
    }
}
