# 客户端（Agent）启动

Agent 运行在实际开发机器上，以当前用户的权限启动 Shell，并主动连接 Server。先按[服务端启动](server.md)或 [Docker 部署](docker.md)准备服务端。

## 1. 准备客户端

从 [Releases](https://github.com/erquren/ErGent/releases) 下载匹配版本、操作系统和架构的原生包并解压。Agent、Server 和网页应使用匹配版本；AI CLI 和开发环境需安装在 Agent 机器上。

在网页中登录并创建一次性设备注册令牌，然后在开发机器的解压目录执行下列命令。

## 2. 注册设备

Linux / macOS：

```sh
./ergent-agent enroll --server https://terminal.example.com --name My-PC
```

Windows PowerShell：

```powershell
./ergent-agent.exe enroll --server https://terminal.example.com --name My-PC
```

将地址换成实际服务端地址，非默认 HTTPS 端口也须写明。按提示粘贴令牌后按 Enter，输入隐藏，无需 Ctrl-D/Ctrl-Z。

同机测试可将地址替换为 `http://127.0.0.1:7777`。跨机器连接（包括 Tailscale）使用 HTTPS；Agent 仅允许回环 HTTP。

## 3. 启动连接

Linux / macOS：

```sh
./ergent-agent run
```

Windows PowerShell：

```powershell
./ergent-agent.exe run
```

默认配置保存在当前工作目录下的 `.local/agent.json`。保持相同工作目录，或在注册和运行时使用相同的 `--config` 路径。已注册设备直接运行 `run`；使用新配置路径注册其他设备，避免覆盖已有凭据。

Agent 上线后，回到网页，在设备旁创建终端。后台服务需自行配置；关闭网页不结束终端，重启 Agent 或机器则无法恢复原终端进程。

## 私有 CA

使用自签名/私有 CA 时，注册时指定公共 CA 证书 PEM：

```sh
./ergent-agent --ca-cert /path/to/root-ca.pem \
  enroll --server https://terminal.example.com --name My-PC
./ergent-agent run
```

Agent 保存 CA 证书的绝对路径，运行时自动复用；文件须保持可读，更新后重启 Agent。浏览器也需信任对应 CA，使用 IP 连接时证书须包含匹配的 IP SAN。提供的是公共证书，不是服务器或 CA 私钥。

自定义配置路径、临时 CA 覆盖、诊断和 Codex 状态提示见[命令参考](cli.md#agent-命令)。
