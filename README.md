# Ergent

浏览器里的个人开发工作台：设备主动连接 Server，网页按机器和终端卡片操作真实 Shell。

当前是 **0.1 开发预览**。已跑通 React / xterm.js → Rust Server → Rust Agent → 本机 PTY

## 使用文档

- **[安装与部署教程](docs/deployment.md)**：发布包安装、本机体验、Linux Server + HTTPS、三平台 Agent、后台运行、手机使用、升级备份与排错。
- **[源码编译教程](docs/building.md)**：安装工具链、只编译 Agent、编译 Server/Web、本机打包与测试。
- **[全平台发布流程](docs/releasing.md)**：GitHub Actions 六平台构建、校验和与 Release 草稿。

第一次使用建议从部署教程开始。已经拿到发布包的用户无需安装编译工具。

## Codex 状态提示

支持处理中、等待确认、本轮完成、中断和工具退出状态，完成时在网页弹出可点击提示。更新 Agent 后新建终端，运行 `"$ERGENT_AGENT_BIN" setup-codex`，再运行 `"$ERGENT_AGENT_BIN" codex`，在 Codex `/hooks` 中审阅并信任 hooks。Windows 命令、通用事件协议与限制见 [工具状态接入](docs/tool-events.md)。

接口与工具无关，后续可通过适配器接入 Claude Code / OpenCode；目前只有 Codex 适配器。

## 本机快速启动

需要 Rust 1.98.1、Node.js 22、pnpm 10.17.1 和 Python 3。当前开发环境的项目内工具链可通过下面的包装脚本调用；其他环境会使用 PATH 中的 cargo/pnpm。

```sh
sh scripts/cargo.sh build --workspace --locked
sh scripts/pnpm.sh install --frozen-lockfile
sh scripts/pnpm.sh build
python3 scripts/dev.py
```

打开 **http://127.0.0.1:7777**。启动脚本自动创建独立的本地数据库、随机管理员密码，并注册本机 Agent。

- 账号与随机密码：`.local/dev/login.json`，不会在启动日志打印密码。
- 数据与 Agent 凭据：`.local/dev/`，不应提交到版本控制。
- 网页左侧点击设备旁的 `+`，选择本机已有目录，即可创建真实终端。
- 在终端中运行本机已经安装的 `codex`、`claude`、`opencode` 等程序。
- 关网页只断开观看；停止开发脚本会停止 Server 和 Agent，并结束该 Agent 管理的终端。

不自动安装、登录或启动任何 AI CLI。Shell 环境与以当前用户运行 Agent 时的环境一致。

局域网访问（将 IP 换为运行服务的电脑的局域网地址）：

```sh
python3 scripts/dev.py --lan 192.168.2.174
```

同一局域网的设备打开 `http://192.168.2.174:7777`，账号密码仍在 `.local/dev/login.json`。该模式监听 `0.0.0.0`，只接受指定局域网 origin 和原来的 loopback origin；本机 Agent 继续通过 loopback 连接。局域网 IP 变化后需更新启动参数。

需要同时通过局域网、Tailscale IP 或机器名访问开发服务时，可显式允许任意来源：

```sh
python3 scripts/dev.py --lan 192.168.2.174 --allow-any-origin
```

该选项监听 `0.0.0.0:7777`，放开 HTTP 与 WebSocket 的 Origin 校验，仍需要账号登录，HTTP 写接口仍校验 CSRF；终端 WebSocket 没有额外的 CSRF token 校验，因此不要把此选项用于公网部署。默认不启用。如果已有独立 Agent 正在运行，追加 `--server-only` 可只启动/停止网页服务。

## 分别运行 Server 与 Agent

先创建管理员。命令从 stdin 读密码，至少 12 字节；可以从密码管理器管道输入，避免密码出现在命令参数中。

```sh
target/debug/ergent-server --database .local/server.db admin-create --username admin
target/debug/ergent-server --database .local/server.db run \
  --listen 127.0.0.1:7777 --origin http://127.0.0.1:7777 \
  --web apps/web/dist
```

网页点击“接入设备”获取一次性 token。在目标机器上运行对应平台编译出的 Agent：

```sh
ergent-agent --config /path/to/agent.json enroll --server https://dev.example.com --name "My machine"
# 从 stdin 输入网页的一次性 token，结束输入后完成注册。
ergent-agent --config /path/to/agent.json run
ergent-agent --config /path/to/agent.json doctor
```

Windows 使用 `.exe` 和本机路径。Server 公网 origin 只接受 HTTPS；Agent 只对 loopback 允许明文 HTTP。Agent 不监听公网端口。完整公网发布仍需完成下文列出的后续工作。

## 当前已有

| 能力 | 状态 |
| --- | --- |
| 管理员登录、Argon2id、SQLite Cookie 会话、CSRF/Origin | 已实现 |
| 一次性设备注册、设备凭据、设备撤销 API | 已实现 |
| 多设备列表、终端卡片、新建与结束真实 Shell | 已实现 |
| 浏览器刷新、切换卡片、Server 重启后回到原 Shell | 已端到端验证 |
| Agent 断线重连，进程生命周期独立于网络连接 | 已实现 |
| 单写入者租约，多窗口显式接管 | 已端到端验证 |
| 创建请求幂等、资源归属校验、有界输出队列 | 已实现基础版本 |
| Rust → TypeScript 类型生成、v1 二进制帧编解码 | 已实现；实际预览链路仍使用 JSON/Base64 |
| Windows / Linux / macOS CI 配置 | 已提供；macOS ARM64 已验证，Linux ARM64 Agent 已实机接入 |

## 已知边界

- 网络协议名为 `ergent.preview.v1`，不是 [最终接口契约](docs/api-contract.md) 的完整实现。
- 恢复的是可见终端屏幕；断线期间的全部滚动历史不持久化。vt100 屏幕引擎已处理常见 `1049` 备用屏幕切换，但仍需验证所有目标 CLI 的模式、滚动区域与半截控制序列恢复。
- Agent/机器重启后旧会话为 lost；不恢复原进程。Server 重启后的结果不明操作不会自动重发。
- 当前队列过载时断开并重新取快照；最终协议的逐浏览器消费 ACK、增量回放和完整内存预算仍待实现。
- Windows 原生 ConPTY 来自 portable-pty；Windows Job Object、凭据 ACL、Windows 后台任务和全平台安装包尚未完整验收。Linux systemd Agent 已实机验证，其他后台运行示例见部署教程。
- UI 当前提供登录、工作台、注册、新建与结束、设备重命名；已退出/失效终端不再显示，历史元数据仍保留用于操作对账。设置、撤销管理页还未提供。
- 生产审计、限额全面覆盖、迁移版本管理、凭据轮换、OpenAPI 与备份恢复演练仍在计划中。HTTPS 部署模板已提供，但尚未完成本项目公网部署验收。

## 开发检查

```sh
sh scripts/cargo.sh fmt --all --check
sh scripts/cargo.sh clippy --workspace --all-targets --locked -- -D warnings
sh scripts/cargo.sh test --workspace --locked
sh scripts/cargo.sh run -p ergent-protocol --example export --locked
sh scripts/pnpm.sh build
sh scripts/pnpm.sh exec playwright install chromium
sh scripts/pnpm.sh test:e2e
```

端到端测试使用临时数据库、临时设备配置和真实 PTY，监听 loopback `17779`。测试结束会关闭所启动的进程；不连接真实公网服务。Rust 测试中的 Unix PTY 用例在 Windows 上跳过，不能因此认为 Windows 已完成实机验收。

## 项目结构

```text
apps/web            React 工作台与 xterm.js
apps/server         登录、SQLite、HTTP API、WebSocket 路由
apps/agent          注册、单实例锁、回连、终端指令处理
crates/protocol     共享 DTO、预览消息、正式帧编码器
crates/terminal     PTY、屏幕快照、写入租约、会话生命周期
packages/protocol-ts 由 Rust 生成的 TypeScript
tests/e2e           真实服务与浏览器集成测试
scripts             本地工具包装与启动脚本
docs                设计、接口、排期、实际实现状态
```

参见 [实际实现状态](docs/implementation-status.md)、[模块与排期](docs/implementation-roadmap.md)、[接口设计](docs/api-contract.md)、[库选型](docs/dependencies.md)。

## 全平台打包与发布

Windows、Linux、macOS 均提供 x86_64 和 ARM64 目标，每个包包含 Server、Agent 与 Web。
本机运行 `python3 scripts/package.py` 打包当前平台；GitHub Actions 的 Release 工作流构建全部六个平台。
推送版本标签后生成全平台 Release 草稿，验收后发布。详见 [发布说明](docs/releasing.md)。

## 登录 IP 封禁

同一来源 IP **累计 3 次账号或密码错误**后永久禁止新的登录，成功登录不会清零此前失败次数。
错误账号、错误密码和已封禁 IP 都返回 HTTP 401、`INVALID_CREDENTIALS`、`账号或密码错误`，不返回封禁原因或重试时间。
计数保存在 SQLite，重启仍生效；封禁仅阻止新登录，不主动踢掉已有会话。
Origin/CSRF 不合法的请求仍按原规则拒绝，不计为密码失败。共享出口 IP 的设备共用计数。

管理员可在服务器本机解封（使用运行服务所用的数据库路径）：

```sh
./ergent-server --database .local/server.db login-unblock 203.0.113.10
```

直接连接默认使用 TCP 对端 IP，忽略客户端传来的转发头。
HTTPS 反向代理部署时，给 Server 添加 `--trusted-proxy 127.0.0.1`（可重复，指定实际代理对端 IP），
并让代理**覆盖** `X-Real-IP` 为它实际观察到的客户端 IP。例如单层 Nginx 代理的 location 中设置：

```nginx
proxy_set_header X-Real-IP $remote_addr;
```

仅显式信任的代理允许提供这个头；缺失、重复或无效的 IP 会拒绝登录。
不读取 X-Forwarded-For / Forwarded，也不自动信任内网地址。多层代理需在边缘正确解析可信链，再由最后一跳覆盖 X-Real-IP。
生产中限制后端只能由代理访问，并使用 `--origin https://你的域名`；不要继续使用开发选项 `--allow-any-origin`。

### 终端名称、配色与网页外观

- 打开终端，点击右上角「重命名」修改显示名称；不会重启 Shell。
- 「终端配色」可选择跟随网页、经典黑、经典白、Dracula、Nord、Solarized 浅色。名称和配色保存在服务端，跨浏览器同步，设备重连不会覆盖。
- 顶部外观图标默认跟随系统，也可手动选择白天/黑夜，偏好保存在当前浏览器。跟随网页的终端在黑夜模式下黑底白字、白天模式下白底黑字，ANSI 基础色随之调整。程序自行输出的真彩色/背景仍由程序控制。
- 此功能只需更新 Server/Web，已有 Agent 无需重启。详见 [终端偏好接口](docs/terminal-preferences.md)。

### 品牌图标

原稿为仓库根目录的 `ErGent_logo_mark.svg` 和 `ErGent_wordmark.svg`。网页资源位于 `apps/web/public/`，包含裁切留白后的 SVG、高清透明英文标识、16/32/48 像素 ICO、180 像素 Apple touch icon、192/512 像素图标及 maskable 图标。后续修改原稿后运行 `node scripts/generate-icons.cjs`，再运行 `sh scripts/pnpm.sh build`。生成工具使用项目已有的 Playwright/Chromium；英文标识输出为 PNG，以固定生成机器上的字体渲染结果。普通构建直接使用已生成资源，无需安装设计字体。

### 界面语言

目前支持简体中文和英文。默认按设备首选语言自动选择：`zh-*` 显示中文，其余显示英文。登录页和工作台右上角的语言图标可选择「跟随设备 / 简体中文 / English」，偏好保存在当前浏览器，并同步到同一站点的其他标签页。切换语言不重连或重建终端；终端输出、命令、路径及自定义名称不翻译。界面词库位于 `apps/web/src/messages.ts`。
