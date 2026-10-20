# Pi Dash 近一个月开发进展总结

## 文档范围

- 项目：[`The-AI-Republic/pi-dash`](https://github.com/The-AI-Republic/pi-dash) 的当前 fork
- 统计周期：2026 年 9 月 19 日至 2026 年 10 月 19 日
- 实际最新提交：2026 年 10 月 17 日的 [`d656243d`](https://github.com/The-AI-Republic/pi-dash/commit/d656243d)
- 对比范围：[`ce3cf2ad...d656243d`](https://github.com/The-AI-Republic/pi-dash/compare/ce3cf2ad...d656243d)
- 统计口径：以变更进入 `main` 的时间为准，包含较早开发、在本周期内合并的提交
- 变更规模：119 条非 Merge 提交，涉及 393 个文件

核对时，本地 `main` 与上游 `The-AI-Republic/pi-dash` 的 `main` 均指向 `d656243d`。因此，本文总结的是当前 fork 已包含的近一个月变更，这些变更同时存在于上游主分支中。

## 核心结论

本周期的开发重点可以归纳为四个方向：

1. **扩大智能体可操作范围。** CLI 和公开 API 从基础工作项操作扩展到标签、项目文档、依赖关系、图片评论、调度记录和手动触发 AI 执行。
2. **完善任务编排控制。** 项目管理员可以在界面配置自动运行开关、运行预算和执行间隔；智能体可以显式等待依赖或停止后续定时执行。
3. **补齐桌面端体验。** 新增内置智能体审批模式、每日更新入口、About 和许可证窗口、退出时清理聊天历史等能力。
4. **集中提升稳定性。** 修复桌面登录和聊天中断、离线 Runner 导致任务永久卡住、数据库连接泄漏、Git 初始化失败、项目邀请 500 等问题。

Runner 版本从 `0.1.23` 更新到 `0.1.28`，桌面应用从 `0.3.1` 更新到 `0.3.3`。

## 新增功能和能力增强

### 1. 聊天语音输入

聊天输入框新增语音输入能力：按住麦克风录音，松开后调用语音识别服务，将结果追加到当前草稿。该功能包含录音计时、60 秒上限、误触过滤、麦克风权限错误提示和未配置时的设置引导。

个人 AI Assistant 设置中新增语音识别配置，可以填写 OpenAI 兼容服务的 Base URL、API Key 和模型名称，并测试连接。服务端按用户加密保存 API Key；音频只转发给用户配置的服务，不在 Pi Dash 中持久化，单次音频上限为 25 MB。

相关提交：

- [`8ad9e2cb`](https://github.com/The-AI-Republic/pi-dash/commit/8ad9e2cb)：增加语音识别配置模型和加密密钥存储
- [`ae81a105`](https://github.com/The-AI-Republic/pi-dash/commit/ae81a105)：增加语音识别配置接口
- [`a44aa536`](https://github.com/The-AI-Republic/pi-dash/commit/a44aa536)：增加音频转写接口
- [`49e8e2e5`](https://github.com/The-AI-Republic/pi-dash/commit/49e8e2e5)：增加语音输入设置界面
- [`31e5e113`](https://github.com/The-AI-Republic/pi-dash/commit/31e5e113)：在聊天输入框接入麦克风和语音输入逻辑

### 2. 桌面内置智能体审批模式

桌面内置智能体新增三档审批模式：

- `Ask every time`：采用只读沙箱，遇到需要额外权限的操作时请求批准
- `Workspace`：允许修改当前工作区，工作区外操作仍需批准
- `Full access`：允许更广泛的本地操作，但静态拒绝规则仍然生效

命令执行、文件修改或网络请求可以在聊天中批准或拒绝，命令还可以选择在当前会话内持续允许。模式按 Runner 和账号保存，新线程会使用保存的模式。

相关提交：[`ccda04d6`](https://github.com/The-AI-Republic/pi-dash/commit/ccda04d6)

### 3. 项目调度器创建、安装和执行历史

项目调度器的使用流程得到完善：

- 原有的“安装调度器”入口替换为统一的 **New Scheduler** 弹窗
- 可以安装工作区已有调度器
- 具有工作区管理员权限的用户可以创建新的调度器定义，并立即安装到当前项目
- 新增可点击的调度器详情页
- 详情页展示调度器配置、执行周期、最近状态、运行历史，以及每次运行关联的工作项
- Cloud Agent 的调度提示词增加项目和调度器身份信息，避免调度任务不知道自己所属的项目

相关提交：

- [`3db73752`](https://github.com/The-AI-Republic/pi-dash/commit/3db73752)：新建或安装调度器的统一弹窗
- [`474123e5`](https://github.com/The-AI-Republic/pi-dash/commit/474123e5)：调度器详情和运行历史
- [`1d7be107`](https://github.com/The-AI-Republic/pi-dash/commit/1d7be107)：调度任务提示词增加项目和调度器信息

### 4. 通过 CLI、API 和 MCP 查询调度器

新增以下只读 CLI 命令：

```text
pidash scheduler list
pidash scheduler get
pidash scheduler runs
```

这些命令可以查询工作区调度器定义、项目安装信息、提示词、执行周期、最近执行情况，以及运行关联的工作项。对应的只读接口也开放给公开 API 和 MCP 连接器。

本周期只提供查询能力；创建、编辑、启停和手动触发调度器尚未通过这组公开接口开放。

相关提交：[`04920ea3`](https://github.com/The-AI-Republic/pi-dash/commit/04920ea3)

### 5. 项目级智能体定时运行设置

项目设置新增 **AI agent ticking** 配置区域，项目管理员可以设置：

- 是否启用定时调用智能体
- 每个工作项可使用的自动运行预算
- 是否取消预算上限
- 自动运行的时间间隔

开发、审查和测试三个阶段的默认执行间隔统一为 3 小时。预算按工作项计算，并在三个阶段间共享。`Re-tick` 每次补充一整份项目预算，例如项目预算为 10 次时，工作项会从 10 次增加到 20 次，再次操作则增加到 30 次。

关闭项目定时运行后，已经排期的自动执行会在领取前停止，不再额外消耗一次预算。人工明确请求的执行仍可运行。

相关提交：

- [`0737a645`](https://github.com/The-AI-Republic/pi-dash/commit/0737a645)：三个阶段统一为 3 小时执行间隔
- [`b1e073ed`](https://github.com/The-AI-Republic/pi-dash/commit/b1e073ed)：`Re-tick` 改为补充一整份预算
- [`4ed6e16a`](https://github.com/The-AI-Republic/pi-dash/commit/4ed6e16a)：项目设置界面
- [`7c355595`](https://github.com/The-AI-Republic/pi-dash/commit/7c355595)：关闭开关时停止已经排期的自动执行
- [`61a0d181`](https://github.com/The-AI-Republic/pi-dash/commit/61a0d181)：修复设置表单中的覆盖和状态同步问题

### 6. 工作项依赖关系

智能体现在可以创建、删除和查询工作项关系：

```text
pidash issue relate
pidash issue unrelate
pidash issue relations
```

支持阻塞、关联、重复、前后顺序和实现关系等类型。工作项详情同时返回 `relations_summary` 和 `has_open_blockers`，智能体提示词也会列出 `blocked_by`、`blocking` 等方向性关系。

关系写入具有幂等性：重复添加已有关系不会制造重复数据；若同一对工作项已有其他关系，则返回冲突而不是静默覆盖。

相关提交：

- [`02880b72`](https://github.com/The-AI-Republic/pi-dash/commit/02880b72)：关系 API、CLI 和智能体工具
- [`87b6242c`](https://github.com/The-AI-Republic/pi-dash/commit/87b6242c)：阻塞摘要和未完成阻塞项判断
- [`7edb59b2`](https://github.com/The-AI-Republic/pi-dash/commit/7edb59b2)：提示词中展示方向性关系

### 7. 显式等待依赖

新增命令：

```text
pidash issue wait <ISSUE-ID>
```

当智能体确认无法绕过当前依赖时，可以调用该命令并结束本次运行。每次成功等待会把该工作项的运行上限增加 1，因此发现依赖并等待的这次运行不会净消耗正常预算。

等待额度最多为一整份项目预算。例如默认预算为 10 次，一个工作项最多可以获得 10 次免费等待。界面和提示词会分别显示实际运行数与等待次数，方便识别长期卡住的任务。

该机制不会监听依赖项并在依赖完成后立即唤醒工作项。智能体会在下一次定时执行时重新读取依赖状态并作出判断。

相关提交：

- [`e7afa2f3`](https://github.com/The-AI-Republic/pi-dash/commit/e7afa2f3)：增加显式等待机制
- [`c0582840`](https://github.com/The-AI-Republic/pi-dash/commit/c0582840)：统一运行数、预算和等待次数的显示口径

### 8. 显式停止后续定时运行

新增命令参数：

```text
pidash run yield --outcome <OUTCOME> --stop-ticking
```

`done`、`blocked`、`waiting_on_human` 等 outcome 现在主要用于说明本次运行的结果，不再自动决定是否停止时钟。只有显式提交 `--stop-ticking`，或项目开关、运行预算、任务状态等系统约束生效时，定时运行才会停止。

界面会显示“由智能体停止”，并在时钟已经停止时隐藏过期的“下一次运行”倒计时。

相关提交：

- [`3869a226`](https://github.com/The-AI-Republic/pi-dash/commit/3869a226)：服务端支持显式停止信号
- [`0a617f49`](https://github.com/The-AI-Republic/pi-dash/commit/0a617f49)：CLI 增加 `--stop-ticking`
- [`07bb896a`](https://github.com/The-AI-Republic/pi-dash/commit/07bb896a)：提示词采用新的停止语义

### 9. 工作项 CLI 和公开 API 扩展

本周期补充了以下工作项操作能力：

- `pidash issue run-ai <ISSUE-ID>`：从终端触发与 Web “Run AI”相同的执行流程
- `issue list`：按状态、状态组、父工作项、标签、优先级和负责人过滤
- `--fields`：只返回需要的字段，减少脚本和智能体处理的无关内容
- `issue create/patch --description-file`：从文件读取 Markdown 描述，`-` 表示从 stdin 读取
- `issue patch --git-work-branch`：记录远端工作分支，供后续智能体继续已有工作；空字符串可清除记录
- 工作项创建和更新 API 原生接受 `description_markdown`

`issue run-ai` 会复用现有调度逻辑。已有活动运行、缺少 Pod 或没有合格 Runner 时会返回明确原因；活动中的智能体不能借此无限重启自身并绕过预算。

相关提交：

- [`a44694f6`](https://github.com/The-AI-Republic/pi-dash/commit/a44694f6)：CLI 和 API 触发 Run AI
- [`5a942236`](https://github.com/The-AI-Republic/pi-dash/commit/5a942236)：列表过滤与字段选择
- [`6dac8b67`](https://github.com/The-AI-Republic/pi-dash/commit/6dac8b67)：API 接受 Markdown 描述
- [`3d18bd3d`](https://github.com/The-AI-Republic/pi-dash/commit/3d18bd3d)：从文件或 stdin 读取描述
- [`185b79ec`](https://github.com/The-AI-Republic/pi-dash/commit/185b79ec)：记录工作分支

### 10. 标签管理

CLI 新增完整的标签操作：

```text
pidash label list
pidash label create
pidash label update
pidash label delete
```

标签可以通过名称或 UUID 指定。工作项创建和修改命令同时增加设置、追加、移除和清空标签的参数，便于脚本及智能体复用项目已有分类。

相关提交：[`9b95917c`](https://github.com/The-AI-Republic/pi-dash/commit/9b95917c)

### 11. 项目文档读取和维护

新增项目文档公开 API 和 CLI：

```text
pidash page list
pidash page get
pidash page create
pidash page update
pidash page archive
pidash page unarchive
```

智能体可以读取项目知识库页面，也可以创建、更新、归档和恢复页面。正文支持 Markdown 或 HTML，服务端会同步生成 Web 编辑器需要的 HTML、JSON 和 Yjs binary，并记录页面版本、提及和反向链接。

接口遵循项目成员、页面公私有、页面所有者和管理员权限。锁定页面会拒绝写入。Markdown 转换保留标题、列表、任务列表、代码块、表格、提及、图片和工作项嵌入等结构。

相关提交：

- [`6d36c14f`](https://github.com/The-AI-Republic/pi-dash/commit/6d36c14f)：项目页面列表和详情读取
- [`67823508`](https://github.com/The-AI-Republic/pi-dash/commit/67823508)：Markdown 到编辑器 HTML 的转换
- [`1fbba87b`](https://github.com/The-AI-Republic/pi-dash/commit/1fbba87b)：将转换内容写入实时协作文档
- [`6745a8d8`](https://github.com/The-AI-Republic/pi-dash/commit/6745a8d8)：页面写入 API
- [`3f6a77a4`](https://github.com/The-AI-Republic/pi-dash/commit/3f6a77a4)：页面写入 CLI

### 12. CLI 图片评论

`pidash comment add` 新增可重复的 `--image` 参数，支持多张图片和纯图片评论。图片会通过公开资产 API 创建上传记录、直传 S3 或 MinIO，并在确认上传后嵌入评论正文。

上传前会先验证全部本地文件，避免上传一部分之后才发现后续文件无效。当前核实范围是 CLI 和公开 API；本周期没有在该仓库中交付 MCP 图片评论能力。

相关提交：

- [`4dc7b47d`](https://github.com/The-AI-Republic/pi-dash/commit/4dc7b47d)：图片评论和资产上传
- [`1ce84ed3`](https://github.com/The-AI-Republic/pi-dash/commit/1ce84ed3)：上传前验证全部图片

### 13. Runner 自动安装智能体

执行 `pidash runner add` 时，如果本机缺少以下智能体 CLI，Runner 可以调用厂商官方安装器：

- Codex
- Claude Code
- Cursor Agent

安装完成后，Runner 会重新解析可执行文件的绝对路径并写入配置，避免守护进程的 `PATH` 尚未更新而找不到刚安装的程序。用户可以通过 `--skip-agent-install` 关闭自动安装。安装器不会替用户完成相应智能体账号的登录。

OpenClaw、Grok 和 Muse Code 本周期仍然只提供官方安装页面引导，没有接入自动安装器。

相关提交：[`35bee43e`](https://github.com/The-AI-Republic/pi-dash/commit/35bee43e)

### 14. CLI 登录简化

新增顶层命令 `pidash login`，作为 `pidash auth login` 的简写。全新安装默认连接 `https://pidash.airepublic.com`，不再要求先输入地址；已有自建地址会继续复用，也可以使用 `--url` 显式覆盖。

相关提交：

- [`3ab40d08`](https://github.com/The-AI-Republic/pi-dash/commit/3ab40d08)：增加顶层登录命令
- [`2ddb526b`](https://github.com/The-AI-Republic/pi-dash/commit/2ddb526b)：默认连接官方托管服务

### 15. 其他界面能力

- **发布草稿：** 草稿详情和预览弹窗新增 **Publish issue** 按钮，可以直接选择项目并发布为正式工作项。相关提交：[`48b5f098`](https://github.com/The-AI-Republic/pi-dash/commit/48b5f098)
- **新用户引导视频：** 欢迎页加入 Pi Dash 核心概念视频和相应说明。相关提交：[`04d4f99d`](https://github.com/The-AI-Republic/pi-dash/commit/04d4f99d)
- **项目页添加 Runner：** 从项目级 AI Workers 页面打开 Add runner 时，自动带入并锁定当前项目。相关提交：[`a102b5c9`](https://github.com/The-AI-Republic/pi-dash/commit/a102b5c9)

### 16. 桌面更新、退出和 About 窗口

桌面应用新增或补齐以下能力：

- 应用持续运行时每天检查一次更新
- 发现更新后在侧栏显示安装按钮
- 用户可以从侧栏安装并重启应用
- 退出时显示确认框，并可选择删除本地聊天历史
- 不同账号使用独立的引擎数据目录
- 新增 About 窗口，展示桌面版本、Pi Dash 和内置引擎许可证、NOTICE 及相关声明

相关提交：

- [`53ae81ad`](https://github.com/The-AI-Republic/pi-dash/commit/53ae81ad)：每日检查更新和侧栏更新按钮
- [`3279c0f9`](https://github.com/The-AI-Republic/pi-dash/commit/3279c0f9)：退出时清理历史及账号数据隔离
- [`3267ceb9`](https://github.com/The-AI-Republic/pi-dash/commit/3267ceb9)：About 和许可证窗口

## 修复的问题

### 1. API 和 Web

#### 项目邀请接口返回 500

项目邀请接口存在两个独立错误：代码对 `WorkspaceMember.objects.filter(...)` 返回的 QuerySet 直接读取 `.role`，并且误把本地邀请列表当作异步任务调用 `.delay()`。现在改为正确读取成员角色，并调用实际的项目邀请任务。

修复后，项目邀请可以正常创建并异步发送；非工作区成员也能沿用现有角色校验流程。

相关提交：[`d656243d`](https://github.com/The-AI-Republic/pi-dash/commit/d656243d)

#### 异步请求和 SSE 泄漏数据库连接

异步 ORM、SSE 和长轮询可能把 PostgreSQL 连接长期留在线程中，异常断开时尤其容易累积，最终耗尽数据库连接。

现在异步数据库调用统一通过事务感知的辅助函数执行：在调用前后释放空闲连接，同时避免关闭仍处于 atomic transaction 中的连接。系统还增加空闲连接兜底设置，并限制 Celery 默认并发规模，以控制共享数据库连接占用。

相关提交：

- [`6ec07231`](https://github.com/The-AI-Republic/pi-dash/commit/6ec07231)：关闭异步请求线程中遗留的连接
- [`8702647e`](https://github.com/The-AI-Republic/pi-dash/commit/8702647e)：增加事务感知，避免错误关闭正在使用的连接

#### 工作项 Peek 无法正确展开

工作项预览的 URL 同步可能覆盖刚发生的完整详情导航，数据尚未加载时还可能生成包含 `undefined` 的链接。现在调整关闭预览和导航的时序，加载期间不再生成无效详情链接，并正确处理修饰键打开新标签页。

相关提交：

- [`166b7e13`](https://github.com/The-AI-Republic/pi-dash/commit/166b7e13)：修复展开按钮导航
- [`25d64ae9`](https://github.com/The-AI-Republic/pi-dash/commit/25d64ae9)：加载期间使用路由中的项目 ID
- [`46cdbf15`](https://github.com/The-AI-Republic/pi-dash/commit/46cdbf15)：避免 URL 同步覆盖展开导航
- [`95d42124`](https://github.com/The-AI-Republic/pi-dash/commit/95d42124)：避免复制或打开 `undefined` 工作项链接

#### 语音转写接口字段不一致

前端此前把录音上传为 `audio`，而后端要求字段名 `file`；错误提示也读取了不一致的字段。现在统一上传字段和错误码，未配置语音识别服务时可以正确跳转到设置页面。

相关提交：[`3caa1b6a`](https://github.com/The-AI-Republic/pi-dash/commit/3caa1b6a)

#### 大页面保存返回 413

实时协作服务原本使用 Express 默认的 100 KB JSON body 限制。页面 HTML 加上 base64 Yjs 状态超过限制时，服务端页面写入会失败。默认限制现已提高到可配置的 20 MB。

相关提交：[`ce1ba019`](https://github.com/The-AI-Republic/pi-dash/commit/ce1ba019)

#### 页面状态变化后仍可能保存

页面在转换过程中被其他用户锁定或归档时，旧实现仍可能按照转换前的状态保存。现在 PATCH 会在数据库行锁下重新检查锁定和归档状态，并且只保存本次实际修改的字段。

相关提交：[`c6e4daff`](https://github.com/The-AI-Republic/pi-dash/commit/c6e4daff)

#### Markdown 转换丢失水平分割线

页面读取为 Markdown 时，转换器只识别普通 `<hr>`，没有识别编辑器实际生成的水平分割线结构。现在可以把编辑器的水平分割线正确转换为 Markdown。

相关提交：[`103b14a2`](https://github.com/The-AI-Republic/pi-dash/commit/103b14a2)

#### Assistant 错误信息被错误归类

Assistant 在运行中抛出的已分类错误会被重新识别成通用的 `internal` 错误，用户只能看到“unexpected error”。现在会优先保留 `AssistantError` 的稳定错误码和用户可读说明，包括被其他异常包装为 `__cause__` 的情况。

相关提交：[`bb3d99c6`](https://github.com/The-AI-Republic/pi-dash/commit/bb3d99c6)

#### Onboarding 视频无法播放

欢迎页嵌入的视频受到 iframe sandbox 限制，播放器无法正常初始化。现在补充跨源播放器需要的权限。

相关提交：[`f6db8670`](https://github.com/The-AI-Republic/pi-dash/commit/f6db8670)

#### Docker 镜像在新版 pnpm 下构建失败

`admin`、`live` 和 `space` 的构建阶段在项目文件复制前全局安装 Turbo。新版 pnpm 把可执行文件放到 `$PNPM_HOME/bin`，而旧 Dockerfile 没有把该目录加入 `PATH`，导致构建失败。三个镜像现已统一补充路径设置。

相关提交：[`ce1ba019`](https://github.com/The-AI-Republic/pi-dash/commit/ce1ba019)

### 2. Runner 和 CLI

#### 离线 Runner 导致工作项永久卡住

Runner 长时间离线后，排队、已分配或运行中的 AgentRun 可能一直保持活动状态，导致定时执行、状态切换和 Run AI 都被 `active_run_exists` 拒绝。

现在服务端会定期清理：

- 默认超过 5 分钟后，释放绑定在离线 Runner 上的排队任务，使其他 Runner 可以领取
- 默认超过 60 分钟后，终结仍滞留在离线 Runner 上的活动运行
- 原 Runner 恢复连接后补发取消，避免原任务与替代执行并行

相关提交：[`70d684af`](https://github.com/The-AI-Republic/pi-dash/commit/70d684af)

#### Runner 重启后云端一直显示忙碌

Runner 关机时如果 `RunFailed` 事件没有送达，旧实现会直接丢弃，云端因而保留悬空运行。现在 Runner 会先把未送达事件写入持久化日志，再尝试发送；下次启动时会重放并继续后台重试。

相关提交：[`971cb605`](https://github.com/The-AI-Republic/pi-dash/commit/971cb605)

#### 会话建立时无限循环并占满 CPU

Redis `XAUTOCLAIM JUSTID` 的真实返回格式与旧代码假设不一致，导致游标错误、同一 pending entry 被无限重复领取。在故障场景中，这会产生大量自旋线程并让 API 主机 CPU 达到 100%。

现在已修正返回格式处理，并在游标没有前进时主动退出。

相关提交：[`64842e19`](https://github.com/The-AI-Republic/pi-dash/commit/64842e19)

#### Git 异常直接阻止任务执行

工作目录已有普通文件但不是 Git 仓库、不支持当前仓库地址或 clone 失败时，旧 Runner 会直接让任务失败。现在会保留普通工作目录供智能体继续工作；只有空目录才自动 clone，已有文件不会被覆盖。

相关提交：[`dfe3628b`](https://github.com/The-AI-Republic/pi-dash/commit/dfe3628b)

#### Clone 失败后无法恢复

Runner 写入自己的 `.pidash` 元数据后，目录会被误判为已有用户文件，导致第一次 clone 失败后不再重试。Checkout 失败还可能留下 `.git` 和不完整工作树，使智能体误认为大量文件被删除。

现在空目录判断会忽略 Runner 自有元数据，并清理本次失败产生的残留，下次执行可以重新尝试。Git 操作同时关闭无终端凭证交互，避免守护进程永久等待。

相关提交：[`de4a0f6c`](https://github.com/The-AI-Republic/pi-dash/commit/de4a0f6c)

#### Windows PowerShell 安装失败

修复内容包括：

- 不再依赖 Windows 上可能未定义的 `$env:HOME`
- 安装子进程不会通过 `exit` 关闭用户当前终端
- 安装完成后更新当前 PowerShell 会话的 `PATH`
- 提示旧 MSI 安装的同名命令可能遮蔽新版本
- 修正固定版本安装时的下载链接

相关提交：[`612b457c`](https://github.com/The-AI-Republic/pi-dash/commit/612b457c)

#### Windows MSI 缺少登录入口

恢复 Windows 开始菜单中的 Pi Dash 登录快捷方式，使用户安装后能直接进入浏览器设备码登录流程。

相关提交：[`b87e105c`](https://github.com/The-AI-Republic/pi-dash/commit/b87e105c)

#### macOS 更新后 Runner 偶发无法启动

launchd 启动刚刚替换的可执行文件时，可能被 macOS AMFI 临时以 `SIGKILL` 终止。现在会识别该场景并额外尝试启动两次，真实配置错误或程序崩溃仍会按原逻辑报错。

相关提交：[`abbd555c`](https://github.com/The-AI-Republic/pi-dash/commit/abbd555c)

#### Muse Code 输出格式解析错误

Runner 以前没有按照 Muse Code 实际的 `record_type / payload_type / payload` 封装解析事件，可能把正常执行误报为 stdout 关闭，也可能无法恢复已有会话。

现在可以正确识别执行开始、增量输出、工具结果和终态，校验会话 UUID，并减少大量无实际内容的记账事件。

相关提交：[`e7062258`](https://github.com/The-AI-Republic/pi-dash/commit/e7062258)

#### CLI 标签解析无限翻页

服务端最后一页仍可能带有新的 `next_cursor`，旧 CLI 因此会持续请求空页。现在同时检查 `next_page_results=false` 和空结果，以可靠结束分页。

相关提交：[`3877366d`](https://github.com/The-AI-Republic/pi-dash/commit/3877366d)

#### 按状态名修改工作项失败

状态接口改为分页 envelope 后，旧 CLI 仍只接受数组。现在同时兼容 `{results: [...]}` 和旧数组响应。

相关提交：[`2f7c05e7`](https://github.com/The-AI-Republic/pi-dash/commit/2f7c05e7)

#### 陈旧 workpad 文件覆盖服务端内容

`workpad update` 成功后，现在默认删除本地暂存正文文件，避免后续运行误把旧文件重新上传。上传失败时文件会保留，用户也可以通过 `--keep` 主动保留。

相关提交：[`4f99e8ea`](https://github.com/The-AI-Republic/pi-dash/commit/4f99e8ea)

### 3. 桌面应用

#### 登录会话不稳定和退出落到 404

桌面 WebView 对跨站 XHR 的 Cookie 处理不稳定，Cookie 轮换后还可能让旧值覆盖新值，导致用户登录约十分钟后反复返回登录页。开源桌面版的退出请求也可能没有真正到达服务端，最终落到 SPA 404 页面。

现在 API 请求和 SSE 使用 Rust 原生传输，并正确管理 Cookie、重连、`Last-Event-ID` 和取消请求。退出会先请求服务端，成功后再清理本地会话并返回登录页。

相关提交：

- [`ab7756cf`](https://github.com/The-AI-Republic/pi-dash/commit/ab7756cf)：原生 Cookie 传输
- [`11c840c3`](https://github.com/The-AI-Republic/pi-dash/commit/11c840c3)：原生 SSE 和 IPC 字节传输
- [`f9e69940`](https://github.com/The-AI-Republic/pi-dash/commit/f9e69940)：修复 Cookie 轮换后周期性退出登录
- [`e98f8f6a`](https://github.com/The-AI-Republic/pi-dash/commit/e98f8f6a)：修复退出请求和 404

#### 连续发送聊天消息会丢失回复

第二次发送消息或预热会话时，旧实现可能替换第一条消息的活动流读取器，使回复同时从界面和本地历史中丢失。现在前端会跟踪正在进行的 turn，在回复完成前保持输入器忙碌；原生层也会拒绝重叠发送。

相关提交：[`63a7e049`](https://github.com/The-AI-Republic/pi-dash/commit/63a7e049)

#### 保存的审批模式没有可靠生效

聊天页面重新挂载或命中缓存时，可能在读取保存模式前预热出一个 Full access 线程；退出清理 WebView profile 时也会丢失审批设置。

现在会先加载保存模式，再启动新线程；偏好按账号保存到原生设置中，重新登录同一账号后恢复。删除聊天历史不会删除审批偏好。

相关提交：

- [`8bd401a2`](https://github.com/The-AI-Republic/pi-dash/commit/8bd401a2)：预热时应用保存模式
- [`0363f250`](https://github.com/The-AI-Republic/pi-dash/commit/0363f250)：退出后保留审批模式

#### 不同账号共用引擎数据

不同账号以前共用一个引擎目录，并且“删除聊天历史”没有删除引擎保存的线程和会话副本。现在每个账号使用独立目录；用户选择删除历史时，会同时清理相应的引擎会话、历史、线程和记忆数据。

相关提交：[`3279c0f9`](https://github.com/The-AI-Republic/pi-dash/commit/3279c0f9)

#### 每条聊天消息都重复配置运行时

旧实现可能在每条消息前重复获取 profile、写配置、签发凭证并注册运行时。现在每个工作区页面首次配置后使用快速路径，只在凭证接近过期时刷新；守护进程停止后仍可重新启动。

相关提交：[`469ff30c`](https://github.com/The-AI-Republic/pi-dash/commit/469ff30c)

#### 本地聊天绑定项目不稳定

本地聊天原先从无稳定顺序的项目列表中选择“第一个”项目。创建新项目后，绑定项目可能变化，并不断创建新的 Runner。现在优先使用已经连接的项目，否则选择默认或最早项目并保存该选择。

相关提交：[`70855d4a`](https://github.com/The-AI-Republic/pi-dash/commit/70855d4a)

#### Agent 已连接后仍持续轮询

Agent runtime 状态条在连接完成后仍每 5 秒轮询项目详情。现在连接后会停止轮询，失联或重新聚焦时再恢复检查。

相关提交：[`5babb7e3`](https://github.com/The-AI-Republic/pi-dash/commit/5babb7e3)

#### 复制和打开链接异常

桌面端曾复制出 `tauri://localhost` 链接，无法分享给浏览器用户；工作项尚未加载时还可能生成 `undefined` 链接。`window.open` 和 `target="_blank"` 请求也会静默无响应。

现在项目、工作项、评论、周期、模块和视图等链接统一转换成托管 Web URL；生成链接失败会给出提示；新窗口请求交给系统默认浏览器处理，并保留 query 和 fragment。

相关提交：

- [`d7966d3d`](https://github.com/The-AI-Republic/pi-dash/commit/d7966d3d)：复制托管 Web 链接
- [`1b9b23a1`](https://github.com/The-AI-Republic/pi-dash/commit/1b9b23a1)：默认开发构建补充 Web origin 和错误反馈
- [`95d42124`](https://github.com/The-AI-Republic/pi-dash/commit/95d42124)：避免 `undefined` 工作项链接
- [`3ce38e04`](https://github.com/The-AI-Republic/pi-dash/commit/3ce38e04)：在默认浏览器打开新窗口请求

#### 更新安装状态丢失

收起侧栏或切换路由后，更新按钮曾丢失“安装中”状态；从启动对话框接受更新时，也没有持续可见的安装状态。现在安装状态由原生层统一维护，重新挂载后仍能显示；失败后可重新尝试。

相关提交：

- [`ea223760`](https://github.com/The-AI-Republic/pi-dash/commit/ea223760)：跨组件重新挂载保留安装状态
- [`127299ec`](https://github.com/The-AI-Republic/pi-dash/commit/127299ec)：启动时接受更新后显示安装按钮状态

#### 设备登录码暴露在命令行参数中

桌面应用以前把设备码作为 CLI 参数传入，本机其他进程可能通过进程列表看到该值。现在改为通过标准输入传递，登录操作本身保持不变。

相关提交：[`77f299f7`](https://github.com/The-AI-Republic/pi-dash/commit/77f299f7)

#### 桌面开发和 Windows 安装兼容问题

- 正确认出 Tauri CLI 内部开发服务器来源，避免原生请求被当作不可信来源拒绝：[`2ce18684`](https://github.com/The-AI-Republic/pi-dash/commit/2ce18684)
- 对齐前端构建与原生传输的默认 API 地址，并在配置不一致时输出诊断：[`ecd7c525`](https://github.com/The-AI-Republic/pi-dash/commit/ecd7c525)
- MSIX 安装跳过运行时 `pidash://` 注册，避免更新后协议指向过期 WindowsApps 路径：[`923f8353`](https://github.com/The-AI-Republic/pi-dash/commit/923f8353)

### 4. Token 用量统计

AgentRun 和 RunnerLiveState 的 Token 用量改为统一的 `usage` JSON 结构，可以保存：

- 输入 Token
- 输出 Token
- 总 Token
- 缓存读取 Token
- 缓存写入 Token
- 推理 Token
- 提供商返回的原始用量对象

历史 API 中的扁平字段继续兼容。该改动同时修复了 Runner 只监听旧事件、没有识别 Codex 实际发送的 `thread/tokenUsage/updated`，导致 Codex 用量缺失的问题。继续同一线程时会扣除运行前基线，避免把前一次运行的 Token 重复计入。

相关提交：[`fb28d123`](https://github.com/The-AI-Republic/pi-dash/commit/fb28d123)

## 工程和发布质量改进

- 桌面 Rust CI 从仅 Linux 扩展到 Linux、Windows 和 macOS，并增加 Clippy 和 `--all-targets` 测试。相关提交：[`42ebb0cb`](https://github.com/The-AI-Republic/pi-dash/commit/42ebb0cb)
- `pidash-ipc` 发生变化时也会触发桌面和 Runner CI。相关提交：[`58c3ee13`](https://github.com/The-AI-Republic/pi-dash/commit/58c3ee13)
- 正式 Tauri 构建默认打包 release profile 的 Runner，避免发布包携带未优化的 debug Runner。相关提交：[`768937c6`](https://github.com/The-AI-Republic/pi-dash/commit/768937c6)
- `desktop-daemon.log` 同时捕获守护进程的 stdout 和 stderr，保留实际 tracing 输出。相关提交：[`e322daa9`](https://github.com/The-AI-Republic/pi-dash/commit/e322daa9)
- 发布 URL 校验不再被 `bake-info.txt` 中的元数据误通过，必须在实际前端资源中找到 API 和 Web origin。相关提交：[`05dea5f5`](https://github.com/The-AI-Republic/pi-dash/commit/05dea5f5)
- 构建结束或失败时清理下载智能体引擎产生的临时目录。相关提交：[`bb992e7b`](https://github.com/The-AI-Republic/pi-dash/commit/bb992e7b)
- `admin`、`live`、`space` 镜像补充 pnpm 全局可执行文件路径，兼容新版 pnpm。相关提交：[`ce1ba019`](https://github.com/The-AI-Republic/pi-dash/commit/ce1ba019)

## 已知限制和使用边界

### 页面并发写入

项目页面的 API 写入会在当前文档状态上生成新的 HTML、JSON 和 Yjs binary，但仍存在一个明确的并发限制：如果某位用户正在浏览器编辑器中打开该页面，实时服务内存中的文档可能在下一次保存时覆盖并发 API 写入。

因此，本周期交付了页面 API/CLI 读写能力及锁定状态检查，但尚未彻底解决“在线编辑器与 API 同时修改同一页面”的冲突。

### 等待依赖不会立即唤醒

`pidash issue wait` 会返还等待运行所消耗的预算，但不会注册依赖完成事件。依赖项完成后，等待中的工作项将在下一次定时执行时重新检查，而不是立即运行。

### 调度器公开接口仅支持读取

CLI、公开 API 和 MCP 连接器目前可以查询调度器和运行记录，但不能通过这些接口创建、修改、启停或立即触发调度器。

### 图片评论范围

当前仓库中已经核实的是 CLI 和公开 API 的图片评论能力。不能据此认定托管 MCP 评论工具也已经支持图片上传。

### 智能体自动安装范围

Runner 自动安装目前覆盖 Codex、Claude Code 和 Cursor Agent。OpenClaw、Grok 和 Muse Code 仍使用安装页面引导，安装后也仍需用户完成智能体自身的登录。

## 版本变化

| 组件            | 周期开始 | 周期结束 |
| --------------- | -------- | -------- |
| Pi Dash Runner  | `0.1.23` | `0.1.28` |
| Pi Dash Desktop | `0.3.1`  | `0.3.3`  |

## 审阅说明

本文基于 Git 历史、提交说明和实际代码差异进行只读审阅。为避免遗漏，本报告按变更进入 `main` 的时间统计，而不是仅按开发提交的作者时间统计。本文没有把仓库中已有的测试视为本次重新执行通过；整理过程中未运行项目测试，也未修改业务代码。
