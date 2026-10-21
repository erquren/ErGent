# Docker 部署 Server 与网页 / Server + web UI

一个镜像包含 **Server 与已构建的网页**，支持原生 HTTPS，**Nginx 是可选项**。Agent 继续以原生程序运行在实际开发机器上，使用该机器的 Shell、文件、开发工具和当前用户权限。容器不需要挂载 Docker socket、开发目录或 Agent 凭据。
One container runs Server and the built web UI. Agents remain native on development machines; do not mount their files, credentials or the Docker socket into Server.

默认连接方式：浏览器 / 原生 Agent → ErGent 原生 HTTPS → `/data/server.db`。已有 Nginx 的用户也可选择本文的[宿主机代理方案](#可选复用宿主机-nginx)或[容器代理方案](#可选复用已有-nginx-容器)，两者都不会安装或强制启动另一套 Nginx。另有仅限本机开发的[显式 HTTP 模式](#仅限本机开发显式-dev-http)。

## 先选部署入口 / Choose the right guide

- 下载的是解压即用的原生包？使用[服务端启动说明](server.md)：推荐 `./start-server.sh`；直接运行二进制必须加 `--web "$PWD/web"`。原生包也支持同机 Nginx、原生 TLS 和本机/SSH 测试，不需要 Docker。
  For an extracted native archive, use the package guide and launch script. Docker is optional; native direct launches need the explicit bundled web path.
- 本文容器数据库是 `/data/server.db`，位于持久化卷；原生包默认是包目录的 `.local/server.db`。若以前显式使用 `data/server.db` 或其他路径，继续指向那份实际数据；切换部署方式不会自动迁移数据库。
  Docker uses `/data/server.db`; native packages default to `.local/server.db`. Preserve any explicitly chosen database path. Switching deployment methods does not migrate data automatically.
- Docker 默认用下面的原生 HTTPS 流程；已有代理可选[宿主机 Nginx](#可选复用宿主机-nginx)或[已有 Nginx 容器](#可选复用已有-nginx-容器)。仅本机开发使用[独立 dev-http 配置](#仅限本机开发显式-dev-http)。只启动一套 Server，并在管理命令中沿用同一 Compose 文件和数据卷。
  Choose direct HTTPS, an existing host/container proxy, or the separate local-development setup. Run one Server and keep the same Compose file and data volume for all management commands.
- `ERGENT_ORIGIN` 只声明浏览器入口，**不启用 TLS**。HTTPS 仍需证书或真实 HTTPS 代理；主机、协议和非默认端口须精确匹配，`localhost` 与 `127.0.0.1` 不可互换。
  Origin does not enable TLS. Match the external scheme, hostname and port exactly; `localhost` and `127.0.0.1` are different origins.
- 首次账户通过 HTTPS 和交互式 `setup-code` 创建；明文 Docker bridge 初始化会被拒绝，即使只映射到宿主机回环。开发模式用同一数据库的 `admin-create`：隐藏输入密码 + Enter 两次。Agent `enroll` 隐藏输入令牌 + Enter 一次，无需手工 EOF。
  Use HTTPS for web setup. Docker bridge is not native loopback; for local dev, `admin-create` uses hidden password + Enter twice. Agent enrollment uses hidden token + Enter once. Keep security checks enabled.

## 前提与镜像状态

- Docker Engine、Docker Compose v2，以及指向服务器的域名和对应的现有 TLS 证书/私钥。以下文件权限示例面向 Linux；rootless Docker、用户命名空间和 SELinux 环境需按实际映射/标签规则调整。
- **不会自动申请或续期证书，不包含 ACME 客户端。** 证书由现有的证书管理流程提供；自签名/私有 CA 证书需要浏览器和原生 Agent 信任对应 CA。
- 当前可从源码构建本地镜像 `ergent-server:local`，镜像发布工作流的目标为 Docker Hub 的 `erquren/ergent`。在实际发布成功前，不能假定目标标签已有可拉取镜像。下文的拉取示例仅适用于维护者确认发布成功后。
- 源码构建需要联网下载基础镜像和锁定依赖，宿主机无需安装 Rust、Node.js 或 pnpm，实际工具链版本见 `Dockerfile`。
- 只运行一个 Server。Agent/WebSocket 路由和重放缓存保存在单进程内存中；不要扩容副本，也不要让两个 Server 同时访问同一数据库。`setup-code` 等短期管理命令不属于第二个 Server。

## 1. 准备现有证书

默认 Compose 将专用证书目录只读挂载为 `/certs`，读取 `fullchain.pem` 和 `privkey.pem`。证书需为该域名的 PEM 格式证书链，私钥与证书匹配。私钥内容不能放入环境变量、镜像、Git、命令行参数或日志。

镜像运行 UID/GID 为 **10001:10001**，目录必须可遍历，两个文件必须可读。不要降低现有 `/etc/letsencrypt` 等证书库的安全权限。可以由宿主机管理员复制所需文件到 ErGent 专用目录：

```sh
sudo install -d -m 0700 -o 10001 -g 10001 /srv/ergent/certs
sudo install -m 0600 -o 10001 -g 10001 \
  /etc/letsencrypt/live/terminal.example.com/fullchain.pem \
  /srv/ergent/certs/fullchain.pem
sudo install -m 0600 -o 10001 -g 10001 \
  /etc/letsencrypt/live/terminal.example.com/privkey.pem \
  /srv/ergent/certs/privkey.pem
```

替换域名和源路径。这些是解引用后的独立副本，不要只复制指向容器外目录的符号链接。目录 `0700`、文件 `0600`、所有者 10001 即可，不要使用 `chmod 777` 或全局可读私钥。

证书续期后，现有证书管理流程还需安全更新这两个专用副本，保持同样的所有者/权限，并重启 Server 让它加载新证书：`docker compose restart server`。证书不是自动热更新的；先核实证书与私钥匹配，再更新和重启，并检查新到期时间。重启会中断连接，宜安排维护窗口。

## 2. 用环境变量配置并启动

进入仓库根目录，在未跟踪的 `.env` 中填写：

```dotenv
ERGENT_ORIGIN=https://terminal.example.com
ERGENT_CERT_DIR=/srv/ergent/certs
ERGENT_HTTPS_PORT=443
```

`ERGENT_ORIGIN` 必须与浏览器地址完全匹配，包括 HTTPS 和非默认端口；不能带路径、尾部 `/`、查询串或通配符。如果把 `ERGENT_HTTPS_PORT` 改为 `8443`，origin 也应为 `https://terminal.example.com:8443`，原生 Agent 同样使用该地址。默认 Compose 发布宿主机 HTTPS 端口到容器的 `7777`；确认该端口未被其他服务占用，并按需要开放防火墙。

先从源码构建，再启动：

```sh
docker compose build --pull server
docker compose up -d --no-build server
docker compose ps
curl --fail https://terminal.example.com/health/ready
```

证书目录必须已经存在；Compose 不会替你创建一个空目录，也不会申请证书。目录里需要两个指定文件，且 UID 10001 能够读取。私有 CA 环境可对检查命令使用 `--cacert /path/to/ca.pem`，不要用关闭 TLS 验证来替代正确的信任配置。

### 环境变量

容器部署以环境变量作为主要配置方式。Server 也保留对应 CLI 参数，但镜像内置健康检查读取的是环境变量；直接用 CLI 覆盖 listen、origin 或 TLS 配置可能造成两者不一致，除非同时调整健康检查。原生二进制可按需要使用 CLI 参数。

- `ERGENT_DATABASE`：镜像默认 `/data/server.db`，数据库及伴随文件必须位于持久化卷。
- `ERGENT_LISTEN`：镜像默认 `0.0.0.0:7777`，所选模式在这个地址提供 HTTPS 或 HTTP，不同时开启两种协议。
- `ERGENT_WEB`：镜像默认 `/app/web`，包含已构建网页。
- `ERGENT_ORIGIN`：外部准确 origin，默认 Compose 要求显式设置 HTTPS 地址。
- `ERGENT_TLS_MODE`：镜像默认 `direct`，要求证书/私钥和 HTTPS origin；`proxy` 用于已有 HTTPS 代理，拒绝配置 TLS 证书；`dev-http` 是显式本机开发模式。各 Compose 文件都明确设置模式，缺少必需配置会报错，不会静默降级到 HTTP。原生二进制未设置模式时，仍按证书配置决定 TLS/既有本机 HTTP 行为，以保留兼容性。
- `ERGENT_TLS_CERT`、`ERGENT_TLS_KEY`：容器内证书链与私钥路径，必须成对设置；默认 Compose 固定为 `/certs/fullchain.pem` 和 `/certs/privkey.pem`。变量仅保存路径，不保存证书或私钥内容。代理和开发 HTTP 模式不设置这两个变量，后端运行 HTTP。
- `ERGENT_ADDITIONAL_ORIGINS`：可选的额外精确 origin，多个以逗号分隔，协议需与主 origin 一致；不用通配符，并确保 TLS 证书覆盖这些入口。
- `ERGENT_TRUSTED_PROXY`：仅在反向代理模式设置，值为一个精确代理对端 IP，多个地址以逗号分隔；不接受 CIDR、域名或通配符。原生 HTTPS 直接对外时不需要设置，也不要信任任意客户端的转发头。

`.env` 中的 `ERGENT_CERT_DIR`、`ERGENT_HTTPS_PORT` 和可选 `ERGENT_IMAGE` 由 Compose 用于宿主机挂载、端口映射及镜像选择，不是 Server 的运行参数。Compose 不会自动把 `.env` 中所有变量传入容器；上面其余 Server 默认值如需修改，应显式添加到所选文件的 `environment`，并同步端口、挂载、健康检查和管理命令。修改 `.env` 后用 `docker compose up -d --no-build server` 重新创建配置；仅 `restart` 不会加载改变后的 Compose 环境变量。

所有模式都保留原有 Origin、CSRF、登录保护和初始化限制。生产部署不要添加 `--allow-any-origin` 或依赖明文 LAN 模式。仅配置 HTTPS origin 并不会自动开启 TLS，必须设置上述证书对，或使用真正终止 TLS 的代理。

### 发布成功后的拉取方式

以下值仍是**占位符**，不是已验证存在的镜像。维护者完成发布并确认拉取权限后，把 `.env` 的 `ERGENT_IMAGE` 设置为对应镜像，推荐使用发布结果中的不可变摘要：

```dotenv
ERGENT_IMAGE=docker.io/erquren/ergent@sha256:REPLACE_WITH_PUBLISHED_DIGEST
```

然后保持证书、环境变量和数据卷配置不变，执行：

```sh
docker compose pull server
docker compose up -d --no-build server
```

也可使用维护者确认存在的版本标签，例如 `erquren/ergent:1.4.0`，并记录实际摘要。在发布结果尚未确认前，使用上面的本地构建路径；不要把拉取失败误判为用户配置问题。

## 3. 领取初始化码并创建账户

在服务器的**交互式终端**中执行：

```sh
docker compose exec server /app/ergent-server --database /data/server.db setup-code
```

[`docker compose exec`](https://docs.docker.com/reference/cli/docker/compose/exec/) 默认分配交互 TTY。不要加 `-T`、重定向输出或放入非交互 CI；普通 Server 服务也不应配置 `tty: true` 来获取初始化码。后台启动不会把码写入 `docker compose logs`，独立交互命令将码显示在控制终端。

打开配置好的 HTTPS 地址，输入初始化码、用户名和两次密码。码有效期 15 分钟，再次执行 `setup-code` 会立即轮换旧码；首次设置完成后永久关闭，不因重启而重新开放。已有账户直接登录，不要再领取初始化码。不要将码或密码写入 `.env`、镜像、命令行参数、聊天、URL 或日志。

原生二进制在本机回环地址上运行时可以使用本机 HTTP 初始化。但 Docker 发布到宿主机 `127.0.0.1` 的连接仍可能经过非回环 bridge 对端；首次设置会同时检查 HTTP 请求的 TCP 对端与客户端地址，因此不能把容器端口映射当作本机 HTTP 的安全等价物。**Docker 首次设置应从原生 HTTPS 或正确配置的 HTTPS 代理入口进行**，不能用 `--allow-any-origin` 绕过检查。

## 4. 在开发机器原生运行 Agent

下载与 Server/网页相匹配的[原生发布包](https://github.com/erquren/ErGent/releases)，在实际开发机器执行（Windows 使用 `.exe`）：

```sh
./ergent-client enroll --server https://terminal.example.com --name My-PC
./ergent-client run
```

网页登录后取得注册令牌；交互终端会隐藏输入，粘贴一次令牌后按 Enter 完成，无需 Ctrl-D/Ctrl-Z。不把令牌写入命令行或日志。Agent 配置、开发文件与 AI CLI 登录信息留在开发机器上。跨机器 Agent 连接必须使用 HTTPS，包括 Tailscale；非默认 HTTPS 端口需写入地址。详见[客户端启动说明](agent.md)。

如果 Server 来自尚未发布的源码提交，应使用同一兼容版本源码构建的原生 Agent，不能假定旧发布包一定兼容。Server、网页和 Agent 的协议必须匹配。

### 私有 CA 与 IP 地址访问

原生 HTTPS 也可以使用 IP origin，例如 `https://192.168.1.10`，但证书必须包含这个**实际 IP 的 Subject Alternative Name（IP SAN）**。只有域名 SAN 的证书不能用于 IP 地址；设置 origin 不会修正不匹配证书。可由已有私有 CA 签发带正确 SAN 的服务器证书，把完整证书链和对应私钥挂载到 Server。核对已有证书：

```sh
openssl x509 -in /srv/ergent/certs/fullchain.pem -noout -ext subjectAltName
```

浏览器需通过操作系统/浏览器支持的方式信任该 CA。原生 Agent 也支持显式提供仅含 CA 证书的 PEM 文件：

```sh
./ergent-client --ca-cert /path/to/root-ca.pem \
  enroll --server https://192.168.1.10 --name My-PC
./ergent-client run
```

`enroll` 将 CA 文件的绝对路径保存为 Agent 配置中的 `caCert`，`run` 自动复用；文件需一直保留且可读。Agent 在启动时构建信任配置，更新 CA 文件后需重启 Agent 才会加载；重启前保存终端中的工作。需要临时覆盖时使用 `./ergent-client --ca-cert /path/to/other-ca.pem run`，不会重写已保存配置。此信任同时用于 HTTPS 与 WSS，并保留公共/系统根证书校验；缺失、无效或空的 CA 文件会明确报错。只把公共 CA 证书提供给 Agent，**不要复制服务器私钥或 CA 私钥到 Agent**，也不要禁用证书或主机名校验。

## 健康检查与部署验收

`/health/live` 仅说明 HTTP/TLS 服务可响应；`/health/ready` 还检查数据库连接。镜像提供 HEALTHCHECK，使用 `docker compose ps` 查看健康状态；普通 Docker Compose 不会仅因 `unhealthy` 自动重启容器，`restart: unless-stopped` 处理的是进程退出。

健康检查不能替代外部 TLS 证书/域名验证、网页登录与 Agent WebSocket 连接测试。启动后检查这些路径，并核实数据在容器重建后仍存在。使用代理时还应核实转发客户端 IP 的信任配置。Docker 的某些用户态网络实现可能隐藏原客户端地址，应检查实际对端与登录限流表现，不要直接采信不可信转发头。

## 持久化与权限

默认 named volume `server-data` 挂载到 `/data`，实际卷名通常为 `ergent_server-data`。数据库包含用户、设备、会话元数据、登录限制、审计和初始化状态。新卷沿用镜像 `/data` 的 UID/GID **10001:10001** 与 `0700` 目录权限；已有卷不会自动修复错误的所有者。

- `docker compose stop`、重新构建或替换容器不会删除卷。
- 不要执行 `docker compose down -v`、`docker volume rm` 或清理该卷的 prune 命令，否则会丢失数据。
- 更改 Compose 项目名可能创建一个新的空卷；先记录实际卷名，不要误以为旧数据已丢失。
- Server 数据卷不是开发文件、Agent 凭据、终端进程或完整终端输出的备份。实时连接和重放缓存仍在内存中；见[终端恢复说明](../TERMINAL_RECOVERY.md)。

若改用宿主机目录，把 Compose 中 `server-data:/data` 替换为绝对路径，例如 `/srv/ergent/data:/data`，不要保留两个 `/data` 挂载。先创建目录：

```sh
sudo install -d -m 0700 -o 10001 -g 10001 /srv/ergent/data
```

迁移已有数据库前停止所有使用它的 Server，复制完整数据目录（包含存在的 WAL/SHM 文件），再设置权限：

```sh
sudo chown -R 10001:10001 /srv/ergent/data
sudo chmod 0700 /srv/ergent/data
sudo find /srv/ergent/data -type f -exec chmod 0600 {} \;
```

必须同时修正**已有数据库文件**的所有者；只给目录写权限不够。Server 会将数据库设为 `0600`，其他 UID 所有的文件可能无法执行该操作。只对 ErGent 专用目录执行这些命令。user namespace/rootless 或 SELinux 环境还需按实际 UID 映射和标签规则配置；不要用 `chmod 777` 或长期 root 运行来规避。

## 备份、升级与回退

升级前保存终端中的工作，备份 Server 数据、各原生 Agent 的配置，以及安全保存的部署配置与 TLS 证书。停止 Agent 会结束其终端进程。不要让新旧 Server 同时打开数据卷，也不要假定数据库迁移可逆。

### 一致性备份

最简单可靠的方式是停止 Server 后完整归档 `/data`。以下命令在仓库根目录执行，备份文件只对当前用户开放；归档成功且可读取后才重启：

```sh
umask 077
mkdir -p .local/backups
backup=".local/backups/server-data-$(date -u +%Y%m%dT%H%M%SZ).tar.gz"
docker compose stop server && \
  docker compose run --rm --no-deps -T --entrypoint tar server \
    -C /data -czf - . > "$backup" && \
  tar -tzf "$backup" >/dev/null && \
  docker compose start server
```

失败时不要把不完整归档当作备份。`tar -tzf` 只检查归档可读性，重要部署还应在隔离环境试恢复。将备份安全保存到异机存储，不要提交 Git。运行中的 SQLite 不能只复制 `server.db` 而忽略 WAL；在线备份应使用正确的 SQLite 备份工具。此命令仅备份 `/data`，不包含外部证书目录与 `.env`。

### 升级

1. 记录当前源码提交/镜像摘要、`.env`、Compose 配置及实际数据卷名，保留升级前的完整备份。本地镜像可用 `docker image tag ergent-server:local ergent-server:before-upgrade` 另存，后续升级请使用不重复标签。
2. 安排维护窗口，保存工作并停止原生 Agent 和 Server：`docker compose stop server`。确保备份对应这一停机时点，必要时重做备份步骤并暂不启动。
3. 源码部署：取得目标版本并执行 `docker compose build --pull server`。已发布镜像部署：将 `ERGENT_IMAGE` 改为已确认的目标摘要，再执行 `docker compose pull server`。不要混淆本地构建和镜像拉取流程。
4. 执行 `docker compose up -d --no-build server`，保留原卷。检查健康、证书、HTTPS 登录，再运行匹配版本的原生 Agent 并检查终端。

### 回退

停止 Server 和原生 Agent，使用升级前的源码/镜像与**同一时点的数据库备份**一起回退。仅替换旧镜像并继续使用新数据库可能失败或损坏状态。

将完整备份恢复到新的空专用卷或目录，保留故障现场；设置 UID/GID 10001 和上述权限，再把 Compose 指向恢复的数据。仅恢复可信归档。镜像拉取模式恢复原 `ERGENT_IMAGE` 摘要；本地构建模式可把保留的旧镜像重新标记为 `ergent-server:local`。用 `up -d --no-build` 启动，避免意外重建新代码。确认健康、登录、设备与数据后再接入匹配 Agent，不要将旧归档覆盖到运行中的数据库，或混合新旧 WAL/SHM。

## 可选：复用宿主机 Nginx

使用 [`deploy/compose.host-proxy.yaml`](../deploy/compose.host-proxy.yaml) **代替**默认 Compose，不能作为 override 叠加。此模式不挂载证书给 Server，不启用其原生 TLS；Nginx 终止 HTTPS，后端只发布 `127.0.0.1:7777`。保留 Nginx 现有证书管理方式。

`.env` 示例：

```dotenv
ERGENT_ORIGIN=https://terminal.example.com
ERGENT_TRUSTED_PROXY=REPLACE_WITH_EXACT_PROXY_PEER_IP
```

`ERGENT_TRUSTED_PROXY` 是 **Server 在容器内看到的 Nginx TCP 对端 IP**，不是浏览器 IP、域名或整个 Docker 网段。标准 Linux bridge 下宿主机 Nginx 通过发布端口连接时，通常为该网络网关，不能照抄 `127.0.0.1`。多个必要代理对端可用逗号分隔的精确 IP，不接受 CIDR 或通配符。

先构建并创建容器/网络，尚不启动；占位符只用于这个准备阶段：

```sh
docker compose --env-file .env -f deploy/compose.host-proxy.yaml build --pull server
docker compose --env-file .env -f deploy/compose.host-proxy.yaml create server
docker network inspect ergent_default \
  --format '{{range .IPAM.Config}}{{println .Gateway}}{{end}}'
```

将适用网关写入 `.env` 作为候选精确代理 IP，再启动：

```sh
docker compose --env-file .env -f deploy/compose.host-proxy.yaml up -d --no-build server
curl --fail http://127.0.0.1:7777/health/ready
```

网关只是常见实现的候选值，投入使用前必须核实实际连接源地址；不要扩大信任范围来“修复”。有宿主机管理权限且已安装 `nsenter`、`tcpdump` 时，可以只观察 TCP 元数据：

```sh
container_id=$(docker compose --env-file .env -f deploy/compose.host-proxy.yaml ps -q server)
container_pid=$(docker inspect --format '{{.State.Pid}}' "$container_id")
sudo nsenter -t "$container_pid" -n \
  tcpdump -nn -i any 'tcp dst port 7777'
```

通过配置好的 Nginx 发起 HTTPS 健康检查，核实来自 Nginx 的连接源 IP，排除容器健康检查的回环连接。不要输出请求正文，不要在抓包时提交密码或初始化码。地址不同则更新 `.env`，用同一 Compose 文件重新 `up -d --no-build`。Docker Desktop/rootless 环境需使用其实际网络诊断方式；上述宿主机 PID 命名空间命令可能不适用。重建网络或更改项目名后重新核实地址。

参考 [`deploy/nginx.host.conf`](../deploy/nginx.host.conf) 合并现有配置，替换域名、证书/私钥路径，保留其他站点与证书续期配置：

- `map` 位于 Nginx `http {}` 作用域，后端为 `http://127.0.0.1:7777`。
- 保留 HTTP/1.1、Upgrade/Connection 和较长超时，支持浏览器/Agent WebSocket；见[Nginx 官方说明](https://nginx.org/en/docs/http/websocket.html)。
- 用 `proxy_set_header X-Real-IP $remote_addr` **覆盖**客户端值。Server 只在 TCP 对端匹配 `ERGENT_TRUSTED_PROXY` 时采信该头。
- 若 Nginx 前面另有 CDN/负载均衡器，应先配置这些上游的精确信任边界，不能直接采信任意转发头。
- 不记录正文、密码、初始化码、Authorization 或 Cookie。检查主机防火墙和 Docker 网络，阻止外部绕过代理访问 HTTP 后端。

按现有方式测试并重载，例如：

```sh
sudo nginx -t && sudo systemctl reload nginx
curl --fail https://terminal.example.com/health/ready
```

后续初始化、管理、备份和升级命令都使用同一 `--env-file .env -f deploy/compose.host-proxy.yaml` 参数，例如：

```sh
docker compose --env-file .env -f deploy/compose.host-proxy.yaml \
  exec server /app/ergent-server --database /data/server.db setup-code
```

即使后端发布到宿主机回环，也通过 Nginx HTTPS 完成初始化，不能假定 Docker bridge 等同于真正回环 HTTP。

## 可选：复用已有 Nginx 容器

使用 [`deploy/compose.container-proxy.yaml`](../deploy/compose.container-proxy.yaml) **代替**其他两个 Compose 文件，不合并它们。此模式不发布后端端口、不创建 Nginx，只加入真实存在的外部网络。

1. 选择现有 Nginx 所在、仅包含受信服务的网络。`ERGENT_PROXY_NETWORK` 填网络的**实际名称**，不是另一个 Compose 文件中的局部键名；用 `docker network inspect 实际网络名` 核实。
2. 在 Nginx 自身配置中预留此网络上的固定 IP，确保位于 IPAM 子网内且无冲突；将该精确静态 IP 填入 `ERGENT_TRUSTED_PROXY`。不能填网关、Server IP、CIDR 或重建后变化的临时地址。
3. Nginx 后端改为 `http://ergent-server:7777`，保留 HTTPS、WebSocket 与覆盖 `X-Real-IP` 的配置。Nginx 容器内的 `127.0.0.1` 指向它自己。
4. 静态 upstream 域名可能缓存后端 IP；ErGent 首次启动或容器 IP 变化后，测试并重载现有 Nginx，或配置其版本支持的 Docker DNS 动态解析。

```dotenv
ERGENT_ORIGIN=https://terminal.example.com
ERGENT_TRUSTED_PROXY=REPLACE_WITH_NGINX_STATIC_IP
ERGENT_PROXY_NETWORK=REPLACE_WITH_EXISTING_NETWORK_NAME
```

替换占位符后，从仓库根目录执行：

```sh
docker compose --env-file .env -f deploy/compose.container-proxy.yaml up -d --build server
docker compose --env-file .env -f deploy/compose.container-proxy.yaml ps
docker compose --env-file .env -f deploy/compose.container-proxy.yaml \
  exec server /app/ergent-server --database /data/server.db setup-code
```

使用已发布镜像时改为 `pull server` 后 `up -d --no-build server`。[`external: true`](https://docs.docker.com/reference/compose-file/networks/#external) 不创建网络，也不替 Nginx 分配静态地址；这些由现有代理部署管理。后续所有管理、备份和升级命令也使用同一 Compose 路径。`expose` 不是防火墙，共享网络中的其他容器仍可访问后端。

三种示例的项目名都为 `ergent`。切换时不并行启动两套 Server，核实复用的数据卷；从代理模式切换到原生 TLS 时检查证书与端口，从原生 TLS 切换到代理模式时检查 TLS 终止与代理 IP。

## 仅限本机开发：显式 dev-http

[`deploy/compose.dev-http.yaml`](../deploy/compose.dev-http.yaml) 是独立的本机开发示例，不与其他 Compose 文件合并。它显式设置 `ERGENT_TLS_MODE=dev-http`，只发布 `127.0.0.1:7777`，origin 为 `http://127.0.0.1:7777`；可通过 `ERGENT_DEV_PORT` 同步调整宿主机端口和 origin。没有 TLS，不适用于公网、局域网或跨机器 Agent，不能把绑定地址改成 `0.0.0.0` 来绕过这一边界。使用默认值时无需证书或 `.env` 文件。

```sh
docker compose -f deploy/compose.dev-http.yaml up -d --build server
```

该示例使用单独项目名 `ergent-dev`，默认数据卷为 `ergent-dev_server-data`，避免将生产数据带入开发模式。不要通过覆盖项目名使它意外复用生产卷。

即使端口只发布到宿主机回环，Docker bridge 下的网页首次账户设置仍会拒绝非回环 TCP 对端。`dev-http` **不放宽** Origin、CSRF、登录保护或初始化校验。确需此模式时，通过已有 `admin-create` 管理命令从标准输入创建账户，而不是修改网页初始化规则。

在普通交互终端中执行：

```sh
docker compose -f deploy/compose.dev-http.yaml \
  exec server /app/ergent-server --database /data/server.db admin-create --username admin
```

`docker compose exec` 默认保留 TTY。程序会隐藏密码输入，输入密码后按 Enter，再输入一次确认并按 Enter；无需 Ctrl-D/Ctrl-Z，不把密码放入参数、环境变量或 shell 历史。两次输入必须一致，密码为 12–1024 个 UTF-8 字节。失败时按错误提示重试，不要假定账户已经创建。

### 兼容非交互 stdin 的脚本

已有管道自动化可继续通过 `exec -T ... admin-create` 从 stdin 提供**一次**密码，并关闭输入流（EOF）结束，非 TTY 分支不要求确认。人工操作优先使用上面的隐藏输入命令，不必使用脚本。确需兼容管道时，以下 Bash 示例隐藏输入、关闭跟踪和自动导出，使用临时变量传递后清理：

```bash
(
  set +x
  set +a
  unset ergent_password
  read -r -s -p 'Password (at least 12 UTF-8 bytes): ' ergent_password || exit 1
  printf '\n'
  printf '%s\n' "$ergent_password" | \
    docker compose -f deploy/compose.dev-http.yaml \
      exec -T server /app/ergent-server --database /data/server.db \
      admin-create --username admin
  result=$?
  unset ergent_password
  exit "$result"
)
```

这里的 `-T` 只用于将密码通过 stdin 传给 `admin-create`，不能用于领取 `setup-code`。密码不作为参数或环境变量，也不要手动把密码替换进命令、启用 `set -x`、保存终端录制或写进日志。只有这个非 TTY 兼容分支接收一次密码、不二次确认；正常交互 TTY 分支会确认。失败时检查错误并重新输入，不要继续假定账户已创建。

账户创建成功后打开上述本机 HTTP 地址登录；首次 CLI 创建会永久关闭网页首次初始化。原生 Agent 也只在同一主机连接此 loopback HTTP 地址，建议使用独立配置：

```sh
./ergent-client --config .local/dev-agent.json \
  enroll --server http://127.0.0.1:7777 --name Local-Dev
./ergent-client --config .local/dev-agent.json run
```

自定义 `ERGENT_DEV_PORT` 时同步修改 Agent 地址。注册令牌在交互终端中隐藏输入，按 Enter 完成。该开发入口不允许给远程 Agent 使用；实际部署改用原生 HTTPS 或已有 HTTPS 代理。

## 可选手动镜像发布（维护者）

`Docker image (manual)` workflow 仅手动触发，`publish` 默认 `false`，只构建不上传。开启 `publish: true` 前需得到项目维护者的明确发布授权；同一源码提交的 Docker validation 通过后才可发布 `linux/amd64` 和 `linux/arm64` 镜像。

目标仓库固定为 [Docker Hub 的 erquren/ergent](https://hub.docker.com/r/erquren/ergent)。在 GitHub 仓库的 **Settings → Secrets and variables → Actions** 配置 `DOCKERHUB_USERNAME`（Variable 或 Secret 均支持，Variable 优先）和 `DOCKERHUB_TOKEN`（Secret，使用对该仓库有写入权限的 Docker Hub 访问令牌）。fork 若需发布到其他镜像仓库，须先修改 workflow 中的镜像地址。

每次上传使用 `Cargo.toml` 中的项目版本作为镜像标签，并检查网页版本一致。例如项目版本为 `1.4.0`，无论从 `main` 分支还是 `v1.4.0` Git 标签手动运行，都会发布 `docker.io/erquren/ergent:1.4.0`；从 Git 标签运行时，仍要求标签恰好为 `v<项目版本>`。新发布不再生成 `sha-<提交SHA>` 标签，也不自动创建 `latest`。提交 SHA 保存在镜像的 `org.opencontainers.image.revision` 标签中，项目版本保存在 `org.opencontainers.image.version` 中。

同一项目版本再次发布会更新该 Docker 标签指向的镜像；发布新版本前同步修改 Rust 和网页版本。需要固定部署内容时使用发布摘要中的 digest。此修改不会删除历史 SHA 标签，不修改 GitHub Release/附件，也没有 tag/release 自动触发入口。

验证和正式发布均使用原生架构机器：AMD64 在 `ubuntu-24.04`，ARM64 在 `ubuntu-24.04-arm` 上构建，不使用 QEMU 模拟编译。正式发布先分别按 digest 上传两种架构的镜像，两者成功后再合并为同一个版本标签的多架构镜像索引；任一架构失败都不会进入版本标签合并步骤。工作流串行执行，避免多个发布同时更新标签。

发布 1.4.0 的步骤：

1. 提交并推送包含此 workflow 和 `scripts/docker_tags.py` 的修改，确保所选分支或标签包含这些文件，且 Rust 与网页版本均为 `1.4.0`。旧 Git 标签若不包含本次修改，仍会执行旧发布规则。
2. 在 GitHub Actions 的 `Docker image (manual)` 中选择目标分支（例如 `main`），勾选 `publish` 后运行。也可使用已登录 GitHub 的 CLI：

   ```sh
   gh workflow run docker-image.yml --ref main -f publish=true
   ```

3. 查看 GitHub Actions 中的 `Docker image (manual)`。双架构 `Docker validation` 全部通过后，两台原生机器分别构建并上传 `linux/amd64` 和 `linux/arm64`。最后的 `Publish version manifest` 合并并检查两种架构，运行摘要列出版本标签和镜像索引摘要。
4. 发布成功后验证镜像索引并拉取：

   ```sh
   docker buildx imagetools inspect erquren/ergent:1.4.0
   docker pull erquren/ergent:1.4.0
   ```

镜像索引应包含 `linux/amd64` 和 `linux/arm64`；Docker 会按运行机器的架构选择镜像。部署时可设置 `ERGENT_IMAGE=erquren/ergent:1.4.0`，推荐记录并使用运行摘要中实际发布的 digest。仅推送版本标签或公开 GitHub Release 不会上传 Docker 镜像。

## 常见问题

- **TLS 文件无法读取**：确认挂载路径存在，目录可遍历，文件为可读的独立副本，所有者为映射后的 UID/GID 10001。不要放宽原证书库权限，不要把私钥内容写进环境变量。
- **证书已续期但浏览器仍显示旧证书**：更新专用副本后重启 Server；使用 Nginx 时遵循其现有重载流程。再次核实域名、证书链与到期时间。
- **数据库写入/chmod 失败**：检查 `/data` 可写，目录及已有文件均为 UID/GID 10001 所有，并核实 bind mount/SELinux；不要删除数据库来“修复”。
- **初始化码缺失**：使用真正交互终端中的 `docker compose exec`，不加 `-T`；普通日志不会包含码，且首次初始化完成后不再提供。
- **`ORIGIN_REJECTED`**：使用与环境变量完全一致的 HTTPS origin，不从明文 bridge 后端进行首次设置，不关闭 origin 校验。
- **所有用户共享登录限流**：代理模式核实精确 TCP 对端和 `X-Real-IP` 覆盖；直接模式核实 Docker 网络是否保留源 IP。重启不会清除数据库中的未过期限制，需要时使用[带审计的 CLI 解锁](cli.md#登录保护与审计)。
- **网页能打开但终端断连**：检查 HTTPS 证书信任、协议版本；使用代理时再检查 WebSocket 和超时配置。
