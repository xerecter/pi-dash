# Auto-sync daemon project runners

## Summary

After a user binds a karma-team account and workspace to a local machine, the
daemon shall continuously reconcile the active cloud projects in that workspace
with the runners hosted by that daemon. Every active project shall have at least
one valid local/cloud runner binding on the machine without requiring
`karma-team runner add --project ...`. A project removed from the cloud shall
eventually have all of its runners on that machine detached and their daemon
runtime resources released.

The existing runner execution, assignment polling, heartbeat, task lifecycle,
chat, and result-upload paths remain unchanged. Project reconciliation runs as
an independent machine-level task and uses the existing runner hot-add and
`RemoveRunner` teardown paths.

## Motivation

The current onboarding flow binds an account and workspace but still requires
the user to register runners project by project. This is easy to forget and
leaves newly created projects unable to execute work until another CLI command
is run. Likewise, cloud project deletion does not currently guarantee that a
stale local runner is removed.

Automatic desired-state reconciliation makes account binding the only required
setup action and repairs state after daemon restarts, transient network errors,
and partially completed runner creation.

## Scope

### In scope

- Start the daemon service after successful account/workspace binding, even
  when no runner is configured yet.
- Poll a machine-token-authenticated, workspace-scoped project snapshot once at
  daemon startup and every 60 seconds thereafter.
- Return both active projects and active cloud runner bindings for the current
  dev machine in a complete, unpaginated snapshot.
- Identify projects by immutable project UUID. Preserve `project_slug` for
  display and backward compatibility.
- Ensure one usable runner binding per active project on the current machine.
- Make server-side automatic runner creation idempotent under concurrent and
  repeated requests by combining a dev-machine row lock, an automatic-binding
  marker, and a database uniqueness constraint on the automatic default-pod
  binding.
- Make local config persistence and runtime activation idempotent.
- Recover when the cloud runner exists but the local config write or hot-add did
  not finish.
- Detect stale local/cloud bindings after two consecutive successful complete
  snapshots and tear them down through the existing runner deletion and
  `RemoveRunner` paths.
- On explicit cloud project deletion, proactively remove related cloud runners
  and enqueue local teardown; periodic reconciliation remains the repair path
  when a message is missed or the daemon was offline.
- Expose automatic binding provenance through the runner API so the UI and
  support tooling can distinguish automatic and manual runners.
- Add Rust and Django tests for idempotency, concurrency, recovery, deletion,
  authorization boundaries, and preservation of existing runner behavior.
- Update CLI help, onboarding text, and runner documentation so manual project
  binding is no longer presented as a required setup step.

### Out of scope

- Synchronizing more than the single workspace bound to the local daemon.
- Creating one runner per non-default pod. Automatic runners use the project's
  default pod; users may still add extra manual runners or runners in other pods.
- Cross-project work stealing.
- Deleting user-selected working directories, repository clones, or Karma
  project directories when a runner is removed.
- Replacing existing runner assignment, heartbeat, execution, chat, or task
  result protocols.
- Removing the manual `runner add` command; it remains available for additional
  runners and non-default pod use cases.

## Functional requirements

1. A successful `karma-team auth login` persists the machine token, workspace,
   and dev-machine identity, then idempotently installs and starts the daemon
   service.
2. A daemon with a valid machine token and dev-machine identity may run with
   zero configured runners.
3. The project snapshot endpoint derives its workspace and dev machine only
   from the authenticated machine token. A caller cannot request another
   workspace or machine.
4. Snapshot responses explicitly declare that they are complete and include
   the workspace UUID/slug, active project UUID/identifier/default pod, and
   active runner bindings for the authenticated dev machine.
5. A snapshot transport error, authentication error, server error, parse error,
   incomplete response, or workspace mismatch must never remove a runner.
6. For every active project with no valid local/cloud runner pair, the daemon
   calls an idempotent ensure endpoint. The endpoint returns the same active
   runner for repeated or concurrent automatic requests.
7. If a suitable manual runner for the same machine/project already exists,
   it satisfies the desired state and no automatic duplicate is created.
8. If the cloud binding exists but local config is absent, ensure returns that
   binding so the daemon can repair local config without creating a new row.
9. If local config exists but the runner is not active in the in-memory daemon
   registry, the daemon retries hot activation without creating another cloud
   runner.
10. Local configuration writes use the existing host-wide config lock. Applying
    the same runner response more than once is a no-op.
11. Every automatically created runner receives a unique working directory and
    uses the currently supported default Karma agent configuration.
12. A project is considered removed only after its immutable UUID appears in
    the explicit deleted-project list of two different successful complete
    snapshots for the same workspace. Project/binding absence and replaying the
    same snapshot are not deletion evidence. Deleted-project UUIDs are retained
    in an independent ledger after the soft-deleted Project row is hard-deleted.
13. Removal cancels any active run/chat, stops the runner's HTTP/heartbeat
    tasks, removes it from daemon registries, removes its `[[runner]]` block,
    and deletes its runner-specific data directory.
14. Removal does not recursively delete the runner's configured working
    directory, whether it was generated automatically or selected by a user.
15. The project-delete transaction removes cloud runner rows. Reconciliation's
    fallback teardown is local-only and idempotent.
16. One project's create/delete failure is logged and retried without blocking
    reconciliation or task execution for other projects.
17. The existing cap of 50 runners per daemon remains enforced. When the number
    of desired projects exceeds available capacity, existing runners continue
    operating and excess projects are reported and retried rather than causing
    daemon startup failure.

## Success criteria

- Binding an account to a workspace containing projects starts the daemon and
  creates/activates one runner for every otherwise-unbound active project.
- Creating a project while the daemon is running produces a usable runner
  without any user command within one normal polling interval.
- Ten concurrent ensure requests for the same machine/project return one runner
  ID and leave exactly one automatic runner row.
- Repeated daemon reconciliation does not add duplicate runner rows or duplicate
  `[[runner]]` blocks.
- A crash after cloud ensure but before local persistence repairs the same
  runner on the next reconciliation.
- A temporary empty/error response does not remove any local runner.
- Deleting a cloud project removes its cloud runners and causes the local daemon
  to release only the corresponding runtime/config/data resources; unrelated
  project runners continue pulling and running tasks.
- Existing runner lifecycle and task-polling test suites remain green.
