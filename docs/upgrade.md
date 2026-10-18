# 升级与平台要求

## 客户端程序更名

客户端二进制由 `ergent-agent` 更名为 `ergent-client`，Windows 使用 `ergent-client.exe`；服务端仍为 `ergent-server`。升级后请同步修改启动命令、后台服务和脚本中的程序路径。已有设备配置继续使用原来的 `.local/agent.json` 或自定义 `--config` 路径，无需重新注册。

如果之前安装过 Codex hooks，请移除其中指向旧客户端程序的 ErGent hook，再执行 `ergent-client setup-codex`，在 Codex `/hooks` 中审阅并信任新配置。

## 升级到 1.3.0

本版新增设备管理与移除、侧栏终端展开/收起和首页设备终端列表，移除终端工作目录展示。客户端程序更名为 `ergent-client`，升级时按上面的说明更新启动路径。

从 1.2.0 升级时保留原服务端数据库和客户端配置，沿用现有设备注册；协议仍为 `ergent.preview.v3`。只重启服务端时，持续运行的客户端会自动重连；替换并重启客户端会结束其原有终端进程，升级前请保存工作。重连与网页恢复边界见[客户端说明](agent.md#服务端掉线与自动重连)。

Docker 发布使用 `1.3.0` 版本标签，AMD64 与 ARM64 均在原生 runner 上构建。原生发布包及 Docker 镜像仍分别发布，部署方式见 [Docker 指南](docker.md)。

## 升级到 1.2.0（历史说明）

先保存终端中的工作，停止 Server/Agent，备份实际使用的数据库、Agent 配置和部署配置，再一起替换 Server、Agent、网页和启动脚本。停止 Agent 会结束其终端进程；不要让新旧 Server 同时访问同一数据库。首次启动会执行 `0005_first_account_setup.sql`，已有账户继续直接登录，网页首次初始化会永久关闭；空数据库通过一次性初始化码或 `admin-create` 创建首个账户。回退时使用旧程序和升级前同一时点的数据库备份。

本版新增原生 HTTPS、Agent 私有 CA 支持及 Docker Server 部署。跨机器连接使用 HTTPS；反向代理部署选择 `--tls-mode proxy`，直接提供 HTTPS 时选择 `--tls-mode direct` 并配置证书和私钥；私有 CA 在 Agent 注册时通过 `--ca-cert` 指定。本机明文测试使用 `--tls-mode dev-http`。启动和初始化见[服务端启动](server.md)，客户端证书配置见[客户端启动](agent.md#私有-ca)，容器部署见 [Docker 部署指南](docker.md)。

## 升级到 1.1.0（历史说明）

请先保存终端中的工作，停止所有 Server/Agent，并备份 `.local/`（使用自定义数据库或配置路径时也需备份对应文件）。Server、Agent 和网页须一起升级到使用 `ergent.preview.v3` 的版本；不要让新旧 Server 同时访问同一数据库。首次升级会将旧登录失败计数（包括永久封禁）清零并记录 `legacy_reset` 审计；回退旧版本需恢复升级前备份。登录限制迁移和审计查询详见[登录保护与审计](cli.md#登录保护与审计)。

## 平台要求

Linux GNU 包以 Ubuntu 22.04 构建（glibc 2.35+），需系统 CA 证书；不支持直接运行于 Alpine/musl。
macOS 构建最低部署目标为 11.0，尚未完成旧版系统实机验收。
Windows 使用 MSVC，需支持 ConPTY 的现代 Windows（Windows 10 1809+ / Windows 11），可能需要匹配架构的 Visual C++ Runtime。
当前提供可解压运行的预览包，不是安装器；未配置 Windows 签名、macOS 签名或公证。

发布包的 `manifest.json` 记录版本、Git 提交、工作区状态及文件校验值；外部 `.sha256` 校验压缩包。Docker 备份和回退见 [Docker 部署指南](docker.md#备份升级与回退)。
