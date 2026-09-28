# karma-team 改名与迁移说明

## 名称约定

| 用途                     | 名称                 |
| ------------------------ | -------------------- |
| 项目、界面标题、CLI 命令 | `karma-team`         |
| 前端工作区包             | `@karma-team/*`      |
| Python 包、Rust 库和模块 | `karma_team`         |
| 环境变量前缀             | `KARMA_TEAM_*`       |
| 共享 IPC 包              | `karma-team-ipc`     |
| 桌面程序                 | `karma-team-desktop` |
| 桌面深链接               | `karma-team://`      |

Python 和 Rust 标识符不能包含连字符，因此代码模块使用下划线。原作者的版权署名和许可证继续保留。

## 新环境

使用仓库中的 `.env.example` 和 `setup.sh` 初始化配置，然后运行：

```bash
pnpm install --frozen-lockfile
pnpm build
docker compose -f docker-compose-local.yml up
pnpm dev
```

CLI 可直接从源码构建，安装后的命令为 `karma-team`：

```bash
cargo install --path runner --locked
karma-team auth login --url https://your-instance.example.com
karma-team runner add --project <project-id>
```

## 已有部署

1. 备份数据库、上传文件、环境配置及 CLI 配置。在切换前停止接收新任务，等待已有任务和 Celery 队列处理完成；任务的 Python 模块路径已改变。
2. 将实际配置迁入新目录。数据库账户、密码、库名、存储桶及已有 Docker 数据卷应沿用实际资源的值；修改配置文件中的名称不会自动迁移这些资源。示例配置中的新名称适用于新建环境。
3. 若继续使用原有 Compose 数据卷，使用 `docker compose --project-name <原项目名> ...` 保持项目标识，并核对服务主机名。服务名已改为 `karma-team-db`、`karma-team-redis`、`karma-team-mq`、`karma-team-minio`。确认挂载的是原数据卷后再启动应用。
4. 同步更新进程管理器、容器入口、CI 配置中的 Python 路径及 `KARMA_TEAM_*` 环境变量，然后正常运行数据库迁移。Django 子应用标签及业务数据表名称保持一致。
5. 检查数据库中自定义提示词、提示词片段覆盖和定时任务中保存的命令、模块路径。内置 CLI 片段现在使用 `karma-team-cli`；按需更新自定义模板，并使用已有的模板重建命令更新内置模板。
6. 先用原 CLI 停止并卸载原守护进程服务，再安装新版 CLI、重新登录并安装新服务；Linux 默认配置目录为 `~/.config/karma-team/`。迁移已有配置时，保留令牌文件权限，检查工作目录中的 `.karma-team/` 路径。
7. 桌面应用标识、深链接协议及浏览器缓存键已更新。重新安装桌面应用并登录，在身份认证提供方同步登记新的回调协议。

## 外部发布

仓库引用以 `github.com/xerecter/karma-team` 为目标，镜像、CLI 安装包及发布标签也已改名。GitHub 仓库、镜像仓库、域名和已发布的安装包需要在对应平台实际创建或迁移；修改源码不会执行这些外部操作。文档中的 `karma-team.example.com` 仅为示例地址。

验证源码改名时，可运行 `pnpm exec turbo run build check:types test check:lint`、`cargo test --workspace --all-targets --locked`，以及 `apps/api` 下的 `pytest`。测试应连接独立的测试数据库。
