# Ergent 发布包 / Native packages

本包包含 Server、Agent、网页和启动脚本，可分别运行 Server 或 Agent。无需 Rust、Node.js、pnpm 或 Python；AI CLI 需自行安装在 Agent 机器上。
Each archive includes Server, Agent, the web UI and launch scripts. No build tools are required; install your AI CLIs separately on the Agent machine.

## 查看命令帮助 / Command help

`./ergent-server --help` 显示命令概览和 Nginx 示例；详细参数请按子命令查看。
Top-level help shows an overview and Nginx example; open each subcommand for its options:

```sh
./ergent-server run --help
./ergent-server setup-code --help
./ergent-server admin-create --help
```

## 启动 Server

先进入解压目录，**推荐使用 `./start-server.sh`**（Windows 使用 `./start-server.cmd`）。脚本会切换到包目录并加载 `web/`；不要在脚本参数中再加 `run`。
From the extracted directory, **use the launch script**. It selects the bundled web UI and passes your options to `run`.

以下多行示例使用 sh/bash；Windows 替换为 `.cmd`/`.exe`，并在 PowerShell 中将命令合并为一行。
Multiline examples use sh/bash. On Windows, use `.cmd`/`.exe` and join each command onto one line in PowerShell.

```sh
./start-server.sh --tls-mode dev-http
```

默认只监听 `127.0.0.1:7777`，浏览器打开 **http://127.0.0.1:7777**。数据库默认是包目录下的 `.local/server.db`。已有数据时继续使用原数据库，不要改路径重新初始化。
The default is local-only HTTP at the exact URL above, with `.local/server.db` in the package directory. Keep using your existing database.

裸 `./start-server.sh` 仍兼容默认回环 HTTP；这里显式指定 `dev-http`，避免混淆部署模式。
The bare script retains legacy loopback HTTP behavior; the example makes development HTTP explicit.

如果自行运行二进制，必须指定包内网页路径；以下命令与上方启动脚本等价（在包目录执行）：
When launching the binary directly, pass the bundled web path explicitly:

```sh
./ergent-server run --web "$PWD/web" --tls-mode dev-http
```

Windows PowerShell 对应 `./ergent-server.exe run --web "$PWD/web" --tls-mode dev-http`。二进制默认网页路径是源码目录的 `apps/web/dist`，直接执行 `ergent-server run` 可能找不到发布包网页。
The binary's default `apps/web/dist` is for source builds, not the extracted archive.

**路径一致 / Same database:** 下文管理命令均在包目录运行。若使用了 `--database` 或 `ERGENT_DATABASE`，将所有 `.local/server.db` 替换为 Server 实际使用的同一文件；后台服务建议使用绝对路径。原生包默认路径不是 Docker 的 `/data/server.db`。
Run management commands from the same package directory, or pass the same absolute database path everywhere. Do not accidentally create a second empty database.

### 选择部署方式 / Choose a deployment mode

只选一种方式，运行一个 Server。`--origin` 是浏览器实际访问的“协议 + 主机 + 端口”，**不会启用 TLS**；HTTPS 需要下面的真实代理或证书配置。
Choose one mode and one Server. **`--origin` does not enable TLS.** It must match the browser's external scheme, host and port.

以下 TLS 选项需使用含对应功能的构建；旧发布包先检查 `./ergent-server run --help`。Server、网页和 Agent 应使用匹配版本。
These TLS options require a build that supports them; check `run --help` on older packages. Keep Server, web UI and Agent versions compatible.

下面三种启动命令均沿用默认 `.local/server.db`（除非已有环境变量覆盖）。若之前显式使用 `data/server.db` 或其他路径，请在所选启动命令中加 `--database` 指向原文件，所有管理命令也使用它；不要切回默认创建空库。
All three examples retain `.local/server.db` unless overridden by your environment. If you previously selected `data/server.db` or another path, pass that same file with `--database` in both startup and management commands.

- `http://localhost:7777` 与 `http://127.0.0.1:7777` 是不同 origin，不能混用；`0.0.0.0` 只是监听地址。
  `localhost` and `127.0.0.1` are different origins. Browse the configured address, never `0.0.0.0`.
- 非默认端口必须写入 origin，例如 `https://terminal.example.com:8443`；不要加路径、尾部 `/`、查询串或通配符。额外入口逐个使用 `--additional-origin URL`，协议须相同。
  Include non-default ports; omit paths, trailing slashes, queries and wildcards. Add each additional exact origin explicitly.

#### A. 复用同机 Nginx / Existing Nginx on the same host

适合已经由 Nginx 管理 HTTPS 和证书的服务器；Ergent 后端仅监听回环 HTTP，不再配置 Server 证书。
Let your existing Nginx terminate HTTPS. Keep the native Server's HTTP backend on loopback:

```sh
./start-server.sh --listen 127.0.0.1:7777 \
  --tls-mode proxy --origin https://terminal.example.com \
  --trusted-proxy 127.0.0.1
```

在现有 Nginx 的 `http {}` 中加入 `map`，并将 `location` 合并到该域名已有的 HTTPS `server {}` 中；保留证书、续期和其他站点配置：
Add the map in Nginx's `http {}` context and the location inside the domain's existing HTTPS server. Keep its certificate and renewal configuration:

```nginx
map $http_upgrade $ergent_connection_upgrade {
    default upgrade;
    ''      close;
}

# Inside the existing HTTPS server for terminal.example.com:
location / {
    proxy_pass http://127.0.0.1:7777;
    proxy_http_version 1.1;
    proxy_set_header Host $http_host;
    proxy_set_header Upgrade $http_upgrade;
    proxy_set_header Connection $ergent_connection_upgrade;
    proxy_set_header X-Real-IP $remote_addr;
    proxy_buffering off;
    proxy_read_timeout 3600s;
    proxy_send_timeout 3600s;
}
```

先测试再按现有方式重载，如 `sudo nginx -t && sudo systemctl reload nginx`。浏览器和 Agent 使用 `https://terminal.example.com`；不使用后端的 HTTP 地址。代理必须覆盖 `X-Real-IP`，不可透传客户端提供的值。上述 `127.0.0.1` 信任值仅适用于这个同机原生示例；Docker 的代理对端地址不同。
Test before reloading. Use the public HTTPS URL for browser and Agent connections. Trust only the actual proxy peer and overwrite `X-Real-IP`; the loopback trust setting above does not apply to Docker networking.

#### B. 原生 HTTPS / Direct TLS

没有 HTTPS 代理时，给 Server 提供已有的 PEM 证书链和匹配私钥：
Without a TLS proxy, supply your existing PEM certificate chain and matching private key:

```sh
./start-server.sh --listen 0.0.0.0:7777 \
  --tls-mode direct --origin https://terminal.example.com:7777 \
  --tls-cert /path/to/fullchain.pem --tls-key /path/to/privkey.pem
```

打开 `https://terminal.example.com:7777`，按需开放该端口。证书须匹配域名或 IP SAN，文件须可由 Server 用户读取；不自动申请或续期，更新文件后需重启 Server。只传文件路径，不把私钥内容放入参数、环境变量或日志。
Use the exact HTTPS URL and allow the chosen port through your firewall. Certificates must match the hostname or IP SAN. Renewal is external; restart Server after replacing the files. Pass paths only, never private-key contents.

#### C. 本机或 SSH 隧道 / Local development or SSH tunnel

仅在本机测试时显式使用回环 HTTP：
For local testing, bind HTTP to loopback only:

```sh
./start-server.sh --tls-mode dev-http \
  --listen 127.0.0.1:7777 --origin http://127.0.0.1:7777
```

浏览器在另一台机器时，可在该机器建立 SSH 隧道，然后仍打开 `http://127.0.0.1:7777`；保持隧道运行：
For a browser on another machine, open an SSH tunnel there and visit the same loopback URL while the tunnel stays running:

```sh
ssh -N -o ExitOnForwardFailure=yes \
  -L 127.0.0.1:7777:127.0.0.1:7777 user@server
```

隧道的本地端口须与浏览器地址及 Server 的 `--origin` 一致；它的目标端口须与 Server 的 `--listen` 一致。不要把 HTTP 后端绑定到公网或 LAN。`dev-http` 不放宽 Origin、CSRF、登录或首次初始化检查；正式跨机器 Agent 连接仍使用 HTTPS。
Match the tunnel's local port to the browser URL and configured origin, and its destination port to the Server listener. Keep the backend private; development mode does not weaken security checks. Use HTTPS for deployed remote Agents.

### 首个账户 / First account

空数据库首次启动时，程序只在服务器真实控制终端显示一个 15 分钟有效的一次性初始化码，不写入普通日志或重定向输出。打开上面选定的地址，输入初始化码、用户名及两次密码；密码须一致且为 12–1024 个 UTF-8 字节。
On first startup, a 15-minute setup code appears only on the Server's controlling terminal. Open your selected URL and enter the code, username and matching password twice (12–1024 UTF-8 bytes).

数据库仅保存初始化码的哈希；有效期内重启不会生成新码。
Only a hash is stored. Restarting while the code is valid does not issue a new one.

后台运行、码遗失或过期时，在服务器交互终端执行，**仍使用同一数据库**：
For a background service or a lost/expired code, rotate it in an interactive terminal using **the same database**:

```sh
./ergent-server --database .local/server.db setup-code
```

这会立即使旧码失效。已有账户直接登录；首次创建成功后网页初始化永久关闭，重启或删除账户不会重新开放。不要把初始化码或密码放入 URL、命令行参数、环境变量、聊天或日志。
This invalidates the previous code. Existing accounts log in directly; setup stays closed after the first account is created. Keep codes and passwords out of URLs, arguments, environment variables and logs.

**远程明文 HTTP 首次初始化会被拒绝**，即使网页能打开。请用配置正确的 HTTPS，或上面的原生回环/SSH 隧道；不要使用 `--allow-any-origin` 或 LAN 开关绕过保护。Docker bridge 不等同于原生回环连接。
**Remote plain-HTTP first-account setup is blocked**, even when the page loads. Use working HTTPS or native loopback/SSH access; do not weaken origin checks. Docker bridge traffic is not native loopback traffic.

### CLI 创建账户 / Create an account in the terminal

服务器操作者也可使用以下命令；已有服务运行时仍须指向其实际数据库：
The Server operator can also create an account directly against the same database:

```sh
./ergent-server --database .local/server.db admin-create --username admin
```

普通交互终端会隐藏输入：输入密码后按 Enter，再输入一次确认并按 Enter，**无需 Ctrl-D/Ctrl-Z**。首次 CLI 创建同样会关闭网页初始化；后续可再创建独立账户，当前没有管理员/普通用户角色之分。
Terminal input is hidden: password + Enter, confirmation + Enter. **No Ctrl-D/Ctrl-Z is needed.** Creating the first account closes web setup; additional independent accounts can be created later. There are currently no separate admin/user roles.

非 TTY 管道仍兼容一次密码 + EOF，供可信秘密管理器等既有自动化使用，不要求二次确认；不要手工把密码写进命令或 shell 历史。Docker 人工操作使用默认 TTY 的 `docker compose exec`，只有非交互管道使用 `exec -T`。
Existing non-TTY automation supplies one password through stdin and closes the stream (EOF), with no confirmation prompt. Do not put secrets in command text. Keep Docker's default TTY for manual input; use `-T` only for pipelines.

`admin-create` 只替代初始化，不使远程明文 HTTP 变安全。创建后通过 HTTPS 或受保护的回环/SSH 页面登录，勿在远程 HTTP 页面输入密码。
`admin-create` only replaces setup. Log in through HTTPS or protected loopback/SSH access afterwards; never enter passwords on remote plaintext HTTP.

## 启动 Agent

网页登录后创建一次性设备注册令牌，在实际开发机器的包目录执行（Windows 使用 `.exe`）：
Create a one-time enrollment token in the web UI, then run on the development machine (use `.exe` on Windows):

```sh
./ergent-client enroll --server https://terminal.example.com --name My-PC
./ergent-client run
```

将地址换成 Server 的实际 HTTPS 地址，非默认端口也要写上。注册令牌隐藏输入，粘贴一次后按 Enter 完成，**无需 Ctrl-D/Ctrl-Z**。不要把令牌放入参数、环境变量或日志。
Use the actual HTTPS URL, including a non-default port. Paste the hidden token once and press Enter; **no Ctrl-D/Ctrl-Z**. Keep it out of arguments, environment variables and logs.

既有非 TTY 自动化仍从 stdin 读取一次令牌，并在输入流关闭（EOF）后完成。
Existing non-TTY automation still sends one token through stdin and closes the stream (EOF).

同机测试可使用 `http://127.0.0.1:7777`；Agent 仅允许回环 HTTP，跨机器直连（包括 Tailscale）必须使用 HTTPS。配置默认在 `.local/agent.json`；切换工作目录或使用 `--config 路径` 时，enroll/run 必须指向同一配置文件。
HTTP is allowed only for loopback testing. Remote connections, including Tailscale, require HTTPS. Enrollment and `run` must use the same configuration file; the default is `.local/agent.json` relative to the working directory.

私有 CA 可在 enroll 前指定 `--ca-cert`，只提供公共 CA 证书 PEM，不提供服务器或 CA 私钥：
For a private CA, provide its public PEM certificate only:

```sh
./ergent-client --ca-cert /path/to/root-ca.pem \
  enroll --server https://terminal.example.com --name My-PC
./ergent-client run
```

Agent 保存 CA 的绝对路径，`run` 自动复用；文件须保持可读，更新后重启 Agent。临时覆盖可用 `--ca-cert 路径 run`。浏览器也需信任该 CA，IP 访问须有匹配 IP SAN，不要关闭证书校验。
The CA path is saved and reused for HTTPS/WSS. Keep it readable and restart Agent after updates. Browsers must also trust the CA; IP connections require a matching IP SAN. Never disable certificate verification.

后台服务需自行配置。关闭网页不结束终端；Agent 或操作系统重启不能恢复原进程。
Configure background services separately. Closing the browser leaves terminals running; restarting Agent or the OS cannot restore their processes.

## 服务端掉线与恢复 / Server outages and recovery

只要 `ergent-client run` 和设备仍在运行，服务端掉线或重启不会结束已有终端和任务。客户端自动重试连接：每次连接结束后的等待依次约为 1、2、4、8、16、30 秒，之后保持约 30 秒，另加不足 0.5 秒随机延迟。断线检测和连接尝试还需要时间；成功连接不会重置本次运行期间的退避计数。服务端恢复后，客户端重新上报终端，仍在运行的 Shell 保留工作目录和进程状态。
While the client process and device remain running, a Server outage or restart does not terminate their terminals or tasks. The client retries after waits of approximately 1, 2, 4, 8, 16 and then 30 seconds, plus less than 0.5 seconds of jitter. Detection and connection attempts take additional time; successful connections do not reset this process's backoff. On reconnection, the client reports its terminals again, preserving running shells and their working directories.

恢复需要沿用服务端数据库和有效设备凭据，保持服务端地址、证书信任和协议兼容。Docker 重建时保留原数据卷；不要为恢复服务端连接而重启客户端。客户端或设备自身重启无法恢复原进程。
Keep the Server database and valid device credentials, with compatible address, certificate trust and protocol settings. Preserve the data volume when recreating Docker containers. Do not restart the client just to reconnect: restarting the client or device cannot restore the original processes.

当前网页断线后约 1.5 秒检查登录状态；若服务端仍不可用，检查失败会返回登录页。服务恢复后刷新页面，必要时重新登录并选择原终端。每终端只保留最近 512 KiB 原始输出缓存；满足条件时续传，否则恢复当前屏幕，完整滚动历史不保证保留。重新登录或刷新后也不能恢复原浏览器滚动历史。断线时未确认的输入不会自动重发。
The web UI checks authentication about 1.5 seconds after disconnection. If the Server is still unavailable, that failed check returns to the login page. Refresh after recovery, sign in if needed, and select the original terminal. Each terminal retains only 512 KiB of raw output: eligible connections resume from their cursor; otherwise they restore the current screen. Full scrollback is not guaranteed, and browser history does not survive re-login or refresh. Unconfirmed input is never replayed automatically.

## 更新与平台要求

先停止 Server/Agent，备份 `.local/`；替换二进制、web 和启动脚本，保留原数据。
停止 Agent 会结束它管理的终端进程，更新前请保存工作。
发布包不含密码、令牌、数据库、AI CLI 或开发环境。
manifest.json 记录版本、Git commit、工作区是否有未提交修改及文件校验值；外部 .sha256 校验压缩包。

### 升级到 1.3.0 / Upgrade to 1.3.0

本版新增设备管理、设备移除、终端展开/收起及首页设备终端列表。客户端由 `ergent-agent` 更名为 `ergent-client`（Windows 使用 `.exe`），服务端仍为 `ergent-server`；更新后台服务及脚本中的程序路径。已有 `.local/agent.json` 或自定义 `--config` 配置继续有效，无需重新注册。之前安装过 Codex hooks 时，请移除指向旧程序的 ErGent hook，再执行 `ergent-client setup-codex` 并在 `/hooks` 中确认信任。
This release adds device management, device removal, collapsible terminal lists and terminal browsing from overview cards. The client executable is renamed from `ergent-agent` to `ergent-client` (`.exe` on Windows); the server remains `ergent-server`. Update service and script paths. Existing client configurations remain valid without re-enrollment. If Codex hooks reference the old executable, remove those ErGent hooks, run `ergent-client setup-codex`, and review trust in `/hooks`.

从 1.2.0 升级时保留原数据库与客户端配置；协议仍为 `ergent.preview.v3`。只重启服务端时客户端自动重连，重启客户端则不能恢复原终端进程。Docker `1.3.0` 镜像在 AMD64/ARM64 原生机器上分别构建后合并发布。
Keep the database and client configuration when upgrading from 1.2.0; the protocol remains `ergent.preview.v3`. Clients reconnect after a server-only restart, but restarting a client cannot restore its terminal processes. Docker `1.3.0` combines native AMD64 and ARM64 builds.

### 升级到 1.2.0（历史说明）/ Upgrade to 1.2.0 (historical)

从 1.1.0 升级时，首次启动会执行 `0005_first_account_setup.sql`。已有账户继续直接登录，网页首次初始化会永久关闭；空数据库使用一次性初始化码或 `admin-create` 创建首个账户。请备份实际使用的数据库、Agent 配置、部署配置和证书；回退时同时恢复旧程序及升级前同一时点的数据库备份。
On upgrade from 1.1.0, startup applies `0005_first_account_setup.sql`. Existing accounts log in directly and first-account setup is permanently closed. Empty databases use a setup code or `admin-create`. Back up the actual database, Agent configuration, deployment configuration and certificates; restore the old programs together with the matching pre-upgrade database backup when rolling back.

跨机器部署检查上文的 HTTPS/代理配置；使用私有 CA 时，Agent 注册需指定 `--ca-cert`，保存的证书路径须保持可读。原生包与 Docker 镜像分别发布，容器部署请按仓库中的 `docs/docker.md` 更新。
Check the HTTPS/proxy configuration above for remote deployments. For a private CA, specify `--ca-cert` at Agent enrollment and keep the saved certificate path readable. Native packages and Docker images are published separately; follow `docs/docker.md` in the repository for container upgrades.

### 平台要求 / Platform requirements

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
