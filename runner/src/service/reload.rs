//! Restart-and-verify helper used after any `config.toml` mutation.
//!
//! Pattern: write file → kick the service → wait for the daemon to prove
//! it's healthy. "Healthy" means two things end to end:
//!
//! 1. IPC socket is reachable and serves `StatusGet`. This tells us the
//!    daemon started, loaded its config + credentials, and reached the
//!    serve-IPC step. If the config change was malformed (bad TOML, bad
//!    credentials, unknown agent), startup fails before this point.
//!
//! 2. The status snapshot reports `connected == true` — i.e. the WebSocket
//!    to the cloud handshook. If this never flips, the config is syntactically
//!    fine but semantically broken (wrong URL, invalid runner_secret, etc.).
//!
//! Callers (TUI Config save, CLI `karma-team configure --<flag>`) turn the
//! `ReloadOutcome` into either success UI or a loud error so the user
//! immediately knows their edit broke the daemon, instead of discovering
//! it later via a silent background failure.
//!
//! Time budget: 5 s for IPC + an additional 30 s for cloud-connected
//! (35 s total worst case). The first stage is usually a few hundred ms on
//! Linux/macOS; the cloud handshake is what typically eats seconds after
//! package swaps, service-manager restarts, DNS, or cloud deploys.

use std::time::{Duration, Instant};

use crate::ipc::client::Client;
use crate::ipc::protocol::{Request, Response};
use crate::util::paths::Paths;

const IPC_TIMEOUT: Duration = Duration::from_secs(5);
const CLOUD_TIMEOUT: Duration = Duration::from_secs(30);
const POLL_INTERVAL: Duration = Duration::from_millis(250);
const PROGRESS_INTERVAL: Duration = Duration::from_secs(5);

/// Outcome of a reload attempt. `ok = true` means the daemon is up and
/// talking to the cloud; a `false` value carries a message explaining which
/// stage failed plus any service-manager output we could capture.
#[derive(Debug, Clone)]
pub struct ReloadOutcome {
    pub ok: bool,
    /// Short, single-line summary suitable for a status banner.
    pub summary: String,
    /// Error details, or per-runner warnings after a partially successful
    /// explicit restart. Safe to show in a popup or stderr.
    pub detail: Option<String>,
    /// Last-known service state string (`active`, `inactive`, `failed`,
    /// or the raw `launchctl list` row). Handy for the TUI even on success.
    pub service_state: String,
}

/// Write-then-restart flow. Starts the service if inactive, restarts if
/// active, then polls IPC and the cloud-connected flag. Always returns
/// something you can render — failures carry a `detail` string, successes
/// carry a summary of "<name> is connected".
pub async fn restart_and_verify(paths: &Paths) -> ReloadOutcome {
    restart_and_verify_with_progress(paths, |_| {}).await
}

pub async fn restart_and_verify_with_progress<F>(paths: &Paths, progress: F) -> ReloadOutcome
where
    F: FnMut(String),
{
    restart_with_cloud_check(paths, progress, CloudCheck::Primary).await
}

/// Explicit `restart` checks each runner independently. Other callers retain
/// their existing config-change verification policy.
pub async fn restart_and_verify_runners_with_progress<F>(
    paths: &Paths,
    progress: F,
) -> ReloadOutcome
where
    F: FnMut(String),
{
    restart_with_cloud_check(paths, progress, CloudCheck::IndividualRunners).await
}

enum CloudCheck {
    Primary,
    IndividualRunners,
}

async fn restart_with_cloud_check<F>(
    paths: &Paths,
    mut progress: F,
    cloud_check: CloudCheck,
) -> ReloadOutcome
where
    F: FnMut(String),
{
    let svc = crate::service::detect();

    // Self-heal: rewrite the service unit if it's gone (manual cleanup, a
    // `karma-team uninstall`/`remove`, or never written for the current install
    // location). Without this, `karma-team restart` on a unit-less machine
    // hard-fails — `karma-team update` only swaps the binary and doesn't touch
    // the unit, and operators should not need to know that `karma-team install`
    // is the magic recovery command.
    //
    // All backends, not just launchd. This started as a launchd fix because
    // that's where it was first reported (launchctl bootstrap returning a
    // cryptic EIO), on the theory that systemd's clearer "Unit file does not
    // exist" left the operator able to recover unaided. It doesn't: a
    // legible error is still a hard failure, it names what's missing but not
    // the fix, and the fix isn't guessable — `karma-team install` writes the unit
    // but then declines to start it when no runner is enrolled. `launchd::start`
    // keeps its precondition check as a last line of defense for direct
    // `karma-team start` callers that don't pass through this self-heal.
    match svc.rewrite_unit_if_missing(paths).await {
        Ok(true) => progress("refreshed missing or outdated service unit".into()),
        Ok(false) => {}
        Err(e) => tracing::warn!("pre-restart service unit check failed: {e:#}"),
    }

    progress("starting runner service".into());
    // `enable_and_start` is idempotent *and* on systemd uses `restart`, so
    // this covers both "first start" and "reload after config change."
    if let Err(e) = svc.enable_and_start().await {
        let state = svc.status().await.unwrap_or_else(|_| "unknown".into());
        return ReloadOutcome {
            ok: false,
            summary: "failed to start runner service".into(),
            detail: Some(format!(
                "service manager rejected start/restart: {e:#}\n\
                 current state: {state}"
            )),
            service_state: state,
        };
    }

    // Stage 1: wait for IPC. A successful `StatusGet` means the daemon got
    // past config load, credential load, and agent init.
    progress(format!(
        "waiting up to {}s for daemon IPC",
        IPC_TIMEOUT.as_secs()
    ));
    let stage1 = wait_for_ipc(paths, IPC_TIMEOUT, &mut progress).await;
    let snapshot = match stage1 {
        Some(s) => s,
        None => {
            let state = svc.status().await.unwrap_or_else(|_| "unknown".into());
            // Ask the backend whether the daemon died with a recognizable
            // signature (SIGKILL = AMFI on macOS, SIGABRT = panic, etc.).
            // Surface that ahead of the raw launchctl/systemctl dump so the
            // operator's first read points them at the actual cause rather
            // than "the IPC didn't answer."
            let diagnosis = svc.diagnose_recent_exit().await;
            let journal = capture_service_detail(&svc).await;
            let detail = match diagnosis {
                Some(diag) => format!(
                    "Daemon did not answer IPC within {}s after restart.\n\n\
                     Diagnosis: {diag}\n\n\
                     Service state: {state}\n\n{journal}",
                    IPC_TIMEOUT.as_secs()
                ),
                None => format!(
                    "Daemon did not answer IPC within {}s after restart.\n\
                     Service state: {state}\n\n{journal}",
                    IPC_TIMEOUT.as_secs()
                ),
            };
            return ReloadOutcome {
                ok: false,
                summary: "runner failed to come up".into(),
                detail: Some(detail),
                service_state: state,
            };
        }
    };

    if matches!(cloud_check, CloudCheck::IndividualRunners) {
        return verify_runner_connections(paths, snapshot, &svc, &mut progress).await;
    }

    // Stage 2: wait for cloud-connected. Already connected? Return now.
    if snapshot.daemon.connected {
        let state = svc.status().await.unwrap_or_else(|_| "unknown".into());
        return ReloadOutcome {
            ok: true,
            summary: format!(
                "{} — connected to {}",
                summarize_runners(&snapshot),
                snapshot.daemon.cloud_url
            ),
            detail: None,
            service_state: state,
        };
    }
    progress(format!(
        "waiting up to {}s for cloud connection",
        CLOUD_TIMEOUT.as_secs()
    ));
    match wait_for_cloud_connected(paths, CLOUD_TIMEOUT, &mut progress).await {
        Some(final_snap) => {
            let state = svc.status().await.unwrap_or_else(|_| "unknown".into());
            ReloadOutcome {
                ok: true,
                summary: format!(
                    "{} — connected to {}",
                    summarize_runners(&final_snap),
                    final_snap.daemon.cloud_url
                ),
                detail: None,
                service_state: state,
            }
        }
        None => {
            let state = svc.status().await.unwrap_or_else(|_| "unknown".into());
            ReloadOutcome {
                ok: false,
                summary: "daemon up but not connected to cloud".into(),
                detail: Some(format!(
                    "Runner started but did not reach the cloud within {}s.\n\
                     Common causes: wrong cloud_url, invalid runner_secret \
                     (try re-registering with `karma-team configure --url ... --token ...`), \
                     or a network reachability problem.\n\
                     Service state: {state}",
                    CLOUD_TIMEOUT.as_secs()
                )),
                service_state: state,
            }
        }
    }
}

async fn verify_runner_connections(
    paths: &Paths,
    initial: crate::ipc::protocol::StatusSnapshot,
    svc: &crate::service::Service,
    progress: &mut impl FnMut(String),
) -> ReloadOutcome {
    if !runner_checks_complete(&initial) {
        progress(format!(
            "waiting up to {}s for individual runner connections",
            CLOUD_TIMEOUT.as_secs()
        ));
    }
    match wait_for_runner_connections(paths, initial, CLOUD_TIMEOUT, progress).await {
        Some(snapshot) => {
            let state = svc.status().await.unwrap_or_else(|_| "unknown".into());
            runner_connection_outcome(&snapshot, state)
        }
        None => {
            let state = svc.status().await.unwrap_or_else(|_| "unknown".into());
            ReloadOutcome {
                ok: false,
                summary: "daemon IPC became unavailable during runner connection checks".into(),
                detail: Some(format!(
                    "Could not obtain a current runner status.\nService state: {state}"
                )),
                service_state: state,
            }
        }
    }
}

fn runner_checks_complete(snapshot: &crate::ipc::protocol::StatusSnapshot) -> bool {
    if snapshot.runners.is_empty() {
        return snapshot.daemon.connected;
    }
    // Give every runner time for its first connection attempt. Once a healthy
    // runner exists and its siblings have a result, report failures without
    // waiting for the failing runners' ongoing background retries.
    snapshot.runners.iter().any(|runner| runner.connected)
        && snapshot
            .runners
            .iter()
            .all(|runner| runner.connected || runner.consecutive_bootstrap_failures > 0)
}

async fn wait_for_runner_connections(
    paths: &Paths,
    initial: crate::ipc::protocol::StatusSnapshot,
    timeout: Duration,
    progress: &mut impl FnMut(String),
) -> Option<crate::ipc::protocol::StatusSnapshot> {
    let start = Instant::now();
    let deadline = start + timeout;
    let mut next_progress = start + PROGRESS_INTERVAL;
    let mut latest = Some(initial);
    loop {
        if latest.as_ref().is_some_and(runner_checks_complete) || Instant::now() >= deadline {
            return latest;
        }
        maybe_emit_wait_progress(
            "individual runner connections",
            start,
            timeout,
            &mut next_progress,
            progress,
        );
        tokio::time::sleep(POLL_INTERVAL.min(deadline.saturating_duration_since(Instant::now())))
            .await;
        if Instant::now() >= deadline {
            return latest;
        }
        let remaining = deadline.saturating_duration_since(Instant::now());
        latest = tokio::time::timeout(remaining, async {
            let mut client = Client::connect(paths.ipc_socket_path()).await.ok()?;
            match client.call(Request::StatusGet).await.ok()? {
                Response::Status(snapshot) => Some(snapshot),
                _ => None,
            }
        })
        .await
        .ok()
        .flatten();
    }
}

fn runner_connection_outcome(
    snapshot: &crate::ipc::protocol::StatusSnapshot,
    service_state: String,
) -> ReloadOutcome {
    let total = snapshot.runners.len();
    let connected = snapshot
        .runners
        .iter()
        .filter(|runner| runner.connected)
        .count();
    if total == 0 {
        return ReloadOutcome {
            ok: snapshot.daemon.connected,
            summary: "no runners reported by the daemon".into(),
            detail: (!snapshot.daemon.connected).then(|| {
                "The daemon is running but its cloud connection could not be confirmed.".into()
            }),
            service_state,
        };
    }
    let failures: Vec<_> = snapshot
        .runners
        .iter()
        .filter(|runner| !runner.connected)
        .map(|runner| {
            let project = runner
                .project_slug
                .as_deref()
                .map(|project| format!(", project {project:?}"))
                .unwrap_or_default();
            let reason = if runner.consecutive_bootstrap_failures > 0 {
                format!(
                    "cloud connection failed ({} failed bootstrap attempts)",
                    runner.consecutive_bootstrap_failures
                )
            } else {
                "cloud connection was not established within the check timeout".into()
            };
            format!(
                "Runner {:?} ({}{project}): {reason}.",
                runner.name, runner.runner_id
            )
        })
        .collect();
    let detail = if failures.is_empty() {
        None
    } else {
        Some(format!(
            "{}\nSee daemon logs for the server or network error for each runner ID.\n\
             Runner connection checks do not stop or remove any runner.",
            failures.join("\n")
        ))
    };
    ReloadOutcome {
        ok: connected > 0,
        summary: format!(
            "{connected}/{total} runners connected to {}",
            snapshot.daemon.cloud_url
        ),
        detail,
        service_state,
    }
}

async fn wait_for_ipc(
    paths: &Paths,
    timeout: Duration,
    progress: &mut impl FnMut(String),
) -> Option<crate::ipc::protocol::StatusSnapshot> {
    let start = Instant::now();
    let deadline = start + timeout;
    let mut next_progress = start + PROGRESS_INTERVAL;
    loop {
        if let Ok(mut c) = Client::connect(paths.ipc_socket_path()).await
            && let Ok(Response::Status(s)) = c.call(Request::StatusGet).await
        {
            return Some(s);
        }
        if Instant::now() >= deadline {
            return None;
        }
        maybe_emit_wait_progress("daemon IPC", start, timeout, &mut next_progress, progress);
        tokio::time::sleep(POLL_INTERVAL).await;
    }
}

async fn wait_for_cloud_connected(
    paths: &Paths,
    timeout: Duration,
    progress: &mut impl FnMut(String),
) -> Option<crate::ipc::protocol::StatusSnapshot> {
    let start = Instant::now();
    let deadline = start + timeout;
    let mut next_progress = start + PROGRESS_INTERVAL;
    loop {
        if let Ok(mut c) = Client::connect(paths.ipc_socket_path()).await
            && let Ok(Response::Status(s)) = c.call(Request::StatusGet).await
            && s.daemon.connected
        {
            return Some(s);
        }
        if Instant::now() >= deadline {
            return None;
        }
        maybe_emit_wait_progress(
            "cloud connection",
            start,
            timeout,
            &mut next_progress,
            progress,
        );
        tokio::time::sleep(POLL_INTERVAL).await;
    }
}

fn maybe_emit_wait_progress(
    label: &str,
    start: Instant,
    timeout: Duration,
    next_progress: &mut Instant,
    progress: &mut impl FnMut(String),
) {
    let now = Instant::now();
    if now < *next_progress {
        return;
    }
    let elapsed = now.saturating_duration_since(start).as_secs();
    progress(format!(
        "still waiting for {label} ({elapsed}s elapsed, {}s max)",
        timeout.as_secs()
    ));
    *next_progress = now + PROGRESS_INTERVAL;
}

/// Best-effort collection of service-manager output to show in the error
/// popup. We ignore failures — if this can't run, the outer error already
/// said "daemon didn't come up," and we don't want to obscure it.
async fn capture_service_detail(svc: &crate::service::Service) -> String {
    match svc.status().await {
        Ok(s) => format!("service status:\n{s}"),
        Err(e) => format!("(could not read service status: {e})"),
    }
}

/// Render a one-liner of configured-runner names for `karma-team install` /
/// reload outcomes. Single-runner returns the bare name; multi-runner
/// joins with `, ` and reports the count.
fn summarize_runners(snap: &crate::ipc::protocol::StatusSnapshot) -> String {
    match snap.runners.len() {
        0 => "(no runners)".to_string(),
        1 => snap.runners[0].name.clone(),
        n => format!(
            "{n} runners ({})",
            snap.runners
                .iter()
                .map(|r| r.name.as_str())
                .collect::<Vec<_>>()
                .join(", "),
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ipc::protocol::StatusSnapshot;
    use serde_json::json;

    fn snapshot(primary_connected: bool, runners: &[(&str, bool, u32)]) -> StatusSnapshot {
        serde_json::from_value(json!({
            "daemon": {
                "cloud_url": "https://cloud.example.test",
                "connected": primary_connected,
                "uptime_secs": 1,
            },
            "runners": runners.iter().enumerate().map(|(i, (name, connected, failures))| {
                json!({
                    "runner_id": uuid::Uuid::from_u128(i as u128 + 1),
                    "name": name,
                    "project_slug": "PROJECT",
                    "status": "idle",
                    "connected": connected,
                    "current_run": null,
                    "approvals_pending": 0,
                    "last_heartbeat": null,
                    "consecutive_bootstrap_failures": failures,
                })
            }).collect::<Vec<_>>(),
        }))
        .unwrap()
    }

    #[test]
    fn failed_primary_does_not_fail_healthy_sibling_restart() {
        let snap = snapshot(false, &[("stale-primary", false, 6), ("healthy", true, 0)]);
        assert!(runner_checks_complete(&snap));
        let outcome = runner_connection_outcome(&snap, "active".into());
        assert!(outcome.ok);
        assert!(outcome.summary.starts_with("1/2 runners connected"));
        let warning = outcome.detail.unwrap();
        assert!(warning.contains("stale-primary"));
        assert!(warning.contains(&snap.runners[0].runner_id.to_string()));
        assert!(warning.contains("PROJECT"));
        assert!(warning.contains("6 failed bootstrap attempts"));
        assert!(!warning.contains("healthy"));
    }

    #[test]
    fn connected_primary_does_not_hide_failed_sibling() {
        let snap = snapshot(true, &[("healthy-primary", true, 0), ("failed", false, 2)]);
        let outcome = runner_connection_outcome(&snap, "active".into());
        assert!(outcome.ok);
        let warning = outcome.detail.unwrap();
        assert!(warning.contains("failed"));
        assert!(!warning.contains("healthy-primary"));
    }

    #[test]
    fn all_runners_connected_succeeds_even_with_stale_primary_flag() {
        let snap = snapshot(false, &[("one", true, 0), ("two", true, 0)]);
        assert!(runner_checks_complete(&snap));
        let outcome = runner_connection_outcome(&snap, "active".into());
        assert!(outcome.ok);
        assert!(outcome.summary.starts_with("2/2 runners connected"));
        assert!(outcome.detail.is_none());
    }

    #[test]
    fn all_disconnected_remains_failure_and_lists_every_runner() {
        let snap = snapshot(true, &[("one", false, 3), ("two", false, 0)]);
        assert!(!runner_checks_complete(&snap));
        let outcome = runner_connection_outcome(&snap, "active".into());
        assert!(!outcome.ok);
        assert!(outcome.summary.starts_with("0/2 runners connected"));
        let errors = outcome.detail.unwrap();
        assert!(errors.contains("one"));
        assert!(errors.contains("two"));
        assert!(errors.contains("within the check timeout"));
    }

    #[test]
    fn no_runners_preserves_daemon_connection_check() {
        for connected in [false, true] {
            let snap = snapshot(connected, &[]);
            assert_eq!(runner_checks_complete(&snap), connected);
            assert_eq!(
                runner_connection_outcome(&snap, "active".into()).ok,
                connected
            );
        }
    }

    #[cfg(unix)]
    fn test_paths(root: &std::path::Path) -> Paths {
        Paths {
            config_dir: root.join("config"),
            data_dir: root.join("data"),
            runtime_dir: root.to_path_buf(),
        }
    }

    #[cfg(unix)]
    fn serve_snapshots(
        paths: &Paths,
        snapshots: Vec<StatusSnapshot>,
    ) -> tokio::task::JoinHandle<()> {
        use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
        use tokio::net::UnixListener;

        let listener = UnixListener::bind(paths.ipc_socket_path()).unwrap();
        tokio::spawn(async move {
            let mut index = 0;
            loop {
                let (stream, _) = listener.accept().await.unwrap();
                let mut stream = BufReader::new(stream);
                let mut request = String::new();
                stream.read_line(&mut request).await.unwrap();
                // Verification only reads state; it must never send runner
                // removal, reconnect, or config mutation requests.
                assert!(matches!(
                    serde_json::from_str::<Request>(&request).unwrap(),
                    Request::StatusGet
                ));
                let reply = &snapshots[index.min(snapshots.len() - 1)];
                index += 1;
                let mut response = serde_json::to_vec(&Response::Status(reply.clone())).unwrap();
                response.push(b'\n');
                stream.get_mut().write_all(&response).await.unwrap();
            }
        })
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn waits_for_pending_runner_and_accepts_recovery() {
        let root = tempfile::tempdir().unwrap();
        let paths = test_paths(root.path());
        let pending = snapshot(false, &[("slow", false, 0), ("healthy", true, 0)]);
        let recovered = snapshot(false, &[("slow", true, 0), ("healthy", true, 0)]);
        let server = serve_snapshots(&paths, vec![pending.clone(), recovered]);
        let result =
            wait_for_runner_connections(&paths, pending, Duration::from_secs(2), &mut |_| {})
                .await
                .unwrap();
        assert!(result.runners.iter().all(|runner| runner.connected));
        assert!(
            runner_connection_outcome(&result, "active".into())
                .detail
                .is_none()
        );
        server.abort();
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn timeout_reports_latest_individual_results() {
        let root = tempfile::tempdir().unwrap();
        let paths = test_paths(root.path());
        let initial = snapshot(false, &[("one", false, 0), ("two", false, 0)]);
        let failing = snapshot(false, &[("one", false, 4), ("two", false, 2)]);
        let server = serve_snapshots(&paths, vec![failing]);
        let result =
            wait_for_runner_connections(&paths, initial, Duration::from_millis(650), &mut |_| {})
                .await
                .unwrap();
        let outcome = runner_connection_outcome(&result, "active".into());
        assert!(!outcome.ok);
        assert!(
            outcome
                .detail
                .unwrap()
                .contains("4 failed bootstrap attempts")
        );
        server.abort();
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn lost_ipc_does_not_report_stale_partial_success() {
        let root = tempfile::tempdir().unwrap();
        let paths = test_paths(root.path());
        let initial = snapshot(true, &[("one", true, 0), ("pending", false, 0)]);
        let result =
            wait_for_runner_connections(&paths, initial, Duration::from_millis(650), &mut |_| {})
                .await;
        assert!(result.is_none());
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn unresponsive_ipc_is_bounded_by_check_timeout() {
        use tokio::net::UnixListener;

        let root = tempfile::tempdir().unwrap();
        let paths = test_paths(root.path());
        let listener = UnixListener::bind(paths.ipc_socket_path()).unwrap();
        let server = tokio::spawn(async move {
            let (_stream, _) = listener.accept().await.unwrap();
            std::future::pending::<()>().await;
        });
        let initial = snapshot(false, &[("pending", false, 0)]);
        let result = tokio::time::timeout(
            Duration::from_secs(2),
            wait_for_runner_connections(&paths, initial, Duration::from_millis(650), &mut |_| {}),
        )
        .await
        .expect("runner check must not hang on an unresponsive IPC connection");
        assert!(result.is_none());
        server.abort();
    }
}
