# 命令与参数参考

本文介绍命令细节。首次使用请先阅读[服务端启动](server.md)和[客户端启动](agent.md)；容器内的命令及环境变量见 [Docker 部署指南](docker.md)。

示例在原生包目录使用 sh/bash 执行。Windows 将二进制替换为 `.exe`，启动脚本替换为 `.cmd`，多行命令在 PowerShell 中合并为一行。

## 查看帮助

```sh
./ergent-server --help
./ergent-server run --help
./ergent-server setup-code --help
./ergent-server admin-create --help
./ergent-server login-unblock --help
./ergent-agent --help
./ergent-agent enroll --help
```

顶层帮助显示命令概览，子命令的 `--help` 显示详细参数。

## Server 命令

| 命令 | 用途 |
| --- | --- |
| `ergent-server run` | 启动服务端与网页 |
| `ergent-server setup-code` | 在真实交互终端领取或轮换首次初始化码 |
| `ergent-server admin-create` | 通过终端输入密码创建账户 |
| `ergent-server login-unblock IP` | 清除指定来源 IP 的登录限制并记录审计 |

### 数据库与网页路径

全局参数 `--database PATH` 对应环境变量 `ERGENT_DATABASE`，默认 `.local/server.db`。运行、初始化、账户创建和解锁命令必须指向同一实际数据库；后台服务建议使用绝对路径。

```sh
./ergent-server --database /srv/ergent/server.db run --web "$PWD/web" --tls-mode dev-http
./ergent-server --database /srv/ergent/server.db setup-code
```

二进制的默认网页目录是源码目录中的 `apps/web/dist`；原生包直接运行二进制时指定 `--web "$PWD/web"`。`start-server.sh` / `.cmd` 已选择包内网页目录，并把参数传给 `run`，不要再加一个 `run`。

### run 常用参数

| 参数 | 环境变量 | 默认值 / 说明 |
| --- | --- | --- |
| `--listen IP:PORT` | `ERGENT_LISTEN` | `127.0.0.1:7777` |
| `--origin URL` | `ERGENT_ORIGIN` | `http://127.0.0.1:7777`；浏览器精确地址，不启用 TLS |
| `--additional-origin URL` | `ERGENT_ADDITIONAL_ORIGINS` | 额外精确来源，可重复；环境变量用逗号分隔，协议须与主来源相同 |
| `--trusted-proxy IP` | `ERGENT_TRUSTED_PROXY` | 精确代理对端 IP，可重复；环境变量用逗号分隔 |
| `--web PATH` | `ERGENT_WEB` | `apps/web/dist`；原生包用包内 `web/` |
| `--tls-mode MODE` | `ERGENT_TLS_MODE` | `direct`、`proxy` 或 `dev-http`；原生二进制未指定时按证书配置决定 |
| `--tls-cert PATH` | `ERGENT_TLS_CERT` | `direct` 模式的 PEM 证书链，须与私钥同时提供 |
| `--tls-key PATH` | `ERGENT_TLS_KEY` | `direct` 模式的匹配 PEM 私钥 |

`direct` 由 Server 提供 HTTPS；`proxy` 由已有反向代理提供 HTTPS；`dev-http` 用于本机 HTTP 测试。启动示例和 Nginx 配置见[服务端启动](server.md)。代理必须覆盖 `X-Real-IP`，只信任实际代理对端的精确 IP，不接受 CIDR 或通配符。

开发参数 `--allow-insecure-lan` 和 `--allow-any-origin` 不放宽首次初始化检查；部署时使用 HTTPS 和精确来源。

### 账户与初始化

后台运行或初始化码遗失时，在真实交互终端使用相同数据库轮换初始化码：

```sh
./ergent-server --database .local/server.db setup-code
```

初始化码有效期 15 分钟，数据库只保存哈希；有效期内重启不会生成新码，轮换会立即使旧码失效。已有账户直接登录，首次账户创建后网页初始化永久关闭。

也可在服务端创建账户：

```sh
./ergent-server --database .local/server.db admin-create --username admin
```

交互终端隐藏输入，密码后按 Enter，再输入一次确认并按 Enter，无需 Ctrl-D/Ctrl-Z。密码须为 12–1024 个 UTF-8 字节。首次 CLI 创建同样关闭网页初始化；后续可再创建独立账户，当前没有管理员/普通用户角色之分。

非 TTY 自动化兼容一次密码 + EOF，不要求二次确认；不把密码写入命令或 shell 历史。Docker 人工操作使用默认 TTY 的 `docker compose exec`，只有非交互管道使用 `exec -T`。`admin-create` 只替代初始化，网页登录仍使用 HTTPS 或受保护的原生回环/SSH 入口。

## Agent 命令

| 命令 | 用途 |
| --- | --- |
| `ergent-agent enroll --server URL --name NAME` | 输入一次性令牌并注册设备 |
| `ergent-agent run` | 使用已保存的配置连接服务端 |
| `ergent-agent doctor` | 检查运行环境 |
| `ergent-agent setup-codex` | 安装 Codex hooks |
| `ergent-agent codex [ARGS...]` | 启动 Codex 并接收完成通知 |
| `ergent-agent tool-event` | 从 stdin 读取工具事件 JSON |

### 配置与证书参数

| 参数 | 说明 |
| --- | --- |
| `--config PATH` | 全局参数，默认 `.local/agent.json`，注册与运行须使用同一文件 |
| `--ca-cert PATH` | 全局参数，公共 CA 证书 PEM；注册时保存绝对路径，运行时可临时覆盖 |
| `enroll --server URL` | 必填，服务端精确 origin；HTTPS 或本机回环 HTTP，不带路径 |
| `enroll --name NAME` | 设备名，默认 `My development machine` |

```sh
./ergent-agent --config /path/to/agent.json enroll --server https://terminal.example.com --name My-PC
./ergent-agent --config /path/to/agent.json run
./ergent-agent --config /path/to/agent.json --ca-cert /path/to/root-ca.pem run
```

注册令牌在交互终端隐藏输入，粘贴一次后按 Enter；既有非 TTY 自动化从 stdin 提供一次令牌并关闭输入流（EOF）。令牌不放入参数、环境变量或日志。

### Codex 状态提示

更新 Agent 后，在网页中新建终端并执行：

```sh
"$ERGENT_AGENT_BIN" setup-codex
"$ERGENT_AGENT_BIN" codex
```

在 Codex `/hooks` 中审阅并信任 hooks 后，网页可显示处理中、等待确认、完成和退出等状态。目前仅提供 Codex 状态适配器，AI CLI 需自行安装和登录。

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
