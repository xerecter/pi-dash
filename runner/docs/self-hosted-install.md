# Self-hosted CLI distribution

The web onboarding and AI Dev Machines pages use the running backend's
`/api/cli/install-config/` endpoint. Installers, binaries, and login commands
all use the configured public service address. GitHub is not involved in this
installation path.

## Configure the deployment

Set these values in `apps/api/.env`:

```dotenv
KARMA_TEAM_PUBLIC_URL=http://172.16.13.81:28000
KARMA_TEAM_CLI_ARTIFACT_DIR=/code/cli-artifacts
KARMA_TEAM_CLI_INSTALLER_DIR=/code/cli-installers
```

`KARMA_TEAM_PUBLIC_URL` is the address reachable from developers' machines,
including the port when required. When unset, the backend uses `WEB_URL`,
then `APP_BASE_URL`, then the request origin. Do not use a Docker-internal
hostname such as `api:8000` for this setting.

Compose mounts `deployments/cli/artifacts/` read-only at `/code/cli-artifacts`
and `runner/` read-only at `/code/cli-installers`. Set
`KARMA_TEAM_CLI_ARTIFACTS_HOST_DIR` in the root `.env` to use a different host
artifact directory. These binaries are generated deployment files and are
not checked into Git or copied into web images.

## Publish a binary

From the repository root, build and publish the host platform's current code:

```bash
sh runner/publish-cli.sh
```

The publisher builds with `cargo build -p karma-team --release --locked` and
atomically replaces the downloadable executable. Existing downloads keep
their open file; new downloads receive the newly published binary.

To publish a binary built on another platform:

```bash
KARMA_TEAM_CLI_TARGET=aarch64-apple-darwin \
  KARMA_TEAM_CLI_LOCAL_BINARY=/path/to/karma-team \
  sh runner/publish-cli.sh
```

Set `KARMA_TEAM_CLI_ARTIFACTS_HOST_DIR` for the publisher as well if you changed
the Compose host directory. Without `KARMA_TEAM_CLI_LOCAL_BINARY`, a non-host
target is built with Cargo `--target` and requires its target toolchain/linker.

Artifacts are stored as `<artifact-dir>/<target>/karma-team`, or
`karma-team.exe` on Windows. The supported target names are:

- `x86_64-unknown-linux-gnu`
- `aarch64-unknown-linux-gnu`
- `x86_64-unknown-linux-musl`
- `aarch64-unknown-linux-musl`
- `x86_64-apple-darwin`
- `aarch64-apple-darwin`
- `x86_64-pc-windows-msvc`

Only platforms with an actual published binary are available. A missing or
unsupported target returns HTTP 404; the backend never downloads from GitHub
as a fallback. An optional Windows MSI can be published at
`<artifact-dir>/karma-team-x86_64-pc-windows-msvc.msi`; the UI only shows its
download link when this file exists.

For the initial deployment, rebuild the changed services:

```bash
docker compose build api web
docker compose up -d --no-deps api web
```

Further binary publications do not require rebuilding or restarting the API.

## Install from the service

Linux / macOS (requires a published binary for the target platform):

```bash
curl -fsSL 'http://172.16.13.81:28000/api/cli/install.sh' | sh
```

Run this as the user who will own the runner, without `sudo`. The shell
installer writes to `$HOME/.local/bin` and adds the installation directory to
the user's `.profile` and applicable Bash/Zsh startup files. Existing settings
are preserved, and repeated installs do not duplicate the PATH entry. Zsh's
`ZDOTDIR` is respected. The installer cannot change its parent shell, so activate
the default directory in the current terminal with:

```bash
export PATH="$HOME/.local/bin${PATH:+:$PATH}"
karma-team --version
```

To use a different writable directory, pass the variable to `sh`, not `curl`:

```bash
curl -fsSL 'http://172.16.13.81:28000/api/cli/install.sh' |
  KARMA_TEAM_INSTALL_DIR="$HOME/bin" sh
```

Use the PATH command printed by the installer for a custom directory. An
unwritable installation directory fails before downloading the binary with an
actionable error. An unwritable startup file only produces a warning: the
binary remains installed, and PATH can be configured manually. Automatic PATH
setup covers sh/Bash/Zsh; other shells require their own PATH configuration.

Runner authentication and the daemon belong to this same user. On Linux, run
subsequent runner commands from a normal user login session with an available
`systemctl --user` bus; switching users with `sudo -u` does not necessarily
create that session. Installing the binary does not migrate an existing root
runner or start a new daemon in headless mode.

Windows PowerShell (requires the Windows binary):

```powershell
irm 'http://172.16.13.81:28000/api/cli/install.ps1' | iex
```

The served installer includes the public service URL as its default
`KARMA_TEAM_API_URL`. It detects the platform, downloads the raw binary from
`/api/cli/bin/<target>/`, and starts `karma-team auth login --url <service>`
when a terminal is available. Headless installation prints the login command
instead. Authentication writes the service URL into the CLI's existing local
configuration; tokens are not embedded in the downloaded installer.

## Change the service address

Change `KARMA_TEAM_PUBLIC_URL` in `apps/api/.env`, then recreate the API:

```bash
docker compose up -d --no-deps --force-recreate api
```

Reload the installation page to receive the new address. No frontend rebuild
is needed for this setting. Also update `WEB_URL` / `APP_BASE_URL` when moving
the entire application, since they are used by other application links.

Already installed clients retain their saved connection. Reconnect them
explicitly when migrating the service:

```bash
karma-team auth login --url https://new-karma.example.com
```

The installer can be rerun to upgrade from the self-hosted binary. The existing
`karma-team update` command uses the upstream release mechanism; use the
self-hosted updater for this deployment's custom builds:

```bash
curl -fsSL 'http://172.16.13.81:28000/api/cli/update.sh' | sh
```

The updater selects the current platform, downloads the published binary,
checks that it is a `karma-team` executable, and atomically replaces the
current user's `~/.local/bin/karma-team`. An active user service is restarted
only when all runners report idle, with no current runs or pending approvals.
If runners are busy or status cannot be confirmed, the binary is still updated
and the script asks you to run `karma-team restart` after tasks finish.
Automatic restart checks use Python 3 or jq; without either, restart is deferred.
These checks are status snapshots, so use `--no-restart` when tasks could arrive
during the update and an uninterrupted daemon is required.

Existing configuration, credentials, and runner registrations are preserved.
Pass `sh -s -- --check` to check without replacing the installed binary, or
`sh -s -- --no-restart` to always defer the daemon restart. Run it as the runner
owner without `sudo`. For a custom installation directory, pass
`KARMA_TEAM_INSTALL_DIR` to `sh`, as with the installer. `--force` replaces even
an identical binary and repeats the idle restart check.

## Uninstall from the service

The matching POSIX uninstaller is available from the same deployment:

```bash
curl -fsSL 'http://172.16.13.81:28000/api/cli/uninstall.sh' | sh
```

For safety, a piped invocation requires an interactive terminal and asks for
confirmation. In automation, pass the confirmation flag explicitly:

```bash
curl -fsSL 'http://172.16.13.81:28000/api/cli/uninstall.sh' |
  sh -s -- --yes
```

Preview the cleanup without stopping services or changing files:

```bash
curl -fsSL 'http://172.16.13.81:28000/api/cli/uninstall.sh' |
  sh -s -- --dry-run --yes
```

The uninstaller runs the installed CLI's `remove --all` first so configured
runner registrations and the shared CLI token are removed before local state is
deleted. If cloud cleanup fails, it exits non-zero and preserves the local
configuration for recovery. Use `--local-only` only when cloud records will be
removed separately:

```bash
curl -fsSL 'http://172.16.13.81:28000/api/cli/uninstall.sh' |
  sh -s -- --local-only --yes
```

After confirmation, it removes the current user's karma-team binary, service
unit, credentials, runner history/logs/cache/runtime data, and the PATH block
that the installer added to shell startup files. It keeps parent directories,
other commands in `~/.local/bin`, and project workspaces or Git checkouts.
Run it as the same non-root user that installed the runner; do not use `sudo`.
