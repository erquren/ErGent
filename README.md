<p align="center">
  <img src="apps/web/public/branding/logo.svg" alt="ErGent Logo" width="160" height="160" />
</p>

# Ergent

**版本：v1.1.0**

浏览器里的个人开发工作台：Agent 主动连接 Server，在网页中按设备管理真实 Shell 终端。

- 多设备、终端卡片、断线重连与单写入者接管
- 在终端运行已安装的 Codex、Claude Code、OpenCode 等 CLI
- 支持终端重命名、配色、明暗主题和中英文界面
- Windows、Linux、macOS 均提供 x86_64 / ARM64 发布包

## 快速开始

### 使用发布包

从 [Releases](https://github.com/erquren/ErGent/releases) 下载对应平台的压缩包。每个包包含 Server、Agent、网页和启动脚本，无需安装 Rust、Node.js、pnpm 或 Python。

解压后按包内 README 创建管理员、启动 Server，再从网页获取一次性令牌接入 Agent。详见 [发布包使用说明](scripts/release/README.md)。AI CLI 需自行安装在 Agent 所在机器。

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
- 公网部署使用 HTTPS 反向代理，并设置正确的 `--origin`；跨机器 Agent 连接必须使用 HTTPS。不要在公网启用 `--allow-any-origin`。
- 登录按来源 IP 使用时间窗口和指数退避：默认 15 分钟内第 5 次失败后等待 30 秒，继续失败逐次翻倍，单次最多 15 分钟。成功登录清零；重启保留尚未到期的限制。可用 `ergent-server --database .local/server.db login-unblock IP --actor 操作人 --reason 原因` 解锁并记录审计。配置、升级和审计查询见 [发布包使用说明](scripts/release/README.md#登录保护与审计)。代理部署需配置 `--trusted-proxy`，并由代理覆盖 `X-Real-IP`。
- 关闭网页不结束终端；Agent 或机器重启后不能恢复原进程。终端浏览器保留最多 2,000 行滚动历史；短暂断线可从每会话 512 KiB 内存重放缓存继续，超出缓存或尺寸/Agent 变化时只恢复当前画面。完整历史不持久化，详见 [终端恢复与保留边界](TERMINAL_RECOVERY.md)。更新前请保存工作并备份数据。
- 发布包为可解压运行的程序，未配置 Windows 签名或 macOS 签名、公证。平台要求和部署细节见 [发布包使用说明](scripts/release/README.md)。

## 历史会话与协议升级

历史会话元数据长期保留；已退出终端释放 PTY 和解析器资源，不占用 20 个并发终端名额。Hello 与会话列表按 64KB 消息上限分块同步，完整接收后才替换列表。此版本使用 `ergent.preview.v3`，Server、Agent 和网页需一起升级；旧协议明确拒绝连接，不截断历史列表。详见 [历史保留与同步协议](INVENTORY_PROTOCOL.md)。

### 升级到 1.1.0

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
python3 scripts/package.py --expect-version v1.1.0
```

本机打包当前平台。推送与项目版本一致的 `v*` 标签后，GitHub Actions 构建并校验全部六个平台。Release 不存在时创建含校验和的草稿；已存在时只补传缺失附件，同名附件须校验一致，不覆盖已有文件。

### 名称规则

设备注册/改名与终端创建/改名统一接受去除首尾 Unicode 空白后的 1–120 个 Unicode 标量值（按字符计数，不按 UTF-8 字节或 UTF-16 码元；组合字形可能由多个标量值组成）。中文与 emoji 使用相同上限；所有位置的控制字符均拒绝，包括首尾换行与制表符。网页和服务端均校验，名称不会被静默截断。
