# karma-team Runner

Local daemon + TUI (`karma-team` binary) that connects a developer machine to the karma-team cloud and drives `karma agent` for assigned tasks.

## Supported agent

This CLI supports only Karma. `karma-team runner add --project WEB` defaults
to `--agent karma`, and an omitted `[runner.agent]` section also defaults to
Karma. Explicitly selecting another agent fails with an error such as:

```text
unsupported agent 'codex'; only the karma agent is supported
```

The restriction applies to runner registration, config saves and daemon
startup, local agent packages, and runtime dispatch. Existing non-Karma
configs remain readable for inspection/removal but are not silently migrated.
To use an existing runner with Karma, configure it explicitly and ensure the
Karma executable is installed and authenticated:

```toml
[runner.agent]
kind = "karma"

[runner.karma]
binary = "karma"
```

Codex, Claude Code, Cursor Agent, OpenClaw, Grok, and Muse Code adapters remain
in the source tree; their CLI entry points are disabled. The Codex-specific
managed enrollment/rebind path is disabled as well.

## Developer-owned agent packages

OSS exposes an optional [agent package seam](docs/agent-packages.md) for
developers who supply their own executable or protocol adapter:

```bash
karma-team runner use-package my-runner --manifest /path/to/agent-package.toml
```

The manifest selects an existing bridge protocol and a local executable. OSS
does not bundle, download, install, authenticate, or update the agent package.
The developer owns those responsibilities and restarts the owning daemon when
idle. Ordinary task folders do not require Git.

## Install

### From your karma-team service

The web onboarding and AI Dev Machines pages obtain installation commands from
the backend at runtime. For the current deployment:

```bash
curl -fsSL 'http://172.16.13.81:28000/api/cli/install.sh' | sh
```

```powershell
irm 'http://172.16.13.81:28000/api/cli/install.ps1' | iex
```

The backend's `KARMA_TEAM_PUBLIC_URL` setting controls both the download and
login addresses. The installer downloads a published platform binary from the
service and starts authentication with that service. Missing platform binaries
return an explicit error. See [Self-hosted CLI distribution](docs/self-hosted-install.md)
for configuration, publishing builds, and changing the service address.

On Linux/macOS, run the installer as the runner's user without `sudo`. It
installs into `~/.local/bin` and configures the user's sh/Bash/Zsh startup files.
For the current terminal, run `export PATH="$HOME/.local/bin${PATH:+:$PATH}"`
after installation, or use the printed PATH command for a custom install
directory. Existing startup settings are preserved; if a startup file cannot
be updated, installation succeeds with a warning and manual PATH instructions.

The corresponding user-scoped uninstaller is served by the same deployment:

```bash
curl -fsSL 'http://172.16.13.81:28000/api/cli/uninstall.sh' | sh
```

It asks for confirmation when run interactively. Use `sh -s -- --dry-run --yes`
to preview the files and service actions, or `sh -s -- --yes` for a confirmed
non-interactive removal. Cloud runner deregistration is attempted before local
state deletion; a cloud failure aborts and preserves credentials. `--local-only`
skips that cloud step. The script removes only the current user's karma-team
binary, daemon, state, logs, cache, runtime data, and installer PATH block; it
does not remove project workspaces, Git checkouts, or other commands.

To update an already installed self-hosted CLI, run the updater as the same
user that owns the runner:

```bash
curl -fsSL 'http://172.16.13.81:28000/api/cli/update.sh' | sh
```

The updater downloads the binary for the current platform, validates it by
running `karma-team --version`, atomically replaces the installed executable,
and restarts an active user service only when all runners report idle. Busy
runners, pending approvals, or an unavailable status defer the restart; run
`karma-team restart` after tasks finish to load the new daemon code. Automatic
restart checks use Python 3 or jq when available; without either, the binary
still updates and the restart is deferred. It does not run login or change
local configuration. Use `sh -s -- --check` to check for an update without
changing files, or `sh -s -- --no-restart` to always defer the daemon restart.
Run it without `sudo`.

### From a source checkout or a custom binary

`runner/install.sh` defaults to installing an existing local executable. It checks the workspace's release/debug builds, `runner/target`, and the installation directory. If none exists, it builds the release binary from source with Cargo. It does not download a prebuilt binary by default.

```bash
# Run from the repository root; install a local build or build from source.
bash ./runner/install.sh

# Select a local executable explicitly. Relative paths use the repository root.
KARMA_TEAM_LOCAL_BIN=/path/to/karma-team bash ./runner/install.sh

# Download a raw executable from your own server.
KARMA_TEAM_BIN_URL=https://downloads.example.com/linux-amd64/karma-team \
  bash ./runner/install.sh

# Select the installation directory as well.
KARMA_TEAM_BIN_URL=https://downloads.example.com/linux-amd64/karma-team \
  KARMA_TEAM_INSTALL_DIR="$HOME/.local/bin" \
  bash ./runner/install.sh
```

| Variable                 | Meaning                                                                                                                           |
| ------------------------ | --------------------------------------------------------------------------------------------------------------------------------- |
| `KARMA_TEAM_LOCAL_BIN`   | Existing executable to install; takes precedence over automatic local discovery.                                                  |
| `KARMA_TEAM_BIN_URL`     | HTTP(S) URL of a raw executable for the target OS and CPU; uses `curl` or `wget`. No Cargo build is needed.                       |
| `KARMA_TEAM_INSTALL_DIR` | Destination directory; defaults to `~/.local/bin`. The installed command is `karma-team` (`karma-team.exe` for an `.exe` source). |
| `KARMA_TEAM_API_URL`     | Service URL for platform download and login. Explicit local/binary sources still take precedence.                                 |

Set only one source variable. An invalid explicit path, failed download, or empty download fails the installation and preserves the previously installed binary. Temporary download files are removed. Archive URLs (`.tar.gz`, `.tar.xz`, `.zip`) must be unpacked first and installed using `KARMA_TEAM_LOCAL_BIN`.

The shell installer starts `karma-team auth login` when a terminal is available and skips authentication in headless environments. When `KARMA_TEAM_API_URL` is set, it passes that address to login. The Windows PowerShell installer also downloads directly from the configured service.

### Upstream GitHub Releases (optional)

Once release artifacts have been published, prebuilt binaries for macOS (arm64, x86_64), Linux (arm64, x86_64 — glibc and musl), and Windows (x86_64) can be installed from GitHub Releases. The generated cargo-dist installers choose the platform artifact. They are separate from the local-first `runner/install.sh` above.

**macOS and Linux:**

```bash
curl --proto '=https' --tlsv1.2 -LsSf \
  https://github.com/xerecter/karma-team/releases/latest/download/karma-team-installer.sh | sh
karma-team auth login
```

**Windows (PowerShell):**

```powershell
irm https://github.com/xerecter/karma-team/releases/latest/download/karma-team-installer.ps1 | iex
karma-team auth login --url https://karma-team.example.com
```

**Windows (Command Prompt):**

```bat
powershell -c "irm https://github.com/xerecter/karma-team/releases/latest/download/karma-team-installer.ps1 | iex"
```

### Upstream release platform notes

| Environment               | Behaviour                                                                                                                                                                                                      |
| ------------------------- | -------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| macOS arm64 / x86_64      | Native builds.                                                                                                                                                                                                 |
| Linux x86_64 / arm64      | Native glibc builds; the installer falls back to the static musl build on older distros (Ubuntu 20.04, Debian 11, RHEL 8, Amazon Linux 2).                                                                     |
| Alpine / musl             | Served by the same musl builds.                                                                                                                                                                                |
| Windows x86_64            | Native build.                                                                                                                                                                                                  |
| Windows arm64             | No native build; the installer serves the x86_64 build, which runs under Windows-on-ARM emulation.                                                                                                             |
| Git Bash / MSYS2 / Cygwin | Supported — the cargo-dist installer maps `MINGW*`/`MSYS*`/`CYGWIN*` to the Windows target and unpacks the Windows build; `install.sh` accepts the resulting `karma-team.exe`.                                 |
| WSL                       | Installs the **Linux** build, which can only drive agents inside the WSL distro. `install.sh` warns about this. If your coding agents are installed on the Windows side, use the PowerShell one-liner instead. |

The installer prompts for your karma-team cloud URL and walks you through
device-code approval in the browser. Successful account/workspace binding
starts the daemon even with zero local runners. The daemon immediately
discovers all active workspace memberships and creates one isolated runner per
otherwise-unbound visible project, including projects created by other members.
Memberships added after login are picked up on the next reconciliation pass
(normally within 60 seconds). Each workspace keeps its own machine credential;
agent CLI commands use the running task's workspace and project. Assigned local
work items run on a current assignee's machine. No project-by-project
registration is required.

### Installing a specific version (pinning / prereleases)

To select a specific upstream release, use the generated cargo-dist installer (`karma-team-installer.sh` / `karma-team-installer.ps1`) and swap `latest` for the tag you want. Self-hosted installers instead serve the binary published by the deployment operator.

```bash
# macOS / Linux — bare installer (no auto-auth)
curl --proto '=https' --tlsv1.2 -LsSf \
  https://github.com/xerecter/karma-team/releases/download/karma-team-v0.1.0/karma-team-installer.sh | sh
```

```powershell
# Windows — bare installer
irm https://github.com/xerecter/karma-team/releases/download/karma-team-v0.1.0/karma-team-installer.ps1 | iex
```

**Prereleases** (tags with a SemVer suffix like `-rc.1`, `-alpha.1`, `-beta.1`) are deliberately excluded from `/releases/latest/`, so the public one-liners above keep serving the last stable release. To install a prerelease for testing, use the tag-pinned URL above with the rc tag, e.g.:

```bash
curl --proto '=https' --tlsv1.2 -LsSf \
  https://github.com/xerecter/karma-team/releases/download/karma-team-v0.1.5-rc.1/karma-team-installer.sh | sh
```

**Prerequisite:** install and authenticate Karma, and make sure `karma --version`
works before adding a runner. `karma-team doctor` checks the Karma installation.

### Alternative install paths

If you'd rather install the binary without the auto-auth (e.g. for a base image, MSI-based deploy, or any CI context where auth happens later), use the underlying cargo-dist installers directly:

```bash
# macOS / Linux
curl --proto '=https' --tlsv1.2 -LsSf \
  https://github.com/xerecter/karma-team/releases/latest/download/karma-team-installer.sh | sh
```

```powershell
# Windows PowerShell
irm https://github.com/xerecter/karma-team/releases/latest/download/karma-team-installer.ps1 | iex
```

Or on Windows, download and run the MSI installer:

<https://github.com/xerecter/karma-team/releases/latest/download/karma-team-x86_64-pc-windows-msvc.msi>

The MSI is a standard double-click Windows Installer. Its final screen carries a **"Sign in to karma-team now"** checkbox: leave it ticked and clicking Finish opens a console window running `karma-team auth login`, so an interactive install finishes setup in one sitting like every other install path.

Two properties of that flow are deliberate:

- **Silent installs spawn nothing.** `msiexec /qn` (Group Policy, Intune, SCCM) skips the installer UI entirely, so the checkbox never runs. Unattended IT deploys are unaffected.
- **Sign-in runs as the installing user, not SYSTEM.** The package is per-machine and its execute sequence runs elevated, but the ExitDialog runs unelevated — which matters, because `auth login` writes the CLI token and workspace binding into the current user's profile.

For silent installs, or if you clear the checkbox, sign in from a terminal: bare `karma-team` with no config drops straight into `auth login`, as does `karma-team auth login`. The MSI also installs a **Start Menu → karma-team → "Sign in to karma-team"** shortcut that runs the same flow — the discoverable path for silent/Group-Policy deploys where the ExitDialog never appears.

Windows release assets also include a `karma-team-x86_64-pc-windows-msvc.zip` archive with `karma-team.exe` for advanced/manual installs.

Then run the setup step manually:

```bash
# 1. Log in as your user. Opens a browser to approve a short code shown in
#    the terminal — same idea as `gh auth login` or `stripe login`. Stores
#    a CLI token at ~/.config/karma-team/config.toml.
karma-team auth login --url https://karma-team.example.com

# Login writes/refreshes the service unit and starts the daemon. The daemon
# discovers all active cloud projects and creates their runners automatically.

# 2. Optional: open the interactive UI.
karma-team tui
```

`karma-team auth login` is the complete normal onboarding flow. It binds the
account, workspace, and dev-machine identity, then starts a zero-runner-capable
daemon. Project creation and deletion are reflected locally on the next
reconciliation pass. Bare `karma-team` with no subcommand also drops into the
login flow when no config exists, so if you installed via the MSI or skipped
auto-auth, you can re-trigger setup just by typing `karma-team`.

Useful follow-ups:

```bash
karma-team auth status               # who am I logged in as; runner list
karma-team auth logout               # revoke the CLI token server-side
karma-team runner add --project X    # optional extra/manual runner
karma-team runner list / remove      # manage runners
```

Automatic bindings use each project's default pod. Use `runner add` only when
you intentionally need an additional runner or a non-default pod.

## Task folders and Git

Direct-mode runners can execute coding and non-coding tasks in an ordinary
working directory. No Git repository or project repository URL is required;
existing files are preserved, and the runner does not initialize Git for you.
This applies to user-connected agents and the built-in desktop agent alike.
The Karma agent is an exception: its `--worktree` mode requires a Git base, so
the runner initializes one with an empty first commit when needed. Existing
workspace files are not included in that commit.

When a repository URL is supplied, the runner still clones into an empty
directory or reuses an existing repository. It refuses to clone over files in
a non-repository directory. Branch checkout only applies to repositories.
Explicit worktree pools remain Git-based; use a direct working directory for
repo-free tasks.

### Private repository authentication

Repository clones use Git credentials from the operating-system account running
the runner daemon. Signing in with `karma-team auth login` authenticates the
runner to the platform; a GitLab or GitHub account connected in project settings
authenticates the platform's repository integration. Neither supplies Git clone
credentials to the runner.

Before running a work item against a private repository, configure one of these
methods on the runner host, under the same operating-system account as the daemon:

- **HTTP(S):** configure a Git credential helper that works without interactive
  prompts. For GitLab, store a personal or project access token with
  `read_repository` permission in that helper. Tasks that push changes also need
  `write_repository`. Use a credential helper backed by the host's credential
  store and keep tokens out of repository URLs, shell history, and project settings.
- **SSH:** use an SSH repository URL and authorize that account's SSH key in the
  Git provider. Ensure the daemon can access the key or its SSH agent. For a
  host not yet in that account's `known_hosts`, runner-initiated SSH clones
  with the default OpenSSH `StrictHostKeyChecking=ask` policy use
  `StrictHostKeyChecking=accept-new`: the first observed host key is saved,
  and any later key change is rejected. This is trust on first use, not
  independent verification of the host's identity. An explicit non-interactive
  SSH host-key policy (`yes`, `no`/`off`, or `accept-new`) in `~/.ssh/config`
  or the system configuration is preserved, as are custom Git SSH commands
  and variants. An explicit `ask` policy is treated like the OpenSSH default
  because the background daemon has no terminal in which to show that prompt.
  For deployments that need
  verified host keys from the outset, provision `known_hosts` through a trusted
  channel and set `GIT_SSH_COMMAND='ssh -o StrictHostKeyChecking=yes -o BatchMode=yes'`
  in the daemon's service environment. Existing `GIT_SSH_COMMAND`, `GIT_SSH`,
  or Git `core.sshCommand` settings are never replaced by the runner. If Git
  uses `includeIf` configuration, automatic first-use trust is disabled: a
  target-directory-specific SSH command cannot be inspected until Git has
  initialized the clone. Provision `known_hosts` in advance in that case.

Check the repository URL currently saved in project settings, then test it from
the daemon's account. On Linux or macOS, this read-only check disables terminal
credential prompts:

```bash
GIT_TERMINAL_PROMPT=0 git ls-remote 'https://gitlab.example.com/team/project.git' HEAD
```

The command should return the repository's `HEAD` hash (or succeed with no output
for an empty repository). `could not read Username`, `terminal prompts disabled`,
or `Authentication failed` means the daemon still lacks usable Git credentials.
A successful check under a different account does not verify the daemon's
credentials. `karma-team doctor` checks that Git is installed, but does not check
access to the project's repository.

After the check succeeds, retry the failed work item. If you changed environment
variables needed by the credential helper or SSH agent, restart the runner service
when it is idle so the daemon receives the updated environment.

## Auto-update

`karma-team` keeps itself current. When the cloud announces a newer `latest_runner_version` in the welcome frame, the running daemon swaps the on-disk `karma-team` binary in place. The currently-running process is **never disturbed** — it keeps its loaded copy until the next natural restart (`karma-team restart`, host reboot, or a service-manager respawn after a crash). This gives you the Claude-Code-style "always current" experience without ever killing in-flight work.

The toggle lives in the General tab's **Daemon settings** card (`karma-team tui` → `auto_update`). Default is **on**; press Enter to flip, then `[w]` to save. With auto-update off, the runner instead surfaces a yellow `⚠ Update v0.1.x available` advisory in the Connection card and on `karma-team status`, and you apply updates manually:

```bash
karma-team update --restart    # swap and restart the daemon in one shot
karma-team update              # swap binary only; apply on the next restart
karma-team update --check      # report whether an update is available
```

`karma-team update` only works for binaries installed via cargo-dist installers such as `karma-team-installer.sh`, `karma-team-installer.ps1`, or the Windows MSI (it reads the cargo-dist install receipt). Source builds, zip-only installs, and `cargo install`'d binaries don't have a receipt and get a clear "reinstall via the installer if you want self-update" error.

### What the advisory states mean

| State                                                     | TUI / `karma-team status`                                            |
| --------------------------------------------------------- | -------------------------------------------------------------------- |
| Running version ≥ `latest_announced` and ≥ `min_required` | nothing shown                                                        |
| Newer `latest_announced`, swap already on disk            | yellow `⚠ Restart to apply v0.1.x`                                   |
| Newer `latest_announced`, auto-update on, swap pending    | yellow `⚠ Update v0.1.x pending swap`                                |
| Newer `latest_announced`, auto-update off                 | yellow `⚠ Update v0.1.x available — run karma-team update --restart` |
| Running version below `min_required` (cloud-set floor)    | red `⛔ Update required: cloud floor v0.1.x`                         |

`min_required` is advisory in the current implementation — the daemon does not refuse new tasks below the floor. The red banner is the user-facing signal that they should act before the cloud bumps the wire-protocol floor and disconnects them.

### Announcing a release from the cloud

The karma-team backend reads two optional environment variables and folds them into every session-create welcome response:

| Env var                 | Effect                                                                                    |
| ----------------------- | ----------------------------------------------------------------------------------------- |
| `LATEST_RUNNER_VERSION` | Drives the yellow "update available" advisory and triggers auto-swap on opted-in runners. |
| `MIN_RUNNER_VERSION`    | Drives the red "update required" advisory.                                                |

Stable runner releases set `LATEST_RUNNER_VERSION` in Cloud SSM automatically (`RELEASING.md` walks through tagging). Set `MIN_RUNNER_VERSION` manually only when you need to force a floor. Leave both unset to skip the announcement in non-Cloud environments.

## Design docs

See `.ai_design/implement_runner/`:

- `runner-design.md` — architecture + committed decisions
- `github-runner-architecture.md` — reference model (GHA self-hosted runner)
- `tui-design.md` — TUI shape + views

## Layout

```
runner/
├── Cargo.toml                # binary crate, edition 2024, MSRV 1.93
├── src/
│   ├── main.rs               # tokio entrypoint
│   ├── lib.rs                # module root
│   ├── cli/                  # clap subcommands: configure / install / uninstall / start / stop / restart / status / tui / doctor / remove / rotate / issue / comment / state / workspace / __run (hidden)
│   ├── daemon/               # supervisor + state machine
│   ├── cloud/                # WS client, message schemas, registration HTTP
│   ├── codex/                # app-server subprocess + JSON-RPC bridge
│   ├── claude_code/          # `claude --print` stream-json subprocess + bridge
│   ├── cursor_agent/         # `cursor-agent --print` stream-json subprocess + bridge
│   ├── openclaw/             # `acpx ... openclaw exec` ACP (JSON-RPC) subprocess + bridge
│   ├── agent/                # agent-agnostic bridge abstraction over the above
│   ├── workspace/            # working_dir resolution + `git clone` on first task
│   ├── approval/             # policy engine + first-writer-wins router
│   ├── ipc/                  # local IPC between daemon and TUI/CLI
│   ├── history/              # JSONL per-run transcripts + recent-runs index
│   ├── service/              # systemd / launchd / Windows scheduled-task setup
│   ├── tui/                  # Ratatui app + views (Status / Runs / Config / Approvals)
│   ├── config/               # TOML config + credential files (0600)
│   └── util/                 # paths, logging, backoff, signal handling
└── tests/                    # integration tests
```

## Development commands

```bash
cargo build                                  # debug build
cargo test                                   # unit + integration tests
cargo check                                  # quick type-check
cargo clippy -- -D warnings                  # lint
```

From a debug build, substitute `./target/debug/karma-team` for `karma-team` in any of the commands above.

## Runtime paths

- Config: platform config directory for `karma-team`
- Data / logs: platform data directory for `karma-team`
- Runtime dir: platform runtime directory for local IPC and the PID file

On Unix-like systems, secrets on disk are written with `0600` and the IPC socket is also `0600`. On Windows, files are stored under the user's profile directories and daemon IPC uses a local named pipe.

## Protocol

Wire version is `4` — bumped on incompatible shape changes. See `src/cloud/protocol.rs` for the exhaustive schema (including the v3→v4 move from WebSocket to per-runner HTTPS long-poll). The runner authenticates to the cloud with a per-runner access token issued from a refresh-token pair; the cloud echoes an accepted `protocol_version` and may include optional `latest_runner_version` / `min_runner_version` advisories in the `welcome` payload (consumed by the auto-update path).

## Test strategy

- **Unit:** `cargo test` — deterministic table-driven tests for protocol serde, approval policy, reconnect backoff, workspace resolve, config roundtrip.
- **Integration:** `tests/protocol_roundtrip.rs` — every client/server variant round-trips; router state machine invariants.
- **Manual QA** (per release): macOS arm64/x64 + Linux x64 + Windows x64 -> first-run `configure` -> `install` -> `start` -> TUI shows connected -> synthetic run via `/api/runners/runs/` -> approval prompt -> decision.

## Release

Managed by `cargo-dist` (see `dist-workspace.toml` + `.github/workflows/release.yml`). Pushing a SemVer tag (e.g. `v0.1.0`) triggers the workflow, which builds binaries for macOS arm64/x64, Linux arm64/x64, and Windows x64, generates shell, PowerShell, and MSI installers, and publishes everything to a GitHub Release.

To cut a release:

```bash
# bump version in runner/Cargo.toml, commit, then:
git tag v0.1.0
git push origin v0.1.0
```
