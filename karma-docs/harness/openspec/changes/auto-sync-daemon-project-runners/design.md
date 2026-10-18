# Auto-sync daemon project runners: design

## 1. Architecture

The cloud is the source of truth for the active project set and cloud runner
rows. The local `config.toml` plus the supervisor's in-memory registry are the
local state to reconcile. Reconciliation is deliberately machine-level: it is
started beside the existing machine control session and never runs inside an
individual runner's polling loop.

```text
auth login
  -> persist machine token + workspace + dev_machine_id
  -> install/start daemon, even with zero runners

daemon ProjectSync loop
  -> GET complete machine-scoped snapshot
  -> compare cloud projects + cloud bindings + local config + live registry
  -> ensure missing binding (idempotent cloud transaction)
  -> persist missing config block (idempotent config mutation)
  -> hot-add missing live RunnerInstance
  -> confirm stale binding in two complete snapshots
  -> idempotent cloud delete + synthesized local RemoveRunner

existing per-runner loops
  -> assignment polling, heartbeat, agent execution, chat, result upload
     remain unchanged
```

The cloud project deletion endpoint also proactively tears down related runners.
This is a latency optimization and cloud-resource cleanup path. It is not the
only correctness mechanism: the snapshot loop repairs missed messages after an
offline period or process restart.

## 2. Cloud data model and idempotency

`Runner` gains a `binding_source` text choice:

- `manual` (default for all existing rows and normal `runner add` calls)
- `auto_project_sync` (created by the ensure endpoint)
- existing `provisioning` remains unchanged because it describes who installed
  and controls the machine (`manual` versus `desktop_bundled`), not why a runner
  was bound to a project.

Automatic runners always target the project's active default pod. A conditional
unique constraint covers:

```text
(dev_machine_id, pod_id, binding_source)
WHERE binding_source = 'auto_project_sync'
  AND revoked_at IS NULL
```

The ensure transaction also locks the `DevMachine` row before checking or
creating a runner. The row lock serializes requests even before the unique
constraint is evaluated; the constraint is the final database-level defense.

Ensure resolution order inside one transaction is:

1. Authenticate a non-revoked machine token and derive its workspace and
   `dev_machine_id`; reject a token without a dev machine.
2. Lock that dev-machine row and verify ownership/non-revocation.
3. Resolve and lock the active project by UUID in the token workspace, then
   lock its default pod. Automatic runners inherit the locked dev machine's
   provisioning mode; desktop-bundled runners continue to obey the existing
   managed-runner cap.
4. Return the oldest active runner already attached to this machine/project,
   regardless of whether it is manual or automatic. A manual binding already
   satisfies desired state.
5. Otherwise create an `auto_project_sync` runner in the default pod.
6. If the conditional uniqueness constraint still races, catch the integrity
   error and re-read the winning automatic runner.

The endpoint returns an enrollment-compatible payload containing `runner_id`,
`runner_name`, `project_id`, `project_identifier`, `pod_id`, `pod_slug`, and
`workspace_slug`. Machine-token runners do not need per-runner secrets.

Automatic names contain a sanitized project identifier and a short machine ID,
so workspace-wide runner-name uniqueness does not collide across projects or
machines.

## 3. Machine-scoped snapshot API

A new machine-token-only endpoint is mounted below `/api/v1/runner/` rather than
changing the response contract of the existing project picker. Its response is:

```json
{
  "complete": true,
  "snapshot_id": "<uuid>",
  "workspace_id": "<uuid>",
  "workspace_slug": "acme",
  "projects": [
    {
      "id": "<project-uuid>",
      "identifier": "WEB",
      "name": "Web",
      "default_pod_id": "<pod-uuid>"
    }
  ],
  "bindings": [
    {
      "runner_id": "<runner-uuid>",
      "runner_name": "auto_WEB_a1b2c3d4",
      "project_id": "<project-uuid>",
      "project_identifier": "WEB",
      "pod_id": "<pod-uuid>",
      "binding_source": "auto_project_sync"
    }
  ],
  "deleted_project_ids": ["<deleted-project-uuid>"]
}
```

The endpoint is unpaginated because the daemon cap is 50, and the project set
must be evaluated as one authoritative snapshot for deletion safety. The query
uses the token's workspace directly; it neither accepts nor infers a caller-
supplied workspace. Active project and pod managers exclude soft-deleted rows.

Bindings include active runner rows for the authenticated dev machine. Deleted
project UUIDs come from an independent workspace-scoped deletion ledger and are
the only destructive evidence. The project-delete transaction idempotently
writes this ledger before runner teardown; a migration backfills existing
soft-deleted projects, so tombstones survive later Project hard deletion. The
separate collections let the daemon distinguish:

- project and cloud/local runner all exist: no-op;
- project and cloud runner exist, local config missing: repair local state;
- project and local runner exist, cloud row missing: preserve local state because
  this may be an intentional runner-only deletion with `purge_local=false`;
- project exists with no runner anywhere on this machine: ensure one;
- project or binding is absent without a tombstone: preserve local state;
- project UUID is tombstoned in two different complete snapshots: remove its
  local runner state (the project-delete transaction already removed cloud rows).

## 4. Local configuration identity

`RunnerConfig` gains optional `project_id` and optional `binding_source` fields.
Both fields use serde defaults so older files continue parsing. New automatic
bindings always populate them. Existing manual runners are lazily backfilled
with `project_id` when their unique workspace/project identifier matches a
snapshot project.

Project UUID is the reconciliation key. `project_slug` remains required because
it is displayed in status and sent in Hello for compatibility. Renaming a
project updates the persisted slug during reconciliation; UUID continuity keeps
the same runner binding.

Applying an enrollment response under the config lock follows these rules:

- an existing identical `runner_id` returns the existing block without pushing
  another block;
- an ensure response for a project already represented locally is a no-op;
- recovery of a distinct cloud `runner_id` is deduplicated only by runner ID,
  so multiple legitimate manual/automatic bindings for one project converge;
- duplicate name, ID, and working-directory validation remains active;
- a config write failure leaves cloud state intact so the next ensure call can
  return and repair the same runner.

## 5. Daemon reconciliation algorithm

`ProjectSync` owns one Tokio task and one reconciliation mutex. Supervisor code
creates only one task per daemon; the mutex additionally prevents an immediate
manual trigger and timer tick from overlapping.

The loop performs an immediate attempt, then waits 60 seconds between attempts.
Transient failures use the existing bounded exponential-backoff utility with
jitter. A successful snapshot resets the backoff.

For each successful complete snapshot:

1. Verify the returned workspace matches the locally bound workspace.
2. Reload `config.toml` under normal read semantics; take short snapshots of the
   live runner registry without holding a lock across network or filesystem IO.
3. Match local runners to projects by `project_id`, with a one-time slug fallback
   for legacy config.
4. For every active project:
   - if a cloud binding and matching local config exist but the runner is not
     live, call `RunnerSpawnCtx::add_runner`;
   - if no valid local/cloud pair exists, call ensure, idempotently persist the
     response, then call `add_runner`;
   - record errors for that project and continue with the next project.
5. For every in-scope local runner whose project UUID or exact cloud binding is
   absent, record the successful snapshot ID in its in-memory missing tracker.
   Reset the tracker whenever the exact binding reappears. A failed or
   malformed snapshot never advances the tracker, and replaying the same
   snapshot ID does not count as another observation.
6. When a runner is missing from two distinct, consecutive complete snapshots,
   call the idempotent cloud delete and deliver a synthesized
   `ServerMsg::RemoveRunner` to its local mailbox. Existing teardown cancels
   work and removes config/data.
7. A local runner missing from cloud bindings while its project remains active
   uses exactly the same two-distinct-complete-snapshot confirmation; an active
   project is not grounds for immediate deletion. This protects a concurrently
   created runner from being deleted by a snapshot fetched just before its
   cloud transaction committed. Capacity is released only after confirmation.
8. Runners explicitly tagged for a different workspace are neither removed nor
   metadata-refreshed by this workspace's snapshot, but still count toward the
   daemon-wide 50-runner capacity. Legacy rows without a workspace tag are
   reconciled only through the current snapshot's binding/identifier evidence.

If the snapshot request fails, `complete` is false, workspace identity does not
match, JSON parsing fails, or authentication fails, the method returns before
updating missing counts. No deletion is possible from an unsuccessful pass.

Before creation, the daemon computes remaining capacity against the existing
50-runner cap. Excess desired projects are logged in deterministic identifier
order and retried on later passes; existing runners are never stopped to make
room.

## 6. Teardown and project deletion

`RunnerSpawnCtx` exposes a local idempotent remove method that sends the same
synthetic `RemoveRunner` envelope used by IPC. It treats an absent mailbox and
already-removed config block as success.

The cloud delete client treats HTTP 204 and 404 as success. Thus these races are
safe:

- project deletion pushes `RemoveRunner` before the sync loop calls delete;
- two sync attempts observe the same stale runner;
- the runner receives cloud and synthesized local removal concurrently;
- the daemon is stopped while the cloud row is already gone.

The project deletion view gathers related runners before marking the project
deleted and invokes the existing runner deletion service for each. That service
enqueues `RemoveRunner`, revokes the runner/session, and removes the cloud row.
If the daemon is offline, the later machine snapshot omits the project/binding,
and local reconciliation performs the missing cleanup.

Local teardown first removes the persisted config entry, signals run/chat and
transport teardown, waits for worker completion, and only then deletes runtime
data. The wait matters because a cancelling worker persists its final run index
and would otherwise recreate a directory that had already been removed.
Repeated deletion requests retain a live teardown owner until this sequence is
complete.

Local teardown releases:

- the active agent process/run and chat lane;
- HTTP/heartbeat/refresh activity;
- supervisor mailbox, hello map, and IPC registry entries;
- the `[[runner]]` config block and per-runner credentials;
- `data/runners/<runner_id>` logs/history/runtime data.

It deliberately preserves `RunnerConfig.workspace.working_dir` and Karma project
directories because they can contain user files or uncommitted work. Recursive
deletion of those paths is not authorized by project deletion.

## 7. Login and service lifecycle

After machine-token exchange succeeds, `auth login` idempotently writes the
service unit and calls the platform service manager's enable/start operation.
The daemon already permits zero runners when machine control credentials are
present; the project-sync task now gives that idle daemon useful work.

If service installation/start fails, authentication remains persisted and the
CLI returns an actionable error. A subsequent `karma-team start` or repeated
login can retry without creating duplicate runners.

Removing the last project runner does not uninstall the shared daemon service.
It stays alive to discover a future cloud project.

On daemon shutdown, machine control, ProjectSync, and IPC mutation ingress are
frozen and awaited before the supervisor snapshots and drains in-flight runs.
This prevents a hot-add/remove operation from changing the registry during the
drain sequence.

## 8. Failure handling and observability

Every sync pass logs a structured summary containing snapshot ID, project count,
binding count, created/recovered/activated/removed counts, skipped-capacity
count, and per-project failures. Tokens and secrets are never logged.

Error policy:

| Failure | Behavior |
|---|---|
| Snapshot network/5xx/parse failure | Retry with backoff; do not change absence counters or delete |
| 401/403 machine token failure | Log terminal auth error and stop sync task; existing runners continue their own lifecycle |
| Ensure conflict won by concurrent request | Re-read and return the winning runner |
| Local config write failure | Keep/reuse cloud runner on next pass; do not create another |
| Hot-add failure after config write | Retry activation from persisted config on next pass |
| One project ensure failure | Continue other projects, retry failed project later |
| Cloud delete timeout | Keep local binding for retry unless a cloud `RemoveRunner` already arrived |
| Local remove delivered twice | Treat absent mailbox/config/data as success |
| Desired projects exceed 50 | Keep existing runners, skip deterministic overflow, log capacity error |

## 9. Test strategy

### Django

- Machine token snapshot is scoped to exactly the token workspace and machine.
- Session, API token without machine identity, wrong machine, and revoked token
  cannot use the machine snapshot/ensure endpoints.
- Soft-deleted projects are absent; active projects and bindings serialize with
  stable UUIDs.
- Repeated ensure returns one runner.
- Threaded concurrent ensure requests produce one automatic runner row and the
  same runner ID; the test uses the real test database, not a mocked ORM.
- A pre-existing manual runner is reused.
- A crash-recovery-style ensure returns an existing automatic runner.
- Project deletion calls runner teardown for every related runner and does not
  affect another project's runners.
- Existing runner create/delete/project contract tests remain green.

### Rust

- Snapshot and ensure response parsing includes project/pod UUID identity.
- Reconciliation planning is a pure function tested for no-op, create, recover,
  activate, stale-local replacement, and confirmed delete actions.
- Failed/incomplete/wrong-workspace snapshots never advance absence counts.
- Missing authoritative `projects` or `bindings` JSON fields fail decoding;
  explicit empty arrays remain valid.
- Two successful missing snapshots are required for delete.
- A stale-but-active-project binding also requires two snapshots, covering the
  fetch-before-concurrent-create race.
- A runner belonging to another workspace is preserved while counting toward
  global capacity.
- Concurrent triggers are serialized by the reconciliation mutex.
- Enrollment response application is idempotent by runner ID.
- Recovery preserves two distinct cloud runner IDs for the same project.
- Local removal is idempotent and preserves unrelated runner config.
- Zero-runner daemon configuration with machine credentials starts project sync.
- Mock HTTP integration verifies repeated passes issue one ensure and no
  duplicate config block.
- Existing runner assignment, heartbeat, task pulling, run completion, chat,
  protocol, and configuration suites remain green.

## 10. Compatibility and rollout

All new Rust config fields and cloud response fields are additive. Older daemons
ignore the extra API fields and continue requiring manual registration. New
daemons talking to a cloud without the snapshot endpoint log that automatic
project sync is unavailable while existing configured runners continue working.

The database migration defaults `binding_source` to `manual`, so current runner
rows retain their behavior. Manual CLI/API runner creation remains unchanged.
The automatic feature activates only when both machine token and dev-machine
identity are present.
