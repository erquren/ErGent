<p align="center">
  <img src="apps/web/public/branding/logo.svg" alt="ErGent Logo" width="160" height="160" />
</p>

# ErGent

ErGent 是一个浏览器里的个人开发工作台，用于集中访问多台设备上的真实 Shell 终端。服务端（Server）提供网页和设备管理，客户端（Agent）运行在开发机器上并主动连接服务端；你可以在网页终端中使用该机器已安装的 Codex、Claude Code、OpenCode 等 CLI。

支持多设备、终端卡片、断线重连、单写入者接管，以及终端重命名、配色、明暗主题和中英文界面。

服务端程序为 `ergent-server`，客户端程序为 `ergent-client`；Windows 下分别为 `ergent-server.exe` 和 `ergent-client.exe`。

A personal development workspace in your browser, with native Agents exposing real shell terminals across your devices.

**当前发布版：v1.6.0** · [下载发布包](https://github.com/erquren/ErGent/releases/tag/v1.6.0) · [v1.6.0 源码](https://github.com/erquren/ErGent/tree/v1.6.0) · [Docker Hub](https://hub.docker.com/r/erquren/ergent)

原生包支持 Windows、Linux、macOS 的 x86_64 / ARM64；Docker 镜像支持 Linux amd64 / arm64，具体标签以实际发布结果为准。

## v1.6.0：Codex 类终端切换与查询恢复

- 修复 Codex 类终端应用在切换终端、恢复画面或没有网页观看时，因等待光标位置查询回复而停滞的问题。客户端（Agent）统一处理光标位置、终端状态和基本设备属性查询，不再依赖网页是否正在观看。
- 已处理的查询不会在网页回放时重复应答，多个窗口观看同一终端也不会重复发送回复；保留单写入者租约和输入不自动重发规则。
- 修复快照恢复在行末延迟换行时可能污染文字颜色的问题。
- 补充真实 PTY 与 Codex 类 TUI 回归，覆盖无人观看、快速切换、尺寸变化、多个窗口观看，以及恢复后的输入和差分重绘。

### 下载与校验

从 [v1.6.0 发布页](https://github.com/erquren/ErGent/releases/tag/v1.6.0) 下载与运行机器匹配的原生包。每个包均包含 Server、Agent、网页和启动脚本；可以分别在服务端机器和开发机器上运行所需组件。

- Windows：`ergent-1.6.0-windows-x86_64.zip` / `ergent-1.6.0-windows-arm64.zip`
- Linux：`ergent-1.6.0-linux-x86_64.tar.gz` / `ergent-1.6.0-linux-arm64.tar.gz`
- macOS：`ergent-1.6.0-macos-x86_64.tar.gz`（Intel）/ `ergent-1.6.0-macos-arm64.tar.gz`（Apple Silicon）

发布页同时提供每个包的 `.sha256` 和汇总 `SHA256SUMS`。发布流水线已通过三系统 Rust 检查、137 项浏览器测试、发布工具测试，以及六个平台的原生测试、打包和完整性校验：[查看发布检查](https://github.com/erquren/ErGent/actions/runs/42764502241)。回归使用 Codex 类 TUI 测试程序，不代表对所有 CLI 版本和使用场景的保证。

### 从 1.5.0 升级

1. 备份实际使用的数据库、Agent 配置和部署配置，先保存或结束终端任务。**重启 Agent 会结束它管理的运行中终端。**
2. 使用对应平台的 1.6.0 原生包更新 Server、网页和 Agent，继续使用原数据库路径与 Agent 配置。无需重新注册，没有新增数据库迁移，协议仍为 `ergent.preview.v3`。
3. 启动后刷新网页。查询修复在 Agent 中，**仅更新网页不能获得完整修复**；发布新包不会自动升级或重启已有设备。

详细步骤见 [v1.6.0 升级文档](https://github.com/erquren/ErGent/blob/v1.6.0/docs/upgrade.md) 和 [发布包使用说明](https://github.com/erquren/ErGent/blob/v1.6.0/scripts/release/README.md)。从源码构建本发布版时请使用 `v1.6.0` 标签，不要假定默认分支与已发布包完全一致。

Docker 镜像独立发布；本次原生包发布不代表 `erquren/ergent:1.6.0` 镜像已可用，标签以实际发布结果为准。

## v1.5.0：终端翻页与弹窗交互（历史说明）

- 触控设备增加“上一页 / 下一页 / 回到最新”：普通终端翻看本地保留的记录，全屏应用发送 PageUp、PageDown、End；翻页不会自动打开键盘。
- Ctrl、Ctrl+C 和 Tab 收入“更多”，主要快捷键保持同一行；桌面鼠标设备隐藏触控快捷键栏。
- 收紧顶部栏和终端标题区，增加可用终端显示空间。
- 统一修复 8 类模态弹窗的误关闭：点击外部或从内部拖到外部松开都不会关闭，使用弹窗内按钮或 Esc 退出。

Docker 版本标签为 `erquren/ergent:1.5.0`，不自动提供 `latest`。部署、升级和终端历史边界见下方文档。

## v1.4.0：手机终端操作与复制（历史说明）

- 手机端上下滑动浏览终端记录，点击终端不自动弹出输入法；使用常驻的“键盘 / 收起键盘”按钮控制输入，输入时收起顶部操作区以腾出显示空间。
- 手机和桌面均支持手动收起、展开左侧设备列表。
- 点击终端底部“复制”，在固定的文本面板中长按选取片段或复制全部；普通输出支持“当前画面”和“已有滚动记录”，保留代码缩进并合并终端自动折行。
- Codex 等应用使用全屏模式时，仅能复制当前画面；已覆盖、清除或丢失的内容不在复制范围内，也不代表完整 AI 对话记录。普通终端最多保留 2,000 行滚动历史，尺寸变化或重新同步可能重置历史。
- 支持局域网 / Tailscale HTTP 下的复制回退；浏览器拒绝一键复制时，可使用手机系统的选字复制菜单。

## 使用文档

- [服务端启动](docs/server.md)
- [客户端（Agent）启动](docs/agent.md)
- [Docker 部署](docs/docker.md)
- [命令与参数参考](docs/cli.md)
- [升级与平台要求](docs/upgrade.md)
- [开发、测试与 CI/CD 发布](docs/development.md)
- [运行行为与使用边界](docs/behavior.md)
