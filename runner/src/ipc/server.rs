use anyhow::{Context, Result};
#[cfg(unix)]
use fs2::FileExt;
use std::collections::HashMap;
#[cfg(unix)]
use std::fs::{File, OpenOptions};
#[cfg(unix)]
use std::io::{Read, Seek, SeekFrom, Write};
#[cfg(unix)]
use std::os::unix::fs::{FileTypeExt, MetadataExt, OpenOptionsExt, PermissionsExt};
#[cfg(unix)]
use std::path::Path;
use std::path::PathBuf;
use std::sync::Arc;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufStream};
#[cfg(windows)]
use tokio::net::windows::named_pipe::{NamedPipeServer as IpcStream, ServerOptions};
#[cfg(unix)]
use tokio::net::{UnixListener, UnixStream as IpcStream};
use uuid::Uuid;

use super::chat;
use super::protocol::{Request, Response, RpcError, StatusSnapshot};
use crate::approval::router::DecisionSource;
use crate::daemon::runner_instance::RunnerInstance;

/// IPC server. Owns the multi-runner instance map and dispatches
/// per-runner requests to the right `RunnerInstance`'s state /
/// approvals / paths.
pub struct IpcServer {
    pub path: PathBuf,
    /// Connection-level state. Used for `daemon_info` (cloud_url,
    /// connected, uptime) and the Status subscribe tick stream. Any of
    /// the `RunnerInstance` state handles works as a tick driver since
    /// the supervisor ticks them all together; we keep a dedicated
    /// "primary" handle for that purpose.
    pub primary_state: crate::daemon::state::StateHandle,
    /// Host-level shutdown signal. This must not be the primary runner's
    /// per-instance state: once the daemon hosts multiple runners, notifying
    /// one instance does not wake the supervisor or machine-level services.
    pub daemon_state: crate::daemon::state::StateHandle,
    pub paths: crate::util::paths::Paths,
    /// All configured runners, keyed by runner_id. Resolved on every
    /// per-runner request via the `runner` selector. Wrapped in `Arc`
    /// so the supervisor can hand it to the IPC task without giving up
    /// ownership.
    pub instances: Arc<std::sync::RwLock<HashMap<Uuid, RunnerInstance>>>,
    pub(crate) spawn_ctx: crate::daemon::supervisor::RunnerSpawnCtx,
}

/// An IPC server whose process-lifetime singleton endpoint has already been
/// reserved.  Preparing the server is intentionally synchronous so daemon
/// startup can fail before any runner or cloud background task is spawned.
pub struct PreparedIpcServer {
    server: IpcServer,
    #[cfg(unix)]
    listener: UnixListener,
    #[cfg(unix)]
    _instance_lock: DaemonInstanceLock,
    #[cfg(windows)]
    first_pipe: NamedPipeServer,
}

impl IpcServer {
    pub fn prepare(self) -> Result<PreparedIpcServer> {
        #[cfg(unix)]
        {
            let (listener, instance_lock) =
                bind_singleton_unix_listener(&self.path, &self.paths.pid_path())?;
            Ok(PreparedIpcServer {
                server: self,
                listener,
                _instance_lock: instance_lock,
            })
        }

        #[cfg(windows)]
        {
            let pipe_name = crate::ipc::windows_pipe_name(&self.path);
            let first_pipe = ServerOptions::new()
                .first_pipe_instance(true)
                .create(&pipe_name)
                .with_context(|| format!("creating named pipe {pipe_name}"))?;
            Ok(PreparedIpcServer {
                server: self,
                first_pipe,
            })
        }
    }
}

impl PreparedIpcServer {
    #[cfg(unix)]
    pub async fn run(self) -> Result<()> {
        let PreparedIpcServer {
            server,
            listener,
            _instance_lock,
        } = self;
        let me = Arc::new(server);
        loop {
            let (stream, _) = listener.accept().await?;
            let server = me.clone();
            tokio::spawn(async move {
                if let Err(e) = server.handle_conn(stream).await {
                    tracing::warn!("ipc conn error: {e:#}");
                }
            });
        }
    }

    #[cfg(windows)]
    pub async fn run(self) -> Result<()> {
        let PreparedIpcServer {
            server: ipc,
            first_pipe: mut pipe_server,
        } = self;
        let pipe_name = crate::ipc::windows_pipe_name(&ipc.path);
        let me = Arc::new(ipc);
        loop {
            pipe_server
                .connect()
                .await
                .with_context(|| format!("accepting named-pipe client at {pipe_name}"))?;
            let connected = pipe_server;
            pipe_server = ServerOptions::new()
                .create(&pipe_name)
                .with_context(|| format!("creating next named pipe {pipe_name}"))?;
            let server = me.clone();
            tokio::spawn(async move {
                if let Err(e) = server.handle_conn(connected).await {
                    tracing::warn!("ipc conn error: {e:#}");
                }
            });
        }
    }
}

impl IpcServer {
    async fn handle_conn(&self, stream: IpcStream) -> Result<()> {
        let mut buf = BufStream::new(stream);
        let mut line = String::new();
        loop {
            line.clear();
            let n = buf.read_line(&mut line).await?;
            if n == 0 {
                break;
            }
            let trimmed = line.trim();
            if trimmed.is_empty() {
                continue;
            }
            let req: Request = match serde_json::from_str(trimmed) {
                Ok(r) => r,
                Err(e) => {
                    let err = Response::Error(RpcError {
                        code: 400,
                        message: format!("invalid request: {e}"),
                    });
                    write_line(&mut buf, &err).await?;
                    continue;
                }
            };
            match self.dispatch(req, &mut buf).await {
                Ok(resp) => write_line(&mut buf, &resp).await?,
                Err(e) => {
                    write_line(
                        &mut buf,
                        &Response::Error(RpcError {
                            code: 500,
                            message: e.to_string(),
                        }),
                    )
                    .await?;
                }
            }
        }
        Ok(())
    }

    async fn dispatch(&self, req: Request, buf: &mut BufStream<IpcStream>) -> Result<Response> {
        match req {
            Request::RunnerActivateLocal { runner } => {
                self.spawn_ctx.activate_configured(&runner).await?;
                Ok(Response::Ack)
            }
            Request::StatusGet => Ok(Response::Status(self.status_snapshot().await)),
            Request::StatusSubscribe => {
                let mut rx = self.primary_state.subscribe();
                let first = self.status_snapshot().await;
                write_line(buf, &Response::Status(first)).await?;
                while rx.changed().await.is_ok() {
                    let snap = self.status_snapshot().await;
                    write_line(buf, &Response::StatusDelta(snap)).await?;
                }
                Ok(Response::Ack)
            }
            Request::ConfigGet => {
                let cfg = crate::config::file::load_config(&self.paths)?;
                Ok(Response::Config(serde_json::to_value(&cfg)?))
            }
            Request::ConfigUpdate { patch, runner } => {
                // ``runner`` is reserved / informational — the patch is
                // deep-merged into the on-disk config (which already
                // contains every `[[runner]]` block) and the daemon's
                // primary state is reloaded. The selector is kept on
                // the wire so a future per-runner live-patch flow can
                // opt in without bumping IPC version again.
                let _ = runner;
                let cfg = apply_config_patch(&self.paths, patch)?;
                self.primary_state.set_config(cfg.clone()).await;
                Ok(Response::Ack)
            }
            Request::RunsList { limit, runner } => {
                // ``runner = None`` means "union across every configured
                // runner" — same shape as RunsGet / ApprovalsList. The
                // TUI relies on this so the Runs tab works before the
                // picker has been seeded by the Config refresh branch.
                let candidates: Vec<RunnerInstance> = match runner.as_deref() {
                    Some(name) => vec![self.resolve_runner(Some(name))?],
                    None => self.all_instances(),
                };
                let cap = limit.unwrap_or(100);
                let mut all = Vec::new();
                for inst in candidates {
                    let index = crate::history::index::RunsIndex::load(&inst.paths)?;
                    all.extend(index.recent(cap));
                }
                // Newest-first across the union, then trim to the cap so
                // a 100-row request returns the freshest 100 globally —
                // not 100 per runner.
                all.sort_by(|a, b| b.started_at.cmp(&a.started_at));
                all.truncate(cap);
                Ok(Response::Runs(all))
            }
            Request::RunsGet { run_id, runner } => {
                // When a runner is named, scope the lookup to that
                // instance's history. Without one, scan every instance
                // — run IDs are globally unique, so the first match is
                // authoritative.
                let candidates: Vec<RunnerInstance> = match runner.as_deref() {
                    Some(name) => vec![self.resolve_runner(Some(name))?],
                    None => self.all_instances(),
                };
                for inst in candidates {
                    let index = crate::history::index::RunsIndex::load(&inst.paths)?;
                    if let Some(summary) = index.runs.get(&run_id).cloned() {
                        let path = inst.paths.runs_dir().join(format!("{run_id}.jsonl"));
                        let events = if path.exists() {
                            crate::history::jsonl::read_all(&path)
                                .await?
                                .into_iter()
                                .map(|e| serde_json::to_value(e).unwrap_or(serde_json::Value::Null))
                                .collect()
                        } else {
                            Vec::new()
                        };
                        return Ok(Response::Run { summary, events });
                    }
                }
                anyhow::bail!("run not found");
            }
            Request::ApprovalsList { runner } => {
                let mut all = Vec::new();
                let candidates: Vec<RunnerInstance> = match runner.as_deref() {
                    Some(name) => vec![self.resolve_runner(Some(name))?],
                    None => self.all_instances(),
                };
                for inst in candidates {
                    all.extend(inst.approvals.list_pending().await);
                }
                Ok(Response::Approvals(all))
            }
            Request::ApprovalsDecide {
                approval_id,
                decision,
                runner,
            } => {
                // Approval IDs are globally unique. With a `runner`
                // selector we route to that instance directly; without
                // one, scan every instance and decide on whichever
                // owns the approval.
                let candidates: Vec<RunnerInstance> = match runner.as_deref() {
                    Some(name) => vec![self.resolve_runner(Some(name))?],
                    None => self.all_instances(),
                };
                for inst in candidates {
                    let resolved = inst
                        .approvals
                        .decide(&approval_id, decision, DecisionSource::Local)
                        .await;
                    if resolved.is_some() {
                        return Ok(Response::Ack);
                    }
                }
                anyhow::bail!("approval not found or already resolved");
            }
            Request::DoctorRun { runner } => {
                let report = crate::cli::doctor::execute(&self.paths, runner.as_deref()).await?;
                Ok(Response::Doctor(report))
            }
            Request::RunnerReconnect => {
                self.primary_state.force_reconnect();
                Ok(Response::Ack)
            }
            Request::RunnerDisconnect => {
                self.daemon_state.shutdown();
                Ok(Response::Ack)
            }
            Request::RunnerRemoveLocal { runner } => {
                // Enter the supervisor's serialized removal path so shutdown
                // can freeze this mutation ingress before taking its drain
                // snapshot. That path persists config first, then delivers the
                // same RemoveRunner frame consumed by RunnerLoop.
                let inst = self.resolve_runner(Some(&runner))?;
                self.spawn_ctx.remove_runner(inst.runner_id).await?;
                Ok(Response::Ack)
            }
            // Local chat (PDASHOSS01-159, IPC v4). The desktop host drives
            // the built-in engine directly over this socket — never through
            // the karma-team chat relay. Streamed `Response::Chat*` frames are
            // written onto `buf` mid-call (same pattern as StatusSubscribe);
            // the terminal frame is returned. See `super::chat`.
            Request::ChatWarm {
                chat_session_id,
                runner,
                cwd,
                model,
                mode,
                local_thread_id,
                local_session_id,
            } => {
                let inst = self.resolve_runner(runner.as_deref())?;
                let mut sink = chat::SocketSink { buf };
                chat::handle_warm(
                    &inst,
                    chat::WarmArgs {
                        chat_session_id,
                        cwd,
                        model,
                        mode,
                        local_thread_id,
                        local_session_id,
                    },
                    &mut sink,
                )
                .await
            }
            Request::ChatSend {
                chat_session_id,
                message_id,
                content,
                runner,
                cwd,
                model,
                mode,
                local_thread_id,
                local_session_id,
            } => {
                let inst = self.resolve_runner(runner.as_deref())?;
                let mut sink = chat::SocketSink { buf };
                chat::handle_send(
                    &inst,
                    chat::SendArgs {
                        chat_session_id,
                        message_id,
                        content,
                        cwd,
                        model,
                        mode,
                        local_thread_id,
                        local_session_id,
                    },
                    &mut sink,
                )
                .await
            }
            Request::ChatCancel {
                chat_session_id,
                runner,
                ..
            } => {
                let inst = self.resolve_runner(runner.as_deref())?;
                chat::handle_cancel(&inst, chat_session_id).await
            }
            Request::ChatClose {
                chat_session_id,
                runner,
                ..
            } => {
                let inst = self.resolve_runner(runner.as_deref())?;
                let mut sink = chat::SocketSink { buf };
                chat::handle_close(&inst, chat_session_id, &mut sink).await
            }
            Request::ChatDecide {
                local_approval_id,
                decision,
                runner,
                ..
            } => {
                let inst = self.resolve_runner(runner.as_deref())?;
                chat::handle_decide(&inst, &local_approval_id, decision).await
            }
        }
    }

    /// Resolve a runner-name selector to a concrete `RunnerInstance`.
    /// When `name` is `None` and the daemon has exactly one runner
    /// (the common single-runner install), return it. When `name` is
    /// `None` and there are multiple, refuse with a hint listing the
    /// configured names — the caller must disambiguate.
    fn resolve_runner(&self, name: Option<&str>) -> Result<RunnerInstance> {
        let instances = self.all_instances();
        let mut names: Vec<_> = instances.iter().map(|i| i.name.clone()).collect();
        names.sort();
        match name {
            Some(n) => instances.into_iter().find(|i| i.name == n).ok_or_else(|| {
                anyhow::anyhow!(
                    "no runner named {:?}; configured: [{}]",
                    n,
                    names.join(", ")
                )
            }),
            None => {
                if instances.len() == 1 {
                    Ok(instances.into_iter().next().unwrap())
                } else {
                    anyhow::bail!(
                        "this daemon hosts {} runners ([{}]); pass --runner <name>",
                        instances.len(),
                        names.join(", "),
                    )
                }
            }
        }
    }

    fn all_instances(&self) -> Vec<RunnerInstance> {
        self.instances
            .read()
            .expect("runner registry poisoned")
            .values()
            .cloned()
            .collect()
    }

    async fn status_snapshot(&self) -> StatusSnapshot {
        let daemon = self.primary_state.daemon_info().await;
        let instances = self.all_instances();
        let mut runners = Vec::with_capacity(instances.len());
        for inst in instances {
            runners.push(inst.state.runner_snapshot().await);
        }
        // Stable order so successive snapshots don't churn rendering.
        runners.sort_by(|a, b| a.name.cmp(&b.name));
        StatusSnapshot { daemon, runners }
    }
}

#[cfg(unix)]
#[derive(Debug)]
struct DaemonInstanceLock {
    _file: File,
}

/// Acquire the daemon's process-lifetime lock, remove only a proven-stale
/// socket, and bind the replacement listener while the lock is held.
///
/// The lock file is deliberately never unlinked. Removing it on shutdown can
/// race a successor that already opened and locked the old inode, allowing a
/// third process to create and lock a new inode at the same path.
#[cfg(unix)]
fn bind_singleton_unix_listener(
    socket_path: &Path,
    lock_path: &Path,
) -> Result<(UnixListener, DaemonInstanceLock)> {
    if let Some(parent) = socket_path.parent() {
        std::fs::create_dir_all(parent).with_context(|| format!("creating {parent:?}"))?;
        // Best-effort tightening is retained here for direct test/embedded
        // callers; normal daemon startup already applies the stronger Paths
        // ownership and permission checks before reaching IPC startup.
        let mut permissions = std::fs::metadata(parent)?.permissions();
        permissions.set_mode(0o700);
        let _ = std::fs::set_permissions(parent, permissions);
    }

    let mut lock_file = open_instance_lock(lock_path)?;
    match lock_file.try_lock_exclusive() {
        Ok(()) => {}
        Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
            let mut holder = String::new();
            let _ = lock_file.read_to_string(&mut holder);
            let holder = holder.trim();
            if holder.is_empty() {
                anyhow::bail!("another karma-team daemon is already running");
            }
            anyhow::bail!("another karma-team daemon is already running (pid {holder})");
        }
        Err(error) => {
            return Err(error).with_context(|| format!("locking daemon instance {lock_path:?}"));
        }
    }

    remove_socket_if_stale(socket_path)?;

    let listener = UnixListener::bind(socket_path)
        .with_context(|| format!("binding unix socket {socket_path:?}"))?;

    let mut permissions = std::fs::metadata(socket_path)?.permissions();
    permissions.set_mode(0o600);
    std::fs::set_permissions(socket_path, permissions)?;

    lock_file.set_len(0)?;
    lock_file.seek(SeekFrom::Start(0))?;
    writeln!(lock_file, "{}", std::process::id())?;
    lock_file.sync_data()?;

    Ok((listener, DaemonInstanceLock { _file: lock_file }))
}

#[cfg(unix)]
fn open_instance_lock(path: &Path) -> Result<File> {
    let mut options = OpenOptions::new();
    options
        .create(true)
        .read(true)
        .write(true)
        .mode(0o600)
        .custom_flags(nix::libc::O_CLOEXEC | nix::libc::O_NOFOLLOW);
    let file = options
        .open(path)
        .with_context(|| format!("opening daemon instance lock {path:?}"))?;
    let metadata = file
        .metadata()
        .with_context(|| format!("inspecting daemon instance lock {path:?}"))?;
    let euid = nix::unistd::Uid::effective().as_raw();
    if !metadata.is_file() || metadata.uid() != euid {
        anyhow::bail!("daemon instance lock {path:?} must be a regular file owned by uid {euid}");
    }
    let mut permissions = metadata.permissions();
    permissions.set_mode(0o600);
    file.set_permissions(permissions)
        .with_context(|| format!("securing daemon instance lock {path:?}"))?;
    Ok(file)
}

#[cfg(unix)]
fn remove_socket_if_stale(path: &Path) -> Result<()> {
    let metadata = match std::fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(error).with_context(|| format!("inspecting IPC socket {path:?}")),
    };
    if !metadata.file_type().is_socket() {
        anyhow::bail!("refusing to replace non-socket IPC path {path:?}");
    }

    match std::os::unix::net::UnixStream::connect(path) {
        Ok(_) => anyhow::bail!("a karma-team daemon is already listening at {path:?}"),
        Err(error) if error.kind() == std::io::ErrorKind::ConnectionRefused => {
            std::fs::remove_file(path)
                .with_context(|| format!("removing stale IPC socket {path:?}"))?;
            Ok(())
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error).with_context(|| {
            format!("probing existing IPC socket {path:?}; refusing to unlink it")
        }),
    }
}

async fn write_line(buf: &mut BufStream<IpcStream>, resp: &Response) -> Result<()> {
    let mut line = serde_json::to_vec(resp)?;
    line.push(b'\n');
    buf.write_all(&line).await?;
    buf.flush().await?;
    Ok(())
}

fn merge_json(cfg: &mut crate::config::schema::Config, patch: serde_json::Value) -> Result<()> {
    let mut base = serde_json::to_value(&*cfg)?;
    deep_merge(&mut base, patch);
    *cfg = serde_json::from_value(base)?;
    Ok(())
}

/// Apply a partial IPC config update as one locked read-modify-write. Project
/// reconciliation and runner add/remove use the same host-wide lock, so a TUI
/// setting change cannot overwrite a runner block created or removed between
/// an unlocked load and write.
fn apply_config_patch(
    paths: &crate::util::paths::Paths,
    patch: serde_json::Value,
) -> Result<crate::config::schema::Config> {
    crate::config::file::mutate_config(paths, move |cfg| merge_json(cfg, patch))
}

fn deep_merge(base: &mut serde_json::Value, patch: serde_json::Value) {
    match (base, patch) {
        (serde_json::Value::Object(b), serde_json::Value::Object(p)) => {
            for (k, v) in p {
                deep_merge(b.entry(k).or_insert(serde_json::Value::Null), v);
            }
        }
        (b, p) => *b = p,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::schema::{Config, DaemonConfig, RunnerConfig};
    use crate::util::paths::Paths;

    #[cfg(unix)]
    #[tokio::test]
    async fn second_daemon_fails_without_unlinking_the_active_socket() {
        let tmp = tempfile::tempdir().unwrap();
        let runtime = tmp.path().join("runtime");
        let socket = runtime.join("karma-team.sock");
        let lock = runtime.join("pid");

        let (_listener, _instance_lock) = bind_singleton_unix_listener(&socket, &lock).unwrap();
        let error = bind_singleton_unix_listener(&socket, &lock).unwrap_err();

        assert!(error.to_string().contains("already running"), "{error:#}");
        assert!(std::os::unix::net::UnixStream::connect(&socket).is_ok());
    }

    #[cfg(unix)]
    #[test]
    fn active_legacy_socket_without_lock_is_never_unlinked() {
        let tmp = tempfile::tempdir().unwrap();
        let runtime = tmp.path().join("runtime");
        std::fs::create_dir_all(&runtime).unwrap();
        let socket = runtime.join("karma-team.sock");
        let lock = runtime.join("pid");
        let _legacy_listener = std::os::unix::net::UnixListener::bind(&socket).unwrap();

        let error = bind_singleton_unix_listener(&socket, &lock).unwrap_err();

        assert!(error.to_string().contains("already listening"), "{error:#}");
        assert!(socket.exists());
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn stale_socket_is_replaced_after_the_old_lock_is_released() {
        let tmp = tempfile::tempdir().unwrap();
        let runtime = tmp.path().join("runtime");
        std::fs::create_dir_all(&runtime).unwrap();
        let socket = runtime.join("karma-team.sock");
        let lock = runtime.join("pid");
        let stale_listener = std::os::unix::net::UnixListener::bind(&socket).unwrap();
        drop(stale_listener);

        let (_listener, _instance_lock) = bind_singleton_unix_listener(&socket, &lock).unwrap();

        assert!(std::os::unix::net::UnixStream::connect(&socket).is_ok());
        assert_eq!(std::fs::metadata(&socket).unwrap().mode() & 0o777, 0o600);
        assert_eq!(
            std::fs::read_to_string(lock).unwrap().trim(),
            std::process::id().to_string()
        );
    }

    #[test]
    fn config_patch_preserves_runner_blocks_under_host_lock() {
        let tmp = tempfile::tempdir().unwrap();
        let paths = Paths {
            config_dir: tmp.path().join("config"),
            data_dir: tmp.path().join("data"),
            runtime_dir: tmp.path().join("runtime"),
        };
        let runner_id = Uuid::new_v4();
        let runner: RunnerConfig = serde_json::from_value(serde_json::json!({
            "name": "auto-web",
            "runner_id": runner_id,
            "workspace_slug": "acme",
            "project_slug": "WEB",
            "workspace": { "working_dir": tmp.path().join("work-web") },
        }))
        .unwrap();
        let config = Config {
            version: 2,
            daemon: DaemonConfig {
                cloud_url: "https://cloud.example".into(),
                ..DaemonConfig::default()
            },
            runners: vec![runner],
            cli: None,
        };
        crate::config::file::write_config(&paths, &config).unwrap();

        let patched = apply_config_patch(
            &paths,
            serde_json::json!({"daemon": {"log_level": "debug"}}),
        )
        .unwrap();

        assert_eq!(patched.daemon.log_level, "debug");
        assert_eq!(patched.daemon.cloud_url, "https://cloud.example");
        assert_eq!(patched.runners.len(), 1);
        assert_eq!(patched.runners[0].runner_id, runner_id);
    }
}
