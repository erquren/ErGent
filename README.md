<p align="center">
  <img src="apps/web/public/branding/logo.svg" alt="ErGent Logo" width="160" height="160" />
</p>

# ErGent

ErGent 是一个浏览器里的个人开发工作台，用于集中访问多台设备上的真实 Shell 终端。服务端（Server）提供网页和设备管理，客户端（Agent）运行在开发机器上并主动连接服务端；你可以在网页终端中使用该机器已安装的 Codex、Claude Code、OpenCode 等 CLI。

支持多设备、终端卡片、断线重连、单写入者接管，以及终端重命名、配色、明暗主题和中英文界面。

服务端程序为 `ergent-server`，客户端程序为 `ergent-client`；Windows 下分别为 `ergent-server.exe` 和 `ergent-client.exe`。

A personal development workspace in your browser, with native Agents exposing real shell terminals across your devices.

**当前版本：v1.3.0** · [下载发布包](https://github.com/erquren/ErGent/releases) · [Docker Hub](https://hub.docker.com/r/erquren/ergent)

原生包支持 Windows、Linux、macOS 的 x86_64 / ARM64；Docker 镜像支持 Linux amd64 / arm64，具体标签以实际发布结果为准。

## 使用文档

- [服务端启动](docs/server.md)
- [客户端（Agent）启动](docs/agent.md)
- [Docker 部署](docs/docker.md)
- [命令与参数参考](docs/cli.md)
- [升级与平台要求](docs/upgrade.md)
- [开发、测试与 CI/CD 发布](docs/development.md)
- [运行行为与使用边界](docs/behavior.md)
