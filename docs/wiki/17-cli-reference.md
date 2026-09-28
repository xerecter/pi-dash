# 17 — `karma-team` CLI Reference

Complete reference for the `karma-team` binary. All commands, all flags, brief examples.

For the _guided_ setup walkthrough use [02 — Cloud Quickstart](./02-cloud-quickstart.md). For the runner's internals see [07 — Runner architecture](./07-runner-architecture.md).

## Global flags

Available on every command:

| Flag                  | Env                     | Default        | Purpose                                              |
| --------------------- | ----------------------- | -------------- | ---------------------------------------------------- |
| `--config-dir <PATH>` | `KARMA_TEAM_CONFIG_DIR` | XDG config dir | Override config location                             |
| `--data-dir <PATH>`   | `KARMA_TEAM_DATA_DIR`   | XDG data dir   | Override data location                               |
| `--log <LEVEL>`       | `KARMA_TEAM_LOG`        | `info`         | Log level: `trace`, `debug`, `info`, `warn`, `error` |
| `--help`              | —                       | —              | Show help for any command                            |
| `--version`           | —                       | —              | Print version                                        |

## `karma-team` (no subcommand)

First-run launcher. If no config exists, drops into `karma-team auth login`. Otherwise prints help.

---

## Setup & auth

### `karma-team auth login`

Browser-based device-code login. Same UX as `gh auth login` / `stripe login`.

```
karma-team auth login [--url <URL>] [--no-browser]
```

| Flag           | Purpose                                                                                     |
| -------------- | ------------------------------------------------------------------------------------------- |
| `--url <URL>`  | Cloud base URL (e.g. `https://karma-team.example.com`). Optional if config already has one. |
| `--no-browser` | Don't try to open the verification URL automatically.                                       |

Writes the CLI token to `config.toml`, then confirms the account and workspace and points you at the cloud URL. Registering a runner is a separate, explicit step — see `karma-team runner add`.

### `karma-team auth status`

Show who you're logged in as and which runners are registered on this host.

```
karma-team auth status
```

### `karma-team auth logout`

Revoke the CLI token server-side and clear it locally. `[[runner]]` blocks are left intact — the daemon keeps running under its own identity.

```
karma-team auth logout [--local-only]
```

| Flag           | Purpose                                                                            |
| -------------- | ---------------------------------------------------------------------------------- |
| `--local-only` | Skip server-side revoke, just clear local token. Useful when cloud is unreachable. |

## Local config

### `karma-team config set default-project <PROJECT>`

Set the local default project for non-interactive CLI calls (e.g. `karma-team issue list` without `--project`).

```
karma-team config set default-project <PROJECT_IDENTIFIER_OR_UUID>
```

---

## Runner management

### `karma-team runner add`

Register a new runner. Uses the CLI token from `karma-team auth login`.

```
karma-team runner add --project <PROJECT> [--name <NAME>] [--workspace <SLUG>]
                  [--pod <POD>] [--working-dir <PATH>] [--agent codex|claude-code]
```

| Flag                | Purpose                                                            |
| ------------------- | ------------------------------------------------------------------ |
| `--project <P>`     | Project identifier (slug or UUID). **Required.**                   |
| `--name <N>`        | Human-friendly runner name. Auto-generated if omitted.             |
| `--workspace <S>`   | Workspace slug. Required if you belong to multiple workspaces.     |
| `--pod <P>`         | Pod within the project. Defaults to project's default pod.         |
| `--working-dir <P>` | Local working dir for clones. Defaults to a path under `data_dir`. |
| `--agent <K>`       | `codex` (default) or `claude-code`.                                |

On the first runner: installs the OS service (systemd user unit / launchd agent / Windows scheduled task) and starts the daemon.

### `karma-team runner list`

List runners configured on this machine.

```
karma-team runner list
```

### `karma-team runner remove`

Deregister a runner (cloud + local).

```
karma-team runner remove <NAME> [--local-only] [-y|--yes]
```

| Flag           | Purpose                                                                                |
| -------------- | -------------------------------------------------------------------------------------- |
| `<NAME>`       | Runner name from `karma-team runner list`. Must match a `[[runner]]` in `config.toml`. |
| `--local-only` | Skip cloud-side delete; just clean local config + data dir.                            |
| `-y`, `--yes`  | Skip the y/N confirm. Required for scripted callers.                                   |

---

## Service lifecycle

### `karma-team install`

Write or refresh the OS service unit. Safe to re-run after binary upgrades.

```
karma-team install [--skip-linger]
```

| Flag            | Purpose                                                                                                       |
| --------------- | ------------------------------------------------------------------------------------------------------------- |
| `--skip-linger` | Linux only — skip `sudo loginctl enable-linger`. Without linger the daemon only starts at login, not at boot. |

### `karma-team uninstall`

Remove the OS service unit. Local config + runner credentials are kept.

```
karma-team uninstall
```

### `karma-team start` / `karma-team stop` / `karma-team restart`

Control the installed service.

```
karma-team start
karma-team stop
karma-team restart
```

### `karma-team status`

Combined service-level + daemon-runtime status.

```
karma-team status [--json]
```

| Flag     | Purpose                                |
| -------- | -------------------------------------- |
| `--json` | Print JSON instead of a human summary. |

If the daemon isn't running, prints just the service-level state and a note (instead of an IPC error).

### `karma-team tui`

Attach the interactive Ratatui UI to the running daemon.

```
karma-team tui [--tab <TAB>]
```

| Flag          | Purpose                                                                                                      |
| ------------- | ------------------------------------------------------------------------------------------------------------ |
| `--tab <TAB>` | Open directly to a tab: `runner` (default), `config`, `runs`, or `approvals`. Accepts 1-based index `1`–`4`. |

Press `q` to quit; the daemon keeps running in the background.

### `karma-team doctor`

Preflight checks: agent installed + on `PATH`, agent authed, git configured, cloud reachable.

```
karma-team doctor [--json] [--runner <NAME>]
```

| Flag              | Purpose                                                                  |
| ----------------- | ------------------------------------------------------------------------ |
| `--json`          | Machine-readable report.                                                 |
| `--runner <NAME>` | Restrict per-runner checks to one runner. Daemon-wide checks always run. |

### `karma-team update`

Swap the on-disk `karma-team` binary for the latest GitHub release. Only works for cargo-dist installs (those leave an install receipt).

```
karma-team update [--check] [--restart]
```

| Flag        | Purpose                                                                    |
| ----------- | -------------------------------------------------------------------------- |
| `--check`   | Report whether an update is available, don't swap.                         |
| `--restart` | After swap, restart the daemon so the new binary takes effect immediately. |

Without `--restart`, the swap only takes effect on the next natural restart (`karma-team restart`, reboot, service respawn). The running process is never disturbed.

For a one-command manual upgrade, run `karma-team update --restart` when the runner is idle.

### `karma-team remove --all`

Full teardown: stop service → uninstall service unit → deregister with cloud → delete local `config.toml` + credentials.

```
karma-team remove --all [--local-only]
```

| Flag           | Purpose                                                                             |
| -------------- | ----------------------------------------------------------------------------------- |
| `--all`        | **Required.** Explicit confirmation that you want to wipe ALL runners on this host. |
| `--local-only` | Delete local state without contacting the cloud.                                    |

> To drop a single runner instead, use `karma-team runner remove <name>`.

---

## Workspace / project / context

### `karma-team workspace me`

Verify CLI credentials end-to-end (token + cloud reachability + TLS + active user). The probe `karma-team doctor` uses.

```
karma-team workspace me
```

### `karma-team project list`

List projects in the active workspace. Prints JSON.

```
karma-team project list
```

### `karma-team context init`

Write `.karma-team/context.md` for a local workspace directory.

```
karma-team context init --project <PROJECT> [--workspace <PATH>]
```

| Flag              | Purpose                                             |
| ----------------- | --------------------------------------------------- |
| `--project <P>`   | Project identifier or UUID. **Required.**           |
| `--workspace <P>` | Local workspace dir. Defaults to current directory. |

---

## Work items

### `karma-team issue get <IDENTIFIER>`

Fetch one work item. Prints full JSON. The payload includes an absolute `url`
pointing at the issue in the web UI, built from the deployment's `WEB_URL` (or
`APP_BASE_URL`); the field is omitted when neither is configured.

```
karma-team issue get ENG-42
```

### `karma-team issue create`

Create a work item under a project.

```
karma-team issue create --title <TITLE> [--project <P>] [--description <D>]
                    [--priority <P>] [--state <S>]
```

| Flag                | Purpose                                                                                                            |
| ------------------- | ------------------------------------------------------------------------------------------------------------------ |
| `--title <T>`       | **Required.**                                                                                                      |
| `--project <P>`     | Project identifier or UUID. If omitted: `KARMA_TEAM_PROJECT_ID` env → local `default_project` → workspace default. |
| `--description <D>` | Plain text or markdown.                                                                                            |
| `--priority <P>`    | `none` \| `low` \| `medium` \| `high` \| `urgent`.                                                                 |
| `--state <S>`       | Initial state — name (case-insensitive) or UUID.                                                                   |

### `karma-team issue list --project <P>`

List work items in a project. Returns the paginated envelope `{count, next_cursor, prev_cursor, results}`.

```
karma-team issue list --project <P> [--cursor <C>] [--per-page <N>] [--order-by <F>]
```

| Flag             | Purpose                                                              |
| ---------------- | -------------------------------------------------------------------- |
| `--project <P>`  | **Required.**                                                        |
| `--cursor <C>`   | Pagination cursor from a prior page's `next_cursor`.                 |
| `--per-page <N>` | Items per page. Server default if omitted.                           |
| `--order-by <F>` | Sort field, e.g. `-created_at` (default), `priority`, `state__name`. |

### `karma-team issue patch <IDENTIFIER>`

Update fields. Pass only the fields you want to change.

```
karma-team issue patch ENG-42 [--state <S>] [--title <T>] [--description <D>] [--priority <P>]
```

| Flag                | Purpose                                            |
| ------------------- | -------------------------------------------------- |
| `--state <S>`       | State name (case-insensitive) or UUID.             |
| `--title <T>`       | New title.                                         |
| `--description <D>` | New description.                                   |
| `--priority <P>`    | `none` \| `low` \| `medium` \| `high` \| `urgent`. |

### `karma-team issue move <IDENTIFIER> --project <P>`

Move a work item into another project in the same workspace.

```
karma-team issue move ENG-42 --project OPS
```

---

## Comments

### `karma-team comment list <IDENTIFIER>`

List comments on a work item.

```
karma-team comment list ENG-42
```

### `karma-team comment add <IDENTIFIER>`

Post a new comment.

```
karma-team comment add ENG-42 (--body <BODY> | --body-file <PATH>) [--as-agent <NAME>] [--agent-run-id <UUID>]
```

| Flag                    | Purpose                                                                    |
| ----------------------- | -------------------------------------------------------------------------- |
| `--body <B>`            | Comment body (plain text or markdown).                                     |
| `--body-file <PATH>`    | Read body from file.                                                       |
| `--as-agent <NAME>`     | Mark the comment as spoken by an AI agent with this display name.          |
| `--agent-run-id <UUID>` | Record the karma-team agent run UUID that produced the agent-spoken reply. |

### `karma-team comment update <IDENTIFIER> <COMMENT_UUID>`

Edit an existing comment owned by you.

```
karma-team comment update ENG-42 <COMMENT_UUID> (--body <BODY> | --body-file <PATH>)
```

---

## AI assistant

### `karma-team ai <COMMAND>`

Send a command to the AI assistant in your connected karma-team cloud and print its
reply. This is the CLI door to the same built-in assistant available on the web
landing page — it operates karma-team **as you** (querying and creating projects,
issues, and comments, and dispatching coding runs), scoped to your permissions.

Everything after `ai` is joined into one message, so quoting is optional:

```
karma-team ai "create an issue in ENG for the flaky login test and set priority high"
karma-team ai what changed on ENG-42 this week
```

Because everything after `ai` is treated as the message, put `--json` and
`--timeout` **before** the command (e.g. `karma-team ai --json what changed on ENG-42`);
otherwise they are swallowed into the message text.

The command opens a fresh assistant thread, sends your message, waits for the
turn to finish (streaming tool activity to stderr), and prints the assistant's
answer on stdout.

| Flag               | Purpose                                                                                                                          |
| ------------------ | -------------------------------------------------------------------------------------------------------------------------------- |
| `--json`           | Emit the raw turn result (`{thread_id, turn_id, messages}`) as one JSON document instead of the human transcript.                |
| `--timeout <SECS>` | Give up waiting after this many seconds (default `180`). The turn may still finish in the cloud; only the local wait is bounded. |

**BYOK (bring your own key).** The assistant is BYOK-only: you must configure an
LLM provider and API key under **Settings → AI assistant** in karma-team. If it is
not set up, `karma-team ai` prints a clear reason and exits non-zero rather than
hanging:

```
{"error":"the karma-team AI assistant is not set up yet. It is BYOK (bring your own key): add your LLM provider and API key under Settings → AI assistant in karma-team, then run `karma-team ai` again"}
```

Exit code is `0` when the assistant replies, and non-zero when the assistant
reports an error, the wait times out, or BYOK is unconfigured — so scripts can
branch on it.

---

## States

### `karma-team state list [PROJECT_OR_ISSUE]`

List workflow states for a project. Without an argument, uses the current issue context from `KARMA_TEAM_ISSUE_IDENTIFIER`.

```
karma-team state list             # uses current issue context
karma-team state list ENG         # project slug
karma-team state list ENG-42      # via work-item identifier
karma-team state list <UUID>      # project UUID
```

---

## Hidden / internal

### `karma-team __run`

Internal daemon entry point. Invoked by systemd / launchd / Windows scheduled task via the generated unit file. **Not a user-facing command.**

---

## Exit codes

`issue`, `comment`, `state`, `workspace`, `project`, `context` print JSON on stdout and JSON on stderr for errors. Their exit codes follow `api_client::EXIT_*` constants — non-zero on any error.

`ai` prints its reply as human-readable text on stdout (or JSON with `--json`), streams tool activity to stderr, and prints a `{"error": ...}` line to stderr on failure. It exits `0` when the assistant replies and non-zero otherwise (BYOK unconfigured, assistant error, or local timeout).

Other commands return `0` on success and surface anyhow errors on failure (non-zero exit).

## Common environment variables

| Variable                      | Used by        | Purpose                                  |
| ----------------------------- | -------------- | ---------------------------------------- |
| `KARMA_TEAM_CONFIG_DIR`       | All            | Override config dir                      |
| `KARMA_TEAM_DATA_DIR`         | All            | Override data dir                        |
| `KARMA_TEAM_LOG`              | All            | Log level                                |
| `KARMA_TEAM_PROJECT_ID`       | `issue create` | Default project when `--project` omitted |
| `KARMA_TEAM_ISSUE_IDENTIFIER` | `state list`   | Default issue context                    |

## Common file locations

| What                   | Linux / macOS                               | Windows                                     |
| ---------------------- | ------------------------------------------- | ------------------------------------------- |
| Config + CLI token     | `~/.config/karma-team/config.toml`          | `%APPDATA%\karma-team\config.toml`          |
| Per-runner credentials | `<data_dir>/runners/<rid>/credentials.toml` | `<data_dir>\runners\<rid>\credentials.toml` |
| Run transcripts        | `<data_dir>/runners/<rid>/history/`         | `<data_dir>\runners\<rid>\history\`         |
| IPC socket / pipe      | `$XDG_RUNTIME_DIR/karma-team/...`           | local named pipe                            |

All secrets and the IPC socket are `0600` on Unix.
