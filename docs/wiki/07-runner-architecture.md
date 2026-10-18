# 07 — Runner Architecture

The runner is the agent on the developer's machine: a single Rust binary (`karma-team`) that authenticates to karma-team cloud, pulls assigned runs, and drives the Karma agent (`karma`) as a subprocess. Legacy bridge implementations remain in the crate but are not selectable.

It lives at `runner/` and is an **independent Cargo crate** — not part of the Turborepo workspace. Edition 2024, MSRV 1.93 (`rust-toolchain.toml` pins the toolchain).

## Module layout

```
runner/src/
├── main.rs               ← tokio entrypoint
├── lib.rs                ← module root
├── cli/                  ← clap subcommands
├── daemon/               ← supervisor + state machine (including project_sync, observability, runner_instance, state)
├── cloud/                ← HTTP client + message schemas (http, projects, protocol, runners; legacy WS support)
├── codex/                ← `codex app-server` subprocess + JSON-RPC bridge (app_server, bridge, jsonrpc, mod, schema)
├── claude_code/          ← Claude Code adapter
├── agent/                ← generic agent trait (so new agents can be wired in)
├── approval/             ← policy engine + first-writer-wins router
├── workspace/            ← per-project working dir + `git clone` on first task
├── ipc/                  ← Unix socket / Windows named pipe between daemon and TUI/CLI
├── history/              ← JSONL per-run transcripts + recent-runs index
├── service/              ← systemd / launchd / Windows scheduled-task installers
├── tui/                  ← Ratatui UI (Status / Runs / Config / Approvals views)
├── config/               ← TOML config + credential files (0600 on Unix)
├── api_client.rs         ← HTTP API client (registration, status, version checks)
└── util/                 ← paths, logging, backoff, signal handling
```

## CLI surface (`cli/`)

The `karma-team` binary is a clap subcommand router:

```
karma-team auth login / logout / status      ← device-code login, token mgmt
karma-team runner add / list / remove        ← optionally manage additional runners on this host
karma-team configure                          ← (re)write config
karma-team install / uninstall                ← install OS service unit
karma-team start / stop / restart             ← daemon lifecycle
karma-team status                             ← service + daemon status
karma-team tui                                ← interactive TUI
karma-team doctor                             ← preflight checks (agent on PATH, cloud reachable)
karma-team update [--check|--restart]         ← self-update via cargo-dist receipt
karma-team issue / comment / state / workspace ← assorted helper commands
karma-team __run                              ← hidden internal run wrapper
```

Bare `karma-team` (no subcommand) drops into `auth login` when no config exists — useful as the first-run path after MSI install.

## Daemon (`daemon/`)

The daemon is the long-lived process. After account/workspace login, its responsibilities:

1. **Supervisor** (`supervisor.rs`) — drives the state machine: connect → register → poll for runs → dispatch → report.
2. **Runner instance** (`runner_instance.rs`) — one daemon can manage multiple runner registrations (normally one per project, plus any intentional manual extras).
3. **ProjectSync** (`project_sync.rs`) — discovers all active workspace memberships, caches a separate machine token per workspace, and fetches each workspace's complete visible-project, machine-binding, and deleted-project tombstone snapshot. It ensures a default-pod runner for each otherwise-unbound project, including projects created by other members, and recovers cloud bindings missing from local config. Local runner removal requires confirmed project deletion. Successful reconciliations repeat every 60 seconds; transient failures retry with bounded backoff. A secondary workspace failure does not prevent the others from synchronizing. This runs alongside the existing per-runner task-polling and dispatch lifecycle.
4. **Observability** (`observability.rs`) — structured logging + metrics.
5. **State** (`state.rs`) — in-memory state shared between IPC, TUI, and supervisor.

Project removal is fail-closed: project or binding absence can result from a
permission change or a runner-only deletion and never proves that the project
was deleted. Cleanup requires the immutable project UUID in the explicit
`deleted_project_ids` list of two different, validated complete snapshots.
Snapshot fetch, completeness, workspace-validation failures, and replaying one
snapshot do not advance removal state. Once confirmed, the daemon removes the
local config block and runner-specific runtime data; project deletion has
already removed the cloud runner transactionally. The configured project
working directory and Git checkout are not deleted. Legacy local rows without
a project UUID fail closed and require manual cleanup. Tombstones live in an
independent deletion ledger, so they remain available after Project retention
hard-deletes the original soft-deleted row.

The daemon talks to the cloud via `cloud/` (HTTPS + WS) and to the local TUI via `ipc/` (Unix socket on Unix, named pipe on Windows — both `0600`).

## Codex / Claude Code bridges

Codex (`codex/`) is the first-class integration:

- `app_server.rs` spawns `codex app-server` as a subprocess.
- `jsonrpc.rs` is the JSON-RPC client side of the bridge.
- `bridge.rs` mediates between the cloud-side run state and the agent process.
- `schema.rs` types the JSON-RPC messages.

Claude Code (`claude_code/`) and the generic `agent/` trait are the abstraction that lets additional agents be wired in without re-doing the orchestration layer. Adding a new agent kind means implementing the trait and wiring it into the supervisor's dispatch.

## Approval router (`approval/`)

When the agent asks for approval (run command, write file, fetch URL — depends on agent policy), the runner has three potential decision sources:

1. The **TUI** (interactive operator on this host)
2. The **cloud** (web UI operator)
3. The local **policy engine** (rules in the runner's config)

Decisions race; **first writer wins**. The router enforces this. The policy engine can pre-approve common patterns to avoid bombing the operator with prompts.

## Workspace (`workspace/`)

A runner can be registered to a project. On the first run for that project, the runner `git clone`s the project's repo into a configured working dir. Subsequent runs reuse the clone (fetch + checkout, not re-clone).

This is also where path constraints live — the runner refuses to operate on paths outside the configured workspace root.

## IPC (`ipc/`)

- **Unix:** Unix domain socket under `$XDG_RUNTIME_DIR/...`, `0600`.
- **Windows:** local named pipe.

Used by:

- TUI ↔ daemon — push state updates, send approval decisions
- CLI subcommands ↔ daemon — `karma-team status`, `karma-team stop`, etc.

## TUI (`tui/`)

Built on Ratatui. Views:

- **Status** — connection state, version banner, agent path
- **Runs** — recent runs + their phase + transcripts
- **Config** — editable daemon settings (auto-update toggle lives here)
- **Approvals** — pending prompts, decision UI

## On-disk layout (Linux example)

```
~/.config/karma-team/             ← config.toml, credentials (0600)
$XDG_DATA_HOME/karma-team/        ← per-run JSONL transcripts, recent-runs index
$XDG_RUNTIME_DIR/karma-team/      ← IPC socket, PID file
```

On Windows, everything goes under the user profile and IPC is via a named pipe.

## Auto-update

The cloud's `welcome` frame can include `latest_runner_version` and `min_runner_version` (driven by the cloud's `LATEST_RUNNER_VERSION` / `MIN_RUNNER_VERSION` env vars). With auto-update enabled, the daemon swaps the on-disk `karma-team` binary in place; the **running process is never disturbed** — it keeps its loaded copy until the next natural restart (`karma-team restart`, host reboot, service-manager respawn).

`karma-team update --restart` is the one-command manual path: swap the binary, restart the daemon, and wait for it to reconnect. `karma-team update` without `--restart` only swaps the binary and applies on the next natural restart. Both paths only work for installs from cargo-dist installers (`karma-team-installer.sh`, `karma-team-installer.ps1`, MSI) — those leave an install receipt the updater reads. Source builds and `cargo install` builds get a clear "reinstall via the installer if you want self-update" error. See [15 — Releasing](./15-releasing.md).

## Where to read next

- [08 — Cloud ↔ runner protocol](./08-cloud-runner-protocol.md) — the wire schema for `cloud/`
- [11 — Authentication](./11-authentication.md) — device-code login + token rotation
- `runner/README.md` — install one-liners, paths, manual QA matrix
- `runner/.ai_design/implement_runner/` (if present) — design docs and committed decisions
