# 客户端（Agent）启动

Agent 运行在实际开发机器上，以当前用户的权限启动 Shell，并主动连接 Server。先按[服务端启动](server.md)或 [Docker 部署](docker.md)准备服务端。

## 1. 准备客户端

从 [Releases](https://github.com/erquren/ErGent/releases) 下载匹配版本、操作系统和架构的原生包并解压。Agent、Server 和网页应使用匹配版本；AI CLI 和开发环境需安装在 Agent 机器上。

在网页中登录并创建一次性设备注册令牌，然后在开发机器的解压目录执行下列命令。

## 2. 注册设备

Linux / macOS：

```sh
./ergent-client enroll --server https://terminal.example.com --name My-PC
```

Windows PowerShell：

```powershell
./ergent-client.exe enroll --server https://terminal.example.com --name My-PC
```

将地址换成实际服务端地址，非默认 HTTPS 端口也须写明。按提示粘贴令牌后按 Enter，输入隐藏，无需 Ctrl-D/Ctrl-Z。

同机测试可将地址替换为 `http://127.0.0.1:7777`。跨机器连接（包括 Tailscale）使用 HTTPS；Agent 仅允许回环 HTTP。

## 3. 启动连接

Linux / macOS：

```sh
./ergent-client run
```

Windows PowerShell：

```powershell
./ergent-client.exe run
```

默认配置保存在当前工作目录下的 `.local/agent.json`。保持相同工作目录，或在注册和运行时使用相同的 `--config` 路径。已注册设备直接运行 `run`；使用新配置路径注册其他设备，避免覆盖已有凭据。

Agent 上线后，回到网页，在设备旁创建终端。后台服务需自行配置；关闭网页不结束终端，重启 Agent 或机器则无法恢复原终端进程。

## 服务端掉线与自动重连

`ergent-client run` 在服务端掉线或重启后会自动重连。只要客户端进程和设备仍在运行，已有终端及其中的任务不会因服务端断线而被结束；断线期间网页无法输入命令或接收新输出。无需为恢复连接而重启客户端。

每次连接结束后，客户端等待约 1、2、4、8、16、30 秒再重试，之后保持约 30 秒，每次额外加入不足 0.5 秒的随机延迟。等待时间不包含断线检测和连接尝试耗时，恢复连接也不会重置本次客户端运行期间的退避计数，因此服务端上线后不一定立即显示设备在线。

服务端恢复后，客户端使用原有凭据自动认证，并重新上报已有终端。仍在运行的 Shell 保留工作目录和进程状态。前提是服务端地址、证书信任和协议仍兼容，且原数据库中的设备凭据有效；Docker 重建应沿用原数据卷。空数据库或已撤销的设备凭据不能通过自动重连恢复接入。

当前网页断线后约 1.5 秒检查登录状态；如果此时服务端仍不可用，检查失败会返回登录页，不会一直在原终端页面等待。服务端恢复后刷新页面，必要时重新登录，再选择原终端即可。这不代表客户端停止或终端任务被结束。

每个终端只保留最近 512 KiB 原始输出缓存。符合续传条件时补齐缺失输出，否则恢复当前屏幕，不能保证补齐全部滚动历史；断线时未确认的输入不会自动重发。客户端或设备自身重启不能恢复原终端进程。详见[终端恢复与保留边界](../TERMINAL_RECOVERY.md)。

## 私有 CA

使用自签名/私有 CA 时，注册时指定公共 CA 证书 PEM：

```sh
./ergent-client --ca-cert /path/to/root-ca.pem \
  enroll --server https://terminal.example.com --name My-PC
./ergent-client run
```

Agent 保存 CA 证书的绝对路径，运行时自动复用；文件须保持可读，更新后重启 Agent。浏览器也需信任对应 CA，使用 IP 连接时证书须包含匹配的 IP SAN。提供的是公共证书，不是服务器或 CA 私钥。

自定义配置路径、临时 CA 覆盖、诊断和 Codex 状态提示见[命令参考](cli.md#agent-命令)。
