<p align="center">
  <img src="apps/web/public/branding/logo.svg" alt="ErGent Logo" width="160" height="160" />
</p>

# Ergent

**版本：v1.2.0**

浏览器里的个人开发工作台：Agent 主动连接 Server，在网页中按设备管理真实 Shell 终端。
A personal development workspace in your browser: Agents connect to Server and expose real shell terminals by device.

- 多设备、终端卡片、断线重连与单写入者接管
- 在终端运行已安装的 Codex、Claude Code、OpenCode 等 CLI
- 支持终端重命名、配色、明暗主题和中英文界面
- Windows、Linux、macOS 均提供 x86_64 / ARM64 发布包

## 快速开始 / Quick start

### 原生发布包 / Native packages

从 [Releases](https://github.com/erquren/ErGent/releases) 下载对应平台的包并解压。Server、Agent、网页和启动脚本均已包含，无需构建工具；AI CLI 另装在 Agent 机器上。
Download and extract the archive for your platform. Server, Agent, web UI and launch scripts are included; install AI CLIs separately on the Agent machine.

进入解压目录，推荐使用启动脚本（Windows 使用 `./start-server.cmd`）：
From the extracted directory, use the launch script:

```sh
./start-server.sh --tls-mode dev-http
```

本机打开 **http://127.0.0.1:7777**。脚本会加载包内 `web/`；若直接运行二进制，使用 `./ergent-server run --web "$PWD/web" --tls-mode dev-http`。数据库默认是包内 `.local/server.db`；已使用自定义路径时继续使用同一文件。
Open the exact loopback URL above. The script selects the bundled UI; a direct binary launch needs `--web "$PWD/web"`. Keep the same database path for startup and administration.

远程部署先选[同机 Nginx 代理、原生 HTTPS 或本机/SSH 测试](scripts/release/README.md#选择部署方式--choose-a-deployment-mode)。`--origin` 必须匹配浏览器地址，**不会启用 TLS**；`localhost` 与 `127.0.0.1` 不可混用，非默认端口须写明。
For remote access, choose the linked Nginx, direct TLS or local/SSH instructions first. **Origin is not TLS configuration**; the browser's scheme, hostname and port must match exactly.

首次用控制终端显示的初始化码创建账户；远程明文 HTTP 首次初始化会被拒绝。后台运行时，用同一数据库执行 `setup-code`，或用 `admin-create` 隐藏输入两次密码、分别按 Enter。网页登录后创建 Agent 令牌，`enroll` 隐藏输入一次令牌、按 Enter，无需 Ctrl-D/Ctrl-Z。完整命令见[发布包使用说明](scripts/release/README.md)。
Create the first account with the terminal's setup code over HTTPS or native loopback/SSH access. `admin-create` asks for password + Enter twice; Agent `enroll` asks for token + Enter once. Neither interactive command needs Ctrl-D/Ctrl-Z. See the package guide for exact commands and existing-database handling.

### Docker 部署 / Docker deployment

单容器包含 Server 和网页，Agent 仍在开发机器原生运行。可挂载已有证书使用原生 HTTPS，或复用已有 Nginx；数据持久化，证书只读挂载，不自动申请/续期。
One container runs Server and the web UI; Agents stay native on development machines. Choose direct TLS with existing certificates or your existing HTTPS proxy. Data is persistent; certificate issuance/renewal remains external.

先从源码构建 `ergent-server:local`；只有确认镜像已发布后才使用拉取方式。配置、初始化与升级见 [Docker 部署指南](docs/docker.md)。
Build `ergent-server:local` from source until a published image is confirmed available. See the Docker guide for configuration, account setup and upgrades.

### 从源码运行

需要 Rust 1.98.1、Node.js 22、pnpm 10.17.1 和 Python 3。

```sh
sh scripts/cargo.sh build --workspace --locked
sh scripts/pnpm.sh install --frozen-lockfile
sh scripts/pnpm.sh build
python3 scripts/dev.py
```

打开 **http://127.0.0.1:7777**，使用 `.local/dev/login.json` 中自动生成的账号和密码登录。点击设备旁的 `+`，选择本机目录即可创建终端。数据和凭据保存在 `.local/dev/`，请勿提交到版本控制。

## Codex 状态提示

更新 Agent 后新建终端，运行：

```sh
"$ERGENT_AGENT_BIN" setup-codex
"$ERGENT_AGENT_BIN" codex
```

在 Codex `/hooks` 中审阅并信任 hooks 后，网页可显示处理中、等待确认、完成和退出等状态。目前仅提供 Codex 状态适配器。

## 结束终端的状态

- `POST /api/v1/sessions/{id}/close` 返回 HTTP 202 和操作 ID，只表示请求已排队。使用同一 `Idempotency-Key` 重试会返回同一操作，不会再次提交。
- 查询 `/api/v1/operations/{id}`：`pending` 表示尚未确认；`accepted` 表示 Agent 已接受终止信号，或进程已经退出。接受关闭后会话为 `terminating`，网页显示“正在结束”，禁止继续输入或重复结束。
- `accepted` 不代表进程已退出。只有 Agent 的进程等待线程确认退出后，会话才变成 `exited` 并带上退出码；会话列表/推送是后续状态来源。即使退出事件先于关闭回复到达，也不会被旧回复改回运行中。
- 发信号失败时操作为 `failed`，会话保持原状态；连接中断、超时或旧 Agent 只回复 `running` 时可能为 `unknown`，应检查会话状态。Agent 重启后无法确认的未退出会话，或进程等待失败的会话为 `lost`。Server、Agent 和网页应一起升级。
- 对 `terminating` 或 `exited` 会话再次结束是幂等操作；仍在结束的进程继续计入活动会话上限。关闭请求不会等待进程退出，也不会阻塞 Agent 的连接循环。

## 安全与使用边界

- 终端以 Agent 当前用户的权限运行。不自动安装或登录 AI CLI。
- 公网部署使用原生 HTTPS（配置现有证书/私钥）或 HTTPS 反向代理，并设置正确的 `--origin`；跨机器 Agent 连接必须使用 HTTPS。不要在公网启用 `--allow-any-origin`。
- 登录按来源 IP 使用时间窗口和指数退避：默认 15 分钟内第 5 次失败后等待 30 秒，继续失败逐次翻倍，单次最多 15 分钟。成功登录清零；重启保留尚未到期的限制。可用 `ergent-server --database .local/server.db login-unblock IP --actor 操作人 --reason 原因` 解锁并记录审计。配置、升级和审计查询见 [发布包使用说明](scripts/release/README.md#登录保护与审计)。代理部署需配置 `--trusted-proxy`，并由代理覆盖 `X-Real-IP`。
- 关闭网页不结束终端；Agent 或机器重启后不能恢复原进程。终端浏览器保留最多 2,000 行滚动历史；短暂断线可从每会话 512 KiB 内存重放缓存继续，超出缓存或尺寸/Agent 变化时只恢复当前画面。完整历史不持久化，详见 [终端恢复与保留边界](TERMINAL_RECOVERY.md)。更新前请保存工作并备份数据。
- 发布包为可解压运行的程序，未配置 Windows 签名或 macOS 签名、公证。平台要求和部署细节见 [发布包使用说明](scripts/release/README.md)。

## 历史会话与协议升级

历史会话元数据长期保留；已退出终端释放 PTY 和解析器资源，不占用 20 个并发终端名额。Hello 与会话列表按 64KB 消息上限分块同步，完整接收后才替换列表。此版本使用 `ergent.preview.v3`，Server、Agent 和网页需一起升级；旧协议明确拒绝连接，不截断历史列表。详见 [历史保留与同步协议](INVENTORY_PROTOCOL.md)。

### 升级到 1.2.0

先保存终端中的工作，停止 Server/Agent，备份实际使用的数据库、Agent 配置和部署配置，再一起替换 Server、Agent、网页和启动脚本。停止 Agent 会结束其终端进程；不要让新旧 Server 同时访问同一数据库。首次启动会执行 `0005_first_account_setup.sql`，已有账户继续直接登录，网页首次初始化会永久关闭；空数据库通过一次性初始化码或 `admin-create` 创建首个账户。回退时使用旧程序和升级前同一时点的数据库备份。

本版新增原生 HTTPS、Agent 私有 CA 支持及 Docker Server 部署。跨机器连接使用 HTTPS；反向代理部署选择 `--tls-mode proxy`，直接提供 HTTPS 时选择 `--tls-mode direct` 并配置证书和私钥；私有 CA 在 Agent 注册时通过 `--ca-cert` 指定。本机明文测试使用 `--tls-mode dev-http`。启动、初始化和证书配置见[发布包使用说明](scripts/release/README.md)，容器部署见 [Docker 部署指南](docs/docker.md)。

### 升级到 1.1.0（历史说明）

请先保存终端中的工作，停止所有 Server/Agent，并备份 `.local/`（使用自定义数据库或配置路径时也需备份对应文件）。Server、Agent 和网页须一起升级到使用 `ergent.preview.v3` 的版本；不要让新旧 Server 同时访问同一数据库。首次升级会将旧登录失败计数（包括永久封禁）清零并记录 `legacy_reset` 审计；回退旧版本需恢复升级前备份。详见[更新与平台要求](scripts/release/README.md#更新与平台要求)和[登录保护与审计](scripts/release/README.md#登录保护与审计)。

## 开发与打包

```sh
sh scripts/cargo.sh fmt --all --check
sh scripts/cargo.sh clippy --workspace --all-targets --locked -- -D warnings
sh scripts/cargo.sh test --workspace --locked
sh scripts/cargo.sh run -p ergent-protocol --example export --locked
sh scripts/pnpm.sh build
sh scripts/pnpm.sh exec playwright install chromium
sh scripts/pnpm.sh test:e2e
python3 scripts/package.py --expect-version v1.2.0
```

本机打包当前平台，打包前会检查 Server、Agent、网页及指定标签的版本一致性。

### CI/CD 与发布

- 分支推送和 Pull Request 运行 `Check`，包含发布工具测试、Rust 格式/Clippy/测试、协议导出一致性及网页构建/E2E；Pull Request 另外运行六平台 `Package validation` 和双架构 `Docker validation`。
- 原生包发布使用小写 `v1.2.0` 标签，并确保标签指向包含上述版本和文档修改的提交；大写 `V1.2.0` 不会触发 Release。`Release` 先调用同一套 `Check`，全部通过后才构建、校验并汇总六个平台的包及 `SHA256SUMS`。
- Release 不存在时创建含校验和的草稿，维护者检查附件和发布说明后手动公开；已存在时只补传缺失附件，同名附件须校验一致，不覆盖已有文件。在分支上手动运行 `Release` 只生成 Actions 构建产物；在版本标签上手动运行也会执行 Release 附件上传。
- Docker 镜像通过独立的 `Docker image (manual)` 手动发布，默认只验证不上传；推送 `v1.2.0` 标签或公开原生包 Release 均不会自动发布镜像。镜像仅使用 `sha-<完整提交SHA>` 标签，不生成 `1.2.0` 或 `latest` 标签，具体见 [Docker 发布说明](docs/docker.md#可选手动镜像发布维护者)。

### 名称规则

设备注册/改名与终端创建/改名统一接受去除首尾 Unicode 空白后的 1–120 个 Unicode 标量值（按字符计数，不按 UTF-8 字节或 UTF-16 码元；组合字形可能由多个标量值组成）。中文与 emoji 使用相同上限；所有位置的控制字符均拒绝，包括首尾换行与制表符。网页和服务端均校验，名称不会被静默截断。
