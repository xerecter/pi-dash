use crate::cli::runner_ops::{ApplyEnrollOptions, EnrollDedupeMode, apply_enroll_response};
use crate::cloud::http::{
    EnrollResponse, EnsureProjectBindingResponse, MachineClient, TransportError,
};
use crate::config::schema::{AgentKind, Config, MAX_RUNNERS_PER_DAEMON, RunnerBindingSource};
use crate::daemon::state::StateHandle;
use crate::daemon::supervisor::RunnerSpawnCtx;
use crate::util::backoff::Backoff;
use crate::util::paths::Paths;
use anyhow::Context;
use serde::de::DeserializeOwned;
use std::collections::{HashMap, HashSet};
use std::future::Future;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::Mutex;
use uuid::Uuid;

const PROJECT_SYNC_INTERVAL: Duration = Duration::from_secs(60);
const PROJECT_SYNC_BACKOFF_CAP_SECS: u64 = 30;

enum SyncPassResult {
    Success,
    Retry(anyhow::Error),
    Stop(anyhow::Error),
}

fn is_fatal_machine_auth(error: &anyhow::Error) -> bool {
    error.chain().any(|cause| {
        cause
            .downcast_ref::<TransportError>()
            .is_some_and(is_fatal_machine_auth_error)
    })
}

fn is_fatal_machine_auth_error(error: &TransportError) -> bool {
    matches!(
        error,
        TransportError::MachineTokenRevoked
            | TransportError::DevMachineRevoked
            | TransportError::MembershipRevoked
            | TransportError::Server {
                status: 401 | 403,
                ..
            }
    )
}

#[cfg(test)]
fn project_sync_interval() -> tokio::time::Interval {
    let mut interval = tokio::time::interval(PROJECT_SYNC_INTERVAL);
    interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    interval
}

pub use crate::cloud::http::{ProjectBindingSnapshot, ProjectRunnerBinding, ProjectSnapshot};

impl ProjectBindingSnapshot {
    pub fn validate_for_workspace(&self, expected_workspace: &str) -> anyhow::Result<()> {
        if !self.complete {
            anyhow::bail!("project binding snapshot is incomplete")
        }
        if self.workspace_slug != expected_workspace {
            anyhow::bail!(
                "project binding snapshot workspace mismatch: expected {:?}, got {:?}",
                expected_workspace,
                self.workspace_slug
            )
        }

        // A complete snapshot is authoritative desired state.  Treat its
        // relationships as a small, self-contained protocol rather than
        // silently dropping records that the planner cannot associate.  A
        // malformed response must fail before the missing-project tracker is
        // advanced; otherwise one bad response could trigger destructive
        // local/cloud removals or create a replacement runner.
        let mut project_ids = HashSet::with_capacity(self.projects.len());
        let mut project_identifiers = HashSet::with_capacity(self.projects.len());
        for project in &self.projects {
            if project.identifier.trim().is_empty() {
                anyhow::bail!(
                    "project binding snapshot contains a project with an empty identifier: {}",
                    project.id
                )
            }
            if !project_ids.insert(project.id) {
                anyhow::bail!(
                    "project binding snapshot contains duplicate project id: {}",
                    project.id
                )
            }
            if !project_identifiers.insert(project.identifier.as_str()) {
                anyhow::bail!(
                    "project binding snapshot contains duplicate project identifier: {:?}",
                    project.identifier
                )
            }
        }

        let mut deleted_project_ids = HashSet::with_capacity(self.deleted_project_ids.len());
        for project_id in &self.deleted_project_ids {
            if !deleted_project_ids.insert(*project_id) {
                anyhow::bail!(
                    "project binding snapshot contains duplicate deleted project id: {}",
                    project_id
                )
            }
            if project_ids.contains(project_id) {
                anyhow::bail!(
                    "project binding snapshot marks an active project as deleted: {}",
                    project_id
                )
            }
        }

        let mut runner_ids = HashSet::with_capacity(self.bindings.len());
        for binding in &self.bindings {
            if !runner_ids.insert(binding.runner_id) {
                anyhow::bail!(
                    "project binding snapshot contains duplicate runner id: {}",
                    binding.runner_id
                )
            }

            let Some(project) = self
                .projects
                .iter()
                .find(|project| project.id == binding.project_id)
            else {
                anyhow::bail!(
                    "project binding snapshot binding {} references unknown project id: {}",
                    binding.runner_id,
                    binding.project_id
                )
            };
            if binding.project_identifier != project.identifier {
                anyhow::bail!(
                    "project binding snapshot binding {} project identifier mismatch: expected {:?}, got {:?}",
                    binding.runner_id,
                    project.identifier,
                    binding.project_identifier
                )
            }
        }
        Ok(())
    }
}

/// The subset of a local config entry needed by the pure planner.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LocalRunnerState {
    pub runner_id: Uuid,
    pub workspace_slug: Option<String>,
    pub project_id: Option<Uuid>,
    pub project_slug: Option<String>,
    pub binding_source: Option<RunnerBindingSource>,
    pub live: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ReconcileAction {
    Ensure { project_id: Uuid },
    Recover { runner_id: Uuid },
    Activate { runner_id: Uuid },
    Noop { project_id: Uuid },
    Remove { runner_id: Uuid },
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ReconcilePlan {
    pub actions: Vec<ReconcileAction>,
    pub skipped_capacity: Vec<Uuid>,
    pub removed_existing: usize,
}

#[derive(Debug, Default, Clone)]
pub struct MissingTracker {
    counts: HashMap<Uuid, u8>,
    last_missing_snapshot: HashMap<Uuid, Uuid>,
}

/// Serializes an immediate reconciliation trigger with the periodic timer.
/// The lock is deliberately independent from the per-runner activation lock:
/// network/config reconciliation must be one-at-a-time, while existing
/// runner loops continue operating concurrently.
#[derive(Clone, Default)]
pub struct SyncTrigger {
    lock: Arc<Mutex<()>>,
}

impl SyncTrigger {
    pub fn new() -> Self {
        Self::default()
    }

    pub async fn run_serialized<F, Fut, T>(&self, operation: F) -> T
    where
        F: FnOnce() -> Fut,
        Fut: Future<Output = T>,
    {
        let _guard = self.lock.lock().await;
        operation().await
    }
}

impl MissingTracker {
    pub fn count(&self, runner_id: Uuid) -> u8 {
        self.counts.get(&runner_id).copied().unwrap_or(0)
    }

    fn mark_missing(&mut self, runner_id: Uuid, snapshot_id: Uuid) -> u8 {
        if self.last_missing_snapshot.get(&runner_id) == Some(&snapshot_id) {
            return self.count(runner_id);
        }
        let count = self.count(runner_id).saturating_add(1);
        self.counts.insert(runner_id, count);
        self.last_missing_snapshot.insert(runner_id, snapshot_id);
        count
    }

    fn mark_present(&mut self, runner_id: Uuid) {
        self.counts.remove(&runner_id);
        self.last_missing_snapshot.remove(&runner_id);
    }

    fn retain_local(&mut self, local_runner_ids: &HashSet<Uuid>) {
        self.counts
            .retain(|runner_id, _| local_runner_ids.contains(runner_id));
        self.last_missing_snapshot
            .retain(|runner_id, _| local_runner_ids.contains(runner_id));
    }
}

/// Compute a deterministic local action list from one complete cloud
/// snapshot. This function deliberately does no I/O, and does not mutate the
/// absence tracker unless the snapshot has passed all validation.
pub fn plan_reconciliation(
    snapshot: &ProjectBindingSnapshot,
    expected_workspace: &str,
    local_runners: &[LocalRunnerState],
    missing: &mut MissingTracker,
) -> anyhow::Result<ReconcilePlan> {
    snapshot.validate_for_workspace(expected_workspace)?;

    let mut projects = snapshot.projects.clone();
    projects.sort_by(|a, b| a.identifier.cmp(&b.identifier).then(a.id.cmp(&b.id)));
    let project_ids: HashSet<Uuid> = projects.iter().map(|p| p.id).collect();
    let deleted_project_ids: HashSet<Uuid> = snapshot.deleted_project_ids.iter().copied().collect();
    let mut project_by_slug = HashMap::new();
    for project in &projects {
        project_by_slug.insert(project.identifier.as_str(), project.id);
    }

    let mut bindings_by_project: HashMap<Uuid, Vec<&ProjectRunnerBinding>> = HashMap::new();
    for binding in &snapshot.bindings {
        if project_ids.contains(&binding.project_id) {
            bindings_by_project
                .entry(binding.project_id)
                .or_default()
                .push(binding);
        }
    }

    let local_project_id = |runner: &LocalRunnerState| {
        runner.project_id.or_else(|| {
            runner
                .project_slug
                .as_deref()
                .and_then(|slug| project_by_slug.get(slug).copied())
        })
    };
    let mut plan = ReconcilePlan::default();
    let mut matched_local = HashSet::new();
    let mut scheduled_removals = HashSet::new();
    let mut desired_ensure_count = 0usize;
    let mut confirmed_missing = HashSet::new();
    let cloud_runner_ids: HashSet<Uuid> = snapshot
        .bindings
        .iter()
        .map(|binding| binding.runner_id)
        .collect();
    let local_runner_ids: HashSet<Uuid> = local_runners
        .iter()
        .map(|runner| runner.runner_id)
        .collect();
    // A removed local row breaks the sequence of missing observations. Drop
    // its history so a later recovery/re-add of the same cloud UUID starts at
    // one observation instead of inheriting a stale destructive count.
    missing.retain_local(&local_runner_ids);
    // Legacy rows without workspace metadata predate account binding. Keep
    // treating them as local to the active workspace so they can be upgraded.
    // An exact runner-id binding in this authoritative snapshot also overrides
    // a stale local workspace tag; without that evidence, an explicitly
    // different workspace remains completely outside this reconciliation.
    let is_in_scope = |runner: &LocalRunnerState| {
        cloud_runner_ids.contains(&runner.runner_id)
            || runner
                .workspace_slug
                .as_deref()
                .is_none_or(|workspace| workspace == expected_workspace)
    };

    // Advance deletion tracking before capacity planning. Project visibility
    // and runner presence are not deletion authority: both can disappear when
    // permissions change or a user deletes only the cloud Runner while asking
    // to retain local files. Only an explicit project UUID tombstone advances
    // the two-distinct-snapshot cleanup rule. Legacy rows without project_id
    // deliberately fail closed because a mutable/reusable slug cannot prove
    // that their project was deleted.
    for local in local_runners {
        let explicitly_deleted = local
            .project_id
            .is_some_and(|project_id| deleted_project_ids.contains(&project_id));
        if is_in_scope(local)
            && explicitly_deleted
            && !cloud_runner_ids.contains(&local.runner_id)
            && missing.mark_missing(local.runner_id, snapshot.snapshot_id) >= 2
        {
            confirmed_missing.insert(local.runner_id);
        } else if !is_in_scope(local)
            || !explicitly_deleted
            || cloud_runner_ids.contains(&local.runner_id)
        {
            missing.mark_present(local.runner_id);
        }
    }
    let occupied_after_removals = local_runners.len().saturating_sub(confirmed_missing.len());
    // Cloud bindings that are not present in local config will be recovered
    // later in this same pass. Reserve their slots before planning any new
    // Ensure actions; otherwise a nearly-full daemon can create a new runner
    // first and leave the already-existing cloud binding unable to recover
    // when config validation enforces the 50-runner cap.
    let recovery_count = snapshot
        .bindings
        .iter()
        .filter(|binding| project_ids.contains(&binding.project_id))
        .filter(|binding| !local_runner_ids.contains(&binding.runner_id))
        .count();
    let occupied_after_removals = occupied_after_removals.saturating_add(recovery_count);

    for project in &projects {
        let cloud_bindings = bindings_by_project
            .get(&project.id)
            .cloned()
            .unwrap_or_default();
        let cloud_binding_ids: HashSet<Uuid> = cloud_bindings
            .iter()
            .map(|binding| binding.runner_id)
            .collect();
        let local_for_project: Vec<&LocalRunnerState> = local_runners
            .iter()
            .filter(|runner| is_in_scope(runner))
            .filter(|runner| {
                local_project_id(runner) == Some(project.id)
                    || cloud_binding_ids.contains(&runner.runner_id)
            })
            .collect();

        if !cloud_bindings.is_empty() {
            // Preserve every local row for an active project. An absent cloud
            // runner can represent an intentional `purge_local=false` delete
            // or a visibility change and is not destructive evidence.
            for local in &local_for_project {
                matched_local.insert(local.runner_id);
                if cloud_binding_ids.contains(&local.runner_id) {
                    if local.live {
                        plan.actions.push(ReconcileAction::Noop {
                            project_id: project.id,
                        });
                    } else {
                        plan.actions.push(ReconcileAction::Activate {
                            runner_id: local.runner_id,
                        });
                    }
                }
            }
            // Cloud rows that are still active but absent from local config
            // need recovery. This is done for every binding, not merely the
            // first, so a manually-added extra runner is not lost.
            for binding in &cloud_bindings {
                if !local_runner_ids.contains(&binding.runner_id) {
                    plan.actions.push(ReconcileAction::Recover {
                        runner_id: binding.runner_id,
                    });
                }
            }
            continue;
        }

        // A newly discovered project without any local or cloud binding needs
        // an idempotent ensure call. Existing local rows are preserved: cloud
        // absence alone is not a project-deletion credential.
        for local in &local_for_project {
            matched_local.insert(local.runner_id);
        }
        if local_for_project.is_empty() {
            desired_ensure_count += 1;
            if occupied_after_removals + desired_ensure_count <= MAX_RUNNERS_PER_DAEMON {
                plan.actions.push(ReconcileAction::Ensure {
                    project_id: project.id,
                });
            } else {
                plan.skipped_capacity.push(project.id);
            }
        }
    }

    // Local rows are removed only after two distinct, validated snapshots
    // carry an explicit tombstone for their immutable project UUID. A failed,
    // mismatched, visibility-reduced, or runner-only snapshot cannot advance
    // the counter.
    for local in local_runners {
        if !is_in_scope(local) || matched_local.contains(&local.runner_id) {
            continue;
        }
        if confirmed_missing.contains(&local.runner_id)
            && scheduled_removals.insert(local.runner_id)
        {
            plan.actions.push(ReconcileAction::Remove {
                runner_id: local.runner_id,
            });
            plan.removed_existing += 1;
        }
    }

    // Removal actions for stale local rows should be stable too, while
    // retaining the project identifier order for ensure/recovery actions.
    plan.actions.sort_by_key(|action| match action {
        // Remove stale local state before ensuring a replacement so the
        // existing 50-runner cap reflects the post-reconciliation capacity.
        ReconcileAction::Remove { runner_id } => (0u8, *runner_id),
        ReconcileAction::Ensure { project_id } => (1u8, *project_id),
        ReconcileAction::Recover { runner_id } => (2u8, *runner_id),
        ReconcileAction::Activate { runner_id } => (3u8, *runner_id),
        ReconcileAction::Noop { project_id } => (4u8, *project_id),
    });
    Ok(plan)
}

/// Decode a response while retaining a precise protocol error at the cloud
/// client boundary. Keeping this generic makes snapshot/ensure parsing easy to
/// exercise without an HTTP server in unit tests.
pub fn decode_json<T: DeserializeOwned>(body: &[u8]) -> anyhow::Result<T> {
    serde_json::from_slice(body).map_err(|error| anyhow::anyhow!("project sync response: {error}"))
}

/// Refresh project identity metadata in the in-memory configuration.
///
/// Project identifiers are mutable display metadata; the UUID is the stable
/// identity used by reconciliation. A runner with an existing `project_id` is
/// therefore only refreshed when that exact project is present in the
/// complete snapshot. Legacy rows without a UUID are upgraded by first using
/// the authoritative cloud binding for the runner, then falling back to the
/// old project identifier. All fields unrelated to project identity are left
/// untouched.
pub fn refresh_config_project_metadata(
    config: &mut Config,
    snapshot: &ProjectBindingSnapshot,
) -> usize {
    let mut changed_runners = 0;

    for runner in &mut config.runners {
        let exact_binding = snapshot
            .bindings
            .iter()
            .find(|binding| binding.runner_id == runner.runner_id);
        if runner
            .workspace_slug
            .as_deref()
            .is_some_and(|workspace| workspace != snapshot.workspace_slug)
            && exact_binding.is_none()
        {
            continue;
        }

        // A binding keyed by the immutable runner UUID is stronger evidence
        // than any cached project/workspace metadata. This also repairs old
        // local rows after a cloud-side runner move. Only when no exact
        // binding exists do we fall back to the stored project UUID/slug.
        let project = exact_binding
            .and_then(|binding| {
                snapshot
                    .projects
                    .iter()
                    .find(|project| project.id == binding.project_id)
            })
            .or_else(|| match runner.project_id {
                Some(project_id) => snapshot
                    .projects
                    .iter()
                    .find(|project| project.id == project_id),
                None => runner.project_slug.as_deref().and_then(|slug| {
                    snapshot
                        .projects
                        .iter()
                        .find(|project| project.identifier == slug)
                }),
            });

        let Some(project) = project else {
            // A missing project is handled by the missing-snapshot tracker and
            // removal planner. Do not rewrite its metadata to another project
            // merely because a legacy slug happens to match.
            continue;
        };

        let mut changed = false;
        if exact_binding.is_some()
            && runner.workspace_slug.as_deref() != Some(snapshot.workspace_slug.as_str())
        {
            runner.workspace_slug = Some(snapshot.workspace_slug.clone());
            changed = true;
        }
        if runner.project_id != Some(project.id) {
            runner.project_id = Some(project.id);
            changed = true;
        }
        if runner.project_slug.as_deref() != Some(project.identifier.as_str()) {
            runner.project_slug = Some(project.identifier.clone());
            changed = true;
        }
        if let Some(binding) = exact_binding {
            if runner.pod_id != Some(binding.pod_id) {
                runner.pod_id = Some(binding.pod_id);
                changed = true;
            }
            if runner.binding_source != Some(binding.binding_source) {
                runner.binding_source = Some(binding.binding_source);
                changed = true;
            }
        }
        if changed {
            changed_runners += 1;
        }
    }

    changed_runners
}

/// Persist project identity metadata under the host-wide config lock.
///
/// The return value is the number of runner rows whose metadata changed. A
/// second call with the same snapshot is consequently a no-op (`0`) while
/// still using the same validated, atomic read-modify-write path as other
/// daemon configuration mutations.
pub fn refresh_local_project_metadata(
    paths: &Paths,
    snapshot: &ProjectBindingSnapshot,
) -> anyhow::Result<usize> {
    let mut changed_runners = 0;
    crate::config::file::mutate_config(paths, |config| {
        changed_runners = refresh_config_project_metadata(config, snapshot);
        Ok(())
    })?;
    Ok(changed_runners)
}

/// Machine-level desired-state reconciler. It owns no runner execution work;
/// it only calls the cloud inventory/ensure APIs and delegates local runtime
/// changes to [`RunnerSpawnCtx`].
pub(crate) struct ProjectSync {
    client: MachineClient,
    spawn_ctx: RunnerSpawnCtx,
    daemon_state: StateHandle,
    workspace_slug: String,
    trigger: SyncTrigger,
    missing: Arc<Mutex<MissingTracker>>,
}

impl ProjectSync {
    pub(crate) fn new(
        client: MachineClient,
        spawn_ctx: RunnerSpawnCtx,
        daemon_state: StateHandle,
        workspace_slug: String,
    ) -> Self {
        Self {
            client,
            spawn_ctx,
            daemon_state,
            workspace_slug,
            trigger: SyncTrigger::new(),
            missing: Arc::new(Mutex::new(MissingTracker::default())),
        }
    }

    /// Run an immediate reconciliation and then poll every minute.
    ///
    /// Transient failures use bounded jittered backoff. Machine-token auth
    /// failures stop this task, but do not stop the daemon or existing runner
    /// loops; those runners retain their current lifecycle independently.
    pub(crate) async fn run(self) {
        let shutdown = self.daemon_state.shutdown_notified();
        let mut backoff = Backoff::with_cap(PROJECT_SYNC_BACKOFF_CAP_SECS);
        let mut workspaces = HashMap::new();
        loop {
            match self.sync_account_workspaces(&mut workspaces).await {
                SyncPassResult::Success => {
                    backoff.reset();
                    tokio::select! {
                        _ = shutdown.notified() => return,
                        _ = tokio::time::sleep(PROJECT_SYNC_INTERVAL) => {}
                    }
                }
                SyncPassResult::Retry(error) => {
                    let delay = backoff.next_delay();
                    tracing::warn!(error = %error, backoff_secs = delay.as_secs(), "project synchronization pass failed; retrying");
                    tokio::select! {
                        _ = shutdown.notified() => return,
                        _ = tokio::time::sleep(delay) => {}
                    }
                }
                SyncPassResult::Stop(error) => {
                    tracing::error!(error = %error, "project synchronization stopped after machine authentication failure");
                    return;
                }
            }
        }
    }

    async fn sync_account_workspaces(
        &self,
        syncs: &mut HashMap<String, ProjectSync>,
    ) -> SyncPassResult {
        let result = async {
            let cfg = crate::config::file::load_config(&self.spawn_ctx.paths)?;
            self.spawn_ctx.check_config_identity(&cfg)?;
            let workspaces = self.client.account_workspaces().await?;
            syncs.retain(|slug, _| workspaces.contains(slug));
            // Preserve credentials across temporary membership loss. The
            // server checks active membership on every request; rejoining
            // should reuse the token held by existing runner loops rather
            // than rotate it out from under them. Login/logout clear the
            // cache when the machine's account identity changes.

            let mut failure = None;
            for slug in workspaces {
                if slug == self.workspace_slug {
                    match self.run_once().await {
                        SyncPassResult::Stop(error) => return Err(error),
                        SyncPassResult::Retry(error) => failure = Some(error),
                        SyncPassResult::Success => {},
                    }
                    continue;
                }
                let outcome = async {
                    if !syncs.contains_key(&slug) {
                        let cfg = crate::config::file::load_config(&self.spawn_ctx.paths)?;
                        self.spawn_ctx.check_config_identity(&cfg)?;
                        let cached = cfg.cli.as_ref().and_then(|cli| {
                            cli.machine_token_for_workspace(Some(&slug)).map(str::to_owned)
                        });
                        let token = match cached {
                            Some(token) => token,
                            None => {
                                let token = self.client.workspace_machine_token(&slug).await?;
                                crate::config::file::mutate_config(&self.spawn_ctx.paths, |cfg| {
                                    self.spawn_ctx.check_config_identity(cfg)?;
                                    cfg.cli.as_mut().context("missing CLI identity")?
                                        .workspace_tokens.insert(slug.clone(), token.clone());
                                    Ok(())
                                })?;
                                token
                            }
                        };
                        syncs.insert(slug.clone(), ProjectSync::new(
                            MachineClient::new(self.client.transport().clone(), self.client.dev_machine_id(), token),
                            self.spawn_ctx.clone(), self.daemon_state.clone(), slug.clone(),
                        ));
                    }
                    syncs[&slug].reconcile_once().await
                }.await;
                if let Err(error) = outcome {
                    // An unavailable/revoked secondary workspace must not
                    // prevent other memberships from acquiring their runners.
                    tracing::warn!(workspace = %slug, error = %error, "workspace project synchronization failed");
                    failure = Some(error);
                }
            }
            Ok::<_, anyhow::Error>(failure)
        }.await;
        match result {
            Ok(None) => SyncPassResult::Success,
            Ok(Some(error)) => SyncPassResult::Retry(error),
            Err(error) if is_fatal_machine_auth(&error) => SyncPassResult::Stop(error),
            Err(error) => SyncPassResult::Retry(error),
        }
    }

    async fn run_once(&self) -> SyncPassResult {
        let trigger = self.trigger.clone();
        trigger
            .run_serialized(|| async {
                match self.reconcile_once().await {
                    Ok(()) => SyncPassResult::Success,
                    Err(error) if is_fatal_machine_auth(&error) => SyncPassResult::Stop(error),
                    Err(error) => SyncPassResult::Retry(error),
                }
            })
            .await
    }

    async fn reconcile_once(&self) -> anyhow::Result<()> {
        let snapshot = self
            .client
            .project_bindings()
            .await
            .map_err(|error| anyhow::Error::new(error).context("fetching project snapshot"))?;
        snapshot.validate_for_workspace(&self.workspace_slug)?;

        self.spawn_ctx
            .check_config_identity(&crate::config::file::load_config(&self.spawn_ctx.paths)?)?;

        let refreshed = refresh_local_project_metadata(&self.spawn_ctx.paths, &snapshot)
            .context("refreshing local project metadata")?;
        if refreshed > 0 {
            tracing::info!(
                count = refreshed,
                "refreshed local project metadata from cloud snapshot"
            );
        }

        // Persisted and live metadata are intentionally updated as two small,
        // lock-bounded steps. Always mirror the current file, even when this
        // pass made no disk change: that heals a daemon which was interrupted
        // after writing config.toml but before refreshing its runtime caches.
        let configured = self.spawn_ctx.configured_runners()?;
        let runtime_refreshed = self.spawn_ctx.refresh_project_metadata(&configured);
        if runtime_refreshed > 0 {
            tracing::info!(
                count = runtime_refreshed,
                "refreshed running project metadata from cloud snapshot"
            );
        }

        let local = self.spawn_ctx.local_runner_states()?;
        let plan = {
            let mut missing = self.missing.lock().await;
            plan_reconciliation(&snapshot, &self.workspace_slug, &local, &mut missing)?
        };
        let configured_by_id: HashMap<Uuid, crate::config::schema::RunnerConfig> = configured
            .into_iter()
            .map(|runner| (runner.runner_id, runner))
            .collect();

        for action in plan.actions {
            match action {
                ReconcileAction::Remove { runner_id } => {
                    if let Err(error) = self.spawn_ctx.remove_runner(runner_id).await {
                        tracing::warn!(%runner_id, error = %error, "local runner removal failed; retrying next pass");
                    }
                }
                ReconcileAction::Ensure { project_id } => {
                    let response = match self.client.ensure_project_binding(&project_id).await {
                        Ok(response) => response,
                        Err(error) => {
                            if is_fatal_machine_auth_error(&error) {
                                return Err(anyhow::Error::new(error)
                                    .context("ensuring project runner binding"));
                            }
                            tracing::warn!(%project_id, error = %error, "automatic runner ensure failed");
                            continue;
                        }
                    };
                    if let Err(error) = self
                        .apply_and_activate(
                            ensure_to_enroll(response),
                            EnrollDedupeMode::RunnerAndProject,
                        )
                        .await
                    {
                        tracing::warn!(%project_id, error = %error, "automatic runner activation failed");
                    }
                }
                ReconcileAction::Recover { runner_id } => {
                    let Some(binding) = snapshot
                        .bindings
                        .iter()
                        .find(|binding| binding.runner_id == runner_id)
                    else {
                        tracing::warn!(%runner_id, "snapshot recovery binding disappeared before activation");
                        continue;
                    };
                    if let Err(error) = self
                        .apply_and_activate(
                            binding_to_enroll(binding, &snapshot.workspace_slug),
                            EnrollDedupeMode::RunnerOnly,
                        )
                        .await
                    {
                        tracing::warn!(%runner_id, error = %error, "cloud runner recovery failed");
                    }
                }
                ReconcileAction::Activate { runner_id } => {
                    let Some(runner) = configured_by_id.get(&runner_id).cloned() else {
                        tracing::warn!(%runner_id, "configured runner disappeared before activation");
                        continue;
                    };
                    if let Err(error) = self.spawn_ctx.add_runner(runner).await {
                        tracing::warn!(%runner_id, error = %error, "configured runner activation failed");
                    }
                }
                ReconcileAction::Noop { .. } => {}
            }
        }
        if !plan.skipped_capacity.is_empty() {
            tracing::warn!(
                count = plan.skipped_capacity.len(),
                "project synchronization reached the runner capacity; excess projects will retry"
            );
        }
        Ok(())
    }

    async fn apply_and_activate(
        &self,
        response: EnrollResponse,
        dedupe_mode: EnrollDedupeMode,
    ) -> anyhow::Result<()> {
        let applied = apply_enroll_response(
            &self.spawn_ctx.paths,
            &response,
            &self.spawn_ctx.cloud_url(),
            ApplyEnrollOptions {
                working_dir: None,
                agent_kind: AgentKind::default(),
                model: None,
                reasoning_effort: None,
                dedupe_mode,
            },
        )
        .await?;
        self.spawn_ctx.add_runner(applied.runner).await
    }
}

fn ensure_to_enroll(response: EnsureProjectBindingResponse) -> EnrollResponse {
    EnrollResponse {
        runner_id: response.runner_id,
        runner_name: response.runner_name,
        refresh_token: response.refresh_token,
        access_token: response.access_token,
        access_token_expires_at: response.access_token_expires_at,
        refresh_token_generation: response.refresh_token_generation,
        workspace_slug: response.workspace_slug,
        pod_slug: response.pod_slug,
        project_identifier: response.project_identifier,
        project_id: Some(response.project_id),
        binding_source: Some(response.binding_source),
        pod_id: Some(response.pod_id),
        long_poll_interval_secs: response.long_poll_interval_secs,
        protocol_version: response.protocol_version,
        machine_token: None,
        machine_token_minted: false,
    }
}

fn binding_to_enroll(binding: &ProjectRunnerBinding, workspace_slug: &str) -> EnrollResponse {
    EnrollResponse {
        runner_id: binding.runner_id,
        runner_name: binding.runner_name.clone(),
        refresh_token: String::new(),
        access_token: String::new(),
        access_token_expires_at: String::new(),
        refresh_token_generation: 0,
        workspace_slug: workspace_slug.to_string(),
        pod_slug: binding.pod_slug.clone(),
        project_identifier: binding.project_identifier.clone(),
        project_id: Some(binding.project_id),
        binding_source: Some(binding.binding_source),
        pod_id: Some(binding.pod_id),
        long_poll_interval_secs: 25,
        protocol_version: 4,
        machine_token: None,
        machine_token_minted: false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::schema::{
        AgentSection, ApprovalPolicySection, ClaudeCodeSection, CodexSection, Config,
        CursorAgentSection, DaemonConfig, GrokSection, MuseCodeSection, OpenClawSection,
        RunnerConfig, WorkspaceSection,
    };
    use crate::util::paths::Paths;
    use std::path::PathBuf;
    use uuid::Uuid;

    fn project(id: Uuid, identifier: &str) -> ProjectSnapshot {
        ProjectSnapshot {
            id,
            identifier: identifier.to_string(),
            name: identifier.to_string(),
            default_pod_id: Some(Uuid::new_v4()),
            default_pod_slug: format!("{identifier}-default"),
        }
    }

    fn snapshot(workspace_slug: &str, projects: Vec<ProjectSnapshot>) -> ProjectBindingSnapshot {
        ProjectBindingSnapshot {
            complete: true,
            snapshot_id: Uuid::new_v4(),
            workspace_id: Uuid::new_v4(),
            workspace_slug: workspace_slug.to_string(),
            projects,
            bindings: Vec::new(),
            deleted_project_ids: Vec::new(),
        }
    }

    fn next_snapshot(previous: &ProjectBindingSnapshot) -> ProjectBindingSnapshot {
        ProjectBindingSnapshot {
            snapshot_id: Uuid::new_v4(),
            ..previous.clone()
        }
    }

    fn tombstone(mut snapshot: ProjectBindingSnapshot, project_id: Uuid) -> ProjectBindingSnapshot {
        snapshot.deleted_project_ids.push(project_id);
        snapshot
    }

    fn config_with_runner(runner: RunnerConfig) -> Config {
        Config {
            version: 2,
            daemon: DaemonConfig {
                cloud_url: "https://cloud.example.test".into(),
                ..DaemonConfig::default()
            },
            runners: vec![runner],
            cli: None,
        }
    }

    fn runner_config(
        runner_id: Uuid,
        project_id: Option<Uuid>,
        project_slug: &str,
        working_dir: PathBuf,
    ) -> RunnerConfig {
        RunnerConfig {
            name: "manual-web".into(),
            runner_id,
            workspace_slug: Some("acme".into()),
            project_slug: Some(project_slug.into()),
            project_id,
            binding_source: Some(RunnerBindingSource::Manual),
            pod_id: Some(Uuid::new_v4()),
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

    fn paths_for(root: &std::path::Path) -> Paths {
        Paths {
            config_dir: root.join("config"),
            data_dir: root.join("data"),
            runtime_dir: root.join("runtime"),
        }
    }

    type CloudCall = (
        &'static str,
        String,
        &'static str,
        u16,
        serde_json::Value,
        Option<serde_json::Value>,
    );

    async fn fake_membership_cloud(calls: Vec<CloudCall>) -> (String, tokio::task::JoinHandle<()>) {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let task = tokio::spawn(async move {
            for (method, path, auth, status, response, expected_body) in calls {
                let (mut socket, _) =
                    tokio::time::timeout(Duration::from_secs(10), listener.accept())
                        .await
                        .expect("expected cloud request")
                        .unwrap();
                let mut bytes = Vec::new();
                let mut chunk = [0; 4096];
                let (headers, body_start, body_length) = loop {
                    let count = socket.read(&mut chunk).await.unwrap();
                    assert!(count > 0);
                    bytes.extend_from_slice(&chunk[..count]);
                    if let Some(end) = bytes.windows(4).position(|w| w == b"\r\n\r\n") {
                        let headers = String::from_utf8(bytes[..end].to_vec()).unwrap();
                        let length = headers
                            .lines()
                            .find_map(|line| {
                                line.to_ascii_lowercase()
                                    .strip_prefix("content-length:")
                                    .map(|s| s.trim().parse::<usize>().unwrap())
                            })
                            .unwrap_or(0);
                        break (headers, end + 4, length);
                    }
                };
                while bytes.len() < body_start + body_length {
                    let count = socket.read(&mut chunk).await.unwrap();
                    assert!(count > 0);
                    bytes.extend_from_slice(&chunk[..count]);
                }
                assert_eq!(
                    headers.lines().next().unwrap(),
                    format!("{method} {path} HTTP/1.1")
                );
                assert!(
                    headers.lines().any(|line| line.eq_ignore_ascii_case(auth)),
                    "wrong workspace credential"
                );
                if let Some(expected) = expected_body {
                    let body: serde_json::Value =
                        serde_json::from_slice(&bytes[body_start..]).unwrap();
                    for (key, value) in expected.as_object().unwrap() {
                        assert_eq!(&body[key], value);
                    }
                }
                let body = response.to_string();
                socket.write_all(format!(
                    "HTTP/1.1 {status} Test\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len(),
                ).as_bytes()).await.unwrap();
            }
        });
        (format!("http://{address}"), task)
    }

    #[tokio::test]
    async fn discovers_new_membership_ensures_runner_and_reuses_cached_workspace_token() {
        use crate::cloud::http::SharedHttpTransport;
        use crate::config::schema::CliSection;
        use serde_json::json;
        let temp = tempfile::tempdir().unwrap();
        let paths = paths_for(temp.path());
        let machine = Uuid::new_v4();
        let project_id = Uuid::new_v4();
        let runner_id = Uuid::new_v4();
        let pod_id = Uuid::new_v4();
        let snapshot_path = format!("/api/v1/runner/dev-machines/{machine}/project-bindings/");
        let ensure_path = format!("{snapshot_path}ensure/");
        let primary_snapshot = json!({
            "complete": true, "snapshot_id": Uuid::new_v4(), "workspace_id": Uuid::new_v4(),
            "workspace_slug": "acme", "projects": [], "bindings": [], "deleted_project_ids": [],
        });
        let mut shared_snapshot = json!({
            "complete": true, "snapshot_id": Uuid::new_v4(), "workspace_id": Uuid::new_v4(),
            "workspace_slug": "shared", "deleted_project_ids": [], "bindings": [],
            "projects": [{"id": project_id, "identifier": "WEB", "name": "Someone else's project", "default_pod_id": pod_id}],
        });
        let binding = json!({
            "runner_id": runner_id, "runner_name": "auto_WEB_shared", "project_id": project_id,
            "project_identifier": "WEB", "pod_id": pod_id, "pod_slug": "WEB-default",
            "workspace_slug": "shared", "binding_source": "auto_project_sync",
        });
        let list = json!({"workspaces": [{"slug": "acme"}, {"slug": "shared"}]});
        let mut calls = vec![
            (
                "GET",
                "/api/v1/auth/workspaces/".into(),
                "x-api-key: mt_primary",
                200,
                json!({"workspaces": [{"slug": "acme"}]}),
                None,
            ),
            (
                "GET",
                snapshot_path.clone(),
                "authorization: Bearer mt_primary",
                200,
                primary_snapshot.clone(),
                None,
            ),
            (
                "GET",
                "/api/v1/auth/workspaces/".into(),
                "x-api-key: mt_primary",
                200,
                list.clone(),
                None,
            ),
            (
                "GET",
                snapshot_path.clone(),
                "authorization: Bearer mt_primary",
                200,
                primary_snapshot.clone(),
                None,
            ),
            (
                "POST",
                "/api/v1/auth/machine-token/".into(),
                "x-api-key: mt_primary",
                201,
                json!({"machine_token": "mt_shared", "workspace_slug": "shared", "dev_machine_id": machine}),
                Some(json!({"workspace_slug": "shared", "dev_machine_id": machine})),
            ),
            (
                "GET",
                snapshot_path.clone(),
                "authorization: Bearer mt_shared",
                200,
                shared_snapshot.clone(),
                None,
            ),
            (
                "POST",
                ensure_path,
                "authorization: Bearer mt_shared",
                201,
                binding.clone(),
                Some(json!({"project_id": project_id})),
            ),
        ];
        shared_snapshot["bindings"] = json!([binding]);
        // Repeated discovery and temporary membership loss followed by a
        // reconciler restart must reuse the existing credential and runner.
        for pass in 0..2 {
            if pass == 1 {
                calls.extend([
                    (
                        "GET",
                        "/api/v1/auth/workspaces/".into(),
                        "x-api-key: mt_primary",
                        200,
                        json!({"workspaces": [{"slug": "acme"}]}),
                        None,
                    ),
                    (
                        "GET",
                        snapshot_path.clone(),
                        "authorization: Bearer mt_primary",
                        200,
                        primary_snapshot.clone(),
                        None,
                    ),
                ]);
            }
            calls.extend([
                (
                    "GET",
                    "/api/v1/auth/workspaces/".into(),
                    "x-api-key: mt_primary",
                    200,
                    list.clone(),
                    None,
                ),
                (
                    "GET",
                    snapshot_path.clone(),
                    "authorization: Bearer mt_primary",
                    200,
                    primary_snapshot.clone(),
                    None,
                ),
                (
                    "GET",
                    snapshot_path.clone(),
                    "authorization: Bearer mt_shared",
                    200,
                    shared_snapshot.clone(),
                    None,
                ),
            ]);
        }
        let (cloud_url, server) = fake_membership_cloud(calls).await;
        let config = Config {
            version: 2,
            daemon: DaemonConfig {
                cloud_url: cloud_url.clone(),
                dev_machine_id: Some(machine),
                ..Default::default()
            },
            cli: Some(CliSection {
                token: Some("mt_primary".into()),
                workspace_slug: Some("acme".into()),
                ..Default::default()
            }),
            runners: vec![],
        };
        crate::config::file::write_config(&paths, &config).unwrap();
        let spawn_ctx = RunnerSpawnCtx::for_project_sync_test(paths.clone(), &config);
        let sync = ProjectSync::new(
            MachineClient::new(
                SharedHttpTransport::new(cloud_url).unwrap(),
                machine,
                "mt_primary".into(),
            ),
            spawn_ctx.clone(),
            StateHandle::new(config),
            "acme".into(),
        );
        let mut syncs = HashMap::new();
        assert!(matches!(
            sync.sync_account_workspaces(&mut syncs).await,
            SyncPassResult::Success
        ));
        assert!(spawn_ctx.configured_runners().unwrap().is_empty());
        for restart in [false, false, true] {
            if restart {
                assert!(matches!(
                    sync.sync_account_workspaces(&mut syncs).await,
                    SyncPassResult::Success
                ));
                assert!(
                    syncs.is_empty(),
                    "removed membership must stop reconciliation"
                );
                assert_eq!(
                    crate::config::file::load_config(&paths)
                        .unwrap()
                        .cli
                        .unwrap()
                        .workspace_tokens["shared"],
                    "mt_shared",
                );
                syncs.clear();
            }
            match sync.sync_account_workspaces(&mut syncs).await {
                SyncPassResult::Success => {}
                SyncPassResult::Retry(error) | SyncPassResult::Stop(error) => {
                    panic!("sync failed: {error:#}")
                }
            }
            let config = crate::config::file::load_config(&paths).unwrap();
            let cli = config.cli.unwrap();
            assert_eq!(cli.token.as_deref(), Some("mt_primary"));
            assert_eq!(cli.workspace_tokens["shared"], "mt_shared");
            assert_eq!(config.runners.len(), 1);
            assert_eq!(config.runners[0].runner_id, runner_id);
            assert_eq!(config.runners[0].workspace_slug.as_deref(), Some("shared"));
            assert_eq!(config.runners[0].project_id, Some(project_id));
            assert!(spawn_ctx.local_runner_states().unwrap()[0].live);
        }
        server.await.unwrap();
    }

    #[test]
    fn refreshes_renamed_project_identifier_without_mutating_runner_metadata() {
        let project_id = Uuid::new_v4();
        let runner_id = Uuid::new_v4();
        let working_dir = PathBuf::from("/workspace/manual-web");
        let runner = runner_config(runner_id, Some(project_id), "OLD_WEB", working_dir.clone());
        let original_name = runner.name.clone();
        let original_pod_id = runner.pod_id;
        let original_binding_source = runner.binding_source;
        let mut config = config_with_runner(runner);
        let snapshot = snapshot("acme", vec![project(project_id, "NEW_WEB")]);

        let changed = refresh_config_project_metadata(&mut config, &snapshot);

        assert_eq!(changed, 1);
        let refreshed = &config.runners[0];
        assert_eq!(refreshed.runner_id, runner_id);
        assert_eq!(refreshed.project_id, Some(project_id));
        assert_eq!(refreshed.project_slug.as_deref(), Some("NEW_WEB"));
        assert_eq!(refreshed.workspace.working_dir, working_dir);
        assert_eq!(refreshed.binding_source, original_binding_source);
        assert_eq!(refreshed.name, original_name);
        assert_eq!(refreshed.pod_id, original_pod_id);
    }

    #[test]
    fn persists_project_metadata_refresh_under_config_lock_idempotently() {
        let temp = tempfile::tempdir().unwrap();
        let paths = paths_for(temp.path());
        let project_id = Uuid::new_v4();
        let runner_id = Uuid::new_v4();
        let working_dir = temp.path().join("working-dir");
        let config = config_with_runner(runner_config(
            runner_id,
            Some(project_id),
            "OLD_WEB",
            working_dir.clone(),
        ));
        crate::config::file::write_config(&paths, &config).unwrap();
        let snapshot = snapshot("acme", vec![project(project_id, "NEW_WEB")]);

        assert_eq!(
            refresh_local_project_metadata(&paths, &snapshot).unwrap(),
            1
        );
        let persisted = crate::config::file::load_config(&paths).unwrap();
        assert_eq!(
            persisted.runners[0].project_slug.as_deref(),
            Some("NEW_WEB")
        );
        assert_eq!(persisted.runners[0].project_id, Some(project_id));
        assert_eq!(persisted.runners[0].workspace.working_dir, working_dir);
        assert_eq!(
            persisted.runners[0].binding_source,
            Some(RunnerBindingSource::Manual)
        );

        assert_eq!(
            refresh_local_project_metadata(&paths, &snapshot).unwrap(),
            0
        );
        let persisted_again = crate::config::file::load_config(&paths).unwrap();
        assert_eq!(
            persisted_again.runners[0].project_slug.as_deref(),
            Some("NEW_WEB")
        );
        assert_eq!(
            persisted_again.runners[0].workspace.working_dir,
            working_dir
        );
        assert_eq!(
            persisted_again.runners[0].binding_source,
            Some(RunnerBindingSource::Manual)
        );
    }

    #[test]
    fn metadata_refresh_does_not_cross_workspace_boundaries() {
        let temp = tempfile::tempdir().unwrap();
        let project_id = Uuid::new_v4();
        let runner_id = Uuid::new_v4();
        let mut runner = runner_config(runner_id, None, "WEB", temp.path().join("other-workspace"));
        runner.workspace_slug = Some("other".into());
        let original_project_id = runner.project_id;
        let original_project_slug = runner.project_slug.clone();
        let mut config = config_with_runner(runner);
        let snapshot = snapshot("acme", vec![project(project_id, "WEB")]);

        assert_eq!(refresh_config_project_metadata(&mut config, &snapshot), 0);
        assert_eq!(config.runners[0].workspace_slug.as_deref(), Some("other"));
        assert_eq!(config.runners[0].project_id, original_project_id);
        assert_eq!(config.runners[0].project_slug, original_project_slug);
    }

    #[test]
    fn authoritative_binding_repairs_an_incorrect_workspace_tag() {
        let temp = tempfile::tempdir().unwrap();
        let project_id = Uuid::new_v4();
        let runner_id = Uuid::new_v4();
        let mut runner = runner_config(
            runner_id,
            Some(Uuid::new_v4()),
            "OLD_WEB",
            temp.path().join("mis-tagged-workspace"),
        );
        runner.workspace_slug = Some("other".into());
        let mut config = config_with_runner(runner);
        let mut snap = snapshot("acme", vec![project(project_id, "WEB")]);
        let binding_pod_id = Uuid::new_v4();
        snap.bindings.push(ProjectRunnerBinding {
            runner_id,
            runner_name: "auto-web".into(),
            project_id,
            project_identifier: "WEB".into(),
            pod_id: binding_pod_id,
            pod_slug: "web-default".into(),
            binding_source: RunnerBindingSource::AutoProjectSync,
        });

        assert_eq!(refresh_config_project_metadata(&mut config, &snap), 1);
        assert_eq!(config.runners[0].workspace_slug.as_deref(), Some("acme"));
        assert_eq!(config.runners[0].project_id, Some(project_id));
        assert_eq!(config.runners[0].project_slug.as_deref(), Some("WEB"));
        assert_eq!(config.runners[0].pod_id, Some(binding_pod_id));
        assert_eq!(
            config.runners[0].binding_source,
            Some(RunnerBindingSource::AutoProjectSync)
        );
    }

    fn local(
        runner_id: Uuid,
        project_id: Option<Uuid>,
        project_slug: Option<&str>,
        live: bool,
    ) -> LocalRunnerState {
        local_in_workspace(runner_id, project_id, project_slug, Some("acme"), live)
    }

    fn local_in_workspace(
        runner_id: Uuid,
        project_id: Option<Uuid>,
        project_slug: Option<&str>,
        workspace_slug: Option<&str>,
        live: bool,
    ) -> LocalRunnerState {
        LocalRunnerState {
            runner_id,
            project_id,
            project_slug: project_slug.map(str::to_string),
            workspace_slug: workspace_slug.map(str::to_string),
            binding_source: Some(RunnerBindingSource::AutoProjectSync),
            live,
        }
    }

    #[test]
    fn complete_snapshot_plans_ensure_for_unbound_project() {
        let project_id = Uuid::new_v4();
        let snap = snapshot("acme", vec![project(project_id, "WEB")]);
        let mut missing = MissingTracker::default();

        let plan = plan_reconciliation(&snap, "acme", &[], &mut missing).unwrap();

        assert_eq!(plan.actions, vec![ReconcileAction::Ensure { project_id }]);
    }

    #[test]
    fn matching_cloud_and_live_local_binding_is_a_noop() {
        let project_id = Uuid::new_v4();
        let runner_id = Uuid::new_v4();
        let mut snap = snapshot("acme", vec![project(project_id, "WEB")]);
        snap.bindings.push(ProjectRunnerBinding {
            runner_id,
            runner_name: "auto-web".into(),
            project_id,
            project_identifier: "WEB".into(),
            pod_id: Uuid::new_v4(),
            pod_slug: "web-default".into(),
            binding_source: RunnerBindingSource::AutoProjectSync,
        });
        let mut missing = MissingTracker::default();

        let plan = plan_reconciliation(
            &snap,
            "acme",
            &[local(runner_id, Some(project_id), Some("WEB"), true)],
            &mut missing,
        )
        .unwrap();

        assert_eq!(plan.actions, vec![ReconcileAction::Noop { project_id }]);
    }

    #[test]
    fn cloud_binding_absence_preserves_local_duplicate_without_a_project_tombstone() {
        let project_id = Uuid::new_v4();
        let cloud_runner_id = Uuid::new_v4();
        let stale_runner_id = Uuid::new_v4();
        let mut snap = snapshot("acme", vec![project(project_id, "WEB")]);
        snap.bindings.push(ProjectRunnerBinding {
            runner_id: cloud_runner_id,
            runner_name: "auto-web".into(),
            project_id,
            project_identifier: "WEB".into(),
            pod_id: Uuid::new_v4(),
            pod_slug: "web-default".into(),
            binding_source: RunnerBindingSource::AutoProjectSync,
        });
        let mut missing = MissingTracker::default();

        let first = plan_reconciliation(
            &snap,
            "acme",
            &[
                local(stale_runner_id, Some(project_id), Some("WEB"), true),
                local(cloud_runner_id, Some(project_id), Some("WEB"), true),
            ],
            &mut missing,
        )
        .unwrap();

        assert_eq!(first.actions, vec![ReconcileAction::Noop { project_id }]);
        assert_eq!(missing.count(stale_runner_id), 0);

        let second_snap = next_snapshot(&snap);
        let second = plan_reconciliation(
            &second_snap,
            "acme",
            &[
                local(stale_runner_id, Some(project_id), Some("WEB"), true),
                local(cloud_runner_id, Some(project_id), Some("WEB"), true),
            ],
            &mut missing,
        )
        .unwrap();

        assert_eq!(second.actions, vec![ReconcileAction::Noop { project_id }]);
        assert_eq!(missing.count(stale_runner_id), 0);
    }

    #[test]
    fn binding_absence_never_deletes_a_concurrently_created_runner() {
        // The cloud creates a runner before MachineControl/CLI persists it.
        // ProjectSync can fetch its snapshot in that narrow gap and then see
        // the new local block. One stale observation must not delete that
        // just-created binding. Even repeated complete snapshots are not
        // deletion authority without an explicit project tombstone.
        let project_id = Uuid::new_v4();
        let runner_id = Uuid::new_v4();
        let snap = snapshot("acme", vec![project(project_id, "WEB")]);
        let runner = local(runner_id, Some(project_id), Some("WEB"), true);
        let mut missing = MissingTracker::default();

        let first = plan_reconciliation(&snap, "acme", std::slice::from_ref(&runner), &mut missing)
            .unwrap();
        assert!(first.actions.is_empty());
        assert_eq!(missing.count(runner_id), 0);

        let second =
            plan_reconciliation(&next_snapshot(&snap), "acme", &[runner], &mut missing).unwrap();
        assert!(second.actions.is_empty());
        assert_eq!(missing.count(runner_id), 0);
    }

    #[test]
    fn project_visibility_absence_without_tombstone_preserves_local_runner() {
        let project_id = Uuid::new_v4();
        let runner_id = Uuid::new_v4();
        let snap = snapshot("acme", vec![]);
        let runner = local(runner_id, Some(project_id), Some("PRIVATE"), true);
        let mut missing = MissingTracker::default();

        let first = plan_reconciliation(&snap, "acme", std::slice::from_ref(&runner), &mut missing)
            .unwrap();
        let second =
            plan_reconciliation(&next_snapshot(&snap), "acme", &[runner], &mut missing).unwrap();

        assert!(first.actions.is_empty());
        assert!(second.actions.is_empty());
        assert_eq!(missing.count(runner_id), 0);
    }

    #[test]
    fn legacy_runner_without_project_uuid_fails_closed_on_tombstone() {
        let deleted_project_id = Uuid::new_v4();
        let runner_id = Uuid::new_v4();
        let snap = tombstone(snapshot("acme", vec![]), deleted_project_id);
        let runner = local(runner_id, None, Some("DELETED"), true);
        let mut missing = MissingTracker::default();

        let first = plan_reconciliation(&snap, "acme", std::slice::from_ref(&runner), &mut missing)
            .unwrap();
        let second =
            plan_reconciliation(&next_snapshot(&snap), "acme", &[runner], &mut missing).unwrap();

        assert!(first.actions.is_empty());
        assert!(second.actions.is_empty());
        assert_eq!(missing.count(runner_id), 0);
    }

    #[test]
    fn replaying_one_project_tombstone_does_not_confirm_deletion() {
        let project_id = Uuid::new_v4();
        let runner_id = Uuid::new_v4();
        let snap = tombstone(snapshot("acme", vec![]), project_id);
        let runner = local(runner_id, Some(project_id), Some("WEB"), true);
        let mut missing = MissingTracker::default();

        let first = plan_reconciliation(&snap, "acme", std::slice::from_ref(&runner), &mut missing)
            .unwrap();
        let replay =
            plan_reconciliation(&snap, "acme", std::slice::from_ref(&runner), &mut missing)
                .unwrap();

        assert!(first.actions.is_empty());
        assert!(replay.actions.is_empty());
        assert_eq!(missing.count(runner_id), 1);

        let confirmed =
            plan_reconciliation(&next_snapshot(&snap), "acme", &[runner], &mut missing).unwrap();
        assert_eq!(
            confirmed.actions,
            vec![ReconcileAction::Remove { runner_id }]
        );
    }

    #[test]
    fn preserves_multiple_cloud_bindings_for_one_project() {
        let project_id = Uuid::new_v4();
        let first_runner_id = Uuid::new_v4();
        let second_runner_id = Uuid::new_v4();
        let mut snap = snapshot("acme", vec![project(project_id, "WEB")]);
        for (runner_id, name) in [
            (first_runner_id, "manual-web"),
            (second_runner_id, "manual-web-extra"),
        ] {
            snap.bindings.push(ProjectRunnerBinding {
                runner_id,
                runner_name: name.into(),
                project_id,
                project_identifier: "WEB".into(),
                pod_id: Uuid::new_v4(),
                pod_slug: "web-default".into(),
                binding_source: RunnerBindingSource::Manual,
            });
        }
        let mut missing = MissingTracker::default();

        let plan = plan_reconciliation(
            &snap,
            "acme",
            &[
                local(first_runner_id, Some(project_id), Some("WEB"), true),
                local(second_runner_id, Some(project_id), Some("WEB"), true),
            ],
            &mut missing,
        )
        .unwrap();

        assert!(
            plan.actions
                .iter()
                .all(|action| !matches!(action, ReconcileAction::Remove { .. })),
            "active cloud bindings must not be treated as stale local rows: {:?}",
            plan.actions
        );
    }

    #[test]
    fn inactive_local_binding_is_activated_without_ensuring_another_runner() {
        let project_id = Uuid::new_v4();
        let runner_id = Uuid::new_v4();
        let mut snap = snapshot("acme", vec![project(project_id, "WEB")]);
        snap.bindings.push(ProjectRunnerBinding {
            runner_id,
            runner_name: "auto-web".into(),
            project_id,
            project_identifier: "WEB".into(),
            pod_id: Uuid::new_v4(),
            pod_slug: "web-default".into(),
            binding_source: RunnerBindingSource::AutoProjectSync,
        });
        let mut missing = MissingTracker::default();

        let plan = plan_reconciliation(
            &snap,
            "acme",
            &[local(runner_id, Some(project_id), Some("WEB"), false)],
            &mut missing,
        )
        .unwrap();

        assert_eq!(plan.actions, vec![ReconcileAction::Activate { runner_id }]);
    }

    #[test]
    fn one_project_tombstone_does_not_remove_and_two_remove() {
        let project_id = Uuid::new_v4();
        let runner_id = Uuid::new_v4();
        let mut missing = MissingTracker::default();
        let mut present = snapshot("acme", vec![project(project_id, "WEB")]);
        present.bindings.push(ProjectRunnerBinding {
            runner_id,
            runner_name: "restored-web".into(),
            project_id,
            project_identifier: "WEB".into(),
            pod_id: Uuid::new_v4(),
            pod_slug: "web-default".into(),
            binding_source: RunnerBindingSource::AutoProjectSync,
        });
        let absent = tombstone(snapshot("acme", vec![]), project_id);
        let runner = local(runner_id, Some(project_id), Some("WEB"), true);

        let first =
            plan_reconciliation(&absent, "acme", std::slice::from_ref(&runner), &mut missing)
                .unwrap();
        assert!(first.actions.is_empty());
        assert_eq!(missing.count(runner_id), 1);

        let invalid = ProjectBindingSnapshot {
            complete: false,
            ..absent.clone()
        };
        assert!(
            plan_reconciliation(
                &invalid,
                "acme",
                std::slice::from_ref(&runner),
                &mut missing,
            )
            .is_err()
        );
        assert_eq!(missing.count(runner_id), 1);

        let second_absent = next_snapshot(&absent);
        let second = plan_reconciliation(
            &second_absent,
            "acme",
            std::slice::from_ref(&runner),
            &mut missing,
        )
        .unwrap();
        assert_eq!(second.actions, vec![ReconcileAction::Remove { runner_id }]);
        assert_eq!(missing.count(runner_id), 2);

        let restored = plan_reconciliation(&present, "acme", &[runner], &mut missing).unwrap();
        assert_eq!(restored.actions, vec![ReconcileAction::Noop { project_id }]);
        assert_eq!(missing.count(runner_id), 0);
    }

    #[test]
    fn wrong_workspace_does_not_advance_missing_counts() {
        let project_id = Uuid::new_v4();
        let runner_id = Uuid::new_v4();
        let mut missing = MissingTracker::default();
        let snap = snapshot("other", vec![]);
        let runner = local(runner_id, Some(project_id), Some("WEB"), true);

        assert!(plan_reconciliation(&snap, "acme", &[runner], &mut missing).is_err());
        assert_eq!(missing.count(runner_id), 0);
    }

    #[test]
    fn authoritative_binding_activates_a_runner_with_an_incorrect_workspace_tag() {
        let project_id = Uuid::new_v4();
        let runner_id = Uuid::new_v4();
        let mut snap = snapshot("acme", vec![project(project_id, "WEB")]);
        snap.bindings.push(ProjectRunnerBinding {
            runner_id,
            runner_name: "acme-web".into(),
            project_id,
            project_identifier: "WEB".into(),
            pod_id: Uuid::new_v4(),
            pod_slug: "web-default".into(),
            binding_source: RunnerBindingSource::AutoProjectSync,
        });
        let runner = local_in_workspace(
            runner_id,
            Some(project_id),
            Some("WEB"),
            Some("other"),
            false,
        );
        let mut missing = MissingTracker::default();

        let first = plan_reconciliation(&snap, "acme", std::slice::from_ref(&runner), &mut missing)
            .unwrap();
        assert_eq!(first.actions, vec![ReconcileAction::Activate { runner_id }]);
        assert_eq!(missing.count(runner_id), 0);
    }

    #[test]
    fn runner_from_another_workspace_without_a_binding_is_never_removed() {
        let project_id = Uuid::new_v4();
        let runner_id = Uuid::new_v4();
        let snap = snapshot("acme", vec![]);
        let runner = local_in_workspace(
            runner_id,
            Some(project_id),
            Some("WEB"),
            Some("other"),
            true,
        );
        let mut missing = MissingTracker::default();

        let first = plan_reconciliation(&snap, "acme", std::slice::from_ref(&runner), &mut missing)
            .unwrap();
        let second =
            plan_reconciliation(&next_snapshot(&snap), "acme", &[runner], &mut missing).unwrap();

        assert!(first.actions.is_empty());
        assert!(second.actions.is_empty());
        assert_eq!(missing.count(runner_id), 0);
    }

    #[test]
    fn other_workspace_runners_consume_capacity_without_becoming_stale() {
        let project_id = Uuid::new_v4();
        let snap = snapshot("acme", vec![project(project_id, "WEB")]);
        let locals = (0..MAX_RUNNERS_PER_DAEMON)
            .map(|_| local_in_workspace(Uuid::new_v4(), None, None, Some("other"), true))
            .collect::<Vec<_>>();
        let mut missing = MissingTracker::default();

        let first = plan_reconciliation(&snap, "acme", &locals, &mut missing).unwrap();
        let second =
            plan_reconciliation(&next_snapshot(&snap), "acme", &locals, &mut missing).unwrap();

        assert!(first.actions.is_empty());
        assert_eq!(first.skipped_capacity, vec![project_id]);
        assert!(second.actions.is_empty());
        assert_eq!(second.skipped_capacity, vec![project_id]);
        assert!(
            locals
                .iter()
                .all(|runner| missing.count(runner.runner_id) == 0)
        );
    }

    #[test]
    fn capacity_below_limit_allows_ensure_with_other_workspace_runners() {
        let project_id = Uuid::new_v4();
        let snap = snapshot("acme", vec![project(project_id, "WEB")]);
        let locals = (0..(MAX_RUNNERS_PER_DAEMON - 1))
            .map(|_| local_in_workspace(Uuid::new_v4(), None, None, Some("other"), true))
            .collect::<Vec<_>>();
        let mut missing = MissingTracker::default();

        let plan = plan_reconciliation(&snap, "acme", &locals, &mut missing).unwrap();

        assert_eq!(plan.actions, vec![ReconcileAction::Ensure { project_id }]);
        assert!(plan.skipped_capacity.is_empty());
    }

    fn assert_malformed_snapshot_rejected(
        snap: ProjectBindingSnapshot,
        local_project_id: Uuid,
        local_runner_id: Uuid,
    ) {
        let runner = local(local_runner_id, Some(local_project_id), Some("WEB"), true);
        let mut missing = MissingTracker::default();
        // A malformed complete snapshot must not consume another absence
        // observation.  Starting at one makes an accidental planner mutation
        // visible without requiring a second call.
        missing.counts.insert(local_runner_id, 1);

        let result = plan_reconciliation(&snap, "acme", &[runner], &mut missing);

        assert!(
            result.is_err(),
            "malformed snapshot was accepted: {result:?}"
        );
        assert_eq!(missing.count(local_runner_id), 1);
    }

    #[test]
    fn snapshot_rejects_binding_for_unknown_project_without_planning_actions() {
        let project_id = Uuid::new_v4();
        let unknown_project_id = Uuid::new_v4();
        let runner_id = Uuid::new_v4();
        let mut snap = snapshot("acme", vec![project(project_id, "WEB")]);
        snap.bindings.push(ProjectRunnerBinding {
            runner_id,
            runner_name: "auto-unknown".into(),
            project_id: unknown_project_id,
            project_identifier: "UNKNOWN".into(),
            pod_id: Uuid::new_v4(),
            pod_slug: "unknown-default".into(),
            binding_source: RunnerBindingSource::AutoProjectSync,
        });

        assert_malformed_snapshot_rejected(snap, project_id, runner_id);
    }

    #[test]
    fn snapshot_rejects_duplicate_project_ids_without_planning_actions() {
        let project_id = Uuid::new_v4();
        let runner_id = Uuid::new_v4();
        let snap = snapshot(
            "acme",
            vec![project(project_id, "WEB"), project(project_id, "OTHER")],
        );

        assert_malformed_snapshot_rejected(snap, project_id, runner_id);
    }

    #[test]
    fn snapshot_rejects_duplicate_deleted_project_ids_without_planning_actions() {
        let project_id = Uuid::new_v4();
        let runner_id = Uuid::new_v4();
        let mut snap = snapshot("acme", vec![]);
        snap.deleted_project_ids = vec![project_id, project_id];

        assert_malformed_snapshot_rejected(snap, project_id, runner_id);
    }

    #[test]
    fn snapshot_rejects_project_marked_active_and_deleted_without_planning_actions() {
        let project_id = Uuid::new_v4();
        let runner_id = Uuid::new_v4();
        let mut snap = snapshot("acme", vec![project(project_id, "WEB")]);
        snap.deleted_project_ids.push(project_id);

        assert_malformed_snapshot_rejected(snap, project_id, runner_id);
    }

    #[test]
    fn snapshot_rejects_duplicate_runner_ids_without_planning_actions() {
        let first_project_id = Uuid::new_v4();
        let second_project_id = Uuid::new_v4();
        let runner_id = Uuid::new_v4();
        let mut snap = snapshot(
            "acme",
            vec![
                project(first_project_id, "WEB"),
                project(second_project_id, "API"),
            ],
        );
        for (project_id, project_identifier) in
            [(first_project_id, "WEB"), (second_project_id, "API")]
        {
            snap.bindings.push(ProjectRunnerBinding {
                runner_id,
                runner_name: format!("auto-{project_identifier}"),
                project_id,
                project_identifier: project_identifier.into(),
                pod_id: Uuid::new_v4(),
                pod_slug: format!("{project_identifier}-default"),
                binding_source: RunnerBindingSource::AutoProjectSync,
            });
        }

        assert_malformed_snapshot_rejected(snap, first_project_id, runner_id);
    }

    #[test]
    fn snapshot_rejects_binding_project_identifier_mismatch_without_planning_actions() {
        let project_id = Uuid::new_v4();
        let runner_id = Uuid::new_v4();
        let mut snap = snapshot("acme", vec![project(project_id, "WEB")]);
        snap.bindings.push(ProjectRunnerBinding {
            runner_id,
            runner_name: "auto-web".into(),
            project_id,
            project_identifier: "NOT_WEB".into(),
            pod_id: Uuid::new_v4(),
            pod_slug: "web-default".into(),
            binding_source: RunnerBindingSource::AutoProjectSync,
        });

        assert_malformed_snapshot_rejected(snap, project_id, runner_id);
    }

    #[test]
    fn project_uuid_survives_identifier_rename() {
        let project_id = Uuid::new_v4();
        let runner_id = Uuid::new_v4();
        let mut snap = snapshot("acme", vec![project(project_id, "NEW_WEB")]);
        snap.bindings.push(ProjectRunnerBinding {
            runner_id,
            runner_name: "manual-web".into(),
            project_id,
            project_identifier: "NEW_WEB".into(),
            pod_id: Uuid::new_v4(),
            pod_slug: "new-web-default".into(),
            binding_source: RunnerBindingSource::Manual,
        });
        let mut missing = MissingTracker::default();

        let plan = plan_reconciliation(
            &snap,
            "acme",
            &[local(runner_id, Some(project_id), Some("OLD_WEB"), true)],
            &mut missing,
        )
        .unwrap();

        assert_eq!(plan.actions, vec![ReconcileAction::Noop { project_id }]);
    }

    #[test]
    fn capacity_overflow_is_deterministic_and_preserves_existing_runners() {
        let mut projects = Vec::new();
        for identifier in ["A", "B", "C"] {
            projects.push(project(Uuid::new_v4(), identifier));
        }
        let locals = (0..MAX_RUNNERS_PER_DAEMON)
            .map(|_| local(Uuid::new_v4(), None, None, true))
            .collect::<Vec<_>>();
        let mut missing = MissingTracker::default();
        let snap = snapshot("acme", projects.clone());

        let plan = plan_reconciliation(&snap, "acme", &locals, &mut missing).unwrap();

        assert!(plan.actions.is_empty());
        assert_eq!(
            plan.skipped_capacity,
            projects.into_iter().map(|p| p.id).collect::<Vec<_>>()
        );
        assert_eq!(plan.removed_existing, 0);
    }

    #[test]
    fn capacity_reserves_slots_for_cloud_bindings_recovered_locally() {
        let mut projects = Vec::new();
        let mut bindings = Vec::new();
        let mut locals = Vec::new();

        for index in 0..(MAX_RUNNERS_PER_DAEMON - 1) {
            let project_id = Uuid::new_v4();
            let runner_id = Uuid::new_v4();
            let identifier = format!("BOUND_{index:02}");
            projects.push(project(project_id, &identifier));
            bindings.push(ProjectRunnerBinding {
                runner_id,
                runner_name: format!("runner-{index}"),
                project_id,
                project_identifier: identifier.clone(),
                pod_id: Uuid::new_v4(),
                pod_slug: "default".into(),
                binding_source: RunnerBindingSource::AutoProjectSync,
            });
            locals.push(local(runner_id, Some(project_id), Some(&identifier), true));
        }

        // This cloud binding already occupies the final capacity slot, but its
        // local config is missing and therefore needs a Recover action.
        let recover_project_id = Uuid::new_v4();
        let recover_runner_id = Uuid::new_v4();
        projects.push(project(recover_project_id, "RECOVER"));
        bindings.push(ProjectRunnerBinding {
            runner_id: recover_runner_id,
            runner_name: "recover-runner".into(),
            project_id: recover_project_id,
            project_identifier: "RECOVER".into(),
            pod_id: Uuid::new_v4(),
            pod_slug: "default".into(),
            binding_source: RunnerBindingSource::AutoProjectSync,
        });

        // There is one more active project with no cloud binding. Its Ensure
        // must be skipped: recovering the existing cloud row above already
        // fills the 50-runner capacity.
        let ensure_project_id = Uuid::new_v4();
        projects.push(project(ensure_project_id, "ENSURE"));

        let mut snap = snapshot("acme", projects);
        snap.bindings = bindings;
        let mut missing = MissingTracker::default();

        let plan = plan_reconciliation(&snap, "acme", &locals, &mut missing).unwrap();

        assert!(plan.actions.contains(&ReconcileAction::Recover {
            runner_id: recover_runner_id
        }));
        assert!(!plan.actions.contains(&ReconcileAction::Ensure {
            project_id: ensure_project_id
        }));
        assert_eq!(plan.skipped_capacity, vec![ensure_project_id]);
    }

    #[test]
    fn confirmed_stale_runner_frees_capacity_for_a_new_project() {
        let stale_project_id = Uuid::new_v4();
        let stale_runner_id = Uuid::new_v4();
        let mut projects = Vec::new();
        let mut bindings = Vec::new();
        let mut locals = Vec::new();
        for identifier in 0..(MAX_RUNNERS_PER_DAEMON - 1) {
            let project_id = Uuid::new_v4();
            let runner_id = Uuid::new_v4();
            projects.push(project(project_id, &format!("ACTIVE_{identifier:02}")));
            bindings.push(ProjectRunnerBinding {
                runner_id,
                runner_name: format!("runner-{identifier}"),
                project_id,
                project_identifier: format!("ACTIVE_{identifier:02}"),
                pod_id: Uuid::new_v4(),
                pod_slug: "default".into(),
                binding_source: RunnerBindingSource::AutoProjectSync,
            });
            locals.push(local(
                runner_id,
                Some(project_id),
                Some(&format!("ACTIVE_{identifier:02}")),
                true,
            ));
        }
        let new_project_id = Uuid::new_v4();
        projects.push(project(new_project_id, "NEW"));
        locals.push(local(
            stale_runner_id,
            Some(stale_project_id),
            Some("REMOVED"),
            true,
        ));
        let mut snap = snapshot("acme", projects);
        snap.bindings = bindings;
        snap.deleted_project_ids.push(stale_project_id);
        let mut missing = MissingTracker::default();
        missing.counts.insert(stale_runner_id, 1);

        let plan = plan_reconciliation(&snap, "acme", &locals, &mut missing).unwrap();

        assert_eq!(
            plan.actions
                .iter()
                .filter(|action| matches!(action, ReconcileAction::Remove { runner_id } if *runner_id == stale_runner_id))
                .count(),
            1
        );
        assert!(plan.actions.contains(&ReconcileAction::Ensure {
            project_id: new_project_id
        }));
        assert!(plan.skipped_capacity.is_empty());
    }

    #[test]
    fn missing_binding_for_active_project_does_not_free_capacity() {
        let active_project_id = Uuid::new_v4();
        let active_runner_id = Uuid::new_v4();
        let stale_runner_id = Uuid::new_v4();
        let mut projects = Vec::new();
        let mut bindings = Vec::new();
        let mut locals = Vec::new();

        projects.push(project(active_project_id, "ACTIVE"));
        bindings.push(ProjectRunnerBinding {
            runner_id: active_runner_id,
            runner_name: "active-runner".into(),
            project_id: active_project_id,
            project_identifier: "ACTIVE".into(),
            pod_id: Uuid::new_v4(),
            pod_slug: "default".into(),
            binding_source: RunnerBindingSource::AutoProjectSync,
        });
        locals.push(local(
            active_runner_id,
            Some(active_project_id),
            Some("ACTIVE"),
            true,
        ));
        // This local row still points at an active project, but its cloud
        // runner id is absent from the binding set. That can be an intentional
        // purge_local=false cloud deletion, so no number of snapshots may
        // remove the local row without an explicit project tombstone.
        locals.push(local(
            stale_runner_id,
            Some(active_project_id),
            Some("ACTIVE"),
            true,
        ));

        for index in 0..(MAX_RUNNERS_PER_DAEMON - 2) {
            let project_id = Uuid::new_v4();
            let runner_id = Uuid::new_v4();
            let identifier = format!("BOUND_{index:02}");
            projects.push(project(project_id, &identifier));
            bindings.push(ProjectRunnerBinding {
                runner_id,
                runner_name: format!("runner-{index}"),
                project_id,
                project_identifier: identifier.clone(),
                pod_id: Uuid::new_v4(),
                pod_slug: "default".into(),
                binding_source: RunnerBindingSource::AutoProjectSync,
            });
            locals.push(local(runner_id, Some(project_id), Some(&identifier), true));
        }

        let new_project_id = Uuid::new_v4();
        projects.push(project(new_project_id, "NEW"));
        let mut snap = snapshot("acme", projects);
        snap.bindings = bindings;
        let mut missing = MissingTracker::default();

        let first = plan_reconciliation(&snap, "acme", &locals, &mut missing).unwrap();

        assert!(!first.actions.contains(&ReconcileAction::Remove {
            runner_id: stale_runner_id
        }));
        assert!(!first.actions.contains(&ReconcileAction::Ensure {
            project_id: new_project_id
        }));
        assert_eq!(first.skipped_capacity, vec![new_project_id]);

        let second =
            plan_reconciliation(&next_snapshot(&snap), "acme", &locals, &mut missing).unwrap();

        assert!(!second.actions.contains(&ReconcileAction::Remove {
            runner_id: stale_runner_id
        }));
        assert!(!second.actions.contains(&ReconcileAction::Ensure {
            project_id: new_project_id
        }));
        assert_eq!(second.skipped_capacity, vec![new_project_id]);
    }

    #[test]
    fn missing_tracker_can_be_inspected_without_exposing_mutable_state() {
        let mut tracker = MissingTracker::default();
        let runner_id = Uuid::new_v4();
        tracker.counts.insert(runner_id, 2);
        assert_eq!(tracker.count(runner_id), 2);
        assert_eq!(tracker.count(Uuid::new_v4()), 0);
    }

    #[test]
    fn snapshot_parser_accepts_projects_without_a_default_pod() {
        let project_id = Uuid::new_v4();
        let snapshot: ProjectBindingSnapshot = decode_json(
            &serde_json::to_vec(&serde_json::json!({
                "complete": true,
                "snapshot_id": Uuid::new_v4(),
                "workspace_id": Uuid::new_v4(),
                "workspace_slug": "acme",
                "projects": [{
                    "id": project_id,
                    "identifier": "WEB",
                    "name": "Web",
                    "default_pod_id": null,
                }],
                "bindings": [],
                "deleted_project_ids": [],
            }))
            .unwrap(),
        )
        .unwrap();

        assert_eq!(snapshot.projects[0].default_pod_id, None);
    }

    #[test]
    fn snapshot_parser_rejects_missing_authoritative_lists() {
        // `complete: true` is only trustworthy when all authoritative
        // collections are present. Treating an omitted tombstone list as
        // `[]` could suppress required cleanup; treating omitted project or
        // binding lists as empty could invent desired-state changes.
        let missing_bindings = serde_json::json!({
            "complete": true,
            "snapshot_id": Uuid::new_v4(),
            "workspace_id": Uuid::new_v4(),
            "workspace_slug": "acme",
            "projects": [],
            "deleted_project_ids": []
        });
        let missing_projects = serde_json::json!({
            "complete": true,
            "snapshot_id": Uuid::new_v4(),
            "workspace_id": Uuid::new_v4(),
            "workspace_slug": "acme",
            "bindings": [],
            "deleted_project_ids": []
        });
        let missing_deleted_projects = serde_json::json!({
            "complete": true,
            "snapshot_id": Uuid::new_v4(),
            "workspace_id": Uuid::new_v4(),
            "workspace_slug": "acme",
            "projects": [],
            "bindings": []
        });

        assert!(
            decode_json::<ProjectBindingSnapshot>(&serde_json::to_vec(&missing_bindings).unwrap())
                .is_err()
        );
        assert!(
            decode_json::<ProjectBindingSnapshot>(&serde_json::to_vec(&missing_projects).unwrap())
                .is_err()
        );
        assert!(
            decode_json::<ProjectBindingSnapshot>(
                &serde_json::to_vec(&missing_deleted_projects).unwrap()
            )
            .is_err()
        );
    }

    #[test]
    fn ensure_response_parser_keeps_project_and_binding_identity() {
        let runner_id = Uuid::new_v4();
        let project_id = Uuid::new_v4();
        let response: crate::cloud::http::EnsureProjectBindingResponse = decode_json(
            &serde_json::to_vec(&serde_json::json!({
                "runner_id": runner_id,
                "runner_name": "auto-web",
                "project_id": project_id,
                "project_identifier": "WEB",
                "pod_id": Uuid::new_v4(),
                "pod_slug": "web-default",
                "workspace_slug": "acme",
                "binding_source": "auto_project_sync",
            }))
            .unwrap(),
        )
        .unwrap();

        assert_eq!(response.runner_id, runner_id);
        assert_eq!(response.project_id, project_id);
        assert_eq!(
            response.binding_source,
            RunnerBindingSource::AutoProjectSync
        );
    }

    #[tokio::test]
    async fn sync_trigger_serializes_overlapping_runs() {
        let trigger = SyncTrigger::new();
        let active = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let max_active = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let mut tasks = Vec::new();
        for _ in 0..8 {
            let trigger = trigger.clone();
            let active = active.clone();
            let max_active = max_active.clone();
            tasks.push(tokio::spawn(async move {
                trigger
                    .run_serialized(|| async {
                        let now = active.fetch_add(1, std::sync::atomic::Ordering::SeqCst) + 1;
                        max_active.fetch_max(now, std::sync::atomic::Ordering::SeqCst);
                        tokio::task::yield_now().await;
                        active.fetch_sub(1, std::sync::atomic::Ordering::SeqCst);
                    })
                    .await;
            }));
        }
        for task in tasks {
            task.await.unwrap();
        }
        assert_eq!(max_active.load(std::sync::atomic::Ordering::SeqCst), 1);
    }

    #[tokio::test(start_paused = true)]
    async fn project_sync_timer_waits_after_the_initial_pass() {
        let mut interval = project_sync_interval();

        // Tokio intervals yield an immediately-ready first tick. The daemon
        // consumes that tick as part of startup, so a second pass must not be
        // ready until the configured polling period has elapsed.
        assert!(
            tokio::time::timeout(std::time::Duration::ZERO, interval.tick())
                .await
                .is_ok()
        );
        assert!(
            tokio::time::timeout(std::time::Duration::ZERO, interval.tick())
                .await
                .is_err()
        );

        tokio::time::advance(std::time::Duration::from_secs(60)).await;
        assert!(
            tokio::time::timeout(std::time::Duration::ZERO, interval.tick())
                .await
                .is_ok()
        );
    }
}
