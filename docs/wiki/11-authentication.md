# 11 — Authentication

Three distinct identity flows live in karma-team. They all terminate at the Django `authentication/` module, but they have very different user experiences and security models.

| Flow                                               | Who uses it                                            | Where it lives                                               |
| -------------------------------------------------- | ------------------------------------------------------ | ------------------------------------------------------------ |
| **Instance admin** ("god mode")                    | The person who first stood up the self-hosted instance | `apps/admin` UI + `karma_team.authentication`                |
| **User login** (password, magic link, OIDC)        | Day-to-day product users                               | `apps/web` + `apps/space` + `karma_team.authentication`      |
| **Runner enrollment** (device-code + legacy token) | The Rust `karma-team` daemon                           | `runner/src/cli/auth` + `karma_team.runner.views.enrollment` |

## 1. Instance admin (one-time bootstrap)

The very first thing you do on a fresh self-hosted instance:

```
http://<your-instance>/god-mode/   ← register the instance admin
```

This is served by `apps/admin/` (port `3001` in dev). Once registered, the admin's credentials are also valid user credentials on the main app — you log into `apps/web` with the same identity.

There is no "create admin" CLI command — it's deliberately a one-shot web flow tied to "the first request to god-mode wins".

## 2. User login (web + space)

Lives in `karma_team/authentication/` (mounted at `/auth/`):

```
authentication/
├── views/         ← sign-in, sign-up, password reset, OIDC callbacks
├── serializers/
├── permissions/
├── middleware/
└── urls/
```

Mechanisms supported (configurable per instance):

- **Email + password** (always available)
- **Magic-link / email OTP**
- **OIDC / SSO** — when configured. Cloud karma-team uses the **home-page OIDC** as the plan source-of-truth: the JWT `plan` claim is cached on `Account` and gates Cloud-specific features. There are **no plan webhooks** — plan updates flow through subsequent OIDC logins. Upgrade UI in the app deep-links back to the home-page user center.

The `space` app (public/guest views) reuses the same auth module but can serve content without a session.

## 3. Runner enrollment

The runner (`karma-team` CLI) needs to authenticate to the cloud without a human at the keyboard for every request. There are two paths:

### A. Device-code flow (recommended)

```bash
karma-team auth login --url https://your-karma-team-instance.com
```

1. CLI requests a device code from the cloud (`/api/v1/runner/enroll/...`).
2. CLI shows the user a short code + URL to open in a browser.
3. User approves in the browser (signed in via flow #2 above).
4. CLI polls until the code is approved → cloud mints a **CLI token**.
5. CLI persists token at `~/.config/karma-team/config.toml` (`0600`).
6. CLI prompts to add a runner inline if none exists on this host.

This is the same UX as `gh auth login` or `stripe login`. Bare `karma-team` (no subcommand) drops into this flow when no config exists — useful after MSI install on Windows.

### B. Headless login

For headless hosts where opening a browser automatically is awkward:

```bash
karma-team auth login --no-browser --url https://karma-team.example.com
karma-team runner add --project <PROJECT_ID>
```

The CLI prints the verification URL and user code; approve it from any browser signed in to karma-team.

## Runner credential model

After login, the dev machine holds one shared **MachineToken**. The CLI and every runner on that local machine use the same token; the runner identity is carried separately by URL `runner_id` or `X-Runner-Id`.

| Token                        | Lifetime   | Use                                                                                                                 |
| ---------------------------- | ---------- | ------------------------------------------------------------------------------------------------------------------- |
| **MachineToken** (`mt_...`)  | Long-lived | Identifies the authenticated dev machine + user + workspace. Stored once in `[cli].token` at login.                 |
| **Runner id**                | Stable     | Identifies the specific runner under that dev machine. Sent in runner URLs or `X-Runner-Id`, not secret.            |
| **Enrollment/refresh token** | Legacy     | Kept only for already-minted `karma-team connect` compatibility paths. New `karma-team runner add` does not use it. |

Cloud-side, MachineToken lifecycle is stored in `runner.MachineToken`; legacy refresh/access helpers remain in `karma_team/runner/services/tokens.py` for compatibility.

`karma-team auth logout` revokes the dev-machine token cloud-side. `karma-team runner remove` removes one runner row/local instance without rotating the shared machine token.

## On-disk security

- Config + credential files: `0600` on Unix.
- IPC socket between daemon and TUI: `0600`.
- On Windows: files under the user profile, IPC over a local named pipe with appropriate ACLs.

## Where to read next

- [08 — Cloud ↔ runner protocol](./08-cloud-runner-protocol.md) — how the machine token and runner id are presented per request
- [07 — Runner architecture](./07-runner-architecture.md) — `runner/src/cli/auth/` and `runner/src/config/`
- [06 — Backend architecture](./06-backend-architecture.md) — `karma_team/authentication/` module structure
