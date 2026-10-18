# 02 — karma-team Cloud Quickstart

## 1. Sign up

1. Open <https://karma-team.example.com>.
2. Click **Sign up**, complete sign-up, verify your email.

## 2. Create workspace + project

1. Create your workspace when prompted.
2. **Projects → + Add project**. Fill in:
   - **Name**, **Identifier**
   - **Repository URL** — git clone URL (e.g. `git@github.com:acme/web.git`)
   - **Default branch** — usually `main`
3. Open **Project → Settings → General** and copy the **project ID**.

## 3. Install an AI agent

Pick one:

- **Claude Code** — install per <https://docs.anthropic.com/en/docs/claude-code>, verify `claude --version`.
- **Codex** — install per <https://github.com/openai/codex>, verify `codex --version`.

## 4. Install `karma-team`

**macOS / Linux:**

```bash
curl --proto '=https' --tlsv1.2 -LsSf \
  https://github.com/xerecter/karma-team/releases/latest/download/install.sh | sh
```

**Windows (PowerShell):**

```powershell
irm https://github.com/xerecter/karma-team/releases/latest/download/install.ps1 | iex
```

When prompted, enter `https://karma-team.example.com` and approve the device-code in the browser.

## 5. Bind this machine and verify automatic runners

```bash
karma-team auth login --url https://karma-team.example.com
karma-team doctor
karma-team status
```

Login starts the daemon even when the local config has no runner yet. The daemon
immediately reads the bound workspace's cloud projects, idempotently creates one
runner per otherwise-unbound project, and keeps them synchronized every minute.
You do not need to bind projects manually. `karma-team runner add` remains
available for an intentional extra runner or non-default pod. `karma-team
doctor` must be clean before continuing.

For private repositories, also configure Git credentials for the operating-system
account running the daemon and verify it can run `git ls-remote` without a prompt.
Platform login and connected Git provider accounts do not supply runner clone
credentials. See [Private repository authentication](../../runner/README.md#private-repository-authentication)
for the setup and verification steps.

## 6. Run your first work item

1. In karma-team, **Work items → + Create work item**.
2. Write a specific title + description + acceptance criteria.
3. Click **Run agent**.
4. Watch in the work item's **Run** panel or `karma-team tui`.
5. Approve prompts as they appear (use **Approve always** for safe commands).

## 7. Review

1. Open the work item → review **diff**, **transcript**, **summary**.
2. To commit: `cd` into the runner working dir (`karma-team status` shows the path), then `git` as normal.

---

## Common commands

```bash
karma-team status              # daemon + runner state
karma-team tui                 # interactive dashboard
karma-team doctor              # preflight checks
karma-team auth status         # who am I logged in as
karma-team runner list         # runners on this host
karma-team runner add --project <ID> # optional extra/manual runner
karma-team runner remove <name>
karma-team restart / stop
karma-team update --restart    # self-update and restart when idle
karma-team --help
```

Full command + flag reference: [17 — `karma-team` CLI reference](./17-cli-reference.md).

## If something breaks

- **Run stuck pending** → `karma-team status`, then `karma-team restart`.
- **Run fails immediately** → `karma-team doctor` for agent and Git installation checks; for clone authentication errors, follow [Private repository authentication](../../runner/README.md#private-repository-authentication).
- **Command not found** → open a new terminal.
- **Browser auth not approving** → use the same browser you signed up in.
- **Headless host** → run `karma-team auth login --no-browser --url https://karma-team.example.com` and approve the printed URL from another browser; project runners are then synchronized automatically.

Discussions: <https://github.com/xerecter/karma-team/discussions> · Bugs: <https://github.com/xerecter/karma-team/issues>
