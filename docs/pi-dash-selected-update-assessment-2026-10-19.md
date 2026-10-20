# Karma Team 对 Pi Dash 指定更新项的适配评估

## 1. 评估范围

本文评估 Karma Team 是否应同步 Pi Dash 在 2026-09-19 至 2026-10-19 期间的指定功能与修复。上游事实来源为：

- `docs/monthly-development-summary-2026-09-19-to-2026-10-19.md`
- Pi Dash 对比区间 `ce3cf2ad...d656243d`
- 当前 Pi Dash `main`：`d656243d`

评估严格限制在当前保留的 22 项。未列出的上游功能不在本文范围内，也不建议因移植某个修复而顺带引入其前置产品功能。

本次只进行了代码、迁移、测试和上游提交差异的静态审阅，没有修改业务代码、运行数据库迁移、构建镜像或重启服务。文中的“可直接修复”表示改动范围相对集中，仍然要求补测试并经过正常发布验证，不表示可以无测试复制文件。

## 2. 结论摘要

22 项的评估结果如下：

- **可直接新增或修复：7 项**：3、4、9、12、16、21、22。
- **需要结合 Karma Team 定制进行适配：9 项**：1、2、6、7、10、14、15、17、18。
- **当前已具备或已规避，无需必要更新：4 项**：5、8、13、19。
- **当前没有对应执行路径，不适用：2 项**：11、20。

| 编号 | 更新项                            | 当前判断               | 建议优先级 | 核心结论                                                 |
| ---- | --------------------------------- | ---------------------- | ---------- | -------------------------------------------------------- |
| 1    | 项目调度器创建、安装和执行历史    | 部分已有，需适配       | P2         | 基础调度能力已存在，缺统一创建入口、详情和运行历史       |
| 2    | 项目级智能体定时运行设置          | 部分已有，需适配       | P1/P2      | 关闭后仍可能派发一次是明确 Bug；设置 UI 和预算语义仍缺失 |
| 3    | CLI 登录简化                      | 可直接新增             | P2         | 只增加 `karma-team login` 别名，不复制 Pi Dash 托管地址  |
| 4    | 发布草稿、项目页添加 Runner       | 可直接新增             | P2         | 两项均为局部前端增强，现有 API 基础已具备                |
| 5    | 项目邀请接口返回 500              | 已修复                 | 无需更新   | 两个上游根因都已修复，并有非邮箱账号扩展和测试           |
| 6    | 异步请求和 SSE 泄漏数据库连接     | 问题存在，需适配       | P0         | 多个异步 ORM、SSE 和长轮询边界仍可能泄漏连接             |
| 7    | 工作项 Peek 无法正确展开          | Web 已修，Desktop 未修 | P1         | 只需适配 Desktop Overlay，主 Web 不应重复修改            |
| 8    | 语音转写接口字段不一致            | 已修复                 | 无需更新   | `file`、`error` 字段和回归测试均已对齐                   |
| 9    | 大页面保存返回 413                | 问题存在，可直接修复   | P1         | Live 服务仍使用 Express 默认 100 KB JSON 限制            |
| 10   | 页面状态变化后仍可能保存          | 存在同类风险，需适配   | P2         | 不引入公开 Page API，只修现有内部页面写入竞态            |
| 11   | Markdown 转换丢失水平分割线       | 当前不适用             | 无需更新   | 当前没有对应 HTML 到 Markdown 转换链路                   |
| 12   | Assistant 错误信息被错误归类      | 问题存在，可直接修复   | P1         | 已分类异常及其 `__cause__` 会降级为 `internal`           |
| 13   | Docker 镜像在新版 pnpm 下构建失败 | 当前已规避             | 可选加固   | 当前明确固定 pnpm `10.32.1`；可统一 PATH，但非必要更新   |
| 14   | 离线 Runner 导致工作项永久卡住    | 问题存在，需适配       | P0         | 现有 heartbeat reaper 无法回收永久离线 Runner 上的运行   |
| 15   | Runner 重启后云端一直显示忙碌     | 问题存在，需适配       | P0         | 关机事件发送失败后没有持久化与重放                       |
| 16   | 会话建立时无限循环并占满 CPU      | 主修复已有，保护缺失   | P1         | 两个 outbox 都需增加 cursor 不前进保护                   |
| 17   | Git 异常直接阻止任务执行          | 问题存在，需适配       | P1         | 应将纯 Git 上下文错误降级，同时保留真实文件系统错误      |
| 18   | Clone 失败后无法恢复              | 问题存在，需与 17 合并 | P1         | 需识别 `.karma-team`、清理本次残留并关闭交互式凭据提示   |
| 19   | Muse Code 输出格式解析错误        | 已完整修复             | 无需更新   | schema、bridge、会话校验和真实 fixture 均已存在          |
| 20   | CLI 标签解析无限翻页              | 当前不适用             | 无需更新   | Karma Team CLI 尚无标签命令和对应分页入口                |
| 21   | 按状态名修改工作项失败            | 问题存在，可直接修复   | P1         | 服务端返回分页 envelope，CLI 仍只接受数组                |
| 22   | 陈旧 workpad 文件覆盖服务端内容   | 问题存在，可直接修复   | P1         | 成功上传后应默认清理普通文件，并提供 `--keep`            |

## 3. 推荐实施批次

### 第一批：明确 Bug 且改动边界较小

建议优先实施 9、12、16、21、22：

- 这些问题均可在当前代码中直接复现或由接口契约明确证明。
- 改动集中，不要求先引入其他产品能力。
- 第 16 项要同时覆盖普通 outbox 和 Karma Team 独有的 machine outbox。
- 第 22 项必须同时更新两套提示词，避免 CLI 行为和智能体指令冲突。

### 第二批：运行稳定性和任务可恢复性

建议实施 6、14、15、17、18：

- 第 6 项解决数据库连接耗尽风险，优先级最高，但涉及多个异步边界，必须逐处适配。
- 第 14、15 项解决两种不同的悬空运行场景：服务端超时回收和 Runner 本地事件持久化，两者不能互相替代。
- 第 17、18 项应作为一个 Runner workspace 专项实施，既允许无 Git 环境继续执行，也确保 clone 失败后能安全重试。

### 第三批：产品能力补齐

建议按产品优先级选择 1、2、3、4、7：

- 第 1 项拆成提示词上下文、创建/安装弹窗、详情及历史三个小批次。
- 第 2 项先修关闭开关竞态，再单独决定是否改变 `Re-tick` 的产品语义和删除旧字段。
- 第 3 项只增加命令别名。
- 第 4 项是两个互不依赖的前端增强，可以分别交付。
- 第 7 项仅处理 Desktop Overlay；主 Web 已经具备修复。

### 第四批：页面并发安全专项

建议最后处理第 10 项：

- 第 10 项需要围绕 Karma Team 现有内部页面写入接口设计事务和字段级保存，不能直接套用上游公开 API 补丁。

无需实施 5、8、11、13、19、20。第 13 项可在以后统一 Dockerfile 时做防御性 PATH 加固，但当前不是已确认故障。

## 4. 逐项评估

### 1. 项目调度器创建、安装和执行历史

**上游变化**

Pi Dash 通过 `3db73752`、`474123e5`、`1d7be107` 增加统一的 New Scheduler 弹窗、项目内创建并安装、绑定详情、关联 AgentRun 历史，以及 Cloud Agent 的项目和调度器身份上下文。

**Karma Team 现状**

- 工作区 Scheduler 定义 CRUD 已存在。
- 项目级安装、编辑、启停、卸载和日历已存在。
- `AgentRun.scheduler_binding` 已存在：`apps/api/karma_team/runner/models.py:990`。
- 项目页面仍使用旧 `InstallSchedulerBindingModal`：`apps/web/core/components/project/scheduler-bindings/bindings-panel.tsx:71`。
- 当前没有 `new-scheduler-modal.tsx`、`binding-detail.tsx` 和绑定详情路由页面。
- 绑定详情 API 没有 `resolved_prompt`、`run_count` 等详情字段：`apps/api/karma_team/app/views/scheduler/views.py:227`。
- Scheduler URL 没有绑定运行历史 `/runs/`：`apps/api/karma_team/app/urls/scheduler.py:28`。
- `AgentRunSerializer` 没有输出 scheduler binding 详情：`apps/api/karma_team/runner/serializers.py:261`。
- Cloud scheduler recipe 未注入项目/调度器身份：`apps/api/karma_team/prompting/recipes.py:113`。

**建议**

可以更新，但必须手工适配。现有模型已经能够关联运行历史，预计不需要为主路径新增数据库字段。建议依次交付：

1. 先增加 Cloud scheduler prompt 上下文。
2. 再增加 New Scheduler 创建/安装弹窗。
3. 最后增加详情页、详情 serializer 和运行历史 API/UI。

**兼容边界与验证**

- 保留 Karma Team 的 Pod、outcome mode、RRULE、私有 Runner 和多人项目成员逻辑。
- 运行历史必须继续实施私有 Runner 可见性限制。
- 创建工作区 Scheduler 和安装到项目是不同权限，也可能发生“创建成功、安装失败”，界面要提供可恢复状态。
- 验证工作区管理员、项目管理员、普通成员三类权限，以及无历史、多页历史、关联工作项已删除或不可见等情况。

### 2. 项目级智能体定时运行设置

**上游变化**

相关提交为 `0737a645`、`b1e073ed`、`4ed6e16a`、`7c355595`、`61a0d181`，覆盖三阶段默认 cadence、项目设置 UI、预算校验、`Re-tick` 语义和关闭开关后的任务领取行为。

**Karma Team 现状**

- 三阶段默认间隔已经通过 `0164_unify_ticking_cadence_3h.py` 统一为 3 小时，无需重复。
- `Project` 已有开关、默认预算和三阶段 cadence，但仍保留 `agent_retick_grant=3`：`apps/api/karma_team/db/models/project.py:142`。
- `Re-tick` 仍读取固定 grant：`apps/api/karma_team/orchestration/scheduling.py:547`。
- 后台任务在增加已用次数并推进 `next_run_at` 后才检查项目开关，且仍可继续派发本次运行：`apps/api/karma_team/bgtasks/agent_ticker.py:220`。
- 项目设置表单没有 AI agent ticking 控件：`apps/web/core/components/project/form.tsx:46`。
- 前端 `IProject` 缺少相应策略字段：`packages/types/src/project/projects.ts:50`。
- serializer 暴露底层字段，但没有预算和 cadence 的边界校验：`apps/api/karma_team/app/serializers/project.py:42`。

**建议**

可以更新，建议拆分为两个决定：

- **Bug 修复部分优先做**：关闭开关后，未领取的自动任务必须在消耗预算和派发前停止；人工明确触发的运行保持原行为。
- **产品语义部分单独确认**：增加设置 UI、字段校验，并决定 `Re-tick` 是否从固定增加 3 次改为增加一整份项目预算。

**兼容边界与验证**

- 上游 `0166_drop_agent_retick_grant` 不能原编号复制，Karma Team 已有自己的 `0166` 迁移。可先停止读取旧字段，稳定后再用新的后续迁移删除。
- 设置表单要保留 `dirtyFields` 语义，只有 cadence 控件实际变化时才同步三个阶段，防止只改名称时覆盖外部更新。
- 保留 `default_agent_executor`、成员自动绑定 Runner 和当前 orchestration 定制。
- 验证关闭前后已经排期、并发领取、人工 Run AI、有限/无限预算和 `Re-tick` 连续调用。

### 3. CLI 登录简化

**Karma Team 现状**

- 只有 `karma-team auth login`，没有顶层 `karma-team login`：`runner/src/cli/mod.rs:67`。
- 登录地址优先级已经支持 `--url`、已有配置、`KARMA_TEAM_API_URL` 和交互输入：`runner/src/cli/auth/login.rs:244`。
- 服务端安装脚本会注入当前部署地址：`apps/api/karma_team/runner/views/cli_downloads.py:165`。
- Web 生成的 `runner add` 命令也显式包含 `--url`：`apps/web/core/components/runners/runner-cli-command.tsx:90`。

**建议**

直接增加顶层 `karma-team login`，复用现有 `auth::login::Args` 和处理函数，同时保留 `karma-team auth login` 以兼容脚本。不要复制上游提交 `2ddb526b` 中的 `https://pidash.airepublic.com` 默认值；这会把自托管客户端引向无关服务。

验证顶层别名、旧命令、显式 `--url`、已有配置、环境变量和无 TTY 错误提示。

### 4. 其他界面能力：发布草稿、项目页添加 Runner

**发布草稿**

- 现有 `handleMoveToProjects` 已能把草稿转为正式工作项，但按钮只在专用 `moveToIssue` 模式展示：`apps/web/core/components/issues/issue-modal/form.tsx:670`。
- 可以复用现有 API，在普通草稿详情/预览增加 `Publish issue`。
- 应覆盖发布成功、没有项目、请求失败和 MobX 集合刷新；不能仅乐观关闭弹窗而不更新列表。

**项目页添加 Runner**

- Add Runner modal 没有 `projectId` prop，打开时把项目清空且始终允许重新选择：`apps/web/core/components/runners/add-runner-modal.tsx:30`。
- 项目级 Runner 页面没有把路由项目传给 modal：`apps/web/app/(all)/[workspaceSlug]/runners/page.tsx:351`。
- 建议在项目页预填并锁定当前项目；工作区级入口继续允许自由选择。

两项均为前端局部新增，不要求 API 或数据库迁移。移植时要保留 Karma Team 的远程 Runner、Dev Machine 和 CLI 安装配置逻辑。

### 5. 项目邀请接口返回 500

**结论：已修复，不应覆盖。**

- 成员角色已经通过 `.values_list("role", flat=True).first()` 获取：`apps/api/karma_team/app/views/project/invite.py:81`。
- 已正确调用 `project_invitation.delay(...)`：同文件 `:117`。
- 当前实现额外支持用户名和任意账号标识，而不是强制邮箱格式：同文件 `:57`。
- `apps/api/karma_team/tests/contract/app/test_account_identifiers.py:147` 已覆盖非邮箱账号、用户名解析、角色匹配和异步任务调用。

直接覆盖上游文件会有恢复邮箱格式限制的回退风险。建议只保留现状，在未来修改邀请流程时继续运行现有 contract tests。

### 6. 异步请求和 SSE 泄漏数据库连接

**Karma Team 现状**

问题仍存在，当前没有上游的 `karma_team/utils/db_async.py` 等价 helper。裸异步 ORM 边界至少包括：

- `apps/api/karma_team/assistant/views/events.py:17`
- `apps/api/karma_team/assistant/tasks.py:20`
- `apps/api/karma_team/runner/views/sessions.py:25`
- `apps/api/karma_team/runner/views/machine_sessions.py:32`
- `apps/api/karma_team/cloud_agent/runtime.py:7`
- `apps/api/karma_team/cloud_agent/github_mcp.py:7`

`settings/common.py` 只有 `idle_in_transaction_session_timeout`，没有普通 idle session 兜底；`bin/docker-entrypoint-worker.sh` 也没有上游的默认并发限制。

**建议**

适配上游 `6ec07231` 和后续修正 `8702647e` 的最终形态：

- 建立 transaction-aware DB async helper，在调用前后回收空闲连接，但不得关闭仍处于 atomic transaction 中的连接。
- 逐处替换 Assistant SSE、Runner/Machine 长轮询、Cloud Agent 和后台任务的 ORM 跨线程调用。
- 为 PostgreSQL 增加普通 idle session 超时兜底。
- 为 Celery worker 设置合理的默认并发上限，同时允许部署显式覆盖。
- Assistant 高频 delta flush 保留单线程/单连接策略，在一次 turn 的 `finally` 中统一关闭，避免每个 token 都反复建连。

**风险与验证**

这是广泛的异步边界变更，不能全局机械替换 `sync_to_async`。应测试事务内 helper、成功/异常/取消的 SSE、两个 long poll 断连、Assistant turn 成功/失败/超时，并使用 `pg_stat_activity` 观察连接是否在断连后回收。还要验证小核与大核容器下的 Celery 并发计算。

### 7. 工作项 Peek 无法正确展开

**结论：主 Web 已修复，Desktop Overlay 尚未修复。**

主 Web 已具备展开前清理 peek URL、route `projectId` fallback、URL sync 规范化和加载保护，并有对应测试。不要重复修改：

- `apps/web/core/components/issues/peek-overview/header.tsx`
- `apps/web/core/components/issues/peek-overview/url-sync.tsx`

Desktop Overlay 的 `desktop-overlay/apps/web/core/components/issues/peek-overview/header.tsx:113` 仍使用 `issueDetails?.project_id` 生成链接，加载期间可能复制或打开 `undefined` 地址，展开时也缺少完整的路由时序保护。

建议只把上游 `95d42124` 及相关导航修复适配到 Desktop Overlay，并保留 `desktopWebUrl` 转换和桌面错误提示。补充桌面 header 测试，覆盖未加载、普通点击、修饰键新标签、复制链接和加载完成后的 URL。

### 8. 语音转写接口字段不一致

**结论：已完整修复。**

- 前端上传 multipart 字段 `file`，并抛出 `response.data`：`packages/services/src/assistant/assistant.service.ts:162`。
- hook 从错误体读取 `error`：`apps/web/core/hooks/use-dictation.tsx:131`。
- 后端读取 `request.FILES["file"]` 并返回稳定 `error`：`apps/api/karma_team/assistant/views/transcribe.py:71`。
- `apps/web/tests/assistant/transcribe-contract.test.ts` 已覆盖契约。

无需再次更新。

### 9. 大页面保存返回 413

**Karma Team 现状**

`apps/live/src/server.ts:65` 仍调用无参数 `express.json()`，使用默认约 100 KB 限制；`apps/live/src/env.ts` 没有 `JSON_BODY_LIMIT`。现有 `apps/api/karma_team/bgtasks/copy_s3_object.py:67` 会调用 `/convert-document/`，因此该问题在当前功能中可达，并不依赖上游新公开 Page API。

**建议**

直接适配上游方案：新增 `JSON_BODY_LIMIT`，默认 `20mb`，并传入 `express.json({ limit: env.JSON_BODY_LIMIT })`。这是可配置上限，不应改成无限制。

验证 100 KB 以上请求恢复成功、4 至 5 MB 文档转换、非法配置以及超过 20 MB 仍返回 413。还应观察大请求的峰值内存。

### 10. 页面状态变化后仍可能保存

**Karma Team 现状**

Karma Team 没有上游 `apps/api/pi_dash/api/views/page.py` 和公开 Page 写入 API，因此不能直接 cherry-pick `c6e4daff`。但现有内部页面写入存在同类竞态：

- `apps/api/karma_team/app/views/page/base.py:154` 和 `:521` 在保存前检查锁定/归档状态，但没有在行锁下重新检查。
- `apps/api/karma_team/app/serializers/page.py:213` 最终执行不带 `update_fields` 的 `instance.save()`，陈旧实例可能覆盖并发状态。

**建议**

只把上游的并发原则适配到现有内部入口：耗时处理后进入 `transaction.atomic()`，通过 `select_for_update()` 重新读取页面并检查锁定/归档状态，只保存请求实际修改的字段。不要为这个修复引入不在本次范围内的公开 Page API。

使用 `update_fields` 时必须包含现有 `Page.save()` 会维护的派生字段，例如 `description_stripped`，以及所需的审计时间/用户字段。验证保存过程中并发锁定、归档、普通内容并发修改、权限变化和无冲突更新。

### 11. Markdown 转换丢失水平分割线

**结论：当前不适用。**

Karma Team 不存在上游的 `utils/markdown_converter.py`，也没有公开 Page API 的 HTML 到 Markdown 输出链路。现有 `utils/markdown.py` 和 `assistant/runtime/markdown.py` 都是 Markdown 到 HTML，不会经过本次修复的 Tiptap `<div data-type="horizontalRule">` 分支。

单独移植几行转换逻辑没有调用者，还会引入不在指定列表中的 Page API 前置能力。未来如果另行决定引入公开 Page/CLI，再把 `103b14a2` 和真实编辑器结构测试作为必需组成部分。

### 12. Assistant 错误信息被错误归类

**Karma Team 现状**

`apps/api/karma_team/assistant/tasks.py:430` 的 `_classify_error()` 只按异常字符串猜测类型，没有优先识别直接抛出的 `AssistantError`，也不遍历 `__cause__`。因此 `QuotaExceeded` 等已分类错误可能退化为 `internal`。现有 `assistant/errors.py` 已有完整 typed taxonomy，修复不需要新数据结构。

**建议**

适配 `bb3d99c6`：沿异常 cause chain 查找 `AssistantError`，原样返回其稳定 code/detail；没有 typed error 时再使用现有文本启发式规则。增加直接 typed error、被包装 error、普通未知 error 和 turn 端到端事件测试。

此项会与第 6 项同时修改 `assistant/tasks.py`，实施时应放在同一开发批次协调，避免互相覆盖。

### 13. Docker 镜像在新版 pnpm 下构建失败

**结论：当前故障条件已被规避，非必要更新。**

- `apps/admin/Dockerfile.admin`、`apps/live/Dockerfile.live`、`apps/space/Dockerfile.space` 都显式执行 `corepack prepare pnpm@10.32.1 --activate`。
- 版本与根 `package.json` 的 `packageManager` 一致，不会在复制项目文件前自动解析到不确定的新版 pnpm。
- `apps/web/Dockerfile.web` 已包含 `$PNPM_HOME/bin`。

上游 `ce1ba019` 解决的是未固定新版 pnpm 把全局可执行文件放到 `$PNPM_HOME/bin` 后找不到 Turbo 的问题。当前 Karma Team 不具备相同触发条件。

可以在统一 Dockerfile 时，把另外三个镜像的 PATH 也改成 `$PNPM_HOME:$PNPM_HOME/bin:$PATH`，作为低风险防御性加固，但不应把它描述为当前必须修复的 Bug。验证方式是对四个镜像分别进行无缓存构建，并确认日志使用 pnpm `10.32.1`。

### 14. 离线 Runner 导致工作项永久卡住

**Karma Team 现状**

现有 heartbeat reaper 依赖 Runner 恢复通信，Runner 永久离线时不能释放 stranded run：

- `apps/api/karma_team/celery.py:83` 只有 mark-offline 和 stall watchdog，没有离线 queued/active sweep。
- `apps/api/karma_team/settings/common.py:465` 没有 5/60 分钟恢复阈值。
- `apps/api/karma_team/runner/tasks.py:100` 没有 queued pin/run recovery。
- `apps/api/karma_team/orchestration/service.py:527` 的 pin 判定只排除 `REVOKED`，未排除离线或心跳陈旧 Runner。

**建议**

适配上游 `70d684af`：

- 默认离线 5 分钟后释放 pinned queued run，允许其他合格 Runner 领取。
- 默认离线 60 分钟后终结仍滞留的活动 run。
- 原 Runner 恢复时补发 cancel，避免旧执行与替代执行并行。
- 不再为已离线或心跳过旧的 Runner 创建新 pin。
- 增加两个周期 sweep 任务和可配置阈值。

**兼容边界与验证**

必须保留工作区成员、项目成员、负责人资格、Dev Machine、managed/private Runner，以及 `session_service.py` 中的自定义终态、项目移交和容量释放逻辑。分别模拟 queued、running、awaiting approval，验证 5/60 分钟边界、重新分配、恢复后 cancel、重复 sweep 幂等和多人权限。

### 15. Runner 重启后云端一直显示忙碌

**Karma Team 现状**

`runner/src/daemon/supervisor.rs:300` 只有约 5 秒的内存 drain；同文件 `:854` 的 `RunFailed(DaemonRestart)` 发送失败或超时后只写日志，随后丢失。当前没有 `drain_journal.rs` 或对应持久化路径。

**建议**

适配上游 `971cb605` 的持久化 journal：关机时先原子落盘未送达事件，发送成功后删除；启动时重放，并对暂时失败保持后台重试。第 14 项的服务端 60 分钟回收只是兜底，不能替代这里的快速恢复。

不能整文件覆盖 supervisor。适配必须保持 Karma Team 多 Runner supervisor、hot-add、动态项目同步和 machine control 的冻结/退出顺序。验证 SIGTERM、SIGKILL、断网、发送前后崩溃、重复投递幂等、journal 损坏容错和成功后的文件清理。

### 16. 会话建立时无限循环并占满 CPU

**Karma Team 现状**

`apps/api/karma_team/runner/services/outbox.py:316` 已不再使用 `justid=True`，因此 redis-py 返回格式误判这一主要根因已经修复，并有现有单元测试。但循环缺少 `next_cursor == cursor` 时退出的防御性保护。Karma Team 独有的 `runner/services/machine_outbox.py:254` 也有相同缺口。

**建议**

在两个 outbox reclaim 循环中都增加 cursor 无进展检测，记录错误并退出。测试 Redis 连续返回相同 cursor、正常多页、空结果和异常路径，确认不会忙循环，也不会漏掉正常 pending entry。

### 17. Git 异常直接阻止任务执行

**Karma Team 现状**

`runner/src/workspace/resolve.rs:6` 仍会为非空非 Git 目录、不可识别 URL 和 clone 失败返回硬错误。`runner/src/daemon/supervisor.rs:3236` 的 Karma agent 路径和 `:3265` 的普通 agent 路径都会直接发送 `RunFailed`。

**建议**

适配 `dfe3628b`，把 Git 定位为可选上下文：已有普通文件的目录保持原样，无法 clone 或 URL scheme 不支持时记录可观测警告，让智能体仍能在工作目录执行。真正无法创建、读取或安全使用工作目录的文件系统错误仍然必须失败。

验证非 Git 普通目录、无效 remote、clone 网络失败、合法仓库、Karma agent 和普通 agent 两条路径。日志应保留 Git 失败原因，避免把可运行降级变成静默故障。

### 18. Clone 失败后无法恢复

**Karma Team 现状**

- `runner/src/workspace/git.rs:12` clone 失败后直接返回，没有清理本次生成的半成品。
- `is_empty_dir` 会把所有元数据都当作用户文件：同文件 `:155`。
- Runner 会写入 `.karma-team/context.md`：`runner/src/daemon/supervisor.rs:3296`。这会让原本空目录在一次失败后不再被视为空目录。
- Git 子进程尚未统一设置 `GIT_TERMINAL_PROMPT=0`。

**建议**

与第 17 项一并适配 `de4a0f6c`。`RUNNER_OWNED_ENTRIES` 必须使用 Karma Team 的 `.karma-team`，不能照搬 `.pidash`。只有预检确认目录原本为空或仅含 Runner 自有元数据时，才可清理本次 clone 产生的 `.git` 和不完整工作树；任何用户文件都不能删除。保留现有 SSH TOFU、BatchMode 和 host-key 逻辑。

验证第一次 clone 中断后重试、仅含 `.karma-team`、存在半成品 `.git`、包含用户文件、SSH 首次连接和缺少凭据。`.karma-team-workpad.md` 如果尚未成功上传，不能被恢复清理逻辑误删。

### 19. Muse Code 输出格式解析错误

**结论：已完整修复。**

- `runner/src/muse_code/schema.rs` 已解析 `record_type`、`payload_type`、`payload`。
- `runner/src/muse_code/bridge.rs` 已处理实际开始事件、增量、工具结果、终态和 UUID session。
- `runner/tests/fixtures/muse_code/` 已包含四组真实 JSONL fixture。

除品牌命名外，当前实现与上游 `e7062258` 的功能结构一致。无需业务更新，保留并运行现有 fixture/bridge 测试即可。

### 20. CLI 标签解析无限翻页

**结论：当前不适用。**

Karma Team CLI 当前没有 `label` 顶层命令、`fetch_project_labels` 或工作项标签参数，上游 `3877366d` 修复的分页循环入口不存在。只为修复这个 Bug 引入整套标签 CLI 会超出本次指定范围。

未来如单独决定增加标签 CLI，应从一开始包含三个停止条件：`next_page_results == false`、结果为空、cursor 重复，并测试多页和异常响应。

### 21. 按状态名修改工作项失败

**Karma Team 现状**

服务端 `apps/api/karma_team/api/views/state.py:149` 使用分页器，`utils/paginator.py:716` 返回带 `results` 的 envelope；CLI 的 `runner/src/cli/resolve.rs:71` 却只接受 `body.as_array()`。因此按状态名称解析在当前接口契约下会失败。

**建议**

适配 `2f7c05e7`，优先读取 `body.results` 数组，同时回退支持旧服务返回的裸数组。增加分页 envelope、裸数组、空结果、未知状态、同名状态和错误响应测试，并通过真实 CLI 执行一次按状态名更新。

### 22. 陈旧 workpad 文件覆盖服务端内容

**Karma Team 现状**

`runner/src/cli/workpad.rs:39` 没有 `--keep`；同文件 `:89` 在 PATCH 成功后保留本地正文文件。后续运行可能把旧内容重新上传，覆盖服务端的新版本。

**建议**

适配 `4f99e8ea`：

- 仅在 PATCH 成功后默认删除普通文件。
- 请求失败时保留文件。
- `--keep` 显式保留。
- 对 stdin `-`、目录、symlink、FIFO 和已不存在路径不执行删除。
- 删除失败只输出警告，不能把已经成功的服务端更新改判为失败。
- 环境变量和提示文本继续使用 `KARMA_TEAM_ISSUE_IDENTIFIER`，不能复制 `PIDASH_*`。

同时更新 `apps/api/karma_team/prompting/fragments/04_karma_team_cli.md`、`09_workpad_setup.md` 及对应 `sections/` 文件，要求每次编辑前重新获取服务端版本。验证成功、HTTP 失败、`--keep`、stdin、symlink、目录、重复调用和删除权限失败。

## 5. 所有移植批次都必须遵守的兼容规则

1. 不使用整文件覆盖方式同步上游。Karma Team 在邀请、Runner、多人项目绑定、Cloud Agent、提示词和编排上已有本地扩展。
2. 不恢复邮箱格式校验。Karma 账号可以不是邮箱，邀请和成员流程必须继续接受普通账号标识。
3. 不破坏“项目成员自动绑定项目并创建/关联 Runner”的多人协作逻辑。
4. 不硬编码 Pi Dash 品牌、托管域名、`.pidash`、`PIDASH_*` 或 `pidash-ipc`；分别使用 Karma Team 自己的名称、可配置部署地址、`.karma-team`、`KARMA_TEAM_*` 和 `karma-team-ipc`。
5. 不复制上游迁移编号。每次先确认 Karma Team 当前 migration graph 的叶节点，再生成本项目的后续迁移。
6. 不因修复不存在的调用路径而引入整套前置产品能力。第 11、20 项目前应保持不动。
7. 涉及 Runner 的改动必须同时验证 owner 创建的项目和“作为成员加入”的项目，确认任务领取、权限、私有 Runner 可见性和容量释放都不回退。

## 6. 建议验证与发布策略

每个批次至少执行受影响包的格式、lint、类型检查和单元/契约测试。跨 API、Runner 和 Web 的功能不能只以单侧单元测试通过作为完成标准。

建议的端到端场景：

1. 准备项目 owner 和普通项目成员两个账号，成员不创建该项目。
2. 确认成员账号自动获得项目绑定及对应 Runner，能领取分配给自己的工作项。
3. 覆盖 Runner 在线、短时离线、永久离线、重启和断网恢复。
4. 覆盖自动 ticking、人工 Run AI、Scheduler 触发和普通手工任务。
5. 验证非邮箱格式账号仍能登录、被邀请、成为项目成员并正常协作。
6. 对数据库连接修复使用 `pg_stat_activity` 做断连前后对比。

推荐按批次独立提交和发布，每批都保留回滚点。第 6、14、15 项涉及运行中的长任务，发布前应先在测试环境制造对应故障场景，而不是只跑 happy path。

## 7. 审阅限制

当前 `karma-team/.git` 是一个 worktree 指针，目标为：

```text
/data/bubble-rag/pidash-deploy/pi-dash/.git/worktrees/karma-team
```

该 worktree 管理目录在审阅时不存在，因此无法在 Karma Team 目录内正常执行 `git status`、`git log` 或 `git diff`。本次评估改用上游 Pi Dash Git 历史、指定提交补丁和两个目录的实际文件内容交叉核对。这个 Git 元数据问题不影响代码现状判断，但在后续开始实施、提交或生成可靠变更清单前应单独恢复；本次没有擅自修改 Git 元数据。

本次也没有运行构建或测试，因此“现有测试已存在”只表示静态确认到测试文件和覆盖意图，不代表本轮重新执行通过。
