# Auto-sync daemon project runners Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: 用 `tdd` 和 `code-review` skills 逐项实施。每个 task 必须先 RED，再 GREEN，再 REFACTOR。

**Goal:** 账号绑定后由 daemon 主动、幂等地对账当前 workspace 的云端 project，并为新增 project 创建独立 runner；删除 project 后释放对应 runner runtime/config/data 资源，同时保持现有任务执行链路不变。

**Architecture:** Django 提供 machine-token scoped 的完整 project/binding 快照和事务幂等的 ensure-runner 接口；Rust daemon 新增独立 `ProjectSync` loop，以 project UUID 做 desired-state reconciliation。配置写入沿用 host-wide lock，runner 激活/删除沿用 `RunnerSpawnCtx` 与现有 `RemoveRunner` 路径；失败快照 fail-closed，连续两次完整快照确认 project 删除。

**Tech Stack:** Rust/Tokio/reqwest/serde/TOML、Django REST Framework/PostgreSQL、pytest-django、Cargo tests。

---

### Task 1: Cloud automatic binding model and machine-scoped APIs

**Files:**
- Modify: `apps/api/karma_team/runner/models.py`
- Create: `apps/api/karma_team/runner/migrations/0027_auto_project_binding.py`
- Create: `apps/api/karma_team/runner/migrations/0029_project_deletion_tombstone.py`
- Modify: `apps/api/karma_team/runner/views/enrollment.py`
- Modify: `apps/api/karma_team/runner/views/projects.py`
- Modify: `apps/api/karma_team/runner/urls.py`
- Modify: `apps/api/karma_team/runner/serializers.py`
- Test: `apps/api/karma_team/tests/unit/runner/test_auto_project_binding.py`

- [ ] **Step 1: 写失败测试**

  Add pytest-django tests that use the real test database and assert:

  - machine-token snapshot returns only the token workspace's active projects and
    only active runners on that token's dev machine;
  - a soft-deleted project is absent and an unrelated workspace is absent;
  - deletion ledger writes are idempotent and survive Project hard deletion;
  - ten sequential/repeated ensure calls return one runner ID;
  - concurrent ensure attempts result in one active automatic binding;
  - an existing manual runner for the same machine/project is returned instead
    of creating an automatic duplicate;
  - invalid/revoked/machine-less credentials are rejected.

- [ ] **Step 2: 运行测试，确认失败**

  Run: `cd apps/api && pytest karma_team/tests/unit/runner/test_auto_project_binding.py -q`

  Expected: FAIL because the machine snapshot route, binding source, and ensure
  endpoint do not exist yet.

- [ ] **Step 3: 写最小实现**

  Add a `RunnerBindingSource` choice with `manual` and `auto_project_sync`, a
  conditional uniqueness constraint for active automatic `(dev_machine, pod)`,
  and a migration defaulting existing rows to `manual`. Add a machine-token-only
  complete snapshot route and an ensure route. The ensure transaction must lock
  the `DevMachine`, resolve the token workspace/project/default pod, reuse any
  active same-machine/project runner, and catch/re-read uniqueness races before
  returning an enrollment-compatible response. Keep the existing manual runner
  creation route behavior unchanged.

- [ ] **Step 4: 运行测试，确认通过**

  Run: `cd apps/api && pytest karma_team/tests/unit/runner/test_auto_project_binding.py -q`

  Expected: PASS, including the concurrent uniqueness test.

- [ ] **Step 5: REFACTOR and verify API contracts**

  Run: `cd apps/api && ruff check karma_team/runner/models.py karma_team/runner/views/enrollment.py karma_team/runner/views/projects.py karma_team/tests/unit/runner/test_auto_project_binding.py`

  Expected: no new lint errors; response fields use UUID strings and never expose
  machine tokens.

- [ ] **Step 6: Commit**

  ```bash
  git add apps/api/karma_team/runner/models.py apps/api/karma_team/runner/migrations/0027_auto_project_binding.py apps/api/karma_team/runner/views/enrollment.py apps/api/karma_team/runner/views/projects.py apps/api/karma_team/runner/urls.py apps/api/karma_team/runner/serializers.py apps/api/karma_team/tests/unit/runner/test_auto_project_binding.py
  git commit -m "feat(api): add idempotent automatic project runner binding"
  ```

### Task 2: Cloud project deletion cleanup

**Files:**
- Modify: `apps/api/karma_team/api/views/project.py`
- Modify: `apps/api/karma_team/runner/services/runner_delete.py`
- Test: `apps/api/karma_team/tests/unit/runner/test_auto_project_binding.py`
- Test: `apps/api/karma_team/tests/contract/app/test_project_app.py`

- [ ] **Step 1: 写失败测试**

  Add tests that create two projects with active runners, delete one project,
  and assert only its runners are revoked/deleted and `remove_runner` is queued
  before session closure. Repeat deletion and assert it is harmless. Verify the
  unrelated project's runner and active run remain untouched.

- [ ] **Step 2: 运行测试，确认失败**

  Run: `cd apps/api && pytest karma_team/tests/unit/runner/test_auto_project_binding.py karma_team/tests/contract/app/test_project_app.py -q`

  Expected: FAIL because project deletion currently does not tear down related
  runner rows/control frames.

- [ ] **Step 3: 写最小实现**

  Before soft-deleting a project, gather its active runners and invoke the
  existing idempotent runner deletion service with local purge enabled. Make
  the service tolerate a runner already revoked/deleted and preserve its
  enqueue-before-revoke ordering. Keep project soft deletion and all existing
  permission/default-project behavior intact.

- [ ] **Step 4: 运行测试，确认通过**

  Run: `cd apps/api && pytest karma_team/tests/unit/runner/test_auto_project_binding.py karma_team/tests/contract/app/test_project_app.py -q`

  Expected: PASS.

- [ ] **Step 5: REFACTOR and verify**

  Run: `cd apps/api && ruff check karma_team/api/views/project.py karma_team/runner/services/runner_delete.py karma_team/tests/contract/app/test_project_app.py`

- [ ] **Step 6: Commit**

  ```bash
  git add apps/api/karma_team/api/views/project.py apps/api/karma_team/runner/services/runner_delete.py apps/api/karma_team/tests/unit/runner/test_auto_project_binding.py apps/api/karma_team/tests/contract/app/test_project_app.py
  git commit -m "feat(api): tear down project runners on deletion"
  ```

### Task 3: Rust project-sync reconciliation core and cloud client

**Files:**
- Modify: `runner/src/config/schema.rs`
- Modify: `runner/src/cloud/http.rs`
- Create: `runner/src/daemon/project_sync.rs`
- Modify: `runner/src/daemon/mod.rs`
- Modify: `runner/src/lib.rs`
- Modify: `runner/src/cli/runner_ops.rs`
- Test: `runner/src/daemon/project_sync.rs`
- Test: `runner/src/config/schema.rs`
- Test: `runner/src/cli/runner_ops.rs`

- [ ] **Step 1: 写失败测试**

  Add Rust tests for pure reconciliation planning and config application:

  - a complete snapshot plans create/recover/activate/no-op actions by project
    UUID;
  - a failed, incomplete, or wrong-workspace snapshot never advances missing
    counters;
  - two different successful project-tombstone snapshots plan removal, one or
    a replayed snapshot does not;
  - project/binding absence and legacy rows without a project UUID fail closed;
  - repeated enrollment application with the same runner ID does not duplicate
    `[[runner]]` blocks;
  - project UUID survives slug rename and stale unrelated runners are preserved;
  - capacity over 50 skips new projects without stopping existing runners.

- [ ] **Step 2: 运行测试，确认失败**

  Run: `cd runner && cargo test project_sync --locked`

  Expected: FAIL because snapshot types, project IDs, and reconciliation module
  do not exist.

- [ ] **Step 3: 写最小实现**

  Extend `RunnerConfig` with serde-defaulted `project_id` and binding source;
  extend `EnrollResponse` and HTTP helpers with project/pod UUIDs; implement a
  machine-token snapshot client and idempotent ensure/delete request helpers.
  Add `project_sync.rs` with:

  - a bounded timer/backoff loop;
  - a per-daemon async mutex so timer and immediate triggers cannot overlap;
  - a pure plan function keyed by project UUID;
  - fail-closed complete-snapshot validation;
  - two-successful-missing-pass tracking;
  - deterministic capacity handling;
  - per-project error isolation.

  Add an idempotent config apply helper that uses `mutate_config` and returns
  the existing runner block when runner ID or project ID is already present.

- [ ] **Step 4: 运行测试，确认通过**

  Run: `cd runner && cargo test project_sync --locked`

  Expected: PASS.

- [ ] **Step 5: REFACTOR and verify Rust package**

  Run: `cd runner && cargo fmt --check && cargo clippy --all-targets --locked -- -D warnings`

  Expected: formatting and clippy pass without changing existing user files.

- [ ] **Step 6: Commit**

  ```bash
  git add runner/src/config/schema.rs runner/src/cloud/http.rs runner/src/daemon/project_sync.rs runner/src/daemon/mod.rs runner/src/lib.rs runner/src/cli/runner_ops.rs
  git commit -m "feat(runner): add project snapshot reconciliation core"
  ```

### Task 4: Daemon lifecycle integration and end-to-end runner teardown

**Files:**
- Modify: `runner/src/daemon/supervisor.rs`
- Modify: `runner/src/daemon/machine_control.rs`
- Modify: `runner/src/daemon/state.rs`
- Modify: `runner/src/cli/auth/login.rs`
- Modify: `runner/src/cli/install.rs`
- Modify: `runner/src/cli/run.rs`
- Modify: `runner/src/ipc/server.rs`
- Modify: `runner/README.md`
- Modify: `README.zh-CN.md`
- Test: `runner/src/daemon/supervisor.rs`
- Test: `runner/src/cli/auth/login.rs`
- Test: `runner/tests/project_sync_lifecycle.rs`

- [ ] **Step 1: 写失败测试**

  Add tests that assert:

  - machine-token + dev-machine config starts with zero runners;
  - login's persisted binding starts the existing service exactly once;
  - one sync task is created per daemon and an immediate/timer overlap is
    serialized;
  - a missing project invokes local RemoveRunner teardown without touching an
    unrelated runner or its task loop;
  - a project created while the daemon is running is hot-added and uses an
    independent working directory;
  - repeated deletion/teardown is idempotent and the working directory remains;
  - existing assignment/heartbeat tests remain green.

- [ ] **Step 2: 运行测试，确认失败**

  Run: `cd runner && cargo test --test project_sync_lifecycle --locked`

  Expected: FAIL because supervisor does not start a project-sync task and
  login does not start the zero-runner service path.

- [ ] **Step 3: 写最小实现**

  Start `ProjectSync` beside machine control in `Supervisor::run`, passing a
  cloneable `RunnerSpawnCtx` and daemon state. Add a context method that
  synthesizes the existing `RemoveRunner` envelope and treats missing runner
  state/config as success. Ensure sync task aborts during shutdown but does not
  abort existing runner loops first. Update login/install lifecycle to write the
  service unit and enable/start it for a valid zero-runner machine binding;
  leave manual `runner add` and all runner polling paths intact. Update status
  diagnostics and docs to describe automatic project binding.

- [ ] **Step 4: 运行测试，确认通过**

  Run: `cd runner && cargo test --all-targets --locked`

  Expected: PASS, including existing protocol, task-polling, Karma, and runner
  lifecycle tests.

- [ ] **Step 5: REFACTOR and full verification**

  Run: `pnpm check:types && cd apps/api && pytest -n auto -m 'not slow' && cd ../../runner && cargo clippy --all-targets --locked -- -D warnings`

  Expected: all checks pass; any pre-existing unrelated failures are recorded
  with their exact command/output and do not get hidden.

- [ ] **Step 6: Commit**

  ```bash
  git add runner/src/daemon/supervisor.rs runner/src/daemon/machine_control.rs runner/src/daemon/state.rs runner/src/cli/auth/login.rs runner/src/cli/install.rs runner/src/cli/run.rs runner/src/ipc/server.rs runner/README.md README.zh-CN.md runner/tests/project_sync_lifecycle.rs
  git commit -m "feat: automatically reconcile daemon project runners"
  ```

## Follow-ups

- Add optional machine-control push notifications as a latency optimization;
  keep polling as the recovery mechanism.
- Add UI affordances for filtering auto-synced versus manually managed runners.
