# Ergent 发布包

本包包含 Server、Agent 和网页资源。同一包可以只运行 Server 或只运行 Agent。
运行不需要 Rust、Node.js、pnpm 或 Python；AI CLI（Codex / Claude Code / OpenCode）需要自行安装在 Agent 所在机器。

## 启动 Server

先进入解压目录。Linux/macOS 使用 `./ergent-server`；Windows PowerShell 使用 `./ergent-server.exe`。

```sh
./ergent-server admin-create --username admin
```

从标准输入输入密码（至少 12 字节）并按回车，然后结束输入：Unix Ctrl-D；Windows Ctrl-Z 后回车。
密码输入没有交互式隐藏提示，请在私人终端操作。

Linux/macOS：`./start-server.sh`。Windows：`./start-server.cmd`。
默认浏览器访问 http://127.0.0.1:7777，用刚创建的账号登录。
启动脚本切换到包目录，网页读取 `web/`，数据写入包目录的 `.local/`。

局域网示例（把 IP 换成实际地址；Windows 换为 .cmd）：

```sh
./start-server.sh --listen 0.0.0.0:7777 --origin http://192.168.1.10:7777 --allow-insecure-lan
```

多个浏览器入口用 `--additional-origin URL` 逐个添加。
`--allow-any-origin` 是显式的开发选项，发布包默认不开启；Tailscale 的 HTTP 浏览器测试可显式使用此选项。
公网部署在 HTTPS 反向代理后，并设置 `--origin https://你的域名`，代理需支持 WebSocket。

## 启动 Agent

网页登录后创建一次性设备注册令牌。进入包目录执行（Windows 使用 .exe）：

```sh
./ergent-agent enroll --server https://你的域名 --name My-PC
./ergent-agent run
```

enroll 从标准输入读取令牌，输入后结束输入，方式同上。
在同一台机器测试可使用 `--server http://127.0.0.1:7777`。
当前 Agent 仅允许 loopback HTTP；跨机器连接（包括 Tailscale）使用 HTTPS。
Agent 配置默认存放 `.local/agent.json`；可在 enroll/run 前使用 `--config 路径` 指定同一文件。
后台服务安装需自行配置；关闭浏览器不结束终端，Agent 或操作系统重启不能恢复原进程。

## 更新与平台要求

先停止 Server/Agent，备份 `.local/`；替换二进制、web 和启动脚本，保留原数据。
停止 Agent 会结束它管理的终端进程，更新前请保存工作。
发布包不含密码、令牌、数据库、AI CLI 或开发环境。
manifest.json 记录版本、Git commit、工作区是否有未提交修改及文件校验值；外部 .sha256 校验压缩包。

Linux GNU 包以 Ubuntu 22.04 构建（glibc 2.35+），需系统 CA 证书；不支持直接运行于 Alpine/musl。
macOS 构建最低部署目标为 11.0，尚未完成旧版系统实机验收。
Windows 使用 MSVC，需支持 ConPTY 的现代 Windows（Windows 10 1809+ / Windows 11），可能需要匹配架构的 Visual C++ Runtime。
当前提供可解压运行的预览包，不是安装器；未配置 Windows 签名、macOS 签名或公证。

## 登录保护与审计

登录失败按规范化后的来源 IP 计数，同一出口的用户共用限制。默认从首次失败开始计算 15 分钟固定窗口，第 5 次失败触发 30 秒退避；等待后若在同一窗口继续失败，退避逐次翻倍（60、120、240……秒），单次上限为 15 分钟。

- 窗口到期后旧计数失效；已经发出的退避仍等待至原截止时间，之后开启新窗口。
- 退避期间的请求返回 HTTP 429、`LOGIN_THROTTLED` 和秒数形式的 `Retry-After`，不会增加计数或延长截止时间。退避结束前，正确密码也不能绕过限制。
- 未触发退避的错误凭据返回 HTTP 401、`INVALID_CREDENTIALS`；成功登录签发会话时原子清零该 IP 的失败状态。
- 状态写入数据库，重启不会重置尚未到期的窗口或退避；过期状态在后续登录时清理。
- 同一 IP 的并发请求串行处理；SQLite 写事务也让多个进程和命令行解锁保持一致。密码校验期间持有写事务，会短暂阻塞其他数据库写入；读取和现有终端数据流不受这把写锁影响。

启动时可配置以下参数；启动脚本会透传给 `run`：

```sh
./start-server.sh --login-window-seconds 900 --login-failure-threshold 5 \
  --login-backoff-seconds 30 --login-max-backoff-seconds 900
```

所有参数必须为正数，时间参数最多 86400 秒，阈值最多 1000000；初始退避不能大于最大退避。更改参数不会缩短已经记录的退避截止时间。共用数据库的 Server 应采用相同设置。

管理员在 Server 主机上解锁（Windows 使用 .exe）：

```sh
./ergent-server --database .local/server.db login-unblock 192.0.2.1 \
  --actor on-call --reason "已确认共享出口误触发"
```

`--actor` 和 `--reason` 为必填，分别最多 120 和 1024 字节，不可为空白。操作会原子保存审计并删除该 IP 的限制；审计写入失败时不会解锁。即使该 IP 已无限制，也会留下记录。输出包含审计 ID，表 `login_guard_audit` 记录 UTC Unix 时间、规范化 IP、操作人标签、原因及清除前的失败次数/窗口起点/退避截止时间。操作人是本地主动填写的标签，不是经过独立认证的身份；数据库管理员仍可修改本地记录，重要环境请将系统日志汇入外部审计系统。没有新增管理 HTTP 接口。

有 SQLite 命令行工具时可只读查询最近的审计记录：

```sh
sqlite3 -readonly .local/server.db "SELECT id,datetime(created_at,'unixepoch'),ip,action,actor,reason,previous_failures FROM login_guard_audit ORDER BY created_at DESC LIMIT 50;"
```

首次升级会将旧 `login_ip_failures` 中所有计数（包括永久封禁）记录为 `legacy_reset` 审计事件并清零。旧数据没有时间戳，不能可靠地迁移成新的时间窗口；此重置只发生一次，之后重启不会重复清零。升级前停止所有访问同一数据库的旧 Server 并备份数据，不要让新旧版本同时访问；回退旧版本请恢复升级前的备份，旧版本不会理解新的限流状态。

HTTPS 代理后必须指定 `--trusted-proxy 代理对端IP` 并由该代理覆盖 `X-Real-IP` 为真实客户端 IP；否则按代理 IP 共用失败计数。
