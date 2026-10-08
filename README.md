<p align="center">
  <img src="apps/web/public/branding/logo.svg" alt="ErGent Logo" width="160" height="160" />
</p>

# Ergent

**版本：v1.0.0**

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

## 安全与使用边界

- 终端以 Agent 当前用户的权限运行。不自动安装或登录 AI CLI。
- 公网部署使用 HTTPS 反向代理，并设置正确的 `--origin`；跨机器 Agent 连接必须使用 HTTPS。不要在公网启用 `--allow-any-origin`。
- 同一来源 IP 累计 3 次登录失败后会永久封禁，成功登录或重启不会清零。可用 `ergent-server --database .local/server.db login-unblock IP` 解封。代理部署需配置 `--trusted-proxy`，并由代理覆盖 `X-Real-IP`。
- 关闭网页不结束终端；Agent 或机器重启后不能恢复原进程。断线期间的完整滚动历史不持久化，更新前请保存工作并备份数据。
- 发布包为可解压运行的程序，未配置 Windows 签名或 macOS 签名、公证。平台要求和部署细节见 [发布包使用说明](scripts/release/README.md)。

## 开发与打包

```sh
sh scripts/cargo.sh fmt --all --check
sh scripts/cargo.sh clippy --workspace --all-targets --locked -- -D warnings
sh scripts/cargo.sh test --workspace --locked
sh scripts/cargo.sh run -p ergent-protocol --example export --locked
sh scripts/pnpm.sh build
sh scripts/pnpm.sh exec playwright install chromium
sh scripts/pnpm.sh test:e2e
python3 scripts/package.py --expect-version v1.0.0
```

本机打包当前平台。推送与项目版本一致的 `v*` 标签后，GitHub Actions 构建并校验全部六个平台，成功后生成含校验和的 Release 草稿。
