use anyhow::{Context, Result};
use nix::unistd::{User, geteuid};
use std::io::IsTerminal;
use std::path::{Path, PathBuf};
use tokio::process::Command;

use super::BootStartOutcome;
use crate::util::paths::Paths;

const UNIT_NAME: &str = "karma-team.service";
const UNIT_VERSION_MARKER: &str = "# karma-team-managed-unit-version=2";

/// Write the unit file and reload the systemd user manager. Does NOT enable
/// or start the unit — that's a separate step so `karma-team install` can gate
/// it on `karma-team configure` completing first.
pub async fn write_unit(paths: &Paths) -> Result<()> {
    let _lock = super::lock_service_definition(paths)?;
    let unit_path = unit_path()?;
    let exe = std::env::current_exe()?;
    let exe_str = super::validate_path_for_unit(&exe)?;
    let config_dir = super::validate_path_for_unit(&paths.config_dir)?;
    let data_dir = super::validate_path_for_unit(&paths.data_dir)?;
    let runtime_dir = super::validate_path_for_unit(&paths.runtime_dir)?;
    // See `service::capture_install_time_path` for why we bake $PATH in.
    let path_env = super::capture_install_time_path();
    let body = render_unit(
        exe_str,
        config_dir,
        data_dir,
        runtime_dir,
        path_env.as_deref(),
    );
    super::atomic_write_service_file(&unit_path, &body)?;
    paths.persist_runtime_dir()?;
    run_systemctl(&["daemon-reload"]).await?;
    println!("installed systemd unit at {}", unit_path.display());
    Ok(())
}

/// Render the user-unit body. `HOME` and `XDG_RUNTIME_DIR` are explicit even
/// though a healthy systemd user manager normally provides both. This keeps a
/// manager whose environment was imported from `sudo`/root from launching the
/// daemon against `/root` or `/run/user/0`. `%h` and `%t` are systemd's
/// canonical home and per-user runtime-directory specifiers; importantly,
/// `%t` is the *base* runtime directory, not a project-scoped
/// `/run/user/$UID/karma-team` path, so `ProjectDirs` appends `karma-team`
/// exactly once.
///
/// `path_env`, when `Some`, is rendered as an `Environment=PATH=...` line
/// so the daemon (and every subprocess it forks) inherits the operator's
/// interactive PATH instead of `systemd --user`'s minimal default. See
/// `service::capture_install_time_path` for the full rationale.
fn render_unit(
    exe: &str,
    config_dir: &str,
    data_dir: &str,
    runtime_dir: &str,
    path_env: Option<&str>,
) -> String {
    let path_line = match path_env {
        Some(p) => format!("\nEnvironment=\"PATH={}\"", systemd_double_quoted_escape(p)),
        None => String::new(),
    };
    format!(
        r#"{UNIT_VERSION_MARKER}
[Unit]
Description=karma-team Runner
After=network-online.target
Wants=network-online.target

[Service]
Type=simple
ExecStart={exe} --runtime-dir="{runtime_dir}" __run
Environment="HOME=%h"
Environment="XDG_RUNTIME_DIR=%t"
Environment="DBUS_SESSION_BUS_ADDRESS=unix:path=%t/bus"
Environment=KARMA_TEAM_CONFIG_DIR={config_dir}
Environment=KARMA_TEAM_DATA_DIR={data_dir}{path_line}
Restart=on-failure
RestartSec=5

[Install]
WantedBy=default.target
"#,
        runtime_dir = systemd_double_quoted_escape(runtime_dir)
    )
}

/// Escape a value to be safe inside a systemd `Environment="KEY=VALUE"`
/// directive. Per `man systemd.syntax(7)`, the only valid C-style escapes
/// inside a double-quoted string are `\a \b \f \n \r \t \v \\ \" \' \s
/// \xXX \NNN`. `\$` and `` \` `` are *not* recognized: systemd-analyze
/// rejects them as `Invalid syntax, ignoring`, silently dropping the
/// entire directive. systemd does no `$VAR` or backtick expansion in
/// `Environment=` values either, so `$` and backtick pass through
/// verbatim and need no escaping.
///
/// We therefore only escape `\` and `"`, the two characters that are
/// genuinely special inside the double quotes. A pathological PATH entry
/// containing either still produces a valid unit; everything else is left
/// untouched.
fn systemd_double_quoted_escape(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    for c in value.chars() {
        match c {
            '\\' => out.push_str("\\\\"),
            '"' => out.push_str("\\\""),
            other => out.push(other),
        }
    }
    out
}

fn unit_body_is_current(body: &str, expected_runtime: &str) -> bool {
    let runtime_arg = format!(
        "--runtime-dir=\"{}\"",
        systemd_double_quoted_escape(expected_runtime)
    );
    body.contains(UNIT_VERSION_MARKER) && body.contains(&runtime_arg)
}

/// Enable the unit at boot/login and bring it up. `restart` (not `start`) so
/// a re-configure against an already-running daemon forces it to reload the
/// freshly-written `credentials.toml`. On a stopped unit `restart` just
/// starts it — same net effect as before for the first-install path.
pub async fn enable_and_start() -> Result<()> {
    run_systemctl(&["enable", UNIT_NAME]).await?;
    run_systemctl(&["restart", UNIT_NAME]).await?;
    Ok(())
}

pub async fn uninstall(_: &Paths) -> Result<()> {
    run_systemctl(&["disable", UNIT_NAME]).await.ok();
    let p = unit_path()?;
    if p.exists() {
        tokio::fs::remove_file(&p).await?;
    }
    run_systemctl(&["daemon-reload"]).await.ok();
    println!("uninstalled systemd unit");
    Ok(())
}

/// Self-heal for `karma-team restart`: rewrite a missing or legacy unit.
///
/// `write_unit` already runs `daemon-reload`, so a unit rewritten here is
/// visible to the manager by the time the caller reaches `enable_and_start`
/// — which matters, because that step runs `systemctl --user enable`, and
/// enable is what fails with "Unit file karma-team.service does not exist."
pub(crate) async fn rewrite_unit_if_missing(paths: &Paths) -> Result<bool> {
    let path = unit_path()?;
    let current = tokio::fs::read_to_string(&path).await.ok();
    let expected_runtime = super::validate_path_for_unit(&paths.runtime_dir)?;
    if current
        .as_deref()
        .is_some_and(|body| unit_body_is_current(body, expected_runtime))
    {
        return Ok(false);
    }
    write_unit(paths).await?;
    Ok(true)
}

pub async fn start() -> Result<()> {
    run_systemctl(&["start", UNIT_NAME]).await
}

pub async fn stop() -> Result<()> {
    run_systemctl(&["stop", UNIT_NAME]).await
}

/// Detect whether `loginctl enable-linger` has already been applied to the
/// current user. Linger is what makes the user-level systemd manager (and
/// therefore our user unit) start at boot rather than at first login.
pub async fn is_linger_enabled() -> Result<bool> {
    let user = current_user_context()?;
    let out = Command::new("loginctl")
        .args(["show-user", &user.name, "--property=Linger"])
        .output()
        .await
        .context("invoking loginctl show-user")?;
    if !out.status.success() {
        anyhow::bail!(
            "loginctl show-user {} failed: {}",
            user.name,
            String::from_utf8_lossy(&out.stderr).trim()
        );
    }
    Ok(String::from_utf8_lossy(&out.stdout).trim() == "Linger=yes")
}

/// Shell out to `sudo loginctl enable-linger <user>`. sudo runs inline on
/// the caller's TTY, so any password prompt is surfaced to the user directly
/// — we don't feed stdin or try to cache credentials ourselves.
pub async fn enable_linger() -> Result<()> {
    let user = current_user_context()?;
    let status = Command::new("sudo")
        .args(["loginctl", "enable-linger", &user.name])
        .status()
        .await
        .context("invoking sudo loginctl enable-linger")?;
    if !status.success() {
        anyhow::bail!(
            "sudo loginctl enable-linger {} exited with {status}",
            user.name
        );
    }
    Ok(())
}

/// Best-effort: enable linger so `karma-team.service` survives reboot without a
/// login. Never fails the caller — every branch that can't apply linger maps
/// to an outcome the caller prints as a hint. Skips the sudo call entirely
/// when there's no TTY (sudo would hang or fail on non-interactive runs).
pub async fn ensure_linger() -> BootStartOutcome {
    match is_linger_enabled().await {
        Ok(true) => return BootStartOutcome::AlreadyEnabled,
        Ok(false) => {}
        Err(e) => return BootStartOutcome::CheckFailed(e.to_string()),
    }
    if !std::io::stdin().is_terminal() {
        return BootStartOutcome::NonInteractive;
    }
    match enable_linger().await {
        Ok(()) => BootStartOutcome::Enabled,
        Err(e) => BootStartOutcome::EnableFailed(e.to_string()),
    }
}

pub async fn status() -> Result<String> {
    let out = user_systemctl_command()?
        .args(["--user", "is-active", UNIT_NAME])
        .output()
        .await
        .context("invoking systemctl")?;
    Ok(String::from_utf8_lossy(&out.stdout).trim().to_string())
}

/// Best-effort diagnosis of a recently-dead daemon. On systemd we lean
/// on `systemctl status` (rich exit-code/signal reporting + the last
/// few journal lines) since that's already what operators reach for
/// here; reload.rs's existing journal capture covers the common cases
/// well enough that a hand-rolled translator would mostly duplicate it.
/// Returns `None` so the caller falls back to its generic IPC-timeout
/// message — we deliberately don't compete with systemd's own diagnostic
/// output on Linux.
pub async fn diagnose_recent_exit() -> Option<String> {
    None
}

fn unit_path() -> Result<PathBuf> {
    let user = current_user_context()?;
    Ok(unit_path_in(&user.home))
}

async fn run_systemctl(args: &[&str]) -> Result<()> {
    let mut full = vec!["--user"];
    full.extend_from_slice(args);
    let status = user_systemctl_command()?
        .args(&full)
        .status()
        .await
        .context("invoking systemctl")?;
    if !status.success() {
        anyhow::bail!("systemctl {full:?} failed: {status}");
    }
    Ok(())
}

#[derive(Debug)]
struct SystemdUserContext {
    name: String,
    home: PathBuf,
    runtime_dir: PathBuf,
}

/// Resolve identity from the effective uid rather than `$USER`/`$HOME`.
///
/// `sudo -u`, `su`, automation wrappers, and long-lived shells can all leave
/// root's environment variables behind while changing the effective uid. The
/// daemon is a per-user service, so the kernel/NSS identity is authoritative.
fn current_user_context() -> Result<SystemdUserContext> {
    let uid = geteuid();
    let user = User::from_uid(uid)
        .context("looking up effective systemd user")?
        .with_context(|| format!("no passwd entry for effective uid {}", uid.as_raw()))?;
    Ok(SystemdUserContext {
        name: user.name,
        home: user.dir,
        runtime_dir: PathBuf::from(format!("/run/user/{}", uid.as_raw())),
    })
}

fn unit_path_in(home: &Path) -> PathBuf {
    home.join(".config/systemd/user").join(UNIT_NAME)
}

/// Construct `systemctl --user` with an environment tied to the effective
/// user. Overwriting all identity/session variables prevents an inherited
/// root bus (`/run/user/0/bus`) from making an install operate on the wrong
/// user manager or fail with a permission error.
fn user_systemctl_command() -> Result<Command> {
    let user = current_user_context()?;
    let mut command = Command::new("systemctl");
    apply_user_manager_environment(&mut command, &user);
    Ok(command)
}

fn apply_user_manager_environment(command: &mut Command, user: &SystemdUserContext) {
    command
        .env("HOME", &user.home)
        .env("USER", &user.name)
        .env("LOGNAME", &user.name)
        .env("XDG_RUNTIME_DIR", &user.runtime_dir)
        .env(
            "DBUS_SESSION_BUS_ADDRESS",
            format!("unix:path={}/bus", user.runtime_dir.display()),
        );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unit_body_sets_systemd_home_and_base_runtime_dir() {
        // Regression: a manager environment imported from root made the
        // daemon write below /run/user/0. The explicit runtime argument pins
        // daemon IPC to the same path selected by the installing CLI.
        let body = render_unit(
            "/home/user/.cargo/bin/karma-team",
            "/home/user/.config/karma-team",
            "/home/user/.local/share/karma-team",
            "/run/user/1000/karma-team",
            None,
        );
        assert!(body.starts_with(UNIT_VERSION_MARKER));
        assert!(body.contains("Environment=\"HOME=%h\""));
        assert!(body.contains("Environment=\"XDG_RUNTIME_DIR=%t\""));
        assert!(body.contains("Environment=\"DBUS_SESSION_BUS_ADDRESS=unix:path=%t/bus\""));
        assert!(body.contains("--runtime-dir=\"/run/user/1000/karma-team\""));
        assert!(!body.contains("/run/user/0"));
    }

    #[test]
    fn unit_body_includes_exec_start_and_dirs() {
        let body = render_unit(
            "/bin/karma-team",
            "/etc/karma-team",
            "/var/lib/karma-team",
            "/var/lib/karma-team/runtime",
            None,
        );
        assert!(body.contains(
            "ExecStart=/bin/karma-team --runtime-dir=\"/var/lib/karma-team/runtime\" __run"
        ));
        assert!(body.contains("Environment=KARMA_TEAM_CONFIG_DIR=/etc/karma-team"));
        assert!(body.contains("Environment=KARMA_TEAM_DATA_DIR=/var/lib/karma-team"));
    }

    #[test]
    fn legacy_or_mismatched_units_are_refreshed() {
        let legacy = "[Service]\nExecStart=/bin/karma-team __run\n";
        assert!(!unit_body_is_current(legacy, "/run/user/1000/karma-team"));

        let current = render_unit(
            "/bin/karma-team",
            "/cfg",
            "/data",
            "/run/user/1000/karma-team",
            None,
        );
        assert!(unit_body_is_current(&current, "/run/user/1000/karma-team"));
        assert!(!unit_body_is_current(&current, "/run/user/1001/karma-team"));
    }

    #[test]
    fn unit_body_omits_path_when_not_captured() {
        let body = render_unit(
            "/bin/karma-team",
            "/etc/karma-team",
            "/var/lib/karma-team",
            "/var/lib/karma-team/runtime",
            None,
        );
        assert!(
            !body.contains("Environment=\"PATH=") && !body.contains("Environment=PATH="),
            "unit body must not declare PATH when path_env is None; got:\n{body}"
        );
    }

    #[test]
    fn unit_body_bakes_in_path_when_provided() {
        let body = render_unit(
            "/bin/karma-team",
            "/etc/karma-team",
            "/var/lib/karma-team",
            "/var/lib/karma-team/runtime",
            Some("/home/user/.local/bin:/usr/local/bin:/usr/bin"),
        );
        assert!(
            body.contains("Environment=\"PATH=/home/user/.local/bin:/usr/local/bin:/usr/bin\""),
            "unit body must include captured PATH; got:\n{body}"
        );
    }

    #[test]
    fn systemd_escape_handles_specials() {
        // Inside `Environment="..."`, only `"` and `\` are valid C-style
        // escapes per `man systemd.syntax(7)`. `$` and backtick have no
        // special meaning in `Environment=` values (no expansion happens),
        // so they must pass through verbatim — escaping them as `\$` /
        // `` \` `` produces an unrecognized C-escape that systemd-analyze
        // rejects as "Invalid syntax, ignoring", silently dropping the
        // entire PATH assignment.
        assert_eq!(systemd_double_quoted_escape("a/b/c"), "a/b/c");
        assert_eq!(systemd_double_quoted_escape("/foo$bar"), "/foo$bar");
        assert_eq!(systemd_double_quoted_escape("/back`tick"), "/back`tick");
        assert_eq!(systemd_double_quoted_escape("/quo\"ted"), "/quo\\\"ted");
        assert_eq!(systemd_double_quoted_escape("a\\b"), "a\\\\b");
    }

    #[test]
    fn unit_path_is_scoped_to_resolved_user_home() {
        assert_eq!(
            unit_path_in(Path::new("/home/laiye")),
            PathBuf::from("/home/laiye/.config/systemd/user/karma-team.service")
        );
    }

    #[test]
    fn systemctl_environment_overrides_inherited_root_session() {
        let user = SystemdUserContext {
            name: "laiye".to_owned(),
            home: PathBuf::from("/home/laiye"),
            runtime_dir: PathBuf::from("/run/user/1000"),
        };
        let mut command = Command::new("systemctl");
        command
            .env("HOME", "/root")
            .env("USER", "root")
            .env("LOGNAME", "root")
            .env("XDG_RUNTIME_DIR", "/run/user/0")
            .env("DBUS_SESSION_BUS_ADDRESS", "unix:path=/run/user/0/bus");

        apply_user_manager_environment(&mut command, &user);

        let env = command
            .as_std()
            .get_envs()
            .map(|(key, value)| {
                (
                    key.to_string_lossy().into_owned(),
                    value.map(|v| v.to_string_lossy().into_owned()),
                )
            })
            .collect::<std::collections::HashMap<_, _>>();
        assert_eq!(
            env.get("HOME").and_then(Option::as_deref),
            Some("/home/laiye")
        );
        assert_eq!(env.get("USER").and_then(Option::as_deref), Some("laiye"));
        assert_eq!(env.get("LOGNAME").and_then(Option::as_deref), Some("laiye"));
        assert_eq!(
            env.get("XDG_RUNTIME_DIR").and_then(Option::as_deref),
            Some("/run/user/1000")
        );
        assert_eq!(
            env.get("DBUS_SESSION_BUS_ADDRESS")
                .and_then(Option::as_deref),
            Some("unix:path=/run/user/1000/bus")
        );
    }
}
