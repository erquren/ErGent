# Ergent 发布包

首次使用请先阅读 [完整部署教程](docs/deployment.md)；自行编译见 [源码构建](docs/building.md)，全平台发布见 [发布流程](docs/releasing.md)。

本包包含 Server、Agent 和网页资源。同一包可以只运行 Server 或只运行 Agent。
运行不需要 Rust、Node.js、pnpm 或 Python；AI CLI（Codex / Claude Code / OpenCode）需要自行安装在 Agent 所在机器。

Codex 状态提示和后续工具适配接口见 [工具状态接入](docs/tool-events.md)。

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

登录失败按来源 IP 累计，达到 3 次后永久拒绝新登录，统一提示账号或密码错误；成功登录不清零失败数，重启不解封。
管理员执行 `./ergent-server --database .local/server.db login-unblock IP` 解封（Windows 使用 .exe）。
HTTPS 代理后必须指定 `--trusted-proxy 代理对端IP` 并由该代理覆盖 `X-Real-IP` 为真实客户端 IP；否则按代理 IP 共用失败计数。
