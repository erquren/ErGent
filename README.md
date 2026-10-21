<p align="center">
  <img src="apps/web/public/branding/logo.svg" alt="ErGent Logo" width="160" height="160" />
</p>

# ErGent

ErGent 是一个浏览器里的个人开发工作台，用于集中访问多台设备上的真实 Shell 终端。服务端（Server）提供网页和设备管理，客户端（Agent）运行在开发机器上并主动连接服务端；你可以在网页终端中使用该机器已安装的 Codex、Claude Code、OpenCode 等 CLI。

支持多设备、终端卡片、断线重连、单写入者接管，以及终端重命名、配色、明暗主题和中英文界面。

服务端程序为 `ergent-server`，客户端程序为 `ergent-client`；Windows 下分别为 `ergent-server.exe` 和 `ergent-client.exe`。

A personal development workspace in your browser, with native Agents exposing real shell terminals across your devices.

**当前版本：v1.4.0** · [下载发布包](https://github.com/erquren/ErGent/releases/tag/v1.4.0) · [Docker Hub](https://hub.docker.com/r/erquren/ergent)

原生包支持 Windows、Linux、macOS 的 x86_64 / ARM64；Docker 镜像支持 Linux amd64 / arm64，具体标签以实际发布结果为准。

## v1.4.0：手机终端操作与复制

- 手机端上下滑动浏览终端记录，点击终端不自动弹出输入法；使用常驻的“键盘 / 收起键盘”按钮控制输入，输入时收起顶部操作区以腾出显示空间。
- 手机和桌面均支持手动收起、展开左侧设备列表。
- 点击终端底部“复制”，在固定的文本面板中长按选取片段或复制全部；普通输出支持“当前画面”和“已有滚动记录”，保留代码缩进并合并终端自动折行。
- Codex 等应用使用全屏模式时，仅能复制当前画面；已覆盖、清除或丢失的内容不在复制范围内，也不代表完整 AI 对话记录。普通终端最多保留 2,000 行滚动历史，尺寸变化或重新同步可能重置历史。
- 支持局域网 / Tailscale HTTP 下的复制回退；浏览器拒绝一键复制时，可使用手机系统的选字复制菜单。

Docker 版本标签为 `erquren/ergent:1.4.0`，不自动提供 `latest`。部署、升级和终端历史边界见下方文档。

## 使用文档

- [服务端启动](docs/server.md)
- [客户端（Agent）启动](docs/agent.md)
- [Docker 部署](docs/docker.md)
- [命令与参数参考](docs/cli.md)
- [升级与平台要求](docs/upgrade.md)
- [开发、测试与 CI/CD 发布](docs/development.md)
- [运行行为与使用边界](docs/behavior.md)
