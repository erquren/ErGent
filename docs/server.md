# 服务端启动

Server 提供网页、账户、设备管理和终端连接入口。客户端运行在实际开发机器上，见[客户端启动](agent.md)。容器部署使用 [Docker 部署指南](docker.md)；本文介绍原生发布包。

## 1. 下载并解压

从 [Releases](https://github.com/erquren/ErGent/releases) 下载对应操作系统及架构的压缩包。包内包含 Server、Agent、网页和启动脚本，无需 Rust、Node.js、pnpm 或 Python。

进入解压目录执行下列命令。已有数据时继续使用原数据库，升级前先阅读[升级说明](upgrade.md)。

## 2. 选择启动方式

### 本机测试

Linux / macOS：

```sh
./start-server.sh --tls-mode dev-http
```

Windows PowerShell：

```powershell
./start-server.cmd --tls-mode dev-http
```

浏览器打开 **http://127.0.0.1:7777**。此模式仅用于本机回环 HTTP；默认数据库是包目录下的 `.local/server.db`。

启动脚本会切换到包目录并加载包内 `web/`，参数直接传给 `run`，无需再写 `run`。直接执行二进制时须显式指定网页目录：

```sh
./ergent-server run --web "$PWD/web" --tls-mode dev-http
```

Windows 对应 `./ergent-server.exe run --web "$PWD/web" --tls-mode dev-http`。直接运行二进制时，相对路径以当前工作目录为准。

### 原生 HTTPS

远程部署可直接使用已有 PEM 证书链和匹配私钥：

```sh
./start-server.sh --listen 0.0.0.0:7777 \
  --tls-mode direct --origin https://terminal.example.com:7777 \
  --tls-cert /path/to/fullchain.pem --tls-key /path/to/privkey.pem
```

替换域名和文件路径，按需开放端口，浏览器访问 **https://terminal.example.com:7777**。证书须匹配域名或 IP SAN，程序不自动申请或续期证书；更新证书后重启 Server。Windows 使用 `.cmd`，并将多行命令合并成一行。

### 已有 Nginx HTTPS 代理

由已有 Nginx 提供 HTTPS，原生 Server 仅监听本机 HTTP：

```sh
./start-server.sh --listen 127.0.0.1:7777 \
  --tls-mode proxy --origin https://terminal.example.com \
  --trusted-proxy 127.0.0.1
```

Nginx 将请求代理到 `http://127.0.0.1:7777`，须支持 WebSocket，并覆盖 `X-Real-IP`。可参考 [Nginx 配置示例](../deploy/nginx.host.conf)，替换域名和证书路径，合并到已有配置后测试并重载。浏览器和 Agent 都访问外部 HTTPS 地址。

这里的可信代理 `127.0.0.1` 仅适用于同机原生代理；Docker 网络使用 [Docker 代理方案](docker.md#可选复用宿主机-nginx)。

### 通过 SSH 隧道测试

Server 按本机测试模式运行。在浏览器所在机器执行并保持隧道运行：

```sh
ssh -N -o ExitOnForwardFailure=yes \
  -L 127.0.0.1:7777:127.0.0.1:7777 user@server
```

浏览器仍访问 **http://127.0.0.1:7777**。隧道本地端口须匹配浏览器地址和 `--origin`，目标端口须匹配 Server 的 `--listen`。

`--origin` 声明浏览器的精确“协议 + 主机 + 端口”，不会启用 TLS。`localhost` 与 `127.0.0.1` 不可混用；非默认端口须写明，地址不带路径、尾部 `/`、查询串或通配符。

## 3. 创建首个账户

空数据库启动时，程序在服务器真实控制终端显示一个 15 分钟有效的一次性初始化码，不写入普通日志或重定向输出。打开选定地址，填写初始化码、用户名及两次密码；密码须一致且为 12–1024 个 UTF-8 字节。

已有账户直接登录。首次账户创建后，网页初始化永久关闭，重启或删除账户不会重新开放。远程明文 HTTP 初始化会被拒绝；使用 HTTPS 或原生回环/SSH 访问。

后台运行、初始化码丢失或过期时，使用同一数据库执行 `setup-code`；也可通过 `admin-create` 创建账户。管理命令、交互输入和数据库路径说明见[命令参考](cli.md#账户与初始化)。

## 4. 连接客户端

登录网页后创建一次性设备注册令牌，按[客户端启动](agent.md)在开发机器上注册并运行 Agent。设备上线后，点击设备旁的 `+`，选择该机器上的目录创建终端。

后台服务需自行配置。路径、TLS 选项和登录保护参数见[命令参考](cli.md)。
