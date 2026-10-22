use anyhow::Result;
use chrono::{DateTime, Utc};
use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};
use tokio::sync::{RwLock, mpsc, oneshot, watch};

use crate::agent::{AgentBridge, AgentCursor, BridgeEvent, RunPayload};
use crate::approval::policy::Policy;
use crate::approval::router::{ApprovalRecord, ApprovalRouter, ApprovalStatus, DecisionSource};
use crate::cloud::http::{
    AckEntry, AttachBody, CredentialsHandle, HttpLoop, InboundEnvelope, MachineClient, PollStatus,
    RunnerCloudClient, RunnerCredentials, SharedHttpTransport,
};
use crate::cloud::protocol::{
    ClientMsg, FailureReason, RunnerStatus, ServerMsg, TokenUsage as WireTokenUsage, WIRE_VERSION,
    WorkspaceState,
};
use crate::config::schema::{
    AgentKind, Config, Credentials, DaemonConfig, RunnerConfig, canonical_for_compare,
};
use crate::daemon::drain_journal;
use crate::daemon::run_event_mirror::RunEventMirror;
use crate::daemon::runner_instance::{RunnerInstance, RunnerTeardownOutcome};
use crate::daemon::runner_out::RunnerOut;
use crate::daemon::state::StateHandle;
use crate::history::index::{RunSummary, RunsIndex};
use crate::history::jsonl::{HistoryEntry, HistoryWriter};
use crate::ipc::protocol::{CurrentRunSummary, version_lt};
use crate::ipc::server::IpcServer;
use crate::util::paths::{Paths, RunnerPaths};

pub struct Supervisor {
    pub config: Config,
    pub creds: Credentials,
    pub paths: Paths,
    pub opts: crate::daemon::Options,
    pub state: StateHandle,
    pub approvals: ApprovalRouter,
}

type HelloRunner = (RunnerOut, StateHandle, Option<String>);
type HelloRunnerMap = HashMap<uuid::Uuid, HelloRunner>;

impl Supervisor {
    pub fn new(
        config: Config,
        creds: Credentials,
        paths: Paths,
        opts: crate::daemon::Options,
    ) -> Self {
        let state = StateHandle::new(config.clone());
        let approvals = ApprovalRouter::new();
        Self {
            config,
            creds,
            paths,
            opts,
            state,
            approvals,
        }
    }

    pub async fn run(self) -> Result<()> {
        self.paths.ensure()?;
        // Re-run config validation here so any future entry path that
        // bypasses `cli::run` (tests, IPC reload, embedded callers)
        // still gets the duplicate-name / nested-working-dir / cap
        // checks before we spawn N RunnerInstances. Cheap (O(n²) over
        // ≤50 runners) and idempotent with the cli::run gate.
        self.config
            .validate()
            .map_err(|e| anyhow::anyhow!("invalid config: {e}"))?;
        let Supervisor {
            config,
            creds: _creds,
            paths,
            opts,
            state,
            approvals: _supervisor_approvals,
        } = self;
        // Daemon-level state has no single runner_id any more — runners
        // come and go under one connection. Per-instance state still
        // holds its own runner_id (set in RunnerInstance::new).

        let transport = if opts.offline {
            None
        } else {
            Some(SharedHttpTransport::new(config.daemon.cloud_url.clone())?)
        };
        // Keep configuration membership separate from the active registry.
        // A configured runner can be temporarily inactive while its workspace
        // credentials are being discovered, but its journal entries must not
        // be mistaken for entries belonging to a deleted runner.
        let configured_runner_ids: Arc<HashSet<uuid::Uuid>> = Arc::new(
            config
                .runners
                .iter()
                .map(|runner| runner.runner_id)
                .collect(),
        );

        let shared_machine_token = config
            .cli
            .as_ref()
            .and_then(|cli| cli.token.as_deref())
            .filter(|token| token.starts_with("mt_"))
            .map(str::to_string);

        let mut instances: Vec<RunnerInstance> = Vec::new();
        for runner_cfg in &config.runners {
            let machine_token = match machine_token_for_runner(&config, runner_cfg) {
                Ok(token) => token,
                Err(error) if !opts.offline => {
                    tracing::warn!(runner = %runner_cfg.name, error = %error, "runner awaits workspace credential discovery");
                    continue;
                }
                Err(_) => None,
            };
            let inst = build_runner_instance(
                runner_cfg.clone(),
                transport.as_ref(),
                machine_token,
                &paths,
                &config.daemon,
            )
            .await?;
            instances.push(inst);
        }

        let mailboxes = Arc::new(RwLock::new(
            instances
                .iter()
                .map(|i| (i.runner_id, i.mailbox_tx.clone()))
                .collect(),
        ));
        let hello_runners = Arc::new(RwLock::new(
            instances
                .iter()
                .map(|i| {
                    (
                        i.runner_id,
                        (
                            i.out.clone(),
                            i.state.clone(),
                            i.config.project_slug.clone(),
                        ),
                    )
                })
                .collect::<HelloRunnerMap>(),
        ));

        // Replay terminal signals from the previous shutdown before IPC,
        // machine control, project sync, or any assignment loop can mutate
        // the runner registry or redeliver work. Transient survivors continue
        // in a background retry loop.
        let mut replay_retry_handle: Option<tokio::task::JoinHandle<()>> = None;
        if !opts.offline {
            replay_drain_journal_once(&paths, &hello_runners, configured_runner_ids.as_ref()).await;
            if !drain_journal::load(&paths).is_empty() {
                let replay_paths = paths.clone();
                let replay_runners = hello_runners.clone();
                let replay_configured_runner_ids = configured_runner_ids.clone();
                replay_retry_handle = Some(tokio::spawn(async move {
                    replay_drain_journal_retry_loop(
                        replay_paths,
                        replay_runners,
                        replay_configured_runner_ids,
                    )
                    .await;
                }));
            }
        }

        // Primary runner for IPC's "default snapshot" — whichever runner
        // happens to be first in config.toml. None when the connection
        // has zero runners yet (a freshly enrolled dev machine), in which
        // case IPC falls back to the daemon-level state.
        let primary = instances.first().cloned();

        // Shared registry for IPC routing, including runners activated
        // after startup by local enrollment or machine control.
        let ipc_instances: HashMap<uuid::Uuid, RunnerInstance> = instances
            .iter()
            .cloned()
            .map(|i| (i.runner_id, i))
            .collect();
        let ipc_instances = Arc::new(std::sync::RwLock::new(ipc_instances));

        if opts.offline {
            tracing::info!("offline mode: HTTP transport disabled");
        }

        // Shared context for bringing a runner's tasks up — used by the
        // startup loop below and by the machine control session's
        // cloud-driven `create_runner` hot-add path.
        let spawn_ctx = RunnerSpawnCtx {
            transport: transport.clone(),
            shared_machine_token: shared_machine_token.clone(),
            paths: paths.clone(),
            daemon: config.daemon.clone(),
            mailboxes: mailboxes.clone(),
            hello_runners: hello_runners.clone(),
            daemon_state: state.clone(),
            instances: ipc_instances.clone(),
            activation_lock: Arc::new(tokio::sync::Mutex::new(())),
            accepting_mutations: Arc::new(AtomicBool::new(true)),
        };

        let ipc = IpcServer {
            path: paths.ipc_socket_path(),
            primary_state: primary
                .as_ref()
                .map(|p| p.state.clone())
                .unwrap_or_else(|| state.clone()),
            daemon_state: state.clone(),
            paths: paths.clone(),
            instances: ipc_instances,
            spawn_ctx: spawn_ctx.clone(),
        }
        .prepare()?;
        let ipc_handle = tokio::spawn(async move {
            if let Err(e) = ipc.run().await {
                tracing::error!("ipc server exited: {e:#}");
            }
        });

        // One RunnerLoop per instance. Each consumes from its mailbox.
        let mut loop_handles: Vec<tokio::task::JoinHandle<()>> = Vec::new();
        let mut http_handles: Vec<tokio::task::JoinHandle<()>> = Vec::new();
        let mut refresh_handles: Vec<tokio::task::JoinHandle<()>> = Vec::new();
        for inst in &instances {
            let tasks = spawn_instance_tasks(inst, &spawn_ctx).await;
            loop_handles.extend(tasks.runner_loop);
            http_handles.extend(tasks.http_loop);
            refresh_handles.extend(tasks.refresh);
        }

        // Machine-level control session: lets the cloud push
        // machine-scoped commands (``create_runner``) down to this
        // machine even when it hosts zero runners. Requires the shared
        // machine token and a persisted dev_machine_id — both written
        // by `karma-team auth login` / modern enrollments; legacy installs
        // without them simply don't get the channel.
        let machine_handle = match (
            transport.clone(),
            shared_machine_token.clone(),
            config.daemon.dev_machine_id,
        ) {
            (Some(shared), Some(token), Some(dev_machine_id)) => {
                let client = MachineClient::new(shared, dev_machine_id, token);
                let control = crate::daemon::machine_control::MachineControl::new(
                    client,
                    spawn_ctx.clone(),
                    state.clone(),
                );
                Some(tokio::spawn(async move { control.run().await }))
            }
            _ => {
                if !opts.offline {
                    tracing::info!(
                        "machine control session disabled (missing dev_machine_id or mt_ token in config)"
                    );
                }
                None
            }
        };

        // Project reconciliation is machine-scoped and intentionally lives
        // beside (rather than inside) the per-runner assignment loops. It is
        // available with zero configured runners as long as the account has a
        // machine token and identity.
        let project_sync_handle = match (
            transport.clone(),
            shared_machine_token.clone(),
            config.daemon.dev_machine_id,
            config
                .cli
                .as_ref()
                .and_then(|cli| cli.workspace_slug.clone()),
        ) {
            (Some(shared), Some(token), Some(dev_machine_id), Some(workspace_slug)) => {
                let client = MachineClient::new(shared, dev_machine_id, token);
                let sync = crate::daemon::project_sync::ProjectSync::new(
                    client,
                    spawn_ctx.clone(),
                    state.clone(),
                    workspace_slug,
                );
                Some(tokio::spawn(async move { sync.run().await }))
            }
            _ => {
                if !opts.offline {
                    tracing::info!(
                        "project sync disabled (missing dev_machine_id, mt_ token, or workspace binding)"
                    );
                }
                None
            }
        };

        let shutdown = state.shutdown_notified();
        let sig = crate::util::signal::shutdown();
        tokio::select! {
            _ = shutdown.notified() => {
                tracing::info!("shutdown requested via IPC");
            }
            r = sig => {
                if let Err(e) = r { tracing::warn!("signal watcher failed: {e:#}"); }
            }
        }

        // Freeze every source that can hot-add/remove runners before taking
        // the in-flight snapshot below. The gate is closed while holding the
        // same activation mutex used by runtime mutations, so an operation
        // already inside its critical section completes first and no later
        // operation can enter. Abort and await the external ingress tasks as
        // well: this prevents ProjectSync, machine control, or IPC from
        // changing the registry while shutdown is draining it.
        spawn_ctx.freeze_mutations().await;
        state.shutdown();
        if let Some(handle) = machine_handle {
            handle.abort();
            let _ = handle.await;
        }
        if let Some(handle) = project_sync_handle {
            handle.abort();
            let _ = handle.await;
        }
        ipc_handle.abort();
        let _ = ipc_handle.await;

        // Stop replay before journaling the current in-flight snapshot. A
        // concurrent replay cleanup could otherwise remove an entry that the
        // shutdown drain has just refreshed for the same run.
        if let Some(handle) = replay_retry_handle {
            handle.abort();
            let _ = handle.await;
        }

        // Drain in-flight runs before tearing down: send RunFailed with
        // DaemonRestart so the cloud transitions each run to FAILED via
        // a deliberate signal instead of leaving them BUSY for the
        // heartbeat reaper to clean up after our successor reconnects.
        // Bounded by a 5s deadline so a slow / unreachable cloud can't
        // hang the shutdown — systemd's default TimeoutStopSec (90s)
        // would still SIGKILL us, but losing a clean error message is
        // better than losing the entire shutdown sequence.
        if let Err(_elapsed) = tokio::time::timeout(
            Duration::from_secs(5),
            drain_in_flight_runs(hello_runners.clone(), &paths),
        )
        .await
        {
            tracing::warn!("drain of in-flight runs timed out at 5s");
        }

        for h in loop_handles {
            h.abort();
        }
        for h in http_handles {
            h.abort();
        }
        for h in refresh_handles {
            h.abort();
        }
        Ok(())
    }
}

/// Construct (but don't start) a `RunnerInstance` from its config
/// block. Shared by the startup loop and the machine control session's
/// hot-add path.
async fn build_runner_instance(
    runner_cfg: RunnerConfig,
    transport: Option<&SharedHttpTransport>,
    shared_machine_token: Option<&str>,
    paths: &Paths,
    daemon: &DaemonConfig,
) -> Result<RunnerInstance> {
    let inst = if let Some(shared) = transport {
        let runner_paths = paths.for_runner(runner_cfg.runner_id);
        let creds =
            load_runner_credentials(&runner_paths, &runner_cfg.name, shared_machine_token).await?;
        let client = RunnerCloudClient::new(runner_cfg.runner_id, creds, shared.clone());
        RunnerInstance::new_http(runner_cfg, paths, client, daemon.clone())
    } else {
        RunnerInstance::new_offline(runner_cfg, paths, daemon.clone())
    };
    inst.paths.ensure()?;
    // IPC must report the configured identity even before the first heartbeat.
    inst.state.set_runner_id(inst.runner_id).await;
    Ok(inst)
}

/// Everything a post-startup code path needs to bring a new runner up
/// in-process (cloud `create_runner` → daemon hot-add, no restart).
/// All fields are cheap clones of the supervisor's shared state.
#[derive(Clone)]
pub(crate) struct RunnerSpawnCtx {
    transport: Option<SharedHttpTransport>,
    shared_machine_token: Option<String>,
    pub(crate) paths: Paths,
    daemon: DaemonConfig,
    mailboxes: Arc<RwLock<HashMap<uuid::Uuid, mpsc::Sender<InboundEnvelope>>>>,
    hello_runners: Arc<RwLock<HelloRunnerMap>>,
    daemon_state: StateHandle,
    instances: Arc<std::sync::RwLock<HashMap<uuid::Uuid, RunnerInstance>>>,
    activation_lock: Arc<tokio::sync::Mutex<()>>,
    /// Closed under `activation_lock` before shutdown drains the runner
    /// registry. Every hot-add/remove path re-checks it while holding the same
    /// lock, so the drain observes a stable set of runner instances.
    accepting_mutations: Arc<AtomicBool>,
}

impl RunnerSpawnCtx {
    #[cfg(test)]
    pub(crate) fn for_project_sync_test(paths: Paths, config: &Config) -> Self {
        Self {
            transport: None,
            shared_machine_token: config.cli.as_ref().and_then(|cli| cli.token.clone()),
            paths,
            daemon: config.daemon.clone(),
            mailboxes: Arc::new(RwLock::new(HashMap::new())),
            hello_runners: Arc::new(RwLock::new(HashMap::new())),
            daemon_state: StateHandle::new(config.clone()),
            instances: Arc::new(std::sync::RwLock::new(HashMap::new())),
            activation_lock: Arc::new(tokio::sync::Mutex::new(())),
            accepting_mutations: Arc::new(AtomicBool::new(true)),
        }
    }

    pub(crate) async fn freeze_mutations(&self) {
        let _activation = self.activation_lock.lock().await;
        self.accepting_mutations.store(false, Ordering::SeqCst);
    }

    fn ensure_accepting_mutations(&self) -> Result<()> {
        anyhow::ensure!(
            self.accepting_mutations.load(Ordering::SeqCst),
            "daemon is shutting down; runner mutations are disabled"
        );
        Ok(())
    }

    pub(crate) async fn activate_configured(&self, name: &str) -> Result<()> {
        let cfg = crate::config::file::load_config(&self.paths)?;
        cfg.validate()
            .map_err(|e| anyhow::anyhow!("invalid config: {e}"))?;
        self.check_config_identity(&cfg)?;
        let runner = cfg
            .runners
            .into_iter()
            .find(|r| r.name == name)
            .ok_or_else(|| anyhow::anyhow!("no configured runner named {name:?}"))?;
        self.add_runner(runner).await
    }

    pub(crate) fn check_config_identity(&self, cfg: &Config) -> Result<()> {
        if cfg.daemon.cloud_url != self.daemon.cloud_url
            || cfg.daemon.dev_machine_id != self.daemon.dev_machine_id
            || cfg.cli.as_ref().and_then(|cli| cli.token.as_deref())
                != self.shared_machine_token.as_deref()
        {
            anyhow::bail!("daemon identity changed; restart before activating a runner");
        }
        Ok(())
    }

    pub(crate) fn cloud_url(&self) -> String {
        self.daemon.cloud_url.clone()
    }

    /// Hot-add: bring a freshly configured runner up in-process.
    ///
    /// Registers the instance in the shared mailbox / hello maps and
    /// spawns the same three tasks the startup loop spawns. Handles are
    /// deliberately detached: on daemon shutdown the tasks die with the
    /// process, and the graceful drain and IPC registry include hot-added runners.
    pub(crate) async fn add_runner(&self, runner_cfg: RunnerConfig) -> Result<()> {
        let _activation = self.activation_lock.lock().await;
        self.ensure_accepting_mutations()?;
        if self
            .instances
            .read()
            .expect("runner registry poisoned")
            .contains_key(&runner_cfg.runner_id)
        {
            return Ok(());
        }
        let cfg = if self.transport.is_some() {
            let cfg = crate::config::file::load_config(&self.paths)?;
            self.check_config_identity(&cfg)?;
            Some(cfg)
        } else {
            None
        };
        let machine_token = cfg
            .as_ref()
            .map(|cfg| machine_token_for_runner(cfg, &runner_cfg))
            .transpose()?
            .flatten();
        let inst = build_runner_instance(
            runner_cfg,
            self.transport.as_ref(),
            machine_token,
            &self.paths,
            &self.daemon,
        )
        .await?;
        {
            let mut mailboxes = self.mailboxes.write().await;
            mailboxes.insert(inst.runner_id, inst.mailbox_tx.clone());
        }
        {
            let mut hello = self.hello_runners.write().await;
            hello.insert(
                inst.runner_id,
                (
                    inst.out.clone(),
                    inst.state.clone(),
                    inst.config.project_slug.clone(),
                ),
            );
        }
        let _tasks = spawn_instance_tasks(&inst, self).await;
        self.instances
            .write()
            .expect("runner registry poisoned")
            .insert(inst.runner_id, inst.clone());
        tracing::info!(runner_id = %inst.runner_id, name = %inst.name, "activated runner in running daemon");
        Ok(())
    }

    pub(crate) fn configured_runners(&self) -> Result<Vec<RunnerConfig>> {
        Ok(crate::config::file::load_config(&self.paths)?.runners)
    }

    pub(crate) fn local_runner_states(
        &self,
    ) -> Result<Vec<crate::daemon::project_sync::LocalRunnerState>> {
        let configured = self.configured_runners()?;
        let live = self.instances.read().expect("runner registry poisoned");
        Ok(configured
            .into_iter()
            .map(|runner| crate::daemon::project_sync::LocalRunnerState {
                runner_id: runner.runner_id,
                workspace_slug: runner.workspace_slug,
                project_id: runner.project_id,
                project_slug: runner.project_slug,
                binding_source: runner.binding_source,
                live: live.contains_key(&runner.runner_id),
            })
            .collect())
    }

    /// Apply cloud-owned project identity changes to already-running
    /// instances without restarting their assignment/chat workers. The
    /// persisted config is updated first by ProjectSync; this method mirrors
    /// that authoritative state into the live registry and the shared state
    /// handle consumed by Hello, HTTP reconnect, IPC status, and new runs.
    pub(crate) fn refresh_project_metadata(&self, configured: &[RunnerConfig]) -> usize {
        let by_id: HashMap<uuid::Uuid, &RunnerConfig> = configured
            .iter()
            .map(|runner| (runner.runner_id, runner))
            .collect();
        let mut refreshed = 0;
        let mut instances = self.instances.write().expect("runner registry poisoned");
        for (runner_id, instance) in instances.iter_mut() {
            let Some(configured_runner) = by_id.get(runner_id) else {
                continue;
            };
            let project_changed = instance.config.project_slug != configured_runner.project_slug
                || instance.config.project_id != configured_runner.project_id;
            instance.config.project_slug = configured_runner.project_slug.clone();
            instance.config.project_id = configured_runner.project_id;
            instance.config.binding_source = configured_runner.binding_source;
            instance.config.pod_id = configured_runner.pod_id;
            instance
                .state
                .set_project_slug(configured_runner.project_slug.clone());
            if project_changed {
                refreshed += 1;
            }
        }
        refreshed
    }

    /// Deliver the same teardown envelope used by the IPC remove command and
    /// wait until the live actor has stopped workers, removed runtime data,
    /// and stripped the config entry.  The ordering is deliberate: keeping
    /// the config entry until runtime cleanup succeeds means a process crash
    /// can reactivate and retry the runner instead of leaving an undiscoverable
    /// orphan under `data/runners/<id>`.
    pub(crate) async fn remove_runner(&self, runner_id: uuid::Uuid) -> Result<()> {
        let _activation = self.activation_lock.lock().await;
        self.ensure_accepting_mutations()?;

        // Read before deleting anything so a legacy working directory nested
        // inside the runner runtime tree remains protected during cleanup.
        let configured_working_dir =
            load_configured_runner_working_dir(&self.paths, runner_id).await?;
        let live = self
            .instances
            .read()
            .expect("runner registry poisoned")
            .get(&runner_id)
            .map(|inst| {
                (
                    inst.mailbox_tx.clone(),
                    inst.teardown_complete_tx.subscribe(),
                )
            });
        self.mailboxes.write().await.remove(&runner_id);
        self.hello_runners.write().await.remove(&runner_id);

        if let Some((mailbox, completion_rx)) = live {
            let envelope = crate::cloud::protocol::Envelope::for_runner(
                runner_id,
                ServerMsg::RemoveRunner {
                    runner_id,
                    reason: Some("project no longer exists in cloud".to_string()),
                },
            );
            if mailbox
                .send(InboundEnvelope {
                    stream_id: None,
                    env: envelope,
                })
                .await
                .is_err()
            {
                // A closed mailbox proves the actor no longer owns runtime
                // resources. Evict the stale registry entry so the next
                // at-least-once delivery can take the inactive cleanup path
                // instead of retrying the same dead sender forever.
                self.instances
                    .write()
                    .expect("runner registry poisoned")
                    .remove(&runner_id);
                // The receiver is dropped only after RunnerLoop exits.  Its
                // completion channel therefore contains the authoritative
                // result for a normal exit; a still-pending value indicates
                // an abnormal task loss and must remain retryable/unacked.
                return match completion_rx.borrow().clone() {
                    RunnerTeardownOutcome::Succeeded => Ok(()),
                    RunnerTeardownOutcome::Failed(detail) => {
                        Err(anyhow::anyhow!("runner teardown failed: {detail}"))
                    }
                    RunnerTeardownOutcome::Pending => Err(anyhow::anyhow!(
                        "runner loop exited before teardown completed"
                    )),
                };
            }
            return await_runner_teardown(completion_rx).await;
        }

        // No actor owns this runner.  Cleanup remains idempotent and follows
        // the same crash-safe ordering as the live path: runtime first,
        // config second.  A failed cleanup leaves the config discoverable for
        // the next ProjectSync/MachineControl retry.
        remove_runner_data_dir_preserving(
            &self.paths,
            runner_id,
            configured_working_dir.as_deref(),
        )?;
        let _ = strip_runner_config(&self.paths, runner_id).await?;
        Ok(())
    }
}

async fn await_runner_teardown(
    mut completion_rx: watch::Receiver<RunnerTeardownOutcome>,
) -> Result<()> {
    loop {
        let outcome = completion_rx.borrow().clone();
        match outcome {
            RunnerTeardownOutcome::Pending => {}
            RunnerTeardownOutcome::Succeeded => return Ok(()),
            RunnerTeardownOutcome::Failed(detail) => {
                return Err(anyhow::anyhow!("runner teardown failed: {detail}"));
            }
        }
        completion_rx.changed().await.map_err(|_| {
            anyhow::anyhow!("runner teardown completion channel closed while pending")
        })?;
    }
}

async fn load_configured_runner_working_dir(
    paths: &Paths,
    runner_id: uuid::Uuid,
) -> Result<Option<std::path::PathBuf>> {
    let paths = paths.clone();
    tokio::task::spawn_blocking(move || {
        let cfg = crate::config::file::load_config(&paths)?;
        Ok::<_, anyhow::Error>(
            cfg.runners
                .into_iter()
                .find(|runner| runner.runner_id == runner_id)
                .map(|runner| runner.workspace.working_dir),
        )
    })
    .await
    .map_err(|err| anyhow::anyhow!("runner config read task failed: {err}"))?
}

async fn strip_runner_config(paths: &Paths, runner_id: uuid::Uuid) -> Result<Config> {
    let paths = paths.clone();
    tokio::task::spawn_blocking(move || {
        crate::config::file::mutate_config_for_cleanup(&paths, |cfg| {
            cfg.runners.retain(|runner| runner.runner_id != runner_id);
            Ok(())
        })
    })
    .await
    .map_err(|err| anyhow::anyhow!("runner config cleanup task failed: {err}"))?
}

/// Handles of the tasks that drive one runner. `None` entries mean the
/// corresponding task was not spawned (offline mode has no HTTP/refresh
/// loop; a taken mailbox refuses a duplicate RunnerLoop).
struct SpawnedInstanceTasks {
    runner_loop: Option<tokio::task::JoinHandle<()>>,
    http_loop: Option<tokio::task::JoinHandle<()>>,
    refresh: Option<tokio::task::JoinHandle<()>>,
}

struct TeardownCompletionGuard {
    runner_id: uuid::Uuid,
    tx: watch::Sender<RunnerTeardownOutcome>,
}

impl Drop for TeardownCompletionGuard {
    fn drop(&mut self) {
        if matches!(*self.tx.borrow(), RunnerTeardownOutcome::Pending) {
            self.tx.send_replace(RunnerTeardownOutcome::Failed(format!(
                "runner loop {} exited before teardown completed",
                self.runner_id
            )));
        }
    }
}

/// The binary this runner's configured agent kind will actually launch.
fn agent_binary_for(config: &crate::config::schema::RunnerConfig) -> &str {
    match config.agent.kind {
        crate::config::schema::AgentKind::Codex => &config.codex.binary,
        crate::config::schema::AgentKind::ClaudeCode => &config.claude_code.binary,
        crate::config::schema::AgentKind::CursorAgent => &config.cursor_agent.binary,
        crate::config::schema::AgentKind::OpenClaw => &config.openclaw.binary,
        crate::config::schema::AgentKind::Grok => &config.grok.binary,
        crate::config::schema::AgentKind::MuseCode => &config.muse_code.binary,
        crate::config::schema::AgentKind::Karma => &config.karma.binary,
    }
}

/// Spawn the RunnerLoop + HttpLoop + refresh loop for one instance.
/// Extracted from `Supervisor::run`'s startup loop so the machine
/// control session's hot-add path spawns byte-identical machinery.
async fn spawn_instance_tasks(inst: &RunnerInstance, ctx: &RunnerSpawnCtx) -> SpawnedInstanceTasks {
    // Probe the agent binary once per daemon start, in the background: the
    // answer is only used for reporting, so it must never delay bringing the
    // runner online (or fail startup if the binary is briefly unavailable
    // during a desktop upgrade).
    {
        let slot = inst.engine_version.clone();
        let binary = agent_binary_for(&inst.config).to_string();
        let env = crate::agent::agent_env_for_config(&inst.config);
        tokio::spawn(async move {
            if let Some(version) = crate::util::shell::binary_version(&binary, &env).await {
                *slot.write().await = Some(version);
            }
        });
    }
    let mut tasks = SpawnedInstanceTasks {
        runner_loop: None,
        http_loop: None,
        refresh: None,
    };
    let mailbox_rx = match inst.take_mailbox_rx().await {
        Some(rx) => rx,
        None => {
            tracing::error!(
                %inst.runner_id,
                "mailbox already taken — refusing to spawn a duplicate RunnerLoop"
            );
            return tasks;
        }
    };
    let runner_paths = inst.paths.clone();
    let runner_config = inst.config.clone();
    let inst_state = inst.state.clone();
    let inst_approvals = inst.approvals.clone();
    let inst_out = inst.out.clone();
    let inst_ack_tx = inst.ack_tx.clone();
    let inst_remove_tx = inst.remove_tx.clone();
    let inst_teardown_complete_tx = inst.teardown_complete_tx.clone();
    let inst_client = inst.client.clone();
    let inst_chat_sessions = inst.chat_sessions.clone();
    let live_mailboxes = ctx.mailboxes.clone();
    let live_hello_runners = ctx.hello_runners.clone();
    let live_instances = ctx.instances.clone();
    let daemon_paths = ctx.paths.clone();
    // The working dir reported to the cloud is this runner's single, exclusive
    // work dir.
    let attach_working_dir = resolve_working_dir(inst);
    let inst_chat_active = inst.chat_active.clone();
    let h = tokio::spawn(async move {
        let _completion_guard = TeardownCompletionGuard {
            runner_id: runner_paths.runner_id,
            tx: inst_teardown_complete_tx.clone(),
        };
        let run = RunnerLoop {
            runner_paths,
            paths: daemon_paths,
            runner_config,
            out: inst_out,
            state: inst_state,
            approvals: inst_approvals,
            inbound: mailbox_rx,
            ack_tx: inst_ack_tx,
            remove_tx: inst_remove_tx,
            teardown_complete_tx: inst_teardown_complete_tx,
            client: inst_client,
            local_chat_sessions: inst_chat_sessions,
            live_mailboxes,
            live_hello_runners,
            live_instances,
            current_run: None,
            current_chat: None,
            local_chat_active: inst_chat_active,
        };
        if let Err(e) = run.run().await {
            tracing::error!("runner loop exited: {e:#}");
        }
    });
    tasks.runner_loop = Some(h);

    if let Some(client) = inst.client.clone() {
        let ack_rx = match inst.take_ack_rx().await {
            Some(rx) => rx,
            None => {
                tracing::error!(
                    %inst.runner_id,
                    "ack receiver already taken — refusing to spawn a duplicate HttpLoop"
                );
                return tasks;
            }
        };
        let http_loop = HttpLoop::new(
            client.clone(),
            inst.mailbox_tx.clone(),
            ack_rx,
            inst.state.rx_status.clone(),
            inst.state.rx_in_flight.clone(),
            inst.state.shutdown_notified(),
            attach_body_for_instance(inst, attach_working_dir),
        )
        .with_state(inst.state.clone())
        .with_teardown_rx(inst.remove_tx.subscribe());
        let close_client = client.clone();
        let local_state = inst.state.clone();
        let daemon_state = ctx.daemon_state.clone();
        let http_handle = tokio::spawn(async move {
            if let Err(e) = http_loop.run().await {
                if e.is_expected_teardown() {
                    tracing::info!("http loop exited after runner teardown: {e:#}");
                } else {
                    tracing::error!("http loop exited: {e:#}");
                }
                local_state.set_connected(false).await;
                let _ = tokio::time::timeout(Duration::from_secs(2), close_client.close_session())
                    .await;
                if e.requires_daemon_restart() {
                    tracing::error!(
                        "http loop hit a local invariant failure; requesting daemon restart"
                    );
                    daemon_state.shutdown();
                }
            }
        });
        tasks.http_loop = Some(http_handle);

        let refresh_state = inst.state.clone();
        let refresh_teardown = inst.remove_tx.subscribe();
        let refresh_handle = tokio::spawn(async move {
            refresh_loop(client, refresh_state, refresh_teardown).await;
        });
        tasks.refresh = Some(refresh_handle);
    }
    tasks
}

/// On daemon shutdown, send `RunFailed{DaemonRestart}` for every
/// runner that currently reports an in-flight run. Without this the
/// cloud is left guessing — its heartbeat reaper eventually fails the
/// run with the cryptic ``reaped by heartbeat: ... in_flight_run=
/// (none)`` message after the daemon restarts and the next session
/// Hello legitimately reports null. This sends a clean terminal
/// signal instead.
///
/// Returns the count of runs we successfully drained. Send errors are
/// logged at warn but do not abort the drain — one runner's
/// unreachable cloud client must not block another runner's drain.
///
/// Sends are issued in parallel via `join_all` and each is bounded by
/// a 2s timeout. The outer 5s timeout in `Supervisor::run` still caps
/// total wall-time, but with up to ~30 concurrent runners on one host
/// a serial loop (each attempt potentially walking through the shared
/// reqwest Client's 60s timeout) would starve later runners entirely.
async fn drain_in_flight_runs(runners: Arc<RwLock<HelloRunnerMap>>, paths: &Paths) -> usize {
    // Snapshot under the read lock so the lock is dropped before any
    // network I/O; concurrent writers (config reloads, etc.) are not
    // expected during shutdown but this keeps the contract clean.
    let snapshot: Vec<(uuid::Uuid, Option<uuid::Uuid>, RunnerOut)> = {
        let guard = runners.read().await;
        guard
            .iter()
            .map(|(rid, (out, state, _))| (*rid, *state.rx_in_flight.borrow(), out.clone()))
            .collect()
    };
    let now = Utc::now();
    const DETAIL: &str = "daemon shutdown requested";
    let pending: Vec<drain_journal::PendingRunFailure> = snapshot
        .iter()
        .filter_map(|(runner_id, in_flight, _)| {
            Some(drain_journal::PendingRunFailure {
                runner_id: *runner_id,
                run_id: (*in_flight)?,
                reason: FailureReason::DaemonRestart,
                detail: DETAIL.to_string(),
                ended_at: now,
                recorded_at: now,
            })
        })
        .collect();

    // Journal before making any network request. Blocking I/O stays off the
    // runtime, and the task continues to completion if the outer shutdown
    // deadline drops this future while the file is being synced.
    {
        let journal_paths = paths.clone();
        match tokio::task::spawn_blocking(move || {
            drain_journal::record_all(&journal_paths, &pending)
        })
        .await
        {
            Ok(Ok(())) => {}
            Ok(Err(error)) => {
                tracing::warn!("failed to journal in-flight runs before drain: {error:#}");
            }
            Err(error) => tracing::warn!("drain journal task panicked: {error:#}"),
        }
    }

    let drains = snapshot
        .into_iter()
        .filter_map(|(runner_id, in_flight, out)| {
            let run_id = in_flight?;
            let msg = ClientMsg::RunFailed {
                run_id,
                reason: FailureReason::DaemonRestart,
                detail: Some(DETAIL.to_string()),
                ended_at: now,
                tokens: None,
                model: None,
            };
            Some(async move {
                // Per-attempt timeout so one stuck cloud client can't keep
                // a parallel sibling from completing in time.
                match send_run_failed_bounded(out, msg, Duration::from_secs(2)).await {
                    RunFailedSend::Delivered => {
                        tracing::info!(%runner_id, %run_id, "drained in-flight run on shutdown");
                        Some(run_id)
                    }
                    RunFailedSend::Failed(error) => {
                        tracing::warn!(
                            %runner_id,
                            %run_id,
                            "drain send failed (journaled for replay on next start): {error:#}"
                        );
                        None
                    }
                    RunFailedSend::TimedOut => {
                        tracing::warn!(
                            %runner_id,
                            %run_id,
                            "drain send timed out at 2s (journaled for replay on next start)"
                        );
                        None
                    }
                }
            })
        });
    let delivered: Vec<uuid::Uuid> = futures_util::future::join_all(drains)
        .await
        .into_iter()
        .flatten()
        .collect();

    // Only confirmed deliveries are removed. Failed and timed-out sends stay
    // durable for the next startup replay.
    {
        let journal_paths = paths.clone();
        let delivered_ids = delivered.clone();
        match tokio::task::spawn_blocking(move || {
            drain_journal::remove(&journal_paths, &delivered_ids)
        })
        .await
        {
            Ok(Ok(())) => {}
            Ok(Err(error)) => {
                tracing::warn!("failed to clear delivered runs from drain journal: {error:#}");
            }
            Err(error) => tracing::warn!("drain journal cleanup task panicked: {error:#}"),
        }
    }
    delivered.len()
}

enum RunFailedSend {
    Delivered,
    Failed(anyhow::Error),
    TimedOut,
}

async fn send_run_failed_bounded(out: RunnerOut, msg: ClientMsg, bound: Duration) -> RunFailedSend {
    match tokio::time::timeout(bound, out.send(msg)).await {
        Ok(Ok(())) => RunFailedSend::Delivered,
        Ok(Err(error)) => RunFailedSend::Failed(error),
        Err(_) => RunFailedSend::TimedOut,
    }
}

fn is_permanent_run_failed_rejection(error: &anyhow::Error) -> bool {
    matches!(
        error.downcast_ref::<crate::cloud::http::TransportError>(),
        Some(crate::cloud::http::TransportError::Server { status, .. })
            if matches!(status, 403 | 404 | 410)
    )
}

/// Replay one pass of terminal signals that survived a previous shutdown.
/// Configured but temporarily inactive runners are retained so project sync
/// or machine control can activate them and a later retry can deliver their
/// signal. Entries for deleted runners, redelivered live runs, and permanent
/// cloud rejections are resolved without another retry. Journal age is not a
/// resolution signal: deployments may disable the server-side offline sweep,
/// and an outage can outlast any fixed local TTL.
async fn replay_drain_journal_once(
    paths: &Paths,
    runners: &Arc<RwLock<HelloRunnerMap>>,
    configured_runner_ids: &HashSet<uuid::Uuid>,
) -> usize {
    let entries = drain_journal::load(paths);
    if entries.is_empty() {
        return 0;
    }

    let mut resolved = Vec::new();
    let mut sends = Vec::new();
    {
        let guard = runners.read().await;
        for entry in entries {
            let Some((out, state, _)) = guard.get(&entry.runner_id) else {
                if configured_runner_ids.contains(&entry.runner_id) {
                    tracing::info!(
                        runner_id = %entry.runner_id,
                        run_id = %entry.run_id,
                        "retaining journaled RunFailed: configured runner is not active yet"
                    );
                } else {
                    tracing::warn!(
                        runner_id = %entry.runner_id,
                        run_id = %entry.run_id,
                        "dropping journaled RunFailed: runner no longer configured"
                    );
                    resolved.push(entry.run_id);
                }
                continue;
            };

            if *state.rx_in_flight.borrow() == Some(entry.run_id) {
                tracing::info!(
                    runner_id = %entry.runner_id,
                    run_id = %entry.run_id,
                    "dropping journaled RunFailed: run is in flight again"
                );
                resolved.push(entry.run_id);
                continue;
            }

            let out = out.clone();
            let in_flight = state.rx_in_flight.clone();
            sends.push(async move {
                // Retry passes can race an assignment after the registry
                // lookup, so check again immediately before delivery.
                if *in_flight.borrow() == Some(entry.run_id) {
                    tracing::info!(
                        runner_id = %entry.runner_id,
                        run_id = %entry.run_id,
                        "dropping journaled RunFailed: run redelivered before send"
                    );
                    return ReplayOutcome::Resolved(entry.run_id);
                }
                let message = ClientMsg::RunFailed {
                    run_id: entry.run_id,
                    reason: entry.reason,
                    detail: Some(entry.detail.clone()),
                    ended_at: entry.ended_at,
                    tokens: None,
                    model: None,
                };
                match send_run_failed_bounded(out, message, Duration::from_secs(10)).await {
                    RunFailedSend::Delivered => {
                        tracing::info!(
                            runner_id = %entry.runner_id,
                            run_id = %entry.run_id,
                            "replayed journaled RunFailed from previous shutdown"
                        );
                        ReplayOutcome::Delivered(entry.run_id)
                    }
                    RunFailedSend::Failed(error) if is_permanent_run_failed_rejection(&error) => {
                        tracing::warn!(
                            runner_id = %entry.runner_id,
                            run_id = %entry.run_id,
                            "dropping journaled RunFailed: cloud rejected it permanently: {error:#}"
                        );
                        ReplayOutcome::Resolved(entry.run_id)
                    }
                    RunFailedSend::Failed(error) => {
                        tracing::warn!(
                            runner_id = %entry.runner_id,
                            run_id = %entry.run_id,
                            "replaying journaled RunFailed failed, will retry: {error:#}"
                        );
                        ReplayOutcome::Retry
                    }
                    RunFailedSend::TimedOut => {
                        tracing::warn!(
                            runner_id = %entry.runner_id,
                            run_id = %entry.run_id,
                            "replaying journaled RunFailed timed out, will retry"
                        );
                        ReplayOutcome::Retry
                    }
                }
            });
        }
    }

    let mut delivered = 0;
    for outcome in futures_util::future::join_all(sends).await {
        match outcome {
            ReplayOutcome::Delivered(run_id) => {
                delivered += 1;
                resolved.push(run_id);
            }
            ReplayOutcome::Resolved(run_id) => resolved.push(run_id),
            ReplayOutcome::Retry => {}
        }
    }
    if let Err(error) = drain_journal::remove(paths, &resolved) {
        tracing::warn!("failed to update drain journal after replay: {error:#}");
    }
    delivered
}

enum ReplayOutcome {
    Delivered(uuid::Uuid),
    Resolved(uuid::Uuid),
    Retry,
}

async fn replay_drain_journal_retry_loop(
    paths: Paths,
    runners: Arc<RwLock<HelloRunnerMap>>,
    configured_runner_ids: Arc<HashSet<uuid::Uuid>>,
) {
    loop {
        tokio::time::sleep(Duration::from_secs(60)).await;
        if drain_journal::load(&paths).is_empty() {
            return;
        }
        replay_drain_journal_once(&paths, &runners, configured_runner_ids.as_ref()).await;
    }
}

/// Watch the `connected` notify and re-emit one `Hello` per `RunnerInstance`
/// every time it fires. Driven by `ConnectionLoop`, which calls
/// `notify_one()` after each successful WS handshake (cold start and every
/// reconnect). Cloud-side `_handle_token_hello` is idempotent on re-Hello,
/// so a second emission for an already-authorised runner is harmless.
///
/// Also flips the daemon-level ``connected`` flag — the per-runner
/// Welcome handler also sets this on each Welcome, but with zero
/// runners there's no Hello/Welcome cycle to fall back on, so the
/// IPC / TUI would otherwise show "cloud offline" forever.
#[allow(dead_code)]
async fn hello_emitter(
    runners: Arc<RwLock<HelloRunnerMap>>,
    connected: Arc<tokio::sync::Notify>,
    daemon_state: StateHandle,
) {
    loop {
        connected.notified().await;
        daemon_state.set_connected(true).await;
        let current_runners: Vec<HelloRunner> =
            { runners.read().await.values().cloned().collect() };
        for (out, state, project_slug) in current_runners {
            // StateHandle is the live source of project identity after a
            // cloud rename. Keep the tuple's legacy fallback for tests and
            // old callers that do not populate it.
            let project_slug = state.project_slug_now().or(project_slug);
            let hello = ClientMsg::Hello {
                runner_id: out.runner_id(),
                version: crate::RUNNER_VERSION.to_string(),
                os: std::env::consts::OS.to_string(),
                arch: std::env::consts::ARCH.to_string(),
                status: *state.rx_status.borrow(),
                in_flight_run: *state.rx_in_flight.borrow(),
                protocol_version: crate::PROTOCOL_VERSION,
                project_slug,
            };
            // Channel-closed means the cloud loop exited; the next
            // reconnect will re-fire the notify and we'll retry then.
            // Best-effort: don't bring down the daemon over a single
            // failed Hello.
            let _ = out.send(hello).await;
        }
    }
}

fn machine_token_for_runner<'a>(cfg: &'a Config, runner: &RunnerConfig) -> Result<Option<&'a str>> {
    let Some(cli) = cfg.cli.as_ref().filter(|cli| {
        cli.token
            .as_deref()
            .is_some_and(|token| token.starts_with("mt_"))
    }) else {
        return Ok(None);
    };
    let primary_workspace = cli.workspace_slug.as_deref().or_else(|| {
        cfg.runners
            .first()
            .and_then(|runner| runner.workspace_slug.as_deref())
    });
    let workspace = runner.workspace_slug.as_deref();
    let token = cli.machine_token_for_workspace(if workspace == primary_workspace {
        None
    } else {
        workspace
    });
    anyhow::ensure!(
        token.is_some(),
        "no machine credential for workspace {:?}",
        runner.workspace_slug
    );
    Ok(token)
}

async fn load_runner_credentials(
    runner_paths: &RunnerPaths,
    runner_name: &str,
    shared_machine_token: Option<&str>,
) -> Result<CredentialsHandle> {
    if let Some(token) = shared_machine_token {
        return Ok(CredentialsHandle::new(
            runner_paths.credentials_path(),
            RunnerCredentials {
                runner_id: runner_paths.runner_id,
                name: runner_name.to_string(),
                refresh_token: token.to_string(),
                refresh_token_generation: 0,
            },
        ));
    }
    crate::cloud::http::load_runner_credentials_from(runner_paths.credentials_path(), runner_name)
        .await
        .map_err(|e| anyhow::anyhow!("{e}"))
}

/// Resolve the local dev-machine working directory the cloud should
/// display for this runner: its single, exclusive `workspace.working_dir`,
/// normalized to an absolute path.
fn resolve_working_dir(inst: &RunnerInstance) -> Option<std::path::PathBuf> {
    Some(canonical_for_compare(&inst.config.workspace.working_dir))
}

/// Remove only the runner-specific runtime tree. The configured working
/// directory is intentionally outside this path and is never deleted by
/// cloud/project reconciliation.
#[cfg(test)]
fn remove_runner_data_dir(paths: &Paths, runner_id: uuid::Uuid) -> Result<()> {
    remove_runner_data_dir_preserving(paths, runner_id, None)
}

fn remove_runner_data_dir_preserving(
    paths: &Paths,
    runner_id: uuid::Uuid,
    working_dir: Option<&std::path::Path>,
) -> Result<()> {
    paths.remove_runner_runtime_dir(runner_id, working_dir)
}

fn attach_body_for_instance(
    inst: &RunnerInstance,
    working_dir: Option<std::path::PathBuf>,
) -> AttachBody {
    let mut agent_versions = HashMap::new();
    let kind_key = format!("{:?}", inst.config.agent.kind).to_ascii_lowercase();
    // The agent binary's own version when we managed to probe it, falling back
    // to the runner's version so the key is never absent (older cloud builds
    // read it positionally). `try_read` keeps this function sync and lock-free:
    // a probe in flight just means this attach reports the fallback and the
    // next reconnect carries the real value.
    let probed = inst
        .engine_version
        .try_read()
        .ok()
        .and_then(|guard| guard.clone());
    agent_versions.insert(
        kind_key,
        probed.unwrap_or_else(|| crate::RUNNER_VERSION.to_string()),
    );
    AttachBody {
        version: crate::RUNNER_VERSION.to_string(),
        os: std::env::consts::OS.to_string(),
        arch: std::env::consts::ARCH.to_string(),
        status: PollStatus::from_wire(
            *inst.state.rx_status.borrow(),
            *inst.state.rx_in_flight.borrow(),
        )
        .status,
        in_flight_run: *inst.state.rx_in_flight.borrow(),
        project_slug: inst.config.project_slug.clone(),
        host_label: hostname().unwrap_or_else(|| inst.config.name.clone()),
        working_dir: working_dir
            .map(|path| path.to_string_lossy().into_owned())
            .unwrap_or_default(),
        agent_versions,
        agent_kind: inst.config.agent.kind,
    }
}

async fn refresh_loop(
    client: RunnerCloudClient,
    state: StateHandle,
    mut teardown_rx: watch::Receiver<bool>,
) {
    // Acquire the shutdown Notify ONCE and pin it across the whole loop
    // so a `notify_one()` that fires between iterations isn't dropped on
    // the floor. Re-acquiring per-iteration would race: each iteration
    // obtains a fresh `Arc<Notify>` view, so a notify between two
    // iterations only wakes the previous (already-dropped) waiter.
    let shutdown = state.shutdown_notified();
    let shutdown_fut = shutdown.notified();
    tokio::pin!(shutdown_fut);
    loop {
        if *teardown_rx.borrow() {
            return;
        }
        let sleep_for = match client.access_token_exp().await {
            Some(exp) => {
                let now = Utc::now();
                let safety = Duration::from_secs(300);
                exp.signed_duration_since(now)
                    .to_std()
                    .unwrap_or_default()
                    .saturating_sub(safety)
            }
            None => Duration::from_secs(60),
        };
        tokio::select! {
            biased;
            _ = &mut shutdown_fut => return,
            changed = teardown_rx.changed() => {
                if changed.is_err() || *teardown_rx.borrow() {
                    return;
                }
            }
            _ = tokio::time::sleep(sleep_for) => {}
        }
        tokio::select! {
            biased;
            _ = &mut shutdown_fut => return,
            changed = teardown_rx.changed() => {
                if changed.is_err() || *teardown_rx.borrow() {
                    return;
                }
            }
            result = client.refresh() => {
                if let Err(e) = result {
                    tracing::error!(runner_id = %client.runner_id(), "scheduled refresh failed: {e:#}");
                }
            }
        }
    }
}

/// Spawn a small task that flips `agent_subprocess_alive` to false the
/// moment the bridge-owned wait task observes the agent subprocess
/// terminating. Independent of stdout-close detection so a `kill -9`
/// path that double-forks or otherwise keeps stdout open is still
/// caught for observability.
///
/// Scoped to `run_id`: if a new run has taken over the in-flight slot
/// by the time this task fires, the alive flag is left alone — the new
/// run's own watcher owns it. Without this guard, run A's exit could
/// stamp `alive=false` on run B's snapshot.
fn spawn_exit_watch(
    state: StateHandle,
    run_id: uuid::Uuid,
    mut exit_rx: tokio::sync::watch::Receiver<Option<crate::agent::ExitSnapshot>>,
) {
    tokio::spawn(async move {
        // A freshly-cloned watch::Receiver marks the current value as
        // "seen", so changed() only fires on values published *after*
        // we subscribed. Check borrow() first to catch the case where
        // the wait task already published Some(ExitSnapshot) before we
        // got here (e.g. agent binary segfaults on startup).
        let already_exited = exit_rx.borrow().is_some();
        if !already_exited {
            if exit_rx.changed().await.is_err() {
                return;
            }
            if exit_rx.borrow().is_none() {
                return;
            }
        }
        // Guard: only stamp alive=false if the in-flight run is still
        // ours. A new run may already have taken over and called
        // set_agent_alive(true); we must not stomp it.
        if *state.rx_in_flight.borrow() != Some(run_id) {
            return;
        }
        state.set_agent_alive(false).await;
    });
}

fn hostname() -> Option<String> {
    std::process::Command::new("hostname")
        .output()
        .ok()
        .and_then(|o| String::from_utf8(o.stdout).ok())
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
}

struct RunnerLoop {
    runner_paths: RunnerPaths,
    /// Daemon-level paths. Threaded in so `ServerMsg::RemoveRunner` can
    /// strip this runner's `[[runner]]` block from `config.toml` under
    /// the host-wide config lock.
    paths: Paths,
    runner_config: crate::config::schema::RunnerConfig,
    out: RunnerOut,
    state: StateHandle,
    approvals: ApprovalRouter,
    inbound: mpsc::Receiver<InboundEnvelope>,
    ack_tx: mpsc::UnboundedSender<AckEntry>,
    /// Latched before the loop exits on `ServerMsg::RemoveRunner` so
    /// background tasks can stop even if they were not already blocked
    /// on the signal.
    remove_tx: tokio::sync::watch::Sender<bool>,
    teardown_complete_tx: watch::Sender<RunnerTeardownOutcome>,
    /// Client refresh uses a detached single-flight task which can still be
    /// rotating credentials after its caller is cancelled.  Teardown waits
    /// for that writer to go idle before deleting this runner's runtime tree.
    client: Option<RunnerCloudClient>,
    /// Local IPC chat is a separate execution lane from `current_chat`.
    /// Closing this registry prevents new sessions and joins/shuts down every
    /// bridge before runtime/config removal completes.
    local_chat_sessions: crate::ipc::chat::ChatRegistry,
    live_mailboxes: Arc<RwLock<HashMap<uuid::Uuid, mpsc::Sender<InboundEnvelope>>>>,
    live_hello_runners: Arc<RwLock<HelloRunnerMap>>,
    live_instances: Arc<std::sync::RwLock<HashMap<uuid::Uuid, RunnerInstance>>>,
    /// In-flight run, if any. Replaced on each Assign and cleared as soon as
    /// the worker task signals completion via `done_rx` — driven by
    /// `tokio::select!` so a new Assign isn't rejected while we wait for the
    /// next inbound frame.
    current_run: Option<CurrentRun>,
    current_chat: Option<CurrentChat>,
    /// Shared with this runner's `RunnerInstance` (and therefore the IPC
    /// server): `true` while a *local* chat turn (PDASHOSS01-159, driven over
    /// the daemon IPC socket, not this cloud loop) holds the working copy. The
    /// assign lane consults it so a managed run and a local chat never modify
    /// the same working copy at the same time (issue AC6).
    local_chat_active: Arc<std::sync::atomic::AtomicBool>,
}

struct CurrentRun {
    run_id: uuid::Uuid,
    cancel: tokio_util::sync::CancellationToken,
    done_rx: oneshot::Receiver<()>,
}

struct CurrentChat {
    chat_session_id: uuid::Uuid,
    tx: mpsc::Sender<ChatCommand>,
    active_rx: watch::Receiver<bool>,
    done_rx: oneshot::Receiver<()>,
}

async fn acknowledge_cancel_without_active_process(out: &RunnerOut, run_id: uuid::Uuid) {
    if let Err(err) = out
        .send(ClientMsg::RunCancelled {
            run_id,
            cancelled_at: Utc::now(),
            tokens: None,
            model: None,
        })
        .await
    {
        tracing::warn!(
            %run_id,
            error = %err,
            "failed to acknowledge cancel for a run with no active process"
        );
    }
}

#[derive(Debug)]
struct ChatTurn {
    message_id: uuid::Uuid,
    content: String,
    cwd: Option<String>,
    model: Option<String>,
    local_thread_id: Option<String>,
    local_session_id: Option<String>,
}

#[derive(Debug)]
struct ChatWarm {
    cwd: Option<String>,
    model: Option<String>,
    local_thread_id: Option<String>,
    local_session_id: Option<String>,
}

#[derive(Debug)]
enum ChatCommand {
    Warm(ChatWarm),
    Message(ChatTurn),
    Cancel {
        reason: Option<String>,
    },
    Close {
        reason: Option<String>,
    },
    /// Tear down this runtime to make room for another session (or to free the
    /// runner's working dir for an assign) WITHOUT closing the
    /// cloud session: it stays open and a later warm/message revives it via
    /// `local_thread_id`. Contrast with `Shutdown`, which emits a terminal
    /// `ChatClosed`.
    Release,
    Shutdown,
}

impl RunnerLoop {
    /// NACK an `Assign` this runner cannot take right now. The cloud frees a
    /// runner the moment its previous run goes terminal in the database,
    /// which can precede the local agent actually stopping (most commonly a
    /// user cancel) — so the matcher may dispatch the next run to a runner
    /// that is still busy. Silently dropping that Assign leaves the run
    /// ASSIGNED forever: the runner never starts it and the cloud shows the
    /// runner busy with nothing running. `RunFailed{AssignRejectedBusy}`
    /// lets the cloud re-queue it for a fresh dispatch.
    ///
    /// Sent from a detached task so a slow cloud cannot stall the message
    /// loop. Delivery failure is logged only — the session-open redeliver
    /// path remains the backstop for a lost NACK.
    fn nack_assign(&self, run_id: uuid::Uuid, detail: String) {
        let out = self.out.clone();
        tokio::spawn(async move {
            let msg = ClientMsg::RunFailed {
                run_id,
                reason: FailureReason::AssignRejectedBusy,
                detail: Some(detail),
                ended_at: Utc::now(),
                tokens: None,
                model: None,
            };
            if let Err(e) = out.send(msg).await {
                tracing::warn!(%run_id, "failed to NACK rejected assign: {e:#}");
            }
        });
    }

    async fn stop_idle_chat_runtime(&mut self) {
        let Some(mut chat) = self.current_chat.take() else {
            return;
        };
        // Release, not Shutdown: swapping the runtime to another session must
        // not close the old session in the cloud — the user may switch back to
        // it from the chat history panel, and a warm/message revives it.
        let _ = chat.tx.send(ChatCommand::Release).await;
        let _ = tokio::time::timeout(Duration::from_secs(5), &mut chat.done_rx).await;
    }

    async fn start_chat_runtime(
        &mut self,
        chat_session_id: uuid::Uuid,
    ) -> mpsc::Sender<ChatCommand> {
        let (tx, rx) = mpsc::channel(8);
        let (active_tx, active_rx) = watch::channel(false);
        let (done_tx, done_rx) = oneshot::channel();
        self.current_chat = Some(CurrentChat {
            chat_session_id,
            tx: tx.clone(),
            active_rx,
            done_rx,
        });
        let mut worker = ChatWorker {
            runner_config: self.runner_config.clone(),
            state: self.state.clone(),
            approvals: self.approvals.clone(),
            out: self.out.clone(),
            command_rx: rx,
            active_tx,
        };
        tokio::spawn(async move {
            worker.run(chat_session_id).await;
            let _ = done_tx.send(());
        });
        tx
    }

    async fn run(mut self) -> Result<()> {
        loop {
            let frame = {
                let inbound = self.inbound.recv();
                tokio::pin!(inbound);
                // `done_rx` exists only while a run is in flight; outside of that
                // window we wait on `pending()` so the select arm is inert.
                tokio::select! {
                    biased;
                    () = wait_done(&mut self.current_run) => {
                        self.current_run = None;
                        continue;
                    }
                    () = wait_chat_done(&mut self.current_chat) => {
                        self.current_chat = None;
                        continue;
                    }
                    f = &mut inbound => f,
                }
            };
            let Some(frame) = frame else { break };
            let mut stream_id = frame.stream_id.clone();
            let frame = frame.env;
            let mut should_break = false;

            match frame.body {
                ServerMsg::Welcome {
                    protocol_version,
                    heartbeat_interval_secs,
                    latest_runner_version,
                    min_runner_version,
                    ..
                } => {
                    if protocol_version != WIRE_VERSION {
                        tracing::warn!(
                            server = protocol_version,
                            local = WIRE_VERSION,
                            "protocol version mismatch",
                        );
                    }
                    if heartbeat_interval_secs > 0 {
                        let _ = self.state.tx_heartbeat_secs.send(heartbeat_interval_secs);
                    }
                    if latest_runner_version.is_some() || min_runner_version.is_some() {
                        tracing::info!(
                            latest = ?latest_runner_version,
                            min = ?min_runner_version,
                            "received runner version advisory from cloud",
                        );
                    }
                    self.state
                        .set_update_advisory(latest_runner_version.clone(), min_runner_version)
                        .await;
                    self.state.set_connected(true).await;

                    // Auto-update path: when the user has opted in, the
                    // cloud is announcing a newer version, no swap is
                    // already in flight, AND the on-disk binary is not
                    // already at the announced version, spawn a
                    // background task to swap. We deliberately do NOT
                    // restart the running daemon — it keeps its loaded
                    // copy until the next natural restart. See
                    // `runner/README.md` (Auto-update).
                    if let Some(latest) = latest_runner_version.as_deref()
                        && version_lt(crate::RUNNER_VERSION, latest)
                        && self.state.on_disk_version().await.as_deref() != Some(latest)
                        && self.state.auto_update_enabled().await
                        && let Some(swap_guard) = self.state.try_claim_swap()
                    {
                        let st = self.state.clone();
                        let latest_owned = latest.to_string();
                        let paths = self.paths.clone();
                        tokio::spawn(async move {
                            let _swap_guard = swap_guard;
                            tracing::info!(
                                target = %latest_owned,
                                "auto-update: swapping karma-team on disk",
                            );
                            match crate::cli::update::check_or_swap(false, &paths).await {
                                Ok(crate::cli::update::SwapOutcome::Swapped {
                                    new_version,
                                    ..
                                }) => {
                                    tracing::info!(
                                        %new_version,
                                        "auto-update: swap complete; restart to apply",
                                    );
                                    st.set_on_disk_version(new_version).await;
                                }
                                Ok(other) => {
                                    tracing::debug!(?other, "auto-update: no swap performed",);
                                }
                                Err(e) => {
                                    // Can be no-receipt (source build), a
                                    // GitHub network/rate-limit error, or a
                                    // failed installer run. Logged at info so
                                    // operators can see the cause without
                                    // editorialising about which it is.
                                    tracing::info!(
                                        error = %e,
                                        "auto-update: swap failed",
                                    );
                                }
                            }
                        });
                    }
                }
                ServerMsg::Assign {
                    run_id,
                    work_item_identifier,
                    project_name,
                    prompt,
                    repo_url,
                    repo_ref,
                    git_work_branch,
                    expected_codex_model,
                    ..
                } => {
                    // Cross-lane exclusion: a runner has one working dir shared
                    // between chat and issue execution, so the two cannot run
                    // concurrently. A concurrent chat means a second runner, not
                    // a parallel lane inside this one (PDASHOSS01-133). Reject an
                    // assign while a chat turn is active; tear an idle chat
                    // runtime down to free the dir.
                    let cloud_chat_active = self
                        .current_chat
                        .as_ref()
                        .is_some_and(|chat| *chat.active_rx.borrow());
                    // A *local* chat turn (driven over the daemon IPC socket,
                    // PDASHOSS01-159) holds the same working copy but lives in
                    // a different actor, so it is invisible to `current_chat` —
                    // consult the shared flag too (issue AC6).
                    let local_chat_active = self
                        .local_chat_active
                        .load(std::sync::atomic::Ordering::SeqCst);
                    if cloud_chat_active || local_chat_active {
                        tracing::warn!(
                            %run_id,
                            cloud_chat_active,
                            local_chat_active,
                            "assign received while chat is active on this runner; rejecting"
                        );
                        // NACK rather than silently dropping, so the cloud
                        // re-queues the run instead of leaving it stuck
                        // ASSIGNED forever (#246).
                        self.nack_assign(
                            run_id,
                            "assign rejected: a chat turn is active on this runner".to_string(),
                        );
                        continue;
                    }
                    if self.current_chat.is_some() {
                        self.stop_idle_chat_runtime().await;
                    }
                    // Intra-assign-lane guard — one issue in flight at a time —
                    // is kept for ALL runners (the assign lane stays
                    // single-tenant; the cloud reaper assumes it, design §3.4).
                    if let Some(current) = &self.current_run {
                        tracing::warn!(
                            %run_id,
                            in_flight = %current.run_id,
                            "assign received while a run is already in flight; rejecting"
                        );
                        self.nack_assign(
                            run_id,
                            format!(
                                "assign rejected: run {} is still in flight on this runner",
                                current.run_id
                            ),
                        );
                        continue;
                    }
                    let cancel = tokio_util::sync::CancellationToken::new();
                    let (done_tx, done_rx) = oneshot::channel();
                    self.current_run = Some(CurrentRun {
                        run_id,
                        cancel: cancel.clone(),
                        done_rx,
                    });
                    // Stamp ``rx_in_flight = Some(run_id)`` synchronously, before
                    // the worker does anything slow. The cloud put this run
                    // into a BUSY status the moment it sent the Assign;
                    // ``reap_stale_busy_runs`` (services/session_service.py)
                    // will fail any BUSY run whose runner Hellos with
                    // ``in_flight_run=null``. Without this early stamp, a
                    // session reconnect during workspace setup (which can
                    // take 30s+ for a fresh clone) reports null truthfully
                    // and the run gets reaped before it ever starts.
                    // The worker re-stamps a fuller summary at the
                    // ``set_current_run`` site below; same run_id, so the
                    // watch channel doesn't toggle in/out of None.
                    self.state
                        .set_current_run(Some(CurrentRunSummary {
                            run_id,
                            thread_id: None,
                            status: "preparing".to_string(),
                            started_at: Utc::now(),
                            events: 0,
                        }))
                        .await;
                    let runner_paths = self.runner_paths.clone();
                    let daemon_paths = self.paths.clone();
                    let runner_config = self.runner_config.clone();
                    let state = self.state.clone();
                    let approvals = self.approvals.clone();
                    let out = self.out.clone();
                    tokio::spawn(async move {
                        let mut worker = AssignWorker {
                            runner_paths,
                            daemon_paths,
                            runner_config,
                            state,
                            approvals,
                            out,
                            cancel,
                        };
                        if let Err(e) = worker
                            .run(AssignedRun {
                                run_id,
                                issue_identifier: work_item_identifier,
                                project_name,
                                prompt,
                                repo_url,
                                repo_ref,
                                git_work_branch,
                                expected_codex_model,
                            })
                            .await
                        {
                            tracing::error!("run {run_id} failed: {e:#}");
                            let _ = worker
                                .out
                                .send(ClientMsg::RunFailed {
                                    run_id,
                                    reason: FailureReason::Internal,
                                    detail: Some(format!("{e:#}")),
                                    ended_at: Utc::now(),
                                    tokens: None,
                                    model: None,
                                })
                                .await;
                            worker.state.set_current_run(None).await;
                        }
                        let _ = done_tx.send(());
                    });
                }
                ServerMsg::Cancel { run_id, reason } => {
                    tracing::info!(%run_id, ?reason, "cancel received");
                    if let Some(run) = &self.current_run {
                        if run.run_id == run_id {
                            run.cancel.cancel();
                        } else {
                            tracing::warn!(
                                "cancel for run {run_id} but active run is {active}; acknowledging stopped target",
                                active = run.run_id,
                            );
                            acknowledge_cancel_without_active_process(&self.out, run_id).await;
                        }
                    } else {
                        // The cloud may cancel an ASSIGNED run before its
                        // Assign frame reaches this daemon, or redeliver a
                        // persisted cancellation after a restart. With no
                        // matching local worker there is no process to stop,
                        // so acknowledge immediately instead of waiting for
                        // heartbeat reconciliation.
                        acknowledge_cancel_without_active_process(&self.out, run_id).await;
                    }
                }
                ServerMsg::Decide {
                    approval_id,
                    decision,
                    ..
                } => {
                    self.approvals
                        .decide(&approval_id.to_string(), decision, DecisionSource::Cloud)
                        .await;
                    self.state.note_approvals_changed();
                }
                ServerMsg::ChatUserMessage {
                    chat_session_id,
                    message_id,
                    content,
                    content_parts: _,
                    local_thread_id,
                    local_session_id,
                    cwd,
                    model,
                } => {
                    // One working dir per runner: chat cannot run during an issue
                    // run. A concurrent chat means a second runner.
                    if self.current_run.is_some() {
                        let _ = self
                            .out
                            .send(ClientMsg::ChatFailed {
                                chat_session_id,
                                code: "runner_busy".into(),
                                detail: Some("runner has an active task".into()),
                                failed_at: Utc::now(),
                            })
                            .await;
                        continue;
                    }
                    let turn = ChatTurn {
                        message_id,
                        content,
                        cwd,
                        model,
                        local_thread_id,
                        local_session_id,
                    };
                    let tx = if let Some(chat) = &self.current_chat {
                        if chat.chat_session_id == chat_session_id {
                            if *chat.active_rx.borrow() {
                                let _ = self
                                    .out
                                    .send(ClientMsg::ChatFailed {
                                        chat_session_id,
                                        code: "chat_turn_active".into(),
                                        detail: Some("runner has an active chat turn".into()),
                                        failed_at: Utc::now(),
                                    })
                                    .await;
                                continue;
                            }
                            chat.tx.clone()
                        } else if *chat.active_rx.borrow() {
                            let _ = self
                                .out
                                .send(ClientMsg::ChatFailed {
                                    chat_session_id,
                                    code: "chat_turn_active".into(),
                                    detail: Some("runner has an active chat turn".into()),
                                    failed_at: Utc::now(),
                                })
                                .await;
                            continue;
                        } else {
                            self.stop_idle_chat_runtime().await;
                            self.start_chat_runtime(chat_session_id).await
                        }
                    } else {
                        self.start_chat_runtime(chat_session_id).await
                    };
                    if tx.send(ChatCommand::Message(turn)).await.is_err() {
                        let _ = self
                            .out
                            .send(ClientMsg::ChatFailed {
                                chat_session_id,
                                code: "chat_runtime_closed".into(),
                                detail: Some("chat runtime closed before accepting message".into()),
                                failed_at: Utc::now(),
                            })
                            .await;
                    }
                }
                ServerMsg::ChatWarm {
                    chat_session_id,
                    local_thread_id,
                    local_session_id,
                    cwd,
                    model,
                } => {
                    // One working dir per runner: skip warm during an issue run.
                    if self.current_run.is_some() {
                        let _ = self
                            .out
                            .send(ClientMsg::ChatEvent {
                                chat_session_id,
                                bridge_seq: 0,
                                kind: "chat_warm_skipped".into(),
                                payload: serde_json::json!({
                                    "reason": "runner_busy",
                                }),
                            })
                            .await;
                        continue;
                    }
                    let warm = ChatWarm {
                        cwd,
                        model,
                        local_thread_id,
                        local_session_id,
                    };
                    let tx = if let Some(chat) = &self.current_chat {
                        if chat.chat_session_id == chat_session_id {
                            if *chat.active_rx.borrow() {
                                let _ = self
                                    .out
                                    .send(ClientMsg::ChatEvent {
                                        chat_session_id,
                                        bridge_seq: 0,
                                        kind: "chat_warm_skipped".into(),
                                        payload: serde_json::json!({
                                            "reason": "chat_turn_active",
                                        }),
                                    })
                                    .await;
                                continue;
                            }
                            chat.tx.clone()
                        } else if *chat.active_rx.borrow() {
                            let _ = self
                                .out
                                .send(ClientMsg::ChatEvent {
                                    chat_session_id,
                                    bridge_seq: 0,
                                    kind: "chat_warm_skipped".into(),
                                    payload: serde_json::json!({
                                        "reason": "chat_turn_active",
                                    }),
                                })
                                .await;
                            continue;
                        } else {
                            self.stop_idle_chat_runtime().await;
                            self.start_chat_runtime(chat_session_id).await
                        }
                    } else {
                        self.start_chat_runtime(chat_session_id).await
                    };
                    if tx.send(ChatCommand::Warm(warm)).await.is_err() {
                        let _ = self
                            .out
                            .send(ClientMsg::ChatEvent {
                                chat_session_id,
                                bridge_seq: 0,
                                kind: "chat_warm_failed".into(),
                                payload: serde_json::json!({
                                    "detail": "chat runtime closed before accepting warm request",
                                }),
                            })
                            .await;
                    }
                }
                ServerMsg::ChatCancel {
                    chat_session_id,
                    reason,
                } => {
                    tracing::info!(%chat_session_id, ?reason, "chat_cancel received");
                    match &self.current_chat {
                        Some(chat) if chat.chat_session_id == chat_session_id => {
                            let _ = chat.tx.send(ChatCommand::Cancel { reason }).await;
                        }
                        _ => {}
                    }
                }
                ServerMsg::ChatClose {
                    chat_session_id,
                    reason,
                } => {
                    tracing::info!(%chat_session_id, ?reason, "chat_close received");
                    if let Some(chat) = &self.current_chat {
                        if chat.chat_session_id == chat_session_id {
                            let _ = chat.tx.send(ChatCommand::Close { reason }).await;
                        }
                    } else {
                        let _ = self
                            .out
                            .send(ClientMsg::ChatClosed {
                                chat_session_id,
                                closed_at: Utc::now(),
                            })
                            .await;
                    }
                }
                ServerMsg::ChatDecide {
                    local_approval_id,
                    decision,
                    ..
                } => {
                    self.approvals
                        .decide(&local_approval_id, decision, DecisionSource::Cloud)
                        .await;
                    self.state.note_approvals_changed();
                }
                ServerMsg::ConfigPush { .. } => {
                    tracing::info!("config_push received (deferred)");
                }
                ServerMsg::Ping { .. } => {
                    // Connection-scoped — handled by the supervisor's
                    // demux task before frames reach the per-runner
                    // mailbox. Should never fire here; if it does, the
                    // demux routing rule was violated.
                    tracing::warn!("Ping arrived at RunnerLoop; demux invariant violated");
                }
                ServerMsg::ResumeAck {
                    run_id,
                    last_seq,
                    status,
                    ..
                } => {
                    tracing::info!(
                        %run_id,
                        ?last_seq,
                        %status,
                        "cloud acked run resume"
                    );
                }
                ServerMsg::Revoke { .. } => {
                    // Per-runner revoke (HTTP transport). The HttpLoop
                    // already triggered shutdown for this runner, but
                    // we may receive the frame as a redelivery.
                    tracing::warn!("runner revoke received; loop will exit");
                    should_break = true;
                }
                ServerMsg::ForceRefresh { .. } => {
                    // Handled inline in HttpLoop. If we receive it
                    // here it's a bug or a redelivery; just log.
                    tracing::debug!(
                        "ForceRefresh arrived at RunnerLoop; HttpLoop should have handled it"
                    );
                }
                ServerMsg::RemoveRunner { runner_id, reason } => {
                    // Per-instance teardown: exit ONLY this RunnerLoop;
                    // the WS connection and other RunnerInstances stay
                    // up. The demux already routed by envelope.runner_id
                    // before we got here, so the rid-mismatch check
                    // below is defensive.
                    if runner_id != self.runner_paths.runner_id {
                        tracing::warn!(
                            "received RemoveRunner for {runner_id}, but this loop is \
                             {}; ignoring",
                            self.runner_paths.runner_id,
                        );
                        continue;
                    }
                    tracing::warn!(
                        "cloud removed runner {runner_id}: {}",
                        reason.as_deref().unwrap_or("(no reason)"),
                    );
                    // Tell the heartbeat task to exit before we drop
                    // out of the loop. Without this it would keep
                    // emitting frames carrying this runner's id and
                    // the cloud would drop each one with an
                    // `unknown rid` warning until the daemon restarts.
                    // This is a teardown latch, not a best-effort notification.
                    // `send_replace` records the value even for offline runners
                    // that currently have no HTTP/refresh watch receiver.
                    self.remove_tx.send_replace(true);
                    self.live_mailboxes
                        .write()
                        .await
                        .remove(&self.runner_paths.runner_id);
                    self.live_hello_runners
                        .write()
                        .await
                        .remove(&self.runner_paths.runner_id);
                    let target_runner_id = self.runner_paths.runner_id;
                    let runner_name = self.runner_config.name.clone();
                    let teardown_result = async {
                        // Capture the persisted path before cleanup so legacy
                        // working copies nested below the runtime root remain
                        // protected.  If config cannot be read, fail closed:
                        // deleting first could destroy an unknown checkout.
                        let teardown_working_dir =
                            load_configured_runner_working_dir(&self.paths, target_runner_id)
                                .await?
                                .unwrap_or_else(|| {
                                    self.runner_config.workspace.working_dir.clone()
                                });

                        // Stop both detached resource owners before removing
                        // their filesystem root.  HTTP/refresh observe the
                        // latched remove signal above; an in-flight refresh is
                        // explicitly joined because it may rotate credentials
                        // from a spawned single-flight task.  Local IPC chat
                        // has its own registry and must be closed separately
                        // from the cloud `current_chat` lane.
                        if let Some(client) = &self.client {
                            client.wait_for_refresh_idle().await;
                        }
                        self.local_chat_sessions.shutdown_all().await?;

                        // Cancel both execution lanes and wait for their final
                        // persistence before deleting runtime data.  Only
                        // after that succeeds do we remove the discoverable
                        // config entry; a crash in either earlier phase leaves
                        // enough state for ProjectSync to retry.
                        finish_runner_teardown(
                            &self.paths,
                            target_runner_id,
                            Some(&teardown_working_dir),
                            &mut self.current_run,
                            &mut self.current_chat,
                        )
                        .await?;
                        strip_runner_config(&self.paths, target_runner_id).await
                    }
                    .await;

                    // Keep the instance registered while teardown is in
                    // progress. Concurrent/repeated remove calls then find the
                    // live owner and wait on the same completion outcome,
                    // instead of racing it with independent cleanup.
                    self.live_instances
                        .write()
                        .expect("runner registry poisoned")
                        .remove(&target_runner_id);
                    match teardown_result {
                        Ok(post) => {
                            tracing::info!(
                                runner = %runner_name,
                                runner_id = %target_runner_id,
                                remaining = post.runners.len(),
                                "runner teardown completed and config entry was removed",
                            );
                            if post.runners.is_empty() {
                                tracing::warn!(
                                    "config.toml has no remaining runners; \
                                     run `karma-team uninstall` to remove the \
                                     karma-team.service systemd unit if you \
                                     no longer need the daemon on this host.",
                                );
                            }
                            self.teardown_complete_tx
                                .send_replace(RunnerTeardownOutcome::Succeeded);
                        }
                        Err(error) => {
                            let detail = format!("{error:#}");
                            tracing::error!(
                                runner = %runner_name,
                                runner_id = %target_runner_id,
                                %detail,
                                "runner teardown failed; config retained for retry",
                            );
                            self.teardown_complete_tx
                                .send_replace(RunnerTeardownOutcome::Failed(detail));
                            // At-least-once runner streams must not ACK a
                            // teardown that failed before config removal.
                            stream_id = None;
                        }
                    }
                    should_break = true;
                }
            }
            if let Some(stream_id) = stream_id {
                let _ = self.ack_tx.send(AckEntry { stream_id });
            }
            if should_break {
                break;
            }
        }
        self.state.set_connected(false).await;
        Ok(())
    }
}

async fn wait_done(current: &mut Option<CurrentRun>) {
    match current {
        Some(run) => {
            let _ = (&mut run.done_rx).await;
        }
        None => std::future::pending().await,
    }
}

async fn wait_chat_done(current: &mut Option<CurrentChat>) {
    match current {
        Some(chat) => {
            let _ = (&mut chat.done_rx).await;
        }
        None => std::future::pending().await,
    }
}

/// Stop runner-owned workers and remove their runtime tree only after every
/// worker has completed its final persistence step. `AssignWorker` writes the
/// runs index while unwinding cancellation, so deleting first would allow that
/// final save to recreate `data/runners/<runner_id>` after a successful
/// RemoveRunner operation.
async fn finish_runner_teardown(
    paths: &Paths,
    runner_id: uuid::Uuid,
    working_dir: Option<&std::path::Path>,
    current_run: &mut Option<CurrentRun>,
    current_chat: &mut Option<CurrentChat>,
) -> Result<()> {
    let mut run = current_run.take();
    if let Some(run) = &run {
        run.cancel.cancel();
    }

    let mut chat = current_chat.take();
    if let Some(chat) = &chat {
        let _ = chat.tx.send(ChatCommand::Shutdown).await;
    }

    if let Some(run) = &mut run {
        let _ = (&mut run.done_rx).await;
    }
    if let Some(chat) = &mut chat {
        let _ = (&mut chat.done_rx).await;
    }

    remove_runner_data_dir_preserving(paths, runner_id, working_dir)
}

fn chat_resume_id(local_session_id: Option<&str>, local_thread_id: Option<&str>) -> Option<String> {
    local_session_id
        .filter(|s| !s.is_empty())
        .or_else(|| local_thread_id.filter(|s| !s.is_empty()))
        .map(ToOwned::to_owned)
}

fn bridge_has_exited(bridge: &AgentBridge) -> bool {
    let handle = bridge.process_handle();
    handle.exit_rx.borrow().is_some()
}

fn elapsed_ms(start: Instant) -> u64 {
    start.elapsed().as_millis().try_into().unwrap_or(u64::MAX)
}

fn bridge_event_label(ev: &BridgeEvent) -> &'static str {
    match ev {
        BridgeEvent::RunStarted { .. } => "run_started",
        BridgeEvent::Raw { .. } => "raw",
        BridgeEvent::ApprovalRequest { .. } => "approval_request",
        BridgeEvent::AwaitingReauth { .. } => "awaiting_reauth",
        BridgeEvent::Completed { .. } => "completed",
        BridgeEvent::Failed { .. } => "failed",
    }
}

fn assistant_delta_text(params: &serde_json::Value) -> Option<&str> {
    if let Some(delta) = params.get("delta") {
        if let Some(text) = delta.as_str() {
            return Some(text);
        }
        if let Some(text) = delta.get("text").and_then(|value| value.as_str()) {
            return Some(text);
        }
    }
    params.get("text").and_then(|value| value.as_str())
}

fn is_assistant_text_delta(method: &str, params: &serde_json::Value) -> bool {
    if method == "item/agentMessage/delta" {
        return assistant_delta_text(params).is_some();
    }
    method == "stream_event/content_block_delta"
        && params
            .get("delta")
            .and_then(|delta| delta.get("type"))
            .and_then(|value| value.as_str())
            == Some("text_delta")
        && params
            .get("delta")
            .and_then(|delta| delta.get("text"))
            .and_then(|value| value.as_str())
            .is_some()
}

fn timing_payload(stage: &str, mut payload: serde_json::Value) -> serde_json::Value {
    if !payload.is_object() {
        payload = serde_json::json!({});
    }
    if let Some(obj) = payload.as_object_mut() {
        obj.insert("stage".into(), serde_json::json!(stage));
        obj.insert("runner_recorded_at".into(), serde_json::json!(Utc::now()));
    }
    payload
}

struct ChatWorker {
    runner_config: crate::config::schema::RunnerConfig,
    state: StateHandle,
    approvals: ApprovalRouter,
    out: RunnerOut,
    command_rx: mpsc::Receiver<ChatCommand>,
    active_tx: watch::Sender<bool>,
}

impl ChatWorker {
    async fn run(&mut self, chat_session_id: uuid::Uuid) {
        // A session's runtime idles out after 5 minutes of silence and the
        // worker exits. Chat runs in the runner's one working directory and
        // performs no git lifecycle of its own — the agent drives git per the
        // prompt (PDASHOSS01-133). Reviving (a new message for the same session
        // id) spawns a fresh worker in the same directory.
        const CHAT_IDLE_TIMEOUT: Duration = Duration::from_secs(5 * 60);

        let mut bridge: Option<AgentBridge> = None;
        let mut workspace_path: Option<std::path::PathBuf> = None;
        let mut bridge_seq = 0u64;
        let mut started_sent = false;

        // B4: the chat lane gets its own graceful-shutdown signal. Before this,
        // only the assign lane observed `shutdown_notified()`, so on daemon
        // shutdown / RemoveRunner the chat loop was aborted with no terminal
        // `ChatClosed`. A turn already in flight finishes first.
        //
        // The `Notified` future is created ONCE and pinned across iterations:
        // `notify_waiters()` does not store a permit, so a fresh future created
        // per loop turn would miss a signal that landed while we were inside a
        // turn (it would then hang until the idle timeout). A pinned future
        // stays registered as a waiter and catches the signal on the next poll.
        let shutdown = self.state.shutdown_notified();
        let notified = shutdown.notified();
        tokio::pin!(notified);
        loop {
            let command = tokio::select! {
                biased;
                _ = &mut notified => {
                    let _ = self
                        .out
                        .send(ClientMsg::ChatClosed {
                            chat_session_id,
                            closed_at: Utc::now(),
                        })
                        .await;
                    break;
                }
                recv = tokio::time::timeout(CHAT_IDLE_TIMEOUT, self.command_rx.recv()) => {
                    match recv {
                        Ok(Some(command)) => command,
                        Ok(None) | Err(_) => break,
                    }
                }
            };
            match command {
                ChatCommand::Warm(warm) => {
                    if let Err(e) = self
                        .handle_warm(
                            chat_session_id,
                            warm,
                            &mut bridge,
                            &mut workspace_path,
                            &mut bridge_seq,
                            &mut started_sent,
                        )
                        .await
                    {
                        let _ = self
                            .out
                            .send(ClientMsg::ChatEvent {
                                chat_session_id,
                                bridge_seq,
                                kind: "chat_warm_failed".into(),
                                payload: serde_json::json!({
                                    "detail": format!("{e:#}"),
                                }),
                            })
                            .await;
                        if let Some(bridge) = bridge.take() {
                            bridge.shutdown(Duration::from_secs(5)).await.ok();
                        }
                        workspace_path.take();
                        started_sent = false;
                    }
                }
                ChatCommand::Message(turn) => {
                    let _ = self.active_tx.send(true);
                    self.state.set_chat_active(true).await;
                    let close_runtime = match self
                        .handle_turn(
                            chat_session_id,
                            turn,
                            &mut bridge,
                            &mut workspace_path,
                            &mut bridge_seq,
                            &mut started_sent,
                        )
                        .await
                    {
                        Ok(close_runtime) => close_runtime,
                        Err(e) => {
                            let _ = self
                                .out
                                .send(ClientMsg::ChatFailed {
                                    chat_session_id,
                                    code: "internal".into(),
                                    detail: Some(format!("{e:#}")),
                                    failed_at: Utc::now(),
                                })
                                .await;
                            if let Some(bridge) = bridge.take() {
                                bridge.shutdown(Duration::from_secs(5)).await.ok();
                            }
                            started_sent = false;
                            false
                        }
                    };
                    let _ = self.active_tx.send(false);
                    self.state.set_chat_active(false).await;
                    if close_runtime {
                        break;
                    }
                }
                ChatCommand::Cancel { reason } => {
                    tracing::debug!(?reason, %chat_session_id, "chat cancel received while idle");
                }
                ChatCommand::Close { reason } => {
                    tracing::debug!(?reason, %chat_session_id, "chat close received while idle");
                    let _ = self
                        .out
                        .send(ClientMsg::ChatClosed {
                            chat_session_id,
                            closed_at: Utc::now(),
                        })
                        .await;
                    break;
                }
                ChatCommand::Release => {
                    // Runtime swap (see `stop_idle_chat_runtime`). No terminal
                    // `ChatClosed` — the cloud session stays open for revival.
                    // The post-loop persist below still commits + pushes.
                    break;
                }
                ChatCommand::Shutdown => {
                    // RemoveRunner / teardown. Emit a terminal `ChatClosed` so
                    // the cloud session doesn't linger in "active turn" state.
                    let _ = self
                        .out
                        .send(ClientMsg::ChatClosed {
                            chat_session_id,
                            closed_at: Utc::now(),
                        })
                        .await;
                    break;
                }
            }
        }
        if let Some(bridge) = bridge.take() {
            bridge.shutdown(Duration::from_secs(5)).await.ok();
        }
        let _ = self.active_tx.send(false);
        self.state.set_chat_active(false).await;
    }

    async fn send_timing_event(
        &self,
        chat_session_id: uuid::Uuid,
        bridge_seq: &mut u64,
        stage: &str,
        payload: serde_json::Value,
    ) {
        *bridge_seq = (*bridge_seq).saturating_add(1);
        let out = self.out.clone();
        let event = ClientMsg::ChatEvent {
            chat_session_id,
            bridge_seq: *bridge_seq,
            kind: "chat_timing".into(),
            payload: timing_payload(stage, payload),
        };
        tokio::spawn(async move {
            match tokio::time::timeout(Duration::from_secs(2), out.send(event)).await {
                Ok(Ok(())) => {}
                Ok(Err(e)) => {
                    tracing::debug!(error = %e, "chat timing event post failed");
                }
                Err(_) => {
                    tracing::debug!("chat timing event post timed out");
                }
            }
        });
    }

    fn send_chat_started_event(&self, chat_session_id: uuid::Uuid, thread_id: String) {
        let out = self.out.clone();
        tokio::spawn(async move {
            if let Err(e) = out
                .send(ClientMsg::ChatStarted {
                    chat_session_id,
                    local_thread_id: thread_id.clone(),
                    local_session_id: Some(thread_id),
                    started_at: Utc::now(),
                })
                .await
            {
                tracing::debug!(error = %e, "chat started event post failed");
            }
        });
    }

    async fn handle_warm(
        &mut self,
        chat_session_id: uuid::Uuid,
        warm: ChatWarm,
        bridge: &mut Option<AgentBridge>,
        workspace_path: &mut Option<std::path::PathBuf>,
        bridge_seq: &mut u64,
        started_sent: &mut bool,
    ) -> Result<()> {
        self.send_timing_event(
            chat_session_id,
            bridge_seq,
            "runner_warm_received",
            serde_json::json!({
                "local_session_id": warm.local_session_id.as_deref(),
                "local_thread_id": warm.local_thread_id.as_deref(),
            }),
        )
        .await;

        if workspace_path.is_none() {
            *workspace_path = Some(
                self.resolve_chat_workspace(chat_session_id, warm.cwd.as_deref())
                    .await?,
            );
        }
        let workspace_path = workspace_path
            .as_deref()
            .ok_or_else(|| anyhow::anyhow!("chat workspace missing"))?;

        if bridge.as_ref().is_some_and(bridge_has_exited) {
            if let Some(bridge) = bridge.take() {
                bridge.shutdown(Duration::from_secs(1)).await.ok();
            }
            *started_sent = false;
        }

        let already_warm = bridge.is_some();
        let resume_id = chat_resume_id(
            warm.local_session_id.as_deref(),
            warm.local_thread_id.as_deref(),
        );
        if bridge.is_none() {
            let spawn_started = Instant::now();
            *bridge = Some(
                AgentBridge::spawn_from_config_with_resume(
                    &self.runner_config,
                    workspace_path,
                    warm.model,
                    resume_id.as_deref(),
                    None,
                )
                .await?,
            );
            self.send_timing_event(
                chat_session_id,
                bridge_seq,
                "bridge_spawned",
                serde_json::json!({
                    "operation": "warm",
                    "duration_ms": elapsed_ms(spawn_started),
                    "resume_id_present": resume_id.is_some(),
                }),
            )
            .await;
        } else {
            self.send_timing_event(
                chat_session_id,
                bridge_seq,
                "bridge_reused",
                serde_json::json!({
                    "operation": "warm",
                }),
            )
            .await;
        }
        let bridge = bridge
            .as_mut()
            .ok_or_else(|| anyhow::anyhow!("chat bridge missing"))?;
        let warm_started = Instant::now();
        let warmed_thread_id = bridge.warm(workspace_path).await?;
        self.send_timing_event(
            chat_session_id,
            bridge_seq,
            "bridge_warmed",
            serde_json::json!({
                "operation": "warm",
                "duration_ms": elapsed_ms(warm_started),
                "already_warm": already_warm,
                "local_session_id": warmed_thread_id.as_deref(),
            }),
        )
        .await;
        match warmed_thread_id.as_ref() {
            Some(thread_id) if !*started_sent => {
                self.send_chat_started_event(chat_session_id, thread_id.clone());
                *started_sent = true;
            }
            _ => {}
        }

        *bridge_seq = (*bridge_seq).saturating_add(1);
        self.out
            .send(ClientMsg::ChatEvent {
                chat_session_id,
                bridge_seq: *bridge_seq,
                kind: "chat_warmed".into(),
                payload: serde_json::json!({
                    "already_warm": already_warm,
                    "local_session_id": warmed_thread_id,
                }),
            })
            .await
            .ok();
        Ok(())
    }

    async fn handle_turn(
        &mut self,
        chat_session_id: uuid::Uuid,
        turn: ChatTurn,
        bridge: &mut Option<AgentBridge>,
        workspace_path: &mut Option<std::path::PathBuf>,
        bridge_seq: &mut u64,
        started_sent: &mut bool,
    ) -> Result<bool> {
        let message_id = turn.message_id;
        self.send_timing_event(
            chat_session_id,
            bridge_seq,
            "runner_message_received",
            serde_json::json!({
                "message_id": message_id,
                "local_session_id": turn.local_session_id.as_deref(),
                "local_thread_id": turn.local_thread_id.as_deref(),
            }),
        )
        .await;

        if workspace_path.is_none() {
            *workspace_path = Some(
                self.resolve_chat_workspace(chat_session_id, turn.cwd.as_deref())
                    .await?,
            );
        }
        let workspace_path = workspace_path
            .as_deref()
            .ok_or_else(|| anyhow::anyhow!("chat workspace missing"))?;

        if bridge.as_ref().is_some_and(bridge_has_exited) {
            if let Some(bridge) = bridge.take() {
                bridge.shutdown(Duration::from_secs(1)).await.ok();
            }
            *started_sent = false;
        }

        let resume_id = chat_resume_id(
            turn.local_session_id.as_deref(),
            turn.local_thread_id.as_deref(),
        );
        if bridge.is_none() {
            let spawn_started = Instant::now();
            *bridge = Some(
                AgentBridge::spawn_from_config_with_resume(
                    &self.runner_config,
                    workspace_path,
                    turn.model.clone(),
                    resume_id.as_deref(),
                    None,
                )
                .await?,
            );
            self.send_timing_event(
                chat_session_id,
                bridge_seq,
                "bridge_spawned",
                serde_json::json!({
                    "operation": "turn",
                    "message_id": message_id,
                    "duration_ms": elapsed_ms(spawn_started),
                    "resume_id_present": resume_id.is_some(),
                }),
            )
            .await;
        } else {
            self.send_timing_event(
                chat_session_id,
                bridge_seq,
                "bridge_reused",
                serde_json::json!({
                    "operation": "turn",
                    "message_id": message_id,
                }),
            )
            .await;
        }
        let bridge = bridge
            .as_mut()
            .ok_or_else(|| anyhow::anyhow!("chat bridge missing"))?;
        let payload = RunPayload {
            run_id: turn.message_id,
            prompt: turn.content,
            model: turn.model,
        };
        let turn_started_at = Instant::now();
        let mut cursor = bridge.run(&payload, workspace_path).await?;
        let turn_id = cursor.thread_id().to_string();
        self.send_timing_event(
            chat_session_id,
            bridge_seq,
            "bridge_turn_started",
            serde_json::json!({
                "message_id": message_id,
                "turn_id": turn_id.as_str(),
                "duration_ms": elapsed_ms(turn_started_at),
            }),
        )
        .await;
        if !*started_sent {
            self.send_chat_started_event(chat_session_id, turn_id.clone());
            *started_sent = true;
        }
        self.out
            .send(ClientMsg::ChatMessageStarted {
                chat_session_id,
                message_id: turn.message_id,
                turn_id: Some(turn_id.clone()),
                started_at: Utc::now(),
            })
            .await
            .ok();

        let mut final_status = "completed".to_string();
        let mut assistant_message: Option<String> = None;
        let mut close_after_turn = false;
        let mut first_agent_event_sent = false;
        let mut first_assistant_text_sent = false;
        loop {
            tokio::select! {
                biased;
                command = self.command_rx.recv() => {
                    match command {
                        Some(ChatCommand::Cancel { reason }) => {
                            tracing::info!(?reason, %chat_session_id, "cancelling active chat turn");
                            bridge.interrupt().await.ok();
                            final_status = "cancelled".into();
                            break;
                        }
                        Some(ChatCommand::Close { reason }) => {
                            tracing::info!(?reason, %chat_session_id, "closing active chat turn");
                            bridge.interrupt().await.ok();
                            final_status = "cancelled".into();
                            close_after_turn = true;
                            break;
                        }
                        // Release shouldn't arrive mid-turn (the supervisor
                        // only swaps idle runtimes), but treat it like a
                        // shutdown of this turn if it ever does.
                        Some(ChatCommand::Release | ChatCommand::Shutdown) | None => {
                            bridge.interrupt().await.ok();
                            final_status = "cancelled".into();
                            break;
                        }
                        Some(ChatCommand::Warm(_)) => {
                            tracing::debug!(
                                %chat_session_id,
                                "chat warm ignored while turn is active"
                            );
                        }
                        Some(ChatCommand::Message(_)) => {
                            tracing::warn!(
                                %chat_session_id,
                                "chat runtime received a second message while a turn is active"
                            );
                        }
                    }
                }
                events = bridge.next_events(&mut cursor) => {
                    let Some(events) = events else {
                        self.send_timing_event(
                            chat_session_id,
                            bridge_seq,
                            "agent_stdout_closed",
                            serde_json::json!({
                                "message_id": message_id,
                                "turn_id": turn_id.as_str(),
                                "since_turn_start_ms": elapsed_ms(turn_started_at),
                            }),
                        ).await;
                        self.out.send(ClientMsg::ChatFailed {
                            chat_session_id,
                            code: "agent_stdout_closed".into(),
                            detail: Some("agent stdout closed".into()),
                            failed_at: Utc::now(),
                        }).await.ok();
                        self.state.set_chat_active(false).await;
                        return Ok(true);
                    };
                    let mut done = false;
                    for ev in events {
                        if !first_agent_event_sent {
                            self.send_timing_event(
                                chat_session_id,
                                bridge_seq,
                                "first_agent_event",
                                serde_json::json!({
                                    "message_id": message_id,
                                    "turn_id": turn_id.as_str(),
                                    "event": bridge_event_label(&ev),
                                    "since_turn_start_ms": elapsed_ms(turn_started_at),
                                }),
                            ).await;
                            first_agent_event_sent = true;
                        }
                        *bridge_seq = (*bridge_seq).saturating_add(1);
                        match ev {
                            BridgeEvent::Raw { method, params, .. } => {
                                let kind = if is_assistant_text_delta(&method, &params) {
                                    "assistant_delta"
                                } else {
                                    "raw"
                                };
                                let first_text_delta_chars =
                                    if !first_assistant_text_sent && kind == "assistant_delta" {
                                        assistant_delta_text(&params)
                                            .filter(|text| !text.is_empty())
                                            .map(|text| text.chars().count())
                                    } else {
                                        None
                                    };
                                self.out.send(ClientMsg::ChatEvent {
                                    chat_session_id,
                                    bridge_seq: *bridge_seq,
                                    kind: kind.into(),
                                    payload: serde_json::json!({
                                        "method": method.as_str(),
                                        "params": params,
                                    }),
                                }).await.ok();
                                if let Some(delta_chars) = first_text_delta_chars {
                                    self.send_timing_event(
                                        chat_session_id,
                                        bridge_seq,
                                        "first_assistant_text",
                                        serde_json::json!({
                                            "message_id": message_id,
                                            "turn_id": turn_id.as_str(),
                                            "method": method.as_str(),
                                            "delta_chars": delta_chars,
                                            "since_turn_start_ms": elapsed_ms(turn_started_at),
                                        }),
                                    ).await;
                                    first_assistant_text_sent = true;
                                }
                            }
                            BridgeEvent::ApprovalRequest {
                                approval_id,
                                kind,
                                payload,
                                reason,
                                ..
                            } => {
                                let policy = Policy::new(&self.runner_config.approval_policy, workspace_path, crate::cloud::protocol::ApprovalMode::Ask);
                                let decision = policy.evaluate(kind, &payload);
                                if let Some(auto) = decision.into_cloud() {
                                    if let Err(e) = bridge.send_approval(&approval_id, auto).await {
                                        self.out.send(ClientMsg::ChatFailed {
                                            chat_session_id,
                                            code: "approval_send_failed".into(),
                                            detail: Some(format!("{e:#}")),
                                            failed_at: Utc::now(),
                                        }).await.ok();
                                        self.state.set_chat_active(false).await;
                                        return Ok(true);
                                    }
                                    continue;
                                }
                                let rec = ApprovalRecord {
                                    approval_id: approval_id.clone(),
                                    runner_id: self.runner_config.runner_id,
                                    run_id: turn.message_id,
                                    kind,
                                    payload: payload.clone(),
                                    reason: reason.clone(),
                                    requested_at: Utc::now(),
                                    expires_at: Some(Utc::now() + chrono::Duration::minutes(10)),
                                    status: crate::approval::router::ApprovalStatus::Pending,
                                };
                                let mut rx = self.approvals.subscribe();
                                self.approvals.open(rec.clone()).await;
                                self.state.note_approvals_changed();
                                self.out.send(ClientMsg::ChatApprovalRequest {
                                    chat_session_id,
                                    local_approval_id: approval_id.clone(),
                                    kind,
                                    payload: payload.clone(),
                                    reason,
                                    expires_at: rec.expires_at,
                                }).await.ok();
                                loop {
                                    match rx.recv().await {
                                        Ok(ApprovalRecord {
                                            approval_id: aid,
                                            status:
                                                ApprovalStatus::Resolved {
                                                    decision, ..
                                                },
                                            ..
                                        }) if aid == approval_id => {
                                            if let Err(e) = bridge.send_approval(&approval_id, decision).await {
                                                self.out.send(ClientMsg::ChatFailed {
                                                    chat_session_id,
                                                    code: "approval_send_failed".into(),
                                                    detail: Some(format!("{e:#}")),
                                                    failed_at: Utc::now(),
                                                }).await.ok();
                                                self.state.set_chat_active(false).await;
                                                return Ok(true);
                                            }
                                            self.state.note_approvals_changed();
                                            break;
                                        }
                                        Ok(_) => continue,
                                        Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => continue,
                                        Err(_) => break,
                                    }
                                }
                            }
                            BridgeEvent::Completed { done_payload, .. } => {
                                assistant_message = assistant_text_from_done_payload(&done_payload);
                                self.send_timing_event(
                                    chat_session_id,
                                    bridge_seq,
                                    "agent_completed",
                                    serde_json::json!({
                                        "message_id": message_id,
                                        "turn_id": turn_id.as_str(),
                                        "since_turn_start_ms": elapsed_ms(turn_started_at),
                                    }),
                                ).await;
                                final_status = "completed".into();
                                done = true;
                                break;
                            }
                            BridgeEvent::Failed { detail, .. } => {
                                self.send_timing_event(
                                    chat_session_id,
                                    bridge_seq,
                                    "agent_failed",
                                    serde_json::json!({
                                        "message_id": message_id,
                                        "turn_id": turn_id.as_str(),
                                        "since_turn_start_ms": elapsed_ms(turn_started_at),
                                        "detail": detail.as_deref(),
                                    }),
                                ).await;
                                self.out.send(ClientMsg::ChatFailed {
                                    chat_session_id,
                                    code: "agent_failed".into(),
                                    detail,
                                    failed_at: Utc::now(),
                                }).await.ok();
                                self.state.set_chat_active(false).await;
                                return Ok(true);
                            }
                            BridgeEvent::RunStarted { .. } | BridgeEvent::AwaitingReauth { .. } => {}
                        }
                    }
                    if done {
                        break;
                    }
                }
            }
        }
        self.send_timing_event(
            chat_session_id,
            bridge_seq,
            "turn_completion",
            serde_json::json!({
                "message_id": message_id,
                "turn_id": turn_id.as_str(),
                "status": final_status.as_str(),
                "has_assistant_message": assistant_message.is_some(),
                "since_turn_start_ms": elapsed_ms(turn_started_at),
            }),
        )
        .await;
        self.out
            .send(ClientMsg::ChatMessageCompleted {
                chat_session_id,
                message_id: turn.message_id,
                turn_id: Some(turn_id),
                assistant_message,
                status: final_status,
                completed_at: Utc::now(),
            })
            .await
            .ok();
        if close_after_turn {
            let _ = self
                .out
                .send(ClientMsg::ChatClosed {
                    chat_session_id,
                    closed_at: Utc::now(),
                })
                .await;
        }
        Ok(close_after_turn)
    }

    async fn resolve_chat_workspace(
        &mut self,
        _chat_session_id: uuid::Uuid,
        cwd: Option<&str>,
    ) -> Result<std::path::PathBuf> {
        // One working dir per runner: chat runs in exactly the same directory as
        // issue runs, `workspace.working_dir`. There is no separate chat
        // workspace and no git lifecycle here — the agent drives git per the
        // prompt. This is the PDASHOSS01-83 fix: a warm-runner chat message does
        // no git operation before the agent spawns.
        let workspace_path = self.runner_config.workspace.working_dir.clone();
        std::fs::create_dir_all(&workspace_path)?;
        if let Some(cwd) = cwd.filter(|s| !s.is_empty()) {
            let requested = std::path::PathBuf::from(cwd);
            let requested = if requested.is_absolute() {
                requested
            } else {
                workspace_path.join(requested)
            };
            if !requested.starts_with(&workspace_path) {
                anyhow::bail!("chat cwd is outside runner workspace");
            }
            return Ok(requested);
        }
        Ok(workspace_path)
    }
}

fn assistant_text_from_done_payload(payload: &serde_json::Value) -> Option<String> {
    if let Some(text) = payload.as_str() {
        return Some(text.to_string());
    }
    let obj = payload.as_object()?;
    for key in ["result", "message", "text", "output", "summary", "content"] {
        if let Some(text) = obj.get(key).and_then(|value| value.as_str())
            && !text.is_empty()
        {
            return Some(text.to_string());
        }
    }
    None
}

fn format_exec_command_detail(
    cmd: &crate::daemon::state::ExecCommandSnapshot,
    now: DateTime<Utc>,
) -> String {
    let cwd = cmd
        .cwd
        .as_deref()
        .map(|c| format!(" in `{c}`"))
        .unwrap_or_default();
    if let Some(completed_at) = cmd.completed_at {
        let runtime = (completed_at - cmd.started_at).num_seconds().max(0);
        let since_completed = (now - completed_at).num_seconds().max(0);
        // Distinguish a clean completion from a non-success terminal
        // (codex `failed`, Claude `is_error: true`) so the failure
        // detail does not call an errored command a "completion".
        let verb = match cmd.completed_success {
            Some(false) => "exited with error",
            Some(true) | None => "completed",
        };
        format!(
            "command context: last observed command tool `{}`{cwd} \
             ({verb} {since_completed}s before failure; ran {runtime}s)",
            cmd.command
        )
    } else {
        let elapsed = (now - cmd.started_at).num_seconds().max(0);
        format!(
            "command context: last command frame `{}`{cwd} \
             (started {elapsed}s ago; no completion frame observed)",
            cmd.command
        )
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum DiagnosticSignalPriority {
    ExitStatus,
    TextError,
    JsonError,
}

fn diagnostic_signal_from_stderr(lines: &[String]) -> Option<String> {
    let mut best: Option<(DiagnosticSignalPriority, String)> = None;
    for line in lines.iter().rev() {
        let Some((priority, signal)) = diagnostic_signal_from_stderr_line(line) else {
            continue;
        };
        if priority == DiagnosticSignalPriority::JsonError {
            return Some(signal);
        }
        if best
            .as_ref()
            .map(|(best_priority, _)| priority > *best_priority)
            .unwrap_or(true)
        {
            best = Some((priority, signal));
        }
    }
    best.map(|(_, signal)| signal)
}

fn diagnostic_signal_from_stderr_line(line: &str) -> Option<(DiagnosticSignalPriority, String)> {
    let trimmed = line.trim();
    if trimmed.is_empty() {
        return None;
    }

    if let Ok(value) = serde_json::from_str::<serde_json::Value>(trimmed)
        && let Some(message) = json_error_message(&value)
    {
        return Some((
            DiagnosticSignalPriority::JsonError,
            format!("stderr JSON error: {message}"),
        ));
    }

    let lower = trimmed.to_ascii_lowercase();
    let has_exit_code = lower.contains("exit code");
    let generic_exit_status = lower.starts_with("process exited with code")
        || lower.starts_with("exited with code")
        || lower.starts_with("exit code ");
    let looks_relevant = lower.contains("error")
        || lower.contains("failed")
        || lower.contains("failure")
        || lower.contains("denied")
        || lower.contains("timed out")
        || lower.contains("timeout")
        || lower.contains("panic")
        || has_exit_code
        || generic_exit_status
        || lower.contains("not logged in")
        || lower.contains("unauthorized")
        || lower.contains("forbidden");
    if looks_relevant {
        let priority = if generic_exit_status {
            DiagnosticSignalPriority::ExitStatus
        } else {
            DiagnosticSignalPriority::TextError
        };
        Some((priority, format!("stderr: {trimmed}")))
    } else {
        None
    }
}

fn json_error_message(value: &serde_json::Value) -> Option<String> {
    for key in ["error", "detail", "message"] {
        let Some(v) = value.get(key) else {
            continue;
        };
        if let Some(s) = v.as_str()
            && !s.trim().is_empty()
        {
            return Some(s.trim().to_string());
        }
        if let Some(s) = v.get("message").and_then(|m| m.as_str())
            && !s.trim().is_empty()
        {
            return Some(s.trim().to_string());
        }
    }
    None
}

fn core_error_summary(base: &str, diagnostic_signal: Option<&str>) -> String {
    let Some(signal) = diagnostic_signal else {
        return base.to_string();
    };
    signal
        .strip_prefix("stderr JSON error: ")
        .or_else(|| signal.strip_prefix("stderr: "))
        .unwrap_or(signal)
        .to_string()
}

fn assemble_failure_detail(
    base: &str,
    last_cmd: Option<&crate::daemon::state::ExecCommandSnapshot>,
    stderr: &crate::agent::StderrSnapshot,
    now: DateTime<Utc>,
) -> String {
    const DETAIL_BYTES_CAP: usize = 4096;
    const STDERR_TAIL_LINES: usize = 10;

    let diagnostic_signal = diagnostic_signal_from_stderr(&stderr.lines);
    let core = core_error_summary(base, diagnostic_signal.as_deref());

    let mut trace: Vec<String> = vec![format!("- watchdog: {base}")];
    if let Some(signal) = diagnostic_signal {
        trace.push(format!("- diagnostic signal: {signal}"));
    }
    if let Some(cmd) = last_cmd {
        trace.push(format!("- {}", format_exec_command_detail(cmd, now)));
    }
    // The stderr ring has already filtered codex tracing noise;
    // `stderr.dropped` is the running count of lines we rejected so
    // the user has an honest signal of how much was suppressed.
    // Emit the section if either we have content to show OR a
    // non-zero dropped count (the latter alone tells the operator
    // "the agent was emitting noise but no signal").
    if !stderr.lines.is_empty() || stderr.dropped > 0 {
        let shown = stderr.lines.len().min(STDERR_TAIL_LINES);
        let suffix = if stderr.dropped > 0 {
            format!(
                " (plus {} low-signal line(s) filtered from agent stderr)",
                stderr.dropped
            )
        } else {
            String::new()
        };
        if shown > 0 {
            let tail = stderr
                .lines
                .iter()
                .rev()
                .take(STDERR_TAIL_LINES)
                .rev()
                .map(|line| format!("  {line}"))
                .collect::<Vec<_>>()
                .join("\n");
            trace.push(format!(
                "- stderr context ({shown} line(s){suffix}):\n{tail}"
            ));
        } else {
            // Filter dropped everything — surface that fact alone
            // rather than emitting an empty stderr block.
            trace.push(format!("- stderr context: empty after filtering{suffix}"));
        }
    }

    let mut joined = format!("Core error: {core}\n\nError trace:\n{}", trace.join("\n"));
    if joined.len() > DETAIL_BYTES_CAP {
        // Truncate on a char boundary, leaving a sentinel so consumers
        // can tell the body was clipped.
        let mut end = DETAIL_BYTES_CAP;
        while !joined.is_char_boundary(end) && end > 0 {
            end -= 1;
        }
        joined.truncate(end);
        joined.push('…');
    }
    joined
}

/// Owns one `Assign`'s lifecycle. Spawned as a task from `RunnerLoop`, so the
/// message loop stays live and can deliver Cancel / Decide frames to us via
/// `self.cancel` and `self.approvals`.
struct AssignWorker {
    runner_paths: RunnerPaths,
    daemon_paths: Paths,
    runner_config: crate::config::schema::RunnerConfig,
    state: StateHandle,
    approvals: ApprovalRouter,
    out: RunnerOut,
    cancel: tokio_util::sync::CancellationToken,
}

struct AssignedRun {
    run_id: uuid::Uuid,
    issue_identifier: Option<String>,
    project_name: Option<String>,
    prompt: String,
    repo_url: Option<String>,
    repo_ref: Option<String>,
    git_work_branch: Option<String>,
    expected_codex_model: Option<String>,
}

struct RunMetadata {
    tokens: Option<WireTokenUsage>,
    model: Option<String>,
}

impl AssignWorker {
    async fn run_metadata(&self) -> RunMetadata {
        let snapshot = self.state.observability_snapshot().await;
        RunMetadata {
            tokens: snapshot.tokens.map(|t| WireTokenUsage {
                input: t.input,
                output: t.output,
                total: t.total,
            }),
            model: snapshot.model,
        }
    }

    /// Pick the right `FailureReason` when the agent subprocess crashes or
    /// exits abnormally. Codex stays on `CodexCrash` so dashboards that
    /// already filter on `"codex_crash"` keep working; Claude (and any
    /// future non-Codex agent) surfaces as the agent-neutral `AgentCrash`
    /// so the two aren't conflated in telemetry.
    fn crash_reason(&self) -> FailureReason {
        match self.runner_config.agent.kind {
            AgentKind::Codex => FailureReason::CodexCrash,
            AgentKind::ClaudeCode
            | AgentKind::CursorAgent
            | AgentKind::OpenClaw
            | AgentKind::Grok
            | AgentKind::MuseCode
            | AgentKind::Karma => FailureReason::AgentCrash,
        }
    }

    async fn run(&mut self, assignment: AssignedRun) -> Result<()> {
        self.handle_assign(assignment).await
    }

    async fn handle_assign(&mut self, assignment: AssignedRun) -> Result<()> {
        // Scope the run id (and issue key) over everything this run does so
        // every agent subprocess the bridges spawn (and re-spawn per turn)
        // inherits the run, work item, and workspace identity — see
        // `util::shell::RUN_ENV`. Login defaults may belong to another workspace.
        let run_id = assignment.run_id;
        crate::util::shell::RUN_ENV
            .scope(
                crate::util::shell::RunEnv {
                    run_id,
                    issue_identifier: assignment.issue_identifier.clone(),
                    workspace_slug: self.runner_config.workspace_slug.clone(),
                    project_id: self.runner_config.project_id,
                },
                self.handle_assign_inner(assignment),
            )
            .await
    }

    async fn handle_assign_inner(&mut self, assignment: AssignedRun) -> Result<()> {
        let AssignedRun {
            run_id,
            issue_identifier: work_item_identifier,
            project_name,
            prompt,
            repo_url,
            repo_ref,
            git_work_branch,
            expected_codex_model,
        } = assignment;
        let cancel = self.cancel.clone();
        // Before a bridge owns the agent process, dropping setup is sufficient
        // to stop the pending CLI/Git command (they use kill_on_drop).
        macro_rules! before_agent {
            ($operation:expr) => {
                tokio::select! {
                    biased;
                    _ = cancel.cancelled() => {
                        self.state.set_current_run(None).await;
                        self.send(ClientMsg::RunCancelled {
                            run_id, cancelled_at: Utc::now(), tokens: None, model: None,
                        }).await;
                        return Ok(());
                    }
                    result = $operation => result,
                }
            };
        }
        // Karma owns the project root; repository bootstrap and the agent
        // session both happen from the dedicated operation directory. Other
        // agent bridges retain the historical runner workspace behavior.
        let runtime_project_slug = self.state.project_slug_now();
        let workspace_path = if self.runner_config.agent.kind == AgentKind::Karma {
            let project_name = project_name
                .as_deref()
                .filter(|name| !name.trim().is_empty())
                .or(runtime_project_slug.as_deref())
                .or(self.runner_config.project_slug.as_deref())
                .unwrap_or(self.runner_config.name.as_str());
            let operation_dir = match before_agent!(crate::karma::project::prepare_operation_dir(
                &self.runner_config.karma.binary,
                project_name,
                &self.runner_config.name,
                work_item_identifier.as_deref(),
                run_id,
            )) {
                Ok(path) => path,
                Err(e) => {
                    self.send(ClientMsg::RunFailed {
                        run_id,
                        reason: FailureReason::WorkspaceSetup,
                        detail: Some(format!("preparing Karma project workspace: {e:#}")),
                        ended_at: Utc::now(),
                        tokens: None,
                        model: None,
                    })
                    .await;
                    self.state.set_current_run(None).await;
                    return Ok(());
                }
            };
            // Bootstrap the repository inside the isolated operation
            // directory. Karma's bridge starts the session directly from
            // this already-bound checkout.
            match before_agent!(crate::workspace::resolve(
                &operation_dir,
                repo_url.as_deref()
            )) {
                Ok(crate::workspace::Resolution::ExistingRepo(path))
                | Ok(crate::workspace::Resolution::Cloned(path))
                | Ok(crate::workspace::Resolution::Directory(path)) => path,
                Err(e) => {
                    // Repository problems degrade to an ordinary directory in
                    // `workspace::resolve`. Only an unusable filesystem path
                    // reaches this branch, so the Karma agent has nowhere safe
                    // to run.
                    let crate::workspace::ResolveError::Io(_) = &e;
                    self.send(ClientMsg::RunFailed {
                        run_id,
                        reason: FailureReason::WorkspaceSetup,
                        detail: Some(format!("preparing Karma repository: {e}")),
                        ended_at: Utc::now(),
                        tokens: None,
                        model: None,
                    })
                    .await;
                    self.state.set_current_run(None).await;
                    return Ok(());
                }
            }
        } else {
            // Resolve the directory the agent runs in: this runner's single,
            // exclusive `workspace.working_dir`. Clone-bootstrap it once if a
            // repo url is provided (`resolve` tolerates a non-git task folder).
            let wd = self.runner_config.workspace.working_dir.clone();
            match before_agent!(crate::workspace::resolve(&wd, repo_url.as_deref())) {
                Ok(crate::workspace::Resolution::ExistingRepo(p))
                | Ok(crate::workspace::Resolution::Cloned(p))
                | Ok(crate::workspace::Resolution::Directory(p)) => p,
                Err(e) => {
                    // Git-specific failures already fall back to a task folder.
                    // A remaining error means the directory itself cannot be
                    // safely used by the agent.
                    let crate::workspace::ResolveError::Io(_) = &e;
                    self.send(ClientMsg::RunFailed {
                        run_id,
                        reason: FailureReason::WorkspaceSetup,
                        detail: Some(e.to_string()),
                        ended_at: Utc::now(),
                        tokens: None,
                        model: None,
                    })
                    .await;
                    // The supervisor stamped ``rx_in_flight = Some(run_id)``
                    // synchronously when the Assign arrived; clear it now so
                    // a session reconnect after the failure doesn't claim a
                    // run that no longer exists.
                    self.state.set_current_run(None).await;
                    return Ok(());
                }
            }
        };

        // A work item can pin execution to an existing remote branch or name
        // a new branch that does not exist yet. Prepare that branch before we
        // report `Accept`, so the cloud sees the exact checkout the agent will
        // use and the agent never has to race the platform's branch setup.
        let prepared_work_branch = if let Some(branch) = git_work_branch.as_deref() {
            match before_agent!(crate::workspace::git::prepare_work_branch(
                &workspace_path,
                branch,
                repo_ref.as_deref(),
            )) {
                Ok(prepared) => Some(prepared),
                Err(err) => {
                    let detail = format!("preparing git work branch `{branch}`: {err:#}");
                    tracing::warn!(
                        %run_id,
                        git_work_branch = branch,
                        error = %err,
                        "failed to prepare assigned git work branch",
                    );
                    self.send(ClientMsg::RunFailed {
                        run_id,
                        reason: FailureReason::WorkspaceSetup,
                        detail: Some(detail),
                        ended_at: Utc::now(),
                        tokens: None,
                        model: None,
                    })
                    .await;
                    self.state.set_current_run(None).await;
                    return Ok(());
                }
            }
        } else {
            None
        };

        if let Some(project_ref) = runtime_project_slug
            .as_deref()
            .or(self.runner_config.project_slug.as_deref())
            && let Err(err) = before_agent!(crate::cli::context::write_context_for_project(
                &self.daemon_paths,
                &workspace_path,
                project_ref,
            ))
        {
            tracing::debug!(
                error = %err,
                "skipping .karma-team/context.md refresh after workspace resolution",
            );
        }

        let ws_state = crate::workspace::git::workspace_state(&workspace_path)
            .await
            .unwrap_or(WorkspaceState {
                branch: None,
                head: None,
                dirty: false,
            });
        self.send(ClientMsg::Accept {
            run_id,
            workspace_state: ws_state,
        })
        .await;

        // History writer.
        let mut hist = HistoryWriter::open(&self.runner_paths, run_id).await?;
        hist.append(&HistoryEntry::Header {
            run_id,
            work_item_id: None,
            prompt_preview: prompt.chars().take(160).collect(),
            started_at: Utc::now(),
            repo_url,
            agent_kind: Some(self.runner_config.agent.kind.wire_name().to_string()),
        })
        .await?;

        // Update state with current run.
        self.state
            .set_current_run(Some(CurrentRunSummary {
                run_id,
                thread_id: None,
                status: "starting".to_string(),
                started_at: Utc::now(),
                events: 0,
            }))
            .await;

        // Record which agent CLI this run will drive — in the daemon log
        // (`journalctl --user-unit karma-team`) and in the transcript header
        // below — so an operator can confirm the invoked tool after the fact.
        tracing::info!(
            %run_id,
            agent = self.runner_config.agent.kind.wire_name(),
            binary = agent_binary_for(&self.runner_config),
            workspace = %workspace_path.display(),
            "starting run",
        );

        // Bridge to the configured agent (Codex or Claude Code). `AgentBridge`
        // hides which CLI is actually being driven from the rest of this
        // worker; the event flow below is identical for both.
        let mut bridge = match before_agent!(AgentBridge::spawn_from_config_for_issue(
            &self.runner_config,
            &workspace_path,
            expected_codex_model.clone(),
            git_work_branch.is_none(),
        )) {
            Ok(b) => b,
            Err(e) => {
                hist.append(&HistoryEntry::Footer {
                    ts: Utc::now(),
                    final_status: "failed".to_string(),
                    done_payload: None,
                    error: Some(e.to_string()),
                })
                .await
                .ok();
                let reason = self.crash_reason();
                self.send(ClientMsg::RunFailed {
                    run_id,
                    reason,
                    detail: Some(format!("{e:#}")),
                    ended_at: Utc::now(),
                    tokens: None,
                    model: None,
                })
                .await;
                self.state.set_current_run(None).await;
                return Ok(());
            }
        };
        // Capture the bridge-owned process handle now that the subprocess
        // is spawned. Subscribe to its `exit_rx` so the live-state snapshot
        // flips `agent_subprocess_alive=false` the moment the wait task
        // observes termination — independent of stdout-close detection.
        // See `.ai_design/runner_agent_bridge/design.md` §4.4.
        let process_handle = bridge.process_handle();
        self.state.set_agent_pid(process_handle.pid).await;
        self.state.set_agent_alive(true).await;
        spawn_exit_watch(self.state.clone(), run_id, process_handle.exit_rx.clone());

        let payload = RunPayload {
            run_id,
            prompt,
            model: expected_codex_model,
        };
        let startup = tokio::select! {
            biased;
            _ = cancel.cancelled() => None,
            result = bridge.run_one_shot(&payload, &workspace_path) => Some(result),
        };
        let Some(startup) = startup else {
            let _ = tokio::time::timeout(Duration::from_secs(2), bridge.interrupt()).await;
            bridge.shutdown(Duration::from_secs(5)).await?;
            hist.append(&HistoryEntry::Footer {
                ts: Utc::now(),
                final_status: "cancelled".into(),
                done_payload: None,
                error: Some("cancelled during agent startup".into()),
            })
            .await
            .ok();
            self.state.set_current_run(None).await;
            self.send(ClientMsg::RunCancelled {
                run_id,
                cancelled_at: Utc::now(),
                tokens: None,
                model: None,
            })
            .await;
            return Ok(());
        };
        let mut cursor = match startup {
            Ok(c) => c,
            Err(e) => {
                let detail = self.build_failure_detail(&format!("{e:#}"), &bridge).await;
                hist.append(&HistoryEntry::Footer {
                    ts: Utc::now(),
                    final_status: "failed".to_string(),
                    done_payload: None,
                    error: Some(detail.clone()),
                })
                .await
                .ok();
                self.send(ClientMsg::RunFailed {
                    run_id,
                    reason: self.crash_reason(),
                    detail: Some(detail),
                    ended_at: Utc::now(),
                    tokens: None,
                    model: None,
                })
                .await;
                bridge.shutdown(Duration::from_secs(5)).await.ok();
                self.state.set_current_run(None).await;
                return Ok(());
            }
        };
        let run_model = cursor.model().map(ToOwned::to_owned);
        self.state.set_model(run_model.clone()).await;
        self.send(ClientMsg::RunStarted {
            run_id,
            thread_id: cursor.thread_id().to_string(),
            // Alias the session id to the thread id, mirroring the chat
            // bridge's warm path (`local_session_id: Some(thread_id)`): the
            // run path's only session handle today is the thread id. `agent_kind`
            // lets the cloud interpret it per-CLI. Both are recorded durably in
            // `agent_metadata`, which — unlike `thread_id` — survives a retry.
            local_session_id: cursor.thread_id().to_string(),
            agent_kind: cursor.agent_kind().to_string(),
            started_at: Utc::now(),
            model: run_model,
        })
        .await;
        hist.append(&HistoryEntry::Lifecycle {
            ts: Utc::now(),
            state: "started".to_string(),
            detail: Some(cursor.thread_id().to_string()),
        })
        .await?;

        // Pump events until terminal.
        let outcome = self
            .pump_events(
                &mut bridge,
                &mut cursor,
                &mut hist,
                &workspace_path,
                prepared_work_branch.as_ref(),
            )
            .await;

        bridge.shutdown(Duration::from_secs(5)).await?;
        let outcome = outcome?;

        let summary = RunSummary {
            run_id,
            work_item_id: None,
            status: outcome.status_label.clone(),
            started_at: Utc::now(),
            ended_at: Some(Utc::now()),
            title: None,
        };
        let mut idx = RunsIndex::load(&self.runner_paths).unwrap_or_default();
        idx.upsert(summary);
        idx.save(&self.runner_paths).ok();

        if outcome.status_label == "cancelled" {
            let terminal_metadata = self.run_metadata().await;
            self.state.set_current_run(None).await;
            self.send(ClientMsg::RunCancelled {
                run_id,
                cancelled_at: Utc::now(),
                tokens: terminal_metadata.tokens,
                model: terminal_metadata.model,
            })
            .await;
            return Ok(());
        }

        self.state.set_current_run(None).await;
        Ok(())
    }

    async fn pump_events(
        &mut self,
        bridge: &mut AgentBridge,
        cursor: &mut AgentCursor,
        hist: &mut HistoryWriter,
        workspace_root: &std::path::Path,
        git_work_branch: Option<&crate::workspace::git::PreparedWorkBranch>,
    ) -> Result<Outcome> {
        // Stall watchdog: if the agent emits no frames for this long AND
        // no approval is in flight (a human-wait is legitimate silence),
        // give up and fail the run. Without this, an unrecognised wait —
        // e.g. a future codex protocol change — leaves the runner blocked
        // on stdout and the cloud showing "running" indefinitely.
        //
        // Threshold is per-agent because Codex emits continuous token-
        // count events while Claude can be silent for the full
        // duration of a single tool call (see `AgentKind::stall_timeout`).
        let stall_timeout = self.runner_config.agent.kind.stall_timeout();

        let shutdown = self.state.shutdown_notified();
        let cancel = self.cancel.clone();
        let mut cancelled = false;
        let mut run_events = RunEventMirror::new(cursor.run_id());
        // Nobody can answer an issue run's questions; stop the run instead of
        // letting the agent re-ask forever. See `daemon::question_guard`.
        let mut questions = crate::daemon::question_guard::UnansweredQuestionGuard::default();
        loop {
            // Re-evaluate at every loop entry: an approval that opened on
            // the previous iteration disarms the watchdog; one that just
            // resolved re-arms it.
            let stall_deadline = if self.approvals.list_pending().await.is_empty() {
                Some(tokio::time::Instant::now() + stall_timeout)
            } else {
                None
            };
            let run_event_flush_deadline = run_events.flush_deadline();
            tokio::select! {
                biased;
                _ = shutdown.notified(), if !cancelled => {
                    cancelled = true;
                    run_events.flush_before_lifecycle(&self.out).await;
                    bridge.interrupt().await.ok();
                }
                _ = cancel.cancelled(), if !cancelled => {
                    let _ = tokio::time::timeout(Duration::from_secs(2), bridge.interrupt()).await;
                    let _ = tokio::time::timeout(Duration::from_secs(2), run_events.flush_before_lifecycle(&self.out)).await;
                    hist.append(&HistoryEntry::Lifecycle {
                        ts: Utc::now(),
                        state: "cancelled".into(),
                        detail: None,
                    }).await.ok();
                    // Shutdown below waits for actual process exit, then kills
                    // the process group if the grace period expires.
                    return Ok(Outcome { status_label: "cancelled".into() });
                }
                _ = async {
                    match run_event_flush_deadline {
                        Some(d) => tokio::time::sleep_until(d).await,
                        None => std::future::pending::<()>().await,
                    }
                } => {
                    run_events.flush(&self.out).await;
                }
                events = bridge.next_events(cursor) => {
                    let Some(events) = events else {
                        let reason = self.crash_reason();
                        let detail = self
                            .build_failure_detail("agent stdout closed", bridge)
                            .await;
                        run_events.flush_before_lifecycle(&self.out).await;
                        let metadata = self.run_metadata().await;
                        self.send(ClientMsg::RunFailed {
                            run_id: cursor.run_id(),
                            reason,
                            detail: Some(detail.clone()),
                            ended_at: Utc::now(),
                            tokens: metadata.tokens,
                            model: metadata.model,
                        }).await;
                        hist.append(&HistoryEntry::Footer {
                            ts: Utc::now(),
                            final_status: "failed".into(),
                            done_payload: None,
                            error: Some(detail),
                        }).await?;
                        return Ok(Outcome { status_label: "failed".into() });
                    };
                    for ev in events {
                        let trip = match &ev {
                            BridgeEvent::Raw { method, params, .. } => {
                                questions.observe(method, params)
                            }
                            _ => None,
                        };
                        if let Some(out) = self
                            .handle_bridge_event(
                                ev,
                                bridge,
                                hist,
                                workspace_root,
                                git_work_branch,
                                &mut run_events,
                            )
                            .await?
                        {
                            return Ok(out);
                        }
                        if let Some(trip) = trip {
                            tracing::warn!(
                                run_id = %cursor.run_id(),
                                asked = trip.asked,
                                "agent keeps asking for human input in an unattended run; stopping it",
                            );
                            bridge.interrupt().await.ok();
                            let detail = trip.failure_detail();
                            run_events.flush_before_lifecycle(&self.out).await;
                            let metadata = self.run_metadata().await;
                            self.send(ClientMsg::RunFailed {
                                run_id: cursor.run_id(),
                                reason: FailureReason::AwaitingHumanInput,
                                detail: Some(detail.clone()),
                                ended_at: Utc::now(),
                                tokens: metadata.tokens,
                                model: metadata.model,
                            }).await;
                            hist.append(&HistoryEntry::Footer {
                                ts: Utc::now(),
                                final_status: "failed".into(),
                                done_payload: None,
                                error: Some(detail),
                            }).await?;
                            return Ok(Outcome { status_label: "failed".into() });
                        }
                    }
                }
                _ = async {
                    match stall_deadline {
                        Some(d) => tokio::time::sleep_until(d).await,
                        None => std::future::pending::<()>().await,
                    }
                } => {
                    let mins = stall_timeout.as_secs() / 60;
                    let base = format!("agent stalled for {mins} minutes without new agent events");
                    let detail = self.build_failure_detail(&base, bridge).await;
                    run_events.flush_before_lifecycle(&self.out).await;
                    let metadata = self.run_metadata().await;
                    self.send(ClientMsg::RunFailed {
                        run_id: cursor.run_id(),
                        reason: FailureReason::Timeout,
                        detail: Some(detail.clone()),
                        ended_at: Utc::now(),
                        tokens: metadata.tokens,
                        model: metadata.model,
                    }).await;
                    hist.append(&HistoryEntry::Footer {
                        ts: Utc::now(),
                        final_status: "failed".into(),
                        done_payload: None,
                        error: Some(detail),
                    }).await?;
                    return Ok(Outcome { status_label: "failed".into() });
                }
            }
        }
    }

    /// Build a `RunFailed.detail` string that includes whatever local
    /// context might help the cloud / UI explain what went wrong:
    /// - the supervisor's own classifier message,
    /// - the strongest diagnostic signal we can infer from stderr,
    /// - the most recent shell command tool the agent kicked off as context,
    /// - the last few filtered lines of agent stderr.
    ///
    /// All inputs are optional and the assembly degrades gracefully when
    /// they're missing. The first line is the best root-cause summary we
    /// have; supporting context is listed under `Error trace:`. The total
    /// payload is bounded so a runaway stderr dump can't balloon a
    /// `RunFailed` body.
    async fn build_failure_detail(&self, base: &str, bridge: &AgentBridge) -> String {
        let last_cmd = self.state.observability_snapshot().await.last_exec_command;
        let stderr = bridge.recent_stderr().await;
        assemble_failure_detail(base, last_cmd.as_ref(), &stderr, Utc::now())
    }

    async fn handle_bridge_event(
        &mut self,
        ev: BridgeEvent,
        bridge: &mut AgentBridge,
        hist: &mut HistoryWriter,
        workspace_root: &std::path::Path,
        git_work_branch: Option<&crate::workspace::git::PreparedWorkBranch>,
        run_events: &mut RunEventMirror,
    ) -> Result<Option<Outcome>> {
        self.state.incr_current_run_events().await;
        // Observability: every bridge event bumps last_event_at + stamps
        // a structure-only kind/summary. The summary is sanitised inside
        // `summary_of` — never includes prompt or model output.
        // Opt-in: extract codex token / turn metrics from Raw frames so
        // operators see them on the runner-status panel.
        let kind = crate::daemon::observability::kind_of(&ev);
        let summary = crate::daemon::observability::summary_of(&ev);
        self.state
            .note_agent_event(Utc::now(), &kind, Some(&summary))
            .await;
        // When a Raw frame carries the agent's human-readable narrative (the
        // "let me first explore…" prose a terminal user sees), mirror it to
        // the cloud verbatim as its own `agent/message` run event so the
        // AgentRun record reads like a live CLI session. Everything else
        // (lifecycle milestones, high-frequency token deltas) keeps the
        // sanitised summary / compaction path so event volume stays bounded.
        let narrative = if let BridgeEvent::Raw { method, params, .. } = &ev {
            crate::daemon::observability::extract_agent_message_text(method, params)
        } else {
            None
        };
        let should_flush = match narrative {
            Some(text) => run_events.push_content(
                "agent/message".to_string(),
                serde_json::json!({
                    "schema": "runner_agent_message_v1",
                    "text": text,
                }),
            ),
            None => run_events.push(kind, summary),
        };
        if should_flush {
            run_events.flush(&self.out).await;
        }
        if let BridgeEvent::Raw { method, params, .. } = &ev {
            match method.as_str() {
                "codex/event/token_count" => {
                    if let Some(usage) =
                        crate::daemon::observability::parse_codex_token_count(params)
                    {
                        self.state.set_tokens(usage).await;
                    }
                }
                "turn/started" => {
                    self.state.incr_turn().await;
                }
                "item/started" | "assistant/message" => {
                    // Failure-detail enrichment: capture the most recent
                    // shell command the agent kicked off. Routing for
                    // codex (`item/started` → commandExecution) and
                    // Claude (`assistant/message` → Bash tool_use) lives
                    // in `extract_exec_command_hint` so it's
                    // unit-testable without a live bridge — a typo in
                    // either method name is caught by tests rather than
                    // by silently absent command-tool context in
                    // production failure details.
                    if let Some(hint) = crate::daemon::observability::extract_exec_command_hint(
                        method.as_str(),
                        params,
                    ) {
                        self.state
                            .note_exec_command(crate::daemon::state::ExecCommandSnapshot {
                                command: hint.command,
                                cwd: hint.cwd,
                                tool_call_id: hint.tool_call_id,
                                started_at: Utc::now(),
                                completed_at: None,
                                completed_success: None,
                            })
                            .await;
                    }
                }
                "item/completed" | "user/toolResult" => {
                    for hint in crate::daemon::observability::extract_exec_command_completion_hints(
                        method.as_str(),
                        params,
                    ) {
                        self.state
                            .note_exec_command_completed(
                                &hint.tool_call_id,
                                hint.success,
                                Utc::now(),
                            )
                            .await;
                    }
                }
                _ => {}
            }
        }
        match ev {
            BridgeEvent::RunStarted { .. } => Ok(None),
            BridgeEvent::Raw {
                method,
                params,
                run_id,
            } => {
                let history_event = match self.runner_config.agent.kind {
                    AgentKind::Karma => HistoryEntry::KarmaEvent {
                        ts: Utc::now(),
                        method: method.clone(),
                        params: params.clone(),
                    },
                    _ => HistoryEntry::CodexEvent {
                        ts: Utc::now(),
                        method: method.clone(),
                        params: params.clone(),
                    },
                };
                hist.append(&history_event).await.ok();
                // Only lifecycle-ish events are mirrored to cloud; raw deltas stay local.
                tracing::trace!(%run_id, method, "agent event");
                Ok(None)
            }
            BridgeEvent::ApprovalRequest {
                run_id,
                approval_id,
                kind,
                payload,
                reason,
            } => {
                run_events.flush_before_lifecycle(&self.out).await;
                let policy = Policy::new(
                    &self.runner_config.approval_policy,
                    workspace_root,
                    crate::cloud::protocol::ApprovalMode::Ask,
                );
                let decision = policy.evaluate(kind, &payload);
                if let Some(auto) = decision.into_cloud() {
                    bridge.send_approval(&approval_id, auto).await.ok();
                    hist.append(&HistoryEntry::Approval {
                        ts: Utc::now(),
                        approval_id: approval_id.clone(),
                        status: format!("auto:{auto:?}"),
                        payload: payload.clone(),
                    })
                    .await
                    .ok();
                    return Ok(None);
                }
                let rec = ApprovalRecord {
                    approval_id: approval_id.clone(),
                    runner_id: self.runner_config.runner_id,
                    run_id,
                    kind,
                    payload: payload.clone(),
                    reason,
                    requested_at: Utc::now(),
                    expires_at: Some(Utc::now() + chrono::Duration::minutes(10)),
                    status: crate::approval::router::ApprovalStatus::Pending,
                };
                // Subscribe BEFORE opening so a Decide that races in between
                // open() and subscribe() doesn't strand the worker waiting on
                // an event it'll never see.
                let mut rx = self.approvals.subscribe();
                self.approvals.open(rec.clone()).await;
                self.state.note_approvals_changed();
                self.send(ClientMsg::ApprovalRequest {
                    run_id,
                    approval_id: uuid_or(&approval_id),
                    kind,
                    payload: payload.clone(),
                    reason: rec.reason.clone(),
                    expires_at: rec.expires_at,
                })
                .await;
                hist.append(&HistoryEntry::Approval {
                    ts: Utc::now(),
                    approval_id: approval_id.clone(),
                    status: "pending".into(),
                    payload,
                })
                .await
                .ok();

                // Wait for a decision (local or cloud). Subscribed before
                // open() above; ApprovalRouter::open also resolves inline if
                // a Decide already arrived for this id.

                loop {
                    let next = tokio::select! {
                        biased;
                        _ = self.cancel.cancelled() => {
                            bridge.interrupt().await.ok();
                            hist.append(&HistoryEntry::Lifecycle {
                                ts: Utc::now(),
                                state: "cancelled".into(),
                                detail: Some("cancelled while awaiting approval".into()),
                            })
                            .await
                            .ok();
                            return Ok(Some(Outcome {
                                status_label: "cancelled".into(),
                            }));
                        }
                        next = rx.recv() => next,
                    };
                    match next {
                        Ok(ApprovalRecord {
                            approval_id: aid,
                            status:
                                ApprovalStatus::Resolved {
                                    decision, source, ..
                                },
                            ..
                        }) if aid == approval_id => {
                            bridge.send_approval(&approval_id, decision).await.ok();
                            hist.append(&HistoryEntry::Approval {
                                ts: Utc::now(),
                                approval_id: approval_id.clone(),
                                status: format!("resolved:{source:?}:{decision:?}"),
                                payload: serde_json::Value::Null,
                            })
                            .await
                            .ok();
                            self.state.note_approvals_changed();
                            return Ok(None);
                        }
                        Ok(_) => continue,
                        Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => continue,
                        Err(_) => break,
                    }
                }
                Ok(None)
            }
            BridgeEvent::AwaitingReauth { run_id, detail } => {
                self.state
                    .set_run_status(RunnerStatus::AwaitingReauth)
                    .await;
                run_events.flush_before_lifecycle(&self.out).await;
                self.send(ClientMsg::RunAwaitingReauth {
                    run_id,
                    detail: detail.clone(),
                })
                .await;
                hist.append(&HistoryEntry::Lifecycle {
                    ts: Utc::now(),
                    state: "awaiting_reauth".into(),
                    detail,
                })
                .await
                .ok();
                Ok(None)
            }
            BridgeEvent::Completed {
                run_id,
                done_payload,
            } => {
                run_events.flush_before_lifecycle(&self.out).await;
                if let Some(branch) = git_work_branch {
                    // Completion still owns a cancellable Git subprocess: a
                    // remote or hook can hang after the agent has finished.
                    let pushed = tokio::select! {
                        biased;
                        _ = self.cancel.cancelled() => {
                            let _ = tokio::time::timeout(Duration::from_secs(2), bridge.interrupt()).await;
                            hist.append(&HistoryEntry::Lifecycle {
                                ts: Utc::now(),
                                state: "cancelled".into(),
                                detail: Some("cancelled while pushing work branch".into()),
                            }).await.ok();
                            return Ok(Some(Outcome { status_label: "cancelled".into() }));
                        }
                        result = crate::workspace::git::push_prepared_work_branch(workspace_root, branch) => result,
                    };
                    if let Err(err) = pushed {
                        let detail = format!("pushing git work branch `{}`: {err:#}", branch.name);
                        tracing::warn!(
                            %run_id,
                            git_work_branch = %branch.name,
                            error = %err,
                            "agent completed but its git branches could not be pushed",
                        );
                        let metadata = self.run_metadata().await;
                        self.send(ClientMsg::RunFailed {
                            run_id,
                            reason: FailureReason::WorkspaceSetup,
                            detail: Some(detail.clone()),
                            ended_at: Utc::now(),
                            tokens: metadata.tokens,
                            model: metadata.model,
                        })
                        .await;
                        hist.append(&HistoryEntry::Footer {
                            ts: Utc::now(),
                            final_status: "failed".into(),
                            done_payload: None,
                            error: Some(detail),
                        })
                        .await
                        .ok();
                        return Ok(Some(Outcome {
                            status_label: "failed".into(),
                        }));
                    }
                }
                let metadata = self.run_metadata().await;
                self.send(ClientMsg::RunCompleted {
                    run_id,
                    done_payload: done_payload.clone(),
                    ended_at: Utc::now(),
                    tokens: metadata.tokens,
                    model: metadata.model,
                })
                .await;
                hist.append(&HistoryEntry::Footer {
                    ts: Utc::now(),
                    final_status: "completed".into(),
                    done_payload: Some(done_payload),
                    error: None,
                })
                .await
                .ok();
                Ok(Some(Outcome {
                    status_label: "completed".into(),
                }))
            }
            BridgeEvent::Failed {
                run_id,
                reason,
                detail,
            } => {
                // Codex / claude-detected failures (e.g.
                // "turn/completed without conclusion", `error` notifications
                // with `willRetry=false`) reach us with whatever bare
                // string the bridge produced. Run them through the same
                // enrichment helper the watchdog and stdout-close paths
                // use so the user sees command-tool context and a
                // stderr tail in the issue activity comment, not just
                // the bridge's classifier text.
                let base = detail
                    .clone()
                    .unwrap_or_else(|| "agent reported failure".to_string());
                let enriched = self.build_failure_detail(&base, bridge).await;
                run_events.flush_before_lifecycle(&self.out).await;
                let metadata = self.run_metadata().await;
                self.send(ClientMsg::RunFailed {
                    run_id,
                    reason,
                    detail: Some(enriched.clone()),
                    ended_at: Utc::now(),
                    tokens: metadata.tokens,
                    model: metadata.model,
                })
                .await;
                hist.append(&HistoryEntry::Footer {
                    ts: Utc::now(),
                    final_status: "failed".into(),
                    done_payload: None,
                    error: Some(enriched),
                })
                .await
                .ok();
                Ok(Some(Outcome {
                    status_label: "failed".into(),
                }))
            }
        }
    }

    async fn send(&self, msg: ClientMsg) {
        if let Err(err) = self.out.send(msg).await {
            tracing::warn!(error = %err, "failed to send runner lifecycle message");
        }
    }
}

struct Outcome {
    status_label: String,
}

/// Best-effort: if codex hands us a non-UUID approval id (it shouldn't, per
/// schema), derive a stable v5 UUID from it so the cloud sees the same id on
/// retries instead of a fresh random one each time.
fn uuid_or(s: &str) -> uuid::Uuid {
    if let Ok(u) = uuid::Uuid::parse_str(s) {
        return u;
    }
    uuid::Uuid::new_v5(&uuid::Uuid::NAMESPACE_OID, s.as_bytes())
}

// End-to-end issue-run tests that drive a real `AssignWorker` against a
// fake agent binary. Kept in their own file under `daemon/tests/`.
#[cfg(test)]
#[path = "tests/issue_run_agent_env.rs"]
mod issue_run_agent_env_tests;

#[cfg(all(test, unix))]
#[path = "tests/karma_workspace_retention.rs"]
mod karma_workspace_retention_tests;

#[cfg(test)]
mod tests {
    //! In-module tests: cover the Supervisor's *internal* helpers that
    //! integration tests in ``runner/tests/`` cannot reach (private
    //! functions). The wire-format / connection-loop side is covered by
    //! ``runner/tests/cloud_ws_fake.rs``.

    use super::*;
    use crate::cloud::protocol::Envelope;
    use crate::config::schema::{
        AgentSection, ApprovalPolicySection, ClaudeCodeSection, CodexSection, CursorAgentSection,
        GrokSection, MuseCodeSection, OpenClawSection, RunnerConfig, WorkspaceSection,
    };
    use crate::daemon::state::ExecCommandSnapshot;
    use chrono::TimeZone;
    use std::path::PathBuf;
    use tokio::sync::Notify;

    pub(super) fn paths_for(root: &std::path::Path) -> Paths {
        Paths {
            config_dir: root.join("config"),
            data_dir: root.join("data"),
            runtime_dir: root.join("runtime"),
        }
    }

    pub(super) fn runner_config(
        name: &str,
        project_slug: &str,
        working_dir: PathBuf,
    ) -> RunnerConfig {
        RunnerConfig {
            name: name.into(),
            runner_id: uuid::Uuid::new_v4(),
            workspace_slug: Some("acme".into()),
            project_slug: Some(project_slug.into()),
            project_id: None,
            binding_source: None,
            pod_id: None,
            workspace: WorkspaceSection { working_dir },
            agent: AgentSection::default(),
            codex: CodexSection::default(),
            claude_code: ClaudeCodeSection::default(),
            cursor_agent: CursorAgentSection::default(),
            openclaw: OpenClawSection::default(),
            grok: GrokSection::default(),
            muse_code: MuseCodeSection::default(),
            karma: crate::config::schema::KarmaSection::default(),
            approval_policy: ApprovalPolicySection::default(),
        }
    }

    #[tokio::test]
    async fn runner_credentials_follow_workspace_and_support_legacy_default() {
        use crate::config::schema::CliSection;
        let tmp = tempfile::tempdir().unwrap();
        let paths = paths_for(tmp.path());
        let primary = runner_config("primary", "WEB", tmp.path().join("primary"));
        let mut secondary = runner_config("secondary", "WEB", tmp.path().join("secondary"));
        secondary.workspace_slug = Some("shared".into());
        let config = Config {
            version: 2,
            daemon: DaemonConfig::default(),
            cli: Some(CliSection {
                token: Some("mt_primary".into()),
                workspace_tokens: [("shared".into(), "mt_shared".into())].into(),
                ..Default::default()
            }),
            runners: vec![primary.clone(), secondary.clone()],
        };
        assert_eq!(
            machine_token_for_runner(&config, &primary).unwrap(),
            Some("mt_primary")
        );
        let token = machine_token_for_runner(&config, &secondary).unwrap();
        assert_eq!(token, Some("mt_shared"));
        let credentials = load_runner_credentials(
            &paths.for_runner(secondary.runner_id),
            &secondary.name,
            token,
        )
        .await
        .unwrap();
        let client = RunnerCloudClient::new(
            secondary.runner_id,
            credentials,
            SharedHttpTransport::new("http://127.0.0.1:1".into()).unwrap(),
        );
        assert_eq!(client.ensure_access_token().await.unwrap().raw, "mt_shared");
        let mut missing = secondary.clone();
        missing.workspace_slug = Some("unknown".into());
        assert!(machine_token_for_runner(&config, &missing).is_err());
    }

    #[test]
    fn inactive_runner_cleanup_removes_only_runner_specific_data() {
        let tmp = tempfile::tempdir().unwrap();
        let paths = paths_for(tmp.path());
        let target = uuid::Uuid::new_v4();
        let unrelated = uuid::Uuid::new_v4();
        let target_dir = paths.for_runner(target).base_dir().to_path_buf();
        let unrelated_dir = paths.for_runner(unrelated).base_dir().to_path_buf();
        std::fs::create_dir_all(&target_dir).unwrap();
        std::fs::create_dir_all(&unrelated_dir).unwrap();
        std::fs::write(target_dir.join("credentials.toml"), "secret").unwrap();
        std::fs::write(unrelated_dir.join("history.jsonl"), "keep").unwrap();

        remove_runner_data_dir(&paths, target).unwrap();

        assert!(!target_dir.exists());
        assert!(unrelated_dir.exists());
        assert!(unrelated_dir.join("history.jsonl").exists());
    }

    #[test]
    fn concurrent_runner_data_cleanup_is_idempotent() {
        // A cloud RemoveRunner frame and the snapshot reconciler can clean the
        // same runner concurrently. Every caller must observe success even if
        // another caller removes the tree between its metadata check and
        // `remove_dir_all`.
        let tmp = tempfile::tempdir().unwrap();
        let paths = Arc::new(paths_for(tmp.path()));
        let runner_id = uuid::Uuid::new_v4();
        let runner_dir = paths.for_runner(runner_id).base_dir().to_path_buf();
        std::fs::create_dir_all(runner_dir.join("nested")).unwrap();
        for index in 0..256 {
            std::fs::write(
                runner_dir.join("nested").join(format!("state-{index}")),
                b"state",
            )
            .unwrap();
        }

        let workers = 16;
        let barrier = Arc::new(std::sync::Barrier::new(workers));
        let handles: Vec<_> = (0..workers)
            .map(|_| {
                let paths = Arc::clone(&paths);
                let barrier = Arc::clone(&barrier);
                std::thread::spawn(move || {
                    barrier.wait();
                    remove_runner_data_dir(&paths, runner_id)
                })
            })
            .collect();

        for handle in handles {
            handle
                .join()
                .expect("cleanup worker panicked")
                .expect("duplicate cleanup must be a no-op");
        }
        assert!(!runner_dir.exists());
    }

    #[tokio::test]
    async fn runner_teardown_deletes_data_only_after_worker_finishes() {
        // A cancelled AssignWorker persists RunsIndex as its final action.
        // If RemoveRunner deletes the runtime tree before waiting for that
        // worker, RunsIndex::save recreates the supposedly released runner
        // directory after teardown has completed.
        let tmp = tempfile::tempdir().unwrap();
        let paths = paths_for(tmp.path());
        let runner_id = uuid::Uuid::new_v4();
        let runner_paths = paths.for_runner(runner_id);
        runner_paths.ensure().unwrap();

        let cancel = tokio_util::sync::CancellationToken::new();
        let worker_cancel = cancel.clone();
        let worker_paths = runner_paths.clone();
        let (done_tx, done_rx) = oneshot::channel();
        let worker = tokio::spawn(async move {
            worker_cancel.cancelled().await;
            let mut index = RunsIndex::default();
            index.upsert(RunSummary {
                run_id: uuid::Uuid::new_v4(),
                work_item_id: None,
                status: "cancelled".into(),
                started_at: Utc::now(),
                ended_at: Some(Utc::now()),
                title: None,
            });
            index.save(&worker_paths).unwrap();
            let _ = done_tx.send(());
        });
        let mut current_run = Some(CurrentRun {
            run_id: uuid::Uuid::new_v4(),
            cancel,
            done_rx,
        });
        let mut current_chat = None;

        finish_runner_teardown(&paths, runner_id, None, &mut current_run, &mut current_chat)
            .await
            .unwrap();
        worker.await.unwrap();

        assert!(current_run.is_none());
        assert!(!runner_paths.base_dir().exists());
    }

    #[test]
    fn resolve_working_dir_absolutizes_runner_path() {
        let tmp = tempfile::tempdir().unwrap();
        let paths = paths_for(tmp.path());
        let relative = PathBuf::from(format!("relative-wd-{}", uuid::Uuid::new_v4()));
        let expected = std::path::absolute(&relative).unwrap();
        let (out_tx, _out_rx) = mpsc::channel::<Envelope<ClientMsg>>(1);
        let inst = RunnerInstance::new(runner_config("main", "WEB", relative), &paths, out_tx);

        assert_eq!(resolve_working_dir(&inst), Some(expected));
    }

    #[tokio::test]
    async fn git_context_problems_do_not_block_normal_agent_spawn() {
        for (case, seed_user_file, repo_url) in [
            ("no-repository", true, None),
            (
                "populated-task-folder",
                true,
                Some("https://example.com/repo.git"),
            ),
            ("unsupported-url", false, Some("file:///must-not-clone")),
            (
                "clone-failure",
                false,
                Some("https://127.0.0.1:1/unreachable.git"),
            ),
        ] {
            for kind in [AgentKind::Codex, AgentKind::ClaudeCode] {
                let tmp = tempfile::tempdir().unwrap();
                let paths = paths_for(tmp.path());
                paths.ensure().unwrap();
                let wd = tmp.path().join(format!("task-folder-{case}"));
                std::fs::create_dir_all(&wd).unwrap();
                if seed_user_file {
                    std::fs::write(wd.join("notes.txt"), "preserve user content").unwrap();
                }
                let mut config = runner_config("general-tasks", "TEST", wd.clone());
                config.project_slug = None; // no cloud context lookup in this test
                config.agent.kind = kind;
                let missing_binary = tmp
                    .path()
                    .join("missing-agent")
                    .to_string_lossy()
                    .into_owned();
                config.codex.binary = missing_binary.clone();
                config.claude_code.binary = missing_binary;
                let runner_paths = paths.for_runner(config.runner_id);
                runner_paths.ensure().unwrap();
                let (tx, mut rx) = mpsc::channel(16);
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
                    cancel: tokio_util::sync::CancellationToken::new(),
                };
                worker
                    .run(AssignedRun {
                        run_id: uuid::Uuid::new_v4(),
                        issue_identifier: None,
                        project_name: None,
                        prompt: "Summarize these notes".into(),
                        repo_url: repo_url.map(str::to_string),
                        repo_ref: None,
                        git_work_branch: None,
                        expected_codex_model: None,
                    })
                    .await
                    .unwrap();
                assert!(matches!(
                    rx.recv().await.unwrap().body,
                    ClientMsg::Accept { .. }
                ));
                // Deliberately absent engine proves the assignment got past
                // workspace setup and reached the agent-specific spawn path.
                let failure = rx.recv().await.unwrap().body;
                assert!(matches!(
                    failure,
                    ClientMsg::RunFailed {
                        reason: FailureReason::CodexCrash | FailureReason::AgentCrash,
                        ..
                    }
                ));
                if seed_user_file {
                    assert_eq!(
                        std::fs::read_to_string(wd.join("notes.txt")).unwrap(),
                        "preserve user content",
                        "{case} changed user content for {kind:?}"
                    );
                }
                assert!(!wd.join(".git").exists(), "{case} cloned for {kind:?}");
            }
        }
    }

    #[tokio::test]
    async fn chat_and_run_resolve_to_the_same_working_dir() {
        // One exclusive dir per runner: the chat lane resolves to exactly the
        // runner's `workspace.working_dir` — the same directory an issue run
        // resolves — with no separate chat workspace path.
        let tmp = tempfile::tempdir().unwrap();
        let wd = tmp.path().join("the-one-dir");
        let config = runner_config("main", "WEB", wd.clone());
        let (tx, _rx) = mpsc::channel::<Envelope<ClientMsg>>(1);
        let (_cmd_tx, cmd_rx) = mpsc::channel(1);
        let (active_tx, _active_rx) = watch::channel(false);
        let mut chat = ChatWorker {
            runner_config: config.clone(),
            state: StateHandle::new(Config {
                version: 2,
                daemon: Default::default(),
                runners: vec![config.clone()],
                cli: None,
            }),
            approvals: ApprovalRouter::new(),
            out: RunnerOut::new(config.runner_id, tx),
            command_rx: cmd_rx,
            active_tx,
        };
        let chat_dir = chat
            .resolve_chat_workspace(uuid::Uuid::new_v4(), None)
            .await
            .unwrap();
        assert_eq!(chat_dir, wd);

        // The issue-run path resolves the very same directory (non-git folder).
        let run_dir = match crate::workspace::resolve(&config.workspace.working_dir, None)
            .await
            .unwrap()
        {
            crate::workspace::Resolution::Directory(p)
            | crate::workspace::Resolution::ExistingRepo(p)
            | crate::workspace::Resolution::Cloned(p) => p,
        };
        assert_eq!(run_dir, chat_dir);
    }

    #[tokio::test]
    async fn cancel_without_active_process_is_acknowledged_immediately() {
        let runner_id = uuid::Uuid::new_v4();
        let run_id = uuid::Uuid::new_v4();
        let (tx, mut rx) = mpsc::channel(1);
        let out = RunnerOut::new(runner_id, tx);

        acknowledge_cancel_without_active_process(&out, run_id).await;

        let env = rx
            .recv()
            .await
            .expect("expected RunCancelled acknowledgement");
        assert_eq!(env.runner_id, Some(runner_id));
        match env.body {
            ClientMsg::RunCancelled {
                run_id: acknowledged_run_id,
                tokens,
                model,
                ..
            } => {
                assert_eq!(acknowledged_run_id, run_id);
                assert_eq!(tokens, None);
                assert_eq!(model, None);
            }
            other => panic!("expected RunCancelled, got {other:?}"),
        }
    }

    #[test]
    fn format_exec_command_detail_does_not_imply_completed_command_is_running() {
        let started_at = Utc.with_ymd_and_hms(2026, 5, 27, 9, 11, 0).unwrap();
        let completed_at = Utc.with_ymd_and_hms(2026, 5, 27, 9, 11, 4).unwrap();
        let now = Utc.with_ymd_and_hms(2026, 5, 27, 9, 44, 0).unwrap();
        let detail = format_exec_command_detail(
            &ExecCommandSnapshot {
                command: "cat .karma-team-workpad.md".into(),
                cwd: None,
                tool_call_id: Some("tool-1".into()),
                started_at,
                completed_at: Some(completed_at),
                completed_success: Some(true),
            },
            now,
        );

        assert!(detail.contains("completed 1976s before failure"));
        assert!(detail.contains("ran 4s"));
        assert!(!detail.contains("no completion event observed"));
    }

    #[test]
    fn format_exec_command_detail_says_when_completion_was_not_seen() {
        let started_at = Utc.with_ymd_and_hms(2026, 5, 27, 9, 11, 0).unwrap();
        let now = Utc.with_ymd_and_hms(2026, 5, 27, 9, 12, 9).unwrap();
        let detail = format_exec_command_detail(
            &ExecCommandSnapshot {
                command: "git fetch origin".into(),
                cwd: Some("/tmp/repo".into()),
                tool_call_id: Some("tool-2".into()),
                started_at,
                completed_at: None,
                completed_success: None,
            },
            now,
        );

        assert_eq!(
            detail,
            "command context: last command frame `git fetch origin` in `/tmp/repo` \
             (started 69s ago; no completion frame observed)"
        );
    }

    #[test]
    fn format_exec_command_detail_distinguishes_errored_completion_from_clean_one() {
        // A command whose completion event reported a non-success
        // terminal status (codex `failed`, Claude `is_error: true`) must
        // not be described as a clean "completion" — that wording reads
        // like success, which can be just as misleading as the original
        // "still running" bug this PR fixes.
        let started_at = Utc.with_ymd_and_hms(2026, 5, 27, 9, 11, 0).unwrap();
        let completed_at = Utc.with_ymd_and_hms(2026, 5, 27, 9, 11, 4).unwrap();
        let now = Utc.with_ymd_and_hms(2026, 5, 27, 9, 12, 0).unwrap();
        let detail = format_exec_command_detail(
            &ExecCommandSnapshot {
                command: "npm test".into(),
                cwd: None,
                tool_call_id: Some("tool-x".into()),
                started_at,
                completed_at: Some(completed_at),
                completed_success: Some(false),
            },
            now,
        );

        assert!(detail.contains("exited with error 56s before failure"));
        assert!(detail.contains("ran 4s"));
        assert!(!detail.contains("completed 56s"));
    }

    #[test]
    fn diagnostic_signal_prefers_json_error_messages() {
        let lines = vec![
            "karma-team 0.1.10".to_string(),
            "Process exited with code 1".to_string(),
            r#"{"error":"GET https://karma-team.example.com/api/v1/users/me/: error sending request"}"#.to_string(),
        ];
        let signal = diagnostic_signal_from_stderr(&lines).unwrap();
        assert_eq!(
            signal,
            "stderr JSON error: GET https://karma-team.example.com/api/v1/users/me/: error sending request"
        );
    }

    #[test]
    fn diagnostic_signal_keeps_non_json_errors() {
        let lines = vec![
            "karma-team 0.1.10".to_string(),
            "gh: error: HTTP 401: Bad credentials".to_string(),
        ];
        let signal = diagnostic_signal_from_stderr(&lines).unwrap();
        assert_eq!(signal, "stderr: gh: error: HTTP 401: Bad credentials");
    }

    #[test]
    fn diagnostic_signal_prefers_substantive_error_over_exit_status() {
        let lines = vec![
            "gh: error: HTTP 401: Bad credentials".to_string(),
            "Process exited with code 1".to_string(),
        ];
        let signal = diagnostic_signal_from_stderr(&lines).unwrap();
        assert_eq!(signal, "stderr: gh: error: HTTP 401: Bad credentials");

        let fallback = diagnostic_signal_from_stderr(&["Process exited with code 1".to_string()])
            .expect("exit status should remain useful as a fallback");
        assert_eq!(fallback, "stderr: Process exited with code 1");
    }

    #[test]
    fn assemble_failure_detail_puts_core_error_first_and_trace_second() {
        let started_at = Utc.with_ymd_and_hms(2026, 5, 29, 5, 25, 53).unwrap();
        let now = Utc.with_ymd_and_hms(2026, 5, 29, 5, 30, 57).unwrap();
        let cmd = ExecCommandSnapshot {
            command: "karma-team workspace me".into(),
            cwd: Some("/work".into()),
            tool_call_id: Some("call-1".into()),
            started_at,
            completed_at: None,
            completed_success: None,
        };
        let stderr = crate::agent::StderrSnapshot {
            lines: vec![
                "karma-team 0.1.10".into(),
                "Process exited with code 1".into(),
                r#"{"error":"GET https://karma-team.example.com/api/v1/users/me/: error sending request"}"#.into(),
            ],
            dropped: 42,
        };

        let detail = assemble_failure_detail(
            "agent stalled for 5 minutes without new agent events",
            Some(&cmd),
            &stderr,
            now,
        );

        let mut lines = detail.lines();
        assert_eq!(
            lines.next(),
            Some(
                "Core error: GET https://karma-team.example.com/api/v1/users/me/: error sending request"
            )
        );
        assert!(detail.contains("\n\nError trace:\n"));
        assert!(
            detail.contains("- watchdog: agent stalled for 5 minutes without new agent events")
        );
        assert!(detail.contains("- diagnostic signal: stderr JSON error: GET https://karma-team.example.com/api/v1/users/me/: error sending request"));
        assert!(detail.contains(
            "- command context: last command frame `karma-team workspace me` in `/work`"
        ));
        assert!(detail.contains(
            "- stderr context (3 line(s) (plus 42 low-signal line(s) filtered from agent stderr))"
        ));
    }

    #[test]
    fn assemble_failure_detail_uses_watchdog_when_no_better_signal_exists() {
        let stderr = crate::agent::StderrSnapshot::default();
        let detail = assemble_failure_detail("agent stdout closed", None, &stderr, Utc::now());

        assert!(detail.starts_with("Core error: agent stdout closed\n\nError trace:\n"));
        assert!(detail.contains("- watchdog: agent stdout closed"));
    }

    /// Build the supervisor's ``hello_runners`` map from a list of
    /// ``RunnerInstance`` exactly the way ``Supervisor::run`` does, then
    /// drive ``hello_emitter`` once and assert each instance's Hello
    /// frame carries the correct per-runner ``project_slug``.
    ///
    /// This is the integration point the original review asked for:
    /// production code calls ``hello_emitter`` to construct the frames
    /// the cloud cross-checks against ``runner.pod.project``. If the
    /// supervisor were to misroute ``project_slug`` between instances,
    /// this test catches it. The wire-format-only test in
    /// ``cloud_ws_fake.rs`` cannot.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn hello_emitter_emits_one_hello_per_instance_with_project_slug() {
        let tmp = tempfile::tempdir().unwrap();
        let paths = paths_for(tmp.path());
        let (out_tx, mut out_rx) = mpsc::channel::<Envelope<ClientMsg>>(8);

        // Two instances, distinct project_slugs and disjoint working_dirs
        // (`Config::validate()` rejects overlap; we mirror that here).
        let inst_web = RunnerInstance::new(
            runner_config("laptop-web", "WEB", tmp.path().join("wd-web")),
            &paths,
            out_tx.clone(),
        );
        let inst_api = RunnerInstance::new(
            runner_config("laptop-api", "API", tmp.path().join("wd-api")),
            &paths,
            out_tx.clone(),
        );
        let runner_web = inst_web.runner_id;
        let runner_api = inst_api.runner_id;

        let runners: Arc<RwLock<HelloRunnerMap>> = Arc::new(RwLock::new(
            [&inst_web, &inst_api]
                .into_iter()
                .map(|i| {
                    (
                        i.runner_id,
                        (
                            i.out.clone(),
                            i.state.clone(),
                            i.config.project_slug.clone(),
                        ),
                    )
                })
                .collect(),
        ));

        let connected = Arc::new(Notify::new());
        let connected_for_task = connected.clone();
        // Daemon-level state handle — the test only cares about Hello
        // emission, but the emitter signature now takes a StateHandle so
        // the IPC's connected flag can be flipped on each WS handshake.
        let daemon_state = StateHandle::new(crate::config::schema::Config {
            version: 2,
            daemon: Default::default(),
            runners: vec![],
            cli: None,
        });
        let task = tokio::spawn(async move {
            hello_emitter(runners, connected_for_task, daemon_state).await;
        });

        connected.notify_one();

        // Collect both Hellos. Supervisor iterates the map; HashMap order
        // is unspecified so we sort by runner_id before asserting.
        let mut received: Vec<(uuid::Uuid, Option<String>)> = Vec::new();
        for _ in 0..2 {
            let env = tokio::time::timeout(std::time::Duration::from_secs(2), out_rx.recv())
                .await
                .expect("timed out waiting for Hello")
                .expect("channel closed before Hello arrived");
            assert_eq!(env.runner_id, env.runner_id); // sanity
            match env.body {
                ClientMsg::Hello {
                    runner_id,
                    project_slug,
                    ..
                } => {
                    received.push((runner_id, project_slug));
                }
                other => panic!("expected Hello, got {other:?}"),
            }
        }

        task.abort();

        received.sort_by_key(|(id, _)| *id);
        let mut expected: Vec<(uuid::Uuid, Option<String>)> = vec![
            (runner_web, Some("WEB".into())),
            (runner_api, Some("API".into())),
        ];
        expected.sort_by_key(|(id, _)| *id);
        assert_eq!(received, expected);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn hello_emitter_reads_project_slug_after_runtime_rename() {
        let tmp = tempfile::tempdir().unwrap();
        let paths = paths_for(tmp.path());
        let (out_tx, mut out_rx) = mpsc::channel::<Envelope<ClientMsg>>(4);
        let inst = RunnerInstance::new(
            runner_config("laptop-web", "OLD_WEB", tmp.path().join("wd-web")),
            &paths,
            out_tx,
        );
        let runners: Arc<RwLock<HelloRunnerMap>> = Arc::new(RwLock::new(
            std::iter::once((
                inst.runner_id,
                (
                    inst.out.clone(),
                    inst.state.clone(),
                    inst.config.project_slug.clone(),
                ),
            ))
            .collect(),
        ));
        inst.state.set_project_slug(Some("WEB".into()));

        let connected = Arc::new(Notify::new());
        let task = tokio::spawn(hello_emitter(
            runners,
            connected.clone(),
            StateHandle::new(crate::config::schema::Config {
                version: 2,
                daemon: Default::default(),
                runners: vec![],
                cli: None,
            }),
        ));
        connected.notify_one();

        let env = tokio::time::timeout(Duration::from_secs(2), out_rx.recv())
            .await
            .expect("timed out waiting for Hello")
            .expect("channel closed before Hello arrived");
        task.abort();
        match env.body {
            ClientMsg::Hello { project_slug, .. } => {
                assert_eq!(project_slug.as_deref(), Some("WEB"));
            }
            other => panic!("expected Hello, got {other:?}"),
        }
    }

    #[test]
    fn spawn_context_refreshes_live_instance_project_metadata() {
        let tmp = tempfile::tempdir().unwrap();
        let paths = paths_for(tmp.path());
        let (out_tx, _out_rx) = mpsc::channel::<Envelope<ClientMsg>>(4);
        let inst = RunnerInstance::new(
            runner_config("laptop-web", "OLD_WEB", tmp.path().join("wd-web")),
            &paths,
            out_tx,
        );
        let runner_id = inst.runner_id;
        let state = inst.state.clone();
        let mut renamed = inst.config.clone();
        let project_id = uuid::Uuid::new_v4();
        renamed.project_slug = Some("WEB".into());
        renamed.project_id = Some(project_id);

        let ctx = RunnerSpawnCtx {
            transport: None,
            shared_machine_token: None,
            paths,
            daemon: Default::default(),
            mailboxes: Arc::new(RwLock::new(HashMap::new())),
            hello_runners: Arc::new(RwLock::new(HashMap::new())),
            daemon_state: StateHandle::new(Config {
                version: 2,
                daemon: Default::default(),
                runners: vec![],
                cli: None,
            }),
            instances: Arc::new(std::sync::RwLock::new(HashMap::from([(runner_id, inst)]))),
            activation_lock: Arc::new(tokio::sync::Mutex::new(())),
            accepting_mutations: Arc::new(AtomicBool::new(true)),
        };

        assert_eq!(ctx.refresh_project_metadata(&[renamed]), 1);
        assert_eq!(state.project_slug_now().as_deref(), Some("WEB"));
        let instances = ctx.instances.read().expect("runner registry poisoned");
        let live = instances.get(&runner_id).expect("live runner");
        assert_eq!(live.config.project_slug.as_deref(), Some("WEB"));
        assert_eq!(live.config.project_id, Some(project_id));
    }

    #[tokio::test]
    async fn shutdown_freeze_rejects_late_runner_activation() {
        let tmp = tempfile::tempdir().unwrap();
        let paths = paths_for(tmp.path());
        paths.ensure().unwrap();
        let runner = runner_config("late-runner", "LATE", tmp.path().join("wd-late"));
        let runner_id = runner.runner_id;
        let ctx = RunnerSpawnCtx {
            transport: None,
            shared_machine_token: None,
            paths,
            daemon: Default::default(),
            mailboxes: Arc::new(RwLock::new(HashMap::new())),
            hello_runners: Arc::new(RwLock::new(HashMap::new())),
            daemon_state: StateHandle::new(Config {
                version: 2,
                daemon: Default::default(),
                runners: vec![],
                cli: None,
            }),
            instances: Arc::new(std::sync::RwLock::new(HashMap::new())),
            activation_lock: Arc::new(tokio::sync::Mutex::new(())),
            accepting_mutations: Arc::new(AtomicBool::new(true)),
        };

        // Freeze must serialize behind a mutation already in its critical
        // section, then permanently reject work that arrives afterwards.
        let mutation_guard = ctx.activation_lock.lock().await;
        let freeze_ctx = ctx.clone();
        let freeze = tokio::spawn(async move { freeze_ctx.freeze_mutations().await });
        tokio::task::yield_now().await;
        assert!(ctx.accepting_mutations.load(Ordering::SeqCst));
        drop(mutation_guard);
        freeze.await.unwrap();

        let error = ctx.add_runner(runner).await.unwrap_err().to_string();

        assert!(error.contains("shutting down"), "{error}");
        assert!(
            !ctx.instances
                .read()
                .expect("runner registry poisoned")
                .contains_key(&runner_id)
        );
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn concurrent_live_remove_waits_for_teardown_and_is_idempotent() {
        let tmp = tempfile::tempdir().unwrap();
        let paths = paths_for(tmp.path());
        paths.ensure().unwrap();
        let working_dir = tmp.path().join("wd");
        std::fs::create_dir_all(&working_dir).unwrap();
        std::fs::write(working_dir.join("checkout-marker"), b"preserve").unwrap();
        let runner = runner_config("remove-once", "WEB", working_dir.clone());
        let runner_id = runner.runner_id;
        let config = Config {
            version: 2,
            daemon: Default::default(),
            runners: vec![runner.clone()],
            cli: None,
        };
        crate::config::file::write_config(&paths, &config).unwrap();

        let (out_tx, _out_rx) = mpsc::channel::<Envelope<ClientMsg>>(4);
        let inst = RunnerInstance::new(runner, &paths, out_tx);
        let runner_dir = inst.paths.base_dir().to_path_buf();
        inst.paths.ensure().unwrap();
        std::fs::write(runner_dir.join("owned-by-live-loop"), b"keep-until-done").unwrap();

        let mailbox_rx = inst.take_mailbox_rx().await.unwrap();
        let mailboxes = Arc::new(RwLock::new(HashMap::from([(
            runner_id,
            inst.mailbox_tx.clone(),
        )])));
        let hello_runners = Arc::new(RwLock::new(HashMap::new()));
        let instances = Arc::new(std::sync::RwLock::new(HashMap::from([(
            runner_id,
            inst.clone(),
        )])));
        let cancel = tokio_util::sync::CancellationToken::new();
        let worker_cancel = cancel.clone();
        let cancel_observed = Arc::new(Notify::new());
        let worker_cancel_observed = cancel_observed.clone();
        let release_worker = Arc::new(Notify::new());
        let worker_release = release_worker.clone();
        let (done_tx, done_rx) = oneshot::channel();
        let worker = tokio::spawn(async move {
            worker_cancel.cancelled().await;
            worker_cancel_observed.notify_one();
            worker_release.notified().await;
            let _ = done_tx.send(());
        });
        let runner_loop = RunnerLoop {
            runner_paths: inst.paths.clone(),
            paths: paths.clone(),
            runner_config: inst.config.clone(),
            out: inst.out.clone(),
            state: inst.state.clone(),
            approvals: inst.approvals.clone(),
            inbound: mailbox_rx,
            ack_tx: inst.ack_tx.clone(),
            remove_tx: inst.remove_tx.clone(),
            teardown_complete_tx: inst.teardown_complete_tx.clone(),
            client: None,
            local_chat_sessions: inst.chat_sessions.clone(),
            live_mailboxes: mailboxes.clone(),
            live_hello_runners: hello_runners.clone(),
            live_instances: instances.clone(),
            current_run: Some(CurrentRun {
                run_id: uuid::Uuid::new_v4(),
                cancel,
                done_rx,
            }),
            current_chat: None,
            local_chat_active: inst.chat_active.clone(),
        };
        let runner_loop_task = tokio::spawn(runner_loop.run());
        let ctx = RunnerSpawnCtx {
            transport: None,
            shared_machine_token: None,
            paths: paths.clone(),
            daemon: Default::default(),
            mailboxes,
            hello_runners,
            daemon_state: StateHandle::new(config.clone()),
            instances,
            activation_lock: Arc::new(tokio::sync::Mutex::new(())),
            accepting_mutations: Arc::new(AtomicBool::new(true)),
        };

        let first_ctx = ctx.clone();
        let first = tokio::spawn(async move { first_ctx.remove_runner(runner_id).await });
        let second_ctx = ctx.clone();
        let second = tokio::spawn(async move { second_ctx.remove_runner(runner_id).await });
        tokio::time::timeout(Duration::from_secs(2), cancel_observed.notified())
            .await
            .expect("runner worker was not cancelled");
        tokio::task::yield_now().await;

        assert!(
            runner_dir.exists(),
            "runtime must remain until the worker finishes"
        );
        assert!(
            crate::config::file::load_config(&paths)
                .unwrap()
                .runners
                .iter()
                .any(|runner| runner.runner_id == runner_id),
            "config must remain discoverable until runtime cleanup completes"
        );
        assert!(!first.is_finished(), "remove returned before teardown");
        assert!(
            !second.is_finished(),
            "concurrent remove returned before teardown"
        );

        release_worker.notify_one();
        tokio::time::timeout(Duration::from_secs(2), first)
            .await
            .expect("first remove timed out")
            .unwrap()
            .unwrap();
        tokio::time::timeout(Duration::from_secs(2), second)
            .await
            .expect("second remove timed out")
            .unwrap()
            .unwrap();
        worker.await.unwrap();
        runner_loop_task.await.unwrap().unwrap();

        assert!(!runner_dir.exists());
        assert!(working_dir.join("checkout-marker").exists());
        assert!(
            crate::config::file::load_config(&paths)
                .unwrap()
                .runners
                .is_empty()
        );
        assert!(
            !ctx.instances
                .read()
                .expect("runner registry poisoned")
                .contains_key(&runner_id)
        );

        // A redelivered command after the actor has exited is a no-op success.
        ctx.remove_runner(runner_id).await.unwrap();
    }

    #[tokio::test]
    async fn teardown_failure_keeps_config_and_inactive_retry_succeeds() {
        let tmp = tempfile::tempdir().unwrap();
        let paths = paths_for(tmp.path());
        paths.ensure().unwrap();
        let working_dir = tmp.path().join("wd");
        std::fs::create_dir_all(&working_dir).unwrap();
        std::fs::write(working_dir.join("checkout-marker"), b"preserve").unwrap();
        let runner = runner_config("retry-remove", "WEB", working_dir.clone());
        let runner_id = runner.runner_id;
        let config = Config {
            version: 2,
            daemon: Default::default(),
            runners: vec![runner.clone()],
            cli: None,
        };
        crate::config::file::write_config(&paths, &config).unwrap();

        let (out_tx, _out_rx) = mpsc::channel::<Envelope<ClientMsg>>(4);
        let inst = RunnerInstance::new(runner, &paths, out_tx);
        let runner_dir = inst.paths.base_dir().to_path_buf();
        std::fs::create_dir_all(runner_dir.parent().unwrap()).unwrap();
        std::fs::write(&runner_dir, b"not-a-directory").unwrap();
        let mailbox_rx = inst.take_mailbox_rx().await.unwrap();
        let mailboxes = Arc::new(RwLock::new(HashMap::from([(
            runner_id,
            inst.mailbox_tx.clone(),
        )])));
        let hello_runners = Arc::new(RwLock::new(HashMap::new()));
        let instances = Arc::new(std::sync::RwLock::new(HashMap::from([(
            runner_id,
            inst.clone(),
        )])));
        let runner_loop = RunnerLoop {
            runner_paths: inst.paths.clone(),
            paths: paths.clone(),
            runner_config: inst.config.clone(),
            out: inst.out.clone(),
            state: inst.state.clone(),
            approvals: inst.approvals.clone(),
            inbound: mailbox_rx,
            ack_tx: inst.ack_tx.clone(),
            remove_tx: inst.remove_tx.clone(),
            teardown_complete_tx: inst.teardown_complete_tx.clone(),
            client: None,
            local_chat_sessions: inst.chat_sessions.clone(),
            live_mailboxes: mailboxes.clone(),
            live_hello_runners: hello_runners.clone(),
            live_instances: instances.clone(),
            current_run: None,
            current_chat: None,
            local_chat_active: inst.chat_active.clone(),
        };
        let runner_loop_task = tokio::spawn(runner_loop.run());
        let ctx = RunnerSpawnCtx {
            transport: None,
            shared_machine_token: None,
            paths: paths.clone(),
            daemon: Default::default(),
            mailboxes,
            hello_runners,
            daemon_state: StateHandle::new(config),
            instances,
            activation_lock: Arc::new(tokio::sync::Mutex::new(())),
            accepting_mutations: Arc::new(AtomicBool::new(true)),
        };

        let error = ctx.remove_runner(runner_id).await.unwrap_err().to_string();
        assert!(error.contains("runner teardown failed"), "{error}");
        runner_loop_task.await.unwrap().unwrap();
        assert!(runner_dir.is_file());
        assert!(working_dir.join("checkout-marker").exists());
        assert!(
            crate::config::file::load_config(&paths)
                .unwrap()
                .runners
                .iter()
                .any(|runner| runner.runner_id == runner_id)
        );
        assert!(
            !ctx.instances
                .read()
                .expect("runner registry poisoned")
                .contains_key(&runner_id)
        );

        std::fs::remove_file(&runner_dir).unwrap();
        ctx.remove_runner(runner_id).await.unwrap();
        assert!(
            crate::config::file::load_config(&paths)
                .unwrap()
                .runners
                .is_empty()
        );
        assert!(working_dir.join("checkout-marker").exists());
    }

    #[tokio::test]
    async fn closed_live_mailbox_is_evicted_and_inactive_retry_succeeds() {
        let tmp = tempfile::tempdir().unwrap();
        let paths = paths_for(tmp.path());
        paths.ensure().unwrap();
        let working_dir = tmp.path().join("wd");
        std::fs::create_dir_all(&working_dir).unwrap();
        std::fs::write(working_dir.join("checkout-marker"), b"preserve").unwrap();
        let runner = runner_config("closed-mailbox", "WEB", working_dir.clone());
        let runner_id = runner.runner_id;
        let config = Config {
            version: 2,
            daemon: Default::default(),
            runners: vec![runner.clone()],
            cli: None,
        };
        crate::config::file::write_config(&paths, &config).unwrap();

        let (out_tx, _out_rx) = mpsc::channel::<Envelope<ClientMsg>>(4);
        let inst = RunnerInstance::new(runner, &paths, out_tx);
        let runner_dir = inst.paths.base_dir().to_path_buf();
        inst.paths.ensure().unwrap();
        std::fs::write(runner_dir.join("orphaned-runtime"), b"retry-me").unwrap();
        let mailbox_rx = inst.take_mailbox_rx().await.unwrap();
        drop(mailbox_rx);

        let ctx = RunnerSpawnCtx {
            transport: None,
            shared_machine_token: None,
            paths: paths.clone(),
            daemon: Default::default(),
            mailboxes: Arc::new(RwLock::new(HashMap::from([(
                runner_id,
                inst.mailbox_tx.clone(),
            )]))),
            hello_runners: Arc::new(RwLock::new(HashMap::new())),
            daemon_state: StateHandle::new(config),
            instances: Arc::new(std::sync::RwLock::new(HashMap::from([(
                runner_id,
                inst.clone(),
            )]))),
            activation_lock: Arc::new(tokio::sync::Mutex::new(())),
            accepting_mutations: Arc::new(AtomicBool::new(true)),
        };

        let error = ctx.remove_runner(runner_id).await.unwrap_err().to_string();
        assert!(error.contains("before teardown completed"), "{error}");
        assert!(runner_dir.exists(), "first failure must retain runtime");
        assert!(
            crate::config::file::load_config(&paths)
                .unwrap()
                .runners
                .iter()
                .any(|runner| runner.runner_id == runner_id),
            "first failure must retain config for retry"
        );
        assert!(
            !ctx.instances
                .read()
                .expect("runner registry poisoned")
                .contains_key(&runner_id),
            "dead actor must not block the inactive retry path"
        );

        ctx.remove_runner(runner_id).await.unwrap();
        assert!(!runner_dir.exists());
        assert!(
            crate::config::file::load_config(&paths)
                .unwrap()
                .runners
                .is_empty()
        );
        assert!(working_dir.join("checkout-marker").exists());
    }

    #[tokio::test]
    async fn refresh_loop_exits_when_runner_teardown_is_latched() {
        let tmp = tempfile::tempdir().unwrap();
        let runner_id = uuid::Uuid::new_v4();
        let credentials = CredentialsHandle::new(
            tmp.path().join("credentials.toml"),
            RunnerCredentials {
                runner_id,
                name: "runner".into(),
                refresh_token: "rt_test".into(),
                refresh_token_generation: 1,
            },
        );
        let transport = SharedHttpTransport::new("http://127.0.0.1:1".into()).unwrap();
        let client = RunnerCloudClient::new(runner_id, credentials, transport);
        let state = StateHandle::new(Config {
            version: 2,
            daemon: Default::default(),
            runners: vec![],
            cli: None,
        });
        let (remove_tx, remove_rx) = watch::channel(false);
        let task = tokio::spawn(refresh_loop(client, state, remove_rx));

        // A removed runner must not leave its token-refresh task sleeping for
        // the default 60-second interval (or holding a refresh future) until
        // the whole daemon exits.
        remove_tx.send(true).unwrap();
        tokio::time::timeout(Duration::from_secs(1), task)
            .await
            .expect("refresh loop did not stop after runner teardown")
            .expect("refresh loop task panicked");
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn drain_in_flight_runs_sends_run_failed_for_busy_runners_only() {
        // Two runners; only one has an in-flight run. Drain should
        // emit RunFailed{DaemonRestart} for that one and skip the idle one.
        let tmp = tempfile::tempdir().unwrap();
        let paths = paths_for(tmp.path());
        let (out_tx, mut out_rx) = mpsc::channel::<Envelope<ClientMsg>>(8);

        let inst_busy = RunnerInstance::new(
            runner_config("busy-runner", "WEB", tmp.path().join("wd-busy")),
            &paths,
            out_tx.clone(),
        );
        let inst_idle = RunnerInstance::new(
            runner_config("idle-runner", "API", tmp.path().join("wd-idle")),
            &paths,
            out_tx.clone(),
        );
        let busy_runner_id = inst_busy.runner_id;
        let active_run_id = uuid::Uuid::new_v4();

        // Stamp Some(rid) on the busy instance, leave idle as None.
        inst_busy
            .state
            .set_current_run(Some(CurrentRunSummary {
                run_id: active_run_id,
                thread_id: None,
                status: "running".into(),
                started_at: Utc::now(),
                events: 0,
            }))
            .await;
        let runners: Arc<RwLock<HelloRunnerMap>> = Arc::new(RwLock::new(
            [&inst_busy, &inst_idle]
                .into_iter()
                .map(|i| {
                    (
                        i.runner_id,
                        (
                            i.out.clone(),
                            i.state.clone(),
                            i.config.project_slug.clone(),
                        ),
                    )
                })
                .collect(),
        ));

        let sent = drain_in_flight_runs(runners, &paths).await;
        assert_eq!(sent, 1, "exactly one runner had an in-flight run");
        assert!(
            drain_journal::load(&paths).is_empty(),
            "a delivered drain must leave no journal entry behind"
        );

        // Receive the one RunFailed envelope.
        let env = tokio::time::timeout(std::time::Duration::from_secs(2), out_rx.recv())
            .await
            .expect("timed out waiting for RunFailed")
            .expect("channel closed before RunFailed arrived");
        assert_eq!(env.runner_id, Some(busy_runner_id));
        match env.body {
            ClientMsg::RunFailed {
                run_id,
                reason,
                detail,
                ..
            } => {
                assert_eq!(run_id, active_run_id);
                assert!(matches!(reason, FailureReason::DaemonRestart));
                assert_eq!(detail.as_deref(), Some("daemon shutdown requested"));
            }
            other => panic!("expected RunFailed, got {other:?}"),
        }

        // No further messages — the idle runner must not have produced one.
        let stray =
            tokio::time::timeout(std::time::Duration::from_millis(100), out_rx.recv()).await;
        assert!(
            stray.is_err(),
            "idle runner unexpectedly produced a frame: {stray:?}"
        );
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn drain_in_flight_runs_no_op_when_all_idle() {
        // Common path: nothing in flight, drain should send nothing.
        let tmp = tempfile::tempdir().unwrap();
        let paths = paths_for(tmp.path());
        let (out_tx, mut out_rx) = mpsc::channel::<Envelope<ClientMsg>>(8);
        let inst = RunnerInstance::new(
            runner_config("idle", "WEB", tmp.path().join("wd")),
            &paths,
            out_tx,
        );
        let runners: Arc<RwLock<HelloRunnerMap>> = Arc::new(RwLock::new(
            std::iter::once(&inst)
                .map(|i| {
                    (
                        i.runner_id,
                        (
                            i.out.clone(),
                            i.state.clone(),
                            i.config.project_slug.clone(),
                        ),
                    )
                })
                .collect(),
        ));

        let sent = drain_in_flight_runs(runners, &paths).await;
        assert_eq!(sent, 0);
        assert!(drain_journal::load(&paths).is_empty());
        let stray =
            tokio::time::timeout(std::time::Duration::from_millis(100), out_rx.recv()).await;
        assert!(
            stray.is_err(),
            "idle drain produced a stray frame: {stray:?}"
        );
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn drain_journals_run_failed_when_send_cannot_be_delivered() {
        let tmp = tempfile::tempdir().unwrap();
        let paths = paths_for(tmp.path());
        let (out_tx, out_rx) = mpsc::channel::<Envelope<ClientMsg>>(8);
        drop(out_rx);

        let inst = RunnerInstance::new(
            runner_config("busy", "WEB", tmp.path().join("wd")),
            &paths,
            out_tx,
        );
        let run_id = uuid::Uuid::new_v4();
        inst.state
            .set_current_run(Some(CurrentRunSummary {
                run_id,
                thread_id: None,
                status: "running".into(),
                started_at: Utc::now(),
                events: 0,
            }))
            .await;
        let runners: Arc<RwLock<HelloRunnerMap>> = Arc::new(RwLock::new(HashMap::from([(
            inst.runner_id,
            (
                inst.out.clone(),
                inst.state.clone(),
                inst.config.project_slug.clone(),
            ),
        )])));

        let sent = drain_in_flight_runs(runners, &paths).await;
        assert_eq!(sent, 0, "send must fail with a dropped receiver");

        let journal = drain_journal::load(&paths);
        assert_eq!(journal.len(), 1);
        assert_eq!(journal[0].runner_id, inst.runner_id);
        assert_eq!(journal[0].run_id, run_id);
        assert!(matches!(journal[0].reason, FailureReason::DaemonRestart));
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn replay_delivers_journaled_run_failed_and_clears_journal() {
        let tmp = tempfile::tempdir().unwrap();
        let paths = paths_for(tmp.path());
        let (out_tx, mut out_rx) = mpsc::channel::<Envelope<ClientMsg>>(8);
        let inst = RunnerInstance::new(
            runner_config("survivor", "WEB", tmp.path().join("wd")),
            &paths,
            out_tx,
        );
        let run_id = uuid::Uuid::new_v4();
        let ended_at = Utc::now();
        drain_journal::record_all(
            &paths,
            &[drain_journal::PendingRunFailure {
                runner_id: inst.runner_id,
                run_id,
                reason: FailureReason::DaemonRestart,
                detail: "daemon shutdown requested".into(),
                ended_at,
                recorded_at: ended_at,
            }],
        )
        .unwrap();
        let runners: Arc<RwLock<HelloRunnerMap>> = Arc::new(RwLock::new(HashMap::from([(
            inst.runner_id,
            (
                inst.out.clone(),
                inst.state.clone(),
                inst.config.project_slug.clone(),
            ),
        )])));
        let configured = HashSet::from([inst.runner_id]);

        let delivered = replay_drain_journal_once(&paths, &runners, &configured).await;
        assert_eq!(delivered, 1);
        assert!(drain_journal::load(&paths).is_empty());

        let envelope = tokio::time::timeout(Duration::from_secs(2), out_rx.recv())
            .await
            .expect("timed out waiting for replayed RunFailed")
            .expect("channel closed");
        assert_eq!(envelope.runner_id, Some(inst.runner_id));
        match envelope.body {
            ClientMsg::RunFailed {
                run_id: received,
                reason,
                ..
            } => {
                assert_eq!(received, run_id);
                assert!(matches!(reason, FailureReason::DaemonRestart));
            }
            other => panic!("expected RunFailed, got {other:?}"),
        }
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn replay_skips_run_that_is_in_flight_again() {
        let tmp = tempfile::tempdir().unwrap();
        let paths = paths_for(tmp.path());
        let (out_tx, mut out_rx) = mpsc::channel::<Envelope<ClientMsg>>(8);
        let inst = RunnerInstance::new(
            runner_config("resumed", "WEB", tmp.path().join("wd")),
            &paths,
            out_tx,
        );
        let run_id = uuid::Uuid::new_v4();
        inst.state
            .set_current_run(Some(CurrentRunSummary {
                run_id,
                thread_id: None,
                status: "running".into(),
                started_at: Utc::now(),
                events: 0,
            }))
            .await;
        let now = Utc::now();
        drain_journal::record_all(
            &paths,
            &[drain_journal::PendingRunFailure {
                runner_id: inst.runner_id,
                run_id,
                reason: FailureReason::DaemonRestart,
                detail: "daemon shutdown requested".into(),
                ended_at: now,
                recorded_at: now,
            }],
        )
        .unwrap();
        let runners: Arc<RwLock<HelloRunnerMap>> = Arc::new(RwLock::new(HashMap::from([(
            inst.runner_id,
            (
                inst.out.clone(),
                inst.state.clone(),
                inst.config.project_slug.clone(),
            ),
        )])));
        let configured = HashSet::from([inst.runner_id]);

        let delivered = replay_drain_journal_once(&paths, &runners, &configured).await;
        assert_eq!(delivered, 0);
        assert!(drain_journal::load(&paths).is_empty());
        let stray = tokio::time::timeout(Duration::from_millis(100), out_rx.recv()).await;
        assert!(
            stray.is_err(),
            "no RunFailed may be sent for a live redelivery: {stray:?}"
        );
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn replay_drops_entry_for_unconfigured_runner() {
        let tmp = tempfile::tempdir().unwrap();
        let paths = paths_for(tmp.path());
        let now = Utc::now();
        drain_journal::record_all(
            &paths,
            &[drain_journal::PendingRunFailure {
                runner_id: uuid::Uuid::new_v4(),
                run_id: uuid::Uuid::new_v4(),
                reason: FailureReason::DaemonRestart,
                detail: "daemon shutdown requested".into(),
                ended_at: now,
                recorded_at: now,
            }],
        )
        .unwrap();
        let runners = Arc::new(RwLock::new(HashMap::new()));

        let delivered = replay_drain_journal_once(&paths, &runners, &HashSet::new()).await;
        assert_eq!(delivered, 0);
        assert!(drain_journal::load(&paths).is_empty());
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn replay_retains_configured_inactive_runner_until_activation() {
        let tmp = tempfile::tempdir().unwrap();
        let paths = paths_for(tmp.path());
        let runner_id = uuid::Uuid::new_v4();
        let run_id = uuid::Uuid::new_v4();
        let now = Utc::now();
        drain_journal::record_all(
            &paths,
            &[drain_journal::PendingRunFailure {
                runner_id,
                run_id,
                reason: FailureReason::DaemonRestart,
                detail: "daemon shutdown requested".into(),
                ended_at: now,
                recorded_at: now,
            }],
        )
        .unwrap();
        let runners: Arc<RwLock<HelloRunnerMap>> = Arc::new(RwLock::new(HashMap::new()));
        let configured = HashSet::from([runner_id]);

        let delivered = replay_drain_journal_once(&paths, &runners, &configured).await;
        assert_eq!(delivered, 0);
        assert_eq!(drain_journal::load(&paths).len(), 1);

        let (out_tx, mut out_rx) = mpsc::channel::<Envelope<ClientMsg>>(8);
        let mut config = runner_config("late-active", "WEB", tmp.path().join("wd"));
        config.runner_id = runner_id;
        let inst = RunnerInstance::new(config, &paths, out_tx);
        runners.write().await.insert(
            runner_id,
            (
                inst.out.clone(),
                inst.state.clone(),
                inst.config.project_slug.clone(),
            ),
        );

        let delivered = replay_drain_journal_once(&paths, &runners, &configured).await;
        assert_eq!(delivered, 1);
        assert!(drain_journal::load(&paths).is_empty());
        let envelope = tokio::time::timeout(Duration::from_secs(2), out_rx.recv())
            .await
            .expect("timed out waiting for delayed replay")
            .expect("channel closed");
        assert_eq!(envelope.runner_id, Some(runner_id));
        assert!(matches!(
            envelope.body,
            ClientMsg::RunFailed {
                run_id: received,
                reason: FailureReason::DaemonRestart,
                ..
            } if received == run_id
        ));
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn replay_does_not_discard_an_old_unconfirmed_failure() {
        let tmp = tempfile::tempdir().unwrap();
        let paths = paths_for(tmp.path());
        let (out_tx, mut out_rx) = mpsc::channel::<Envelope<ClientMsg>>(8);
        let inst = RunnerInstance::new(
            runner_config("old-entry", "WEB", tmp.path().join("wd")),
            &paths,
            out_tx,
        );
        let run_id = uuid::Uuid::new_v4();
        let ended_at = Utc::now() - chrono::Duration::days(7);
        drain_journal::record_all(
            &paths,
            &[drain_journal::PendingRunFailure {
                runner_id: inst.runner_id,
                run_id,
                reason: FailureReason::DaemonRestart,
                detail: "daemon shutdown requested".into(),
                ended_at,
                recorded_at: ended_at,
            }],
        )
        .unwrap();
        let runners: Arc<RwLock<HelloRunnerMap>> = Arc::new(RwLock::new(HashMap::from([(
            inst.runner_id,
            (
                inst.out.clone(),
                inst.state.clone(),
                inst.config.project_slug.clone(),
            ),
        )])));
        let configured = HashSet::from([inst.runner_id]);

        let delivered = replay_drain_journal_once(&paths, &runners, &configured).await;

        assert_eq!(delivered, 1);
        assert!(drain_journal::load(&paths).is_empty());
        let envelope = tokio::time::timeout(Duration::from_secs(2), out_rx.recv())
            .await
            .expect("timed out waiting for old replay")
            .expect("channel closed");
        assert!(matches!(
            envelope.body,
            ClientMsg::RunFailed {
                run_id: received,
                reason: FailureReason::DaemonRestart,
                ..
            } if received == run_id
        ));
    }
}
