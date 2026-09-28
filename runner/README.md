# karma-team Runner

Local daemon + TUI (`karma-team` binary) that connects a developer machine to the karma-team cloud and drives `codex app-server` for assigned tasks.

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

Set only one source variable. An invalid explicit path, failed download, or empty download fails the installation and preserves the previously installed binary. Temporary download files are removed. Archive URLs (`.tar.gz`, `.tar.xz`, `.zip`) must be unpacked first and installed using `KARMA_TEAM_LOCAL_BIN`.

Custom URLs control this installation only. The shell installer starts `karma-team auth login` when a terminal is available and skips authentication in headless environments. These source variables apply to `install.sh`; the Windows PowerShell wrapper below uses GitHub Releases.

### GitHub Releases

Once release artifacts have been published, prebuilt binaries for macOS (arm64, x86_64), Linux (arm64, x86_64 — glibc and musl), and Windows (x86_64) can be installed from GitHub Releases. The generated cargo-dist installers choose the platform artifact. They are separate from the local-first `runner/install.sh` above.

**macOS and Linux:**

```bash
curl --proto '=https' --tlsv1.2 -LsSf \
  https://github.com/xerecter/karma-team/releases/latest/download/karma-team-installer.sh | sh
karma-team auth login
```

**Windows (PowerShell):**

```powershell
irm https://github.com/xerecter/karma-team/releases/latest/download/install.ps1 | iex
```

**Windows (Command Prompt):**

```bat
powershell -c "irm https://github.com/xerecter/karma-team/releases/latest/download/install.ps1 | iex"
```

### Platform notes

| Environment               | Behaviour                                                                                                                                                                                                      |
| ------------------------- | -------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| macOS arm64 / x86_64      | Native builds.                                                                                                                                                                                                 |
| Linux x86_64 / arm64      | Native glibc builds; the installer falls back to the static musl build on older distros (Ubuntu 20.04, Debian 11, RHEL 8, Amazon Linux 2).                                                                     |
| Alpine / musl             | Served by the same musl builds.                                                                                                                                                                                |
| Windows x86_64            | Native build.                                                                                                                                                                                                  |
| Windows arm64             | No native build; the installer serves the x86_64 build, which runs under Windows-on-ARM emulation.                                                                                                             |
| Git Bash / MSYS2 / Cygwin | Supported — the cargo-dist installer maps `MINGW*`/`MSYS*`/`CYGWIN*` to the Windows target and unpacks the Windows build; `install.sh` accepts the resulting `karma-team.exe`.                                 |
| WSL                       | Installs the **Linux** build, which can only drive agents inside the WSL distro. `install.sh` warns about this. If your coding agents are installed on the Windows side, use the PowerShell one-liner instead. |

The installer will prompt for your karma-team cloud URL, walk you through device-code approval in the browser, and offer to register this host as a runner — for the typical dev-laptop case, that's the entire setup.

### Installing a specific version (pinning / prereleases)

To select a specific release, use the generated cargo-dist installer (`karma-team-installer.sh` / `karma-team-installer.ps1`) and swap `latest` for the tag you want. The PowerShell wrapper uses the latest release regardless of the URL used to fetch the wrapper; the source-checkout shell installer selects local files or `KARMA_TEAM_BIN_URL`.

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

**Prerequisite:** the runner shells out to [Codex](https://github.com/openai/codex) — install it and make sure `codex --version` works before running `karma-team configure`. `karma-team doctor` checks this.

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

Then run the setup steps manually:

```bash
# 1. Log in as your user. Opens a browser to approve a short code shown in
#    the terminal — same idea as `gh auth login` or `stripe login`. Stores
#    a CLI token at ~/.config/karma-team/config.toml.
karma-team auth login --url https://karma-team.example.com

# 2. Register this host as a runner. Uses the token from step 1 to mint
#    runner credentials cloud-side; no enrollment-token paste needed. On
#    the first runner, installs the systemd user unit, launchd agent, or
#    Windows per-user scheduled task and starts the daemon.
karma-team runner add --project WEB

# 3. Optional: open the interactive UI.
karma-team tui
```

`karma-team auth login` prompts to add a runner inline when no runner exists yet on the host — for the dev-laptop case, that single command is enough. Bare `karma-team` with no subcommand also drops into the login flow when no config exists, so if you installed via the MSI or skipped auto-auth, you can re-trigger setup just by typing `karma-team`.

Useful follow-ups:

```bash
karma-team auth status               # who am I logged in as; runner list
karma-team auth logout               # revoke the CLI token server-side
karma-team runner add --project X    # add another runner
karma-team runner list / remove      # manage runners
```

## Task folders and Git

Direct-mode runners can execute coding and non-coding tasks in an ordinary
working directory. No Git repository or project repository URL is required;
existing files are preserved, and the runner does not initialize Git for you.
This applies to user-connected agents and the built-in desktop agent alike.

When a repository URL is supplied, the runner still clones into an empty
directory or reuses an existing repository. It refuses to clone over files in
a non-repository directory. Branch checkout only applies to repositories.
Explicit worktree pools remain Git-based; use a direct working directory for
repo-free tasks.

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
