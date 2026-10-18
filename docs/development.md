# 开发、测试与发布

以下命令在源码仓库根目录执行。

## 从源码运行

需要 Rust 1.98.1、Node.js 22、pnpm 10.17.1 和 Python 3。

```sh
sh scripts/cargo.sh build --workspace --locked
sh scripts/pnpm.sh install --frozen-lockfile
sh scripts/pnpm.sh build
python3 scripts/dev.py
```

打开 **http://127.0.0.1:7777**，使用 `.local/dev/login.json` 中自动生成的账号和密码登录。点击设备旁的 `+`，选择本机目录即可创建终端。数据和凭据保存在 `.local/dev/`，请勿提交到版本控制。

## 测试与打包

```sh
sh scripts/cargo.sh fmt --all --check
sh scripts/cargo.sh clippy --workspace --all-targets --locked -- -D warnings
sh scripts/cargo.sh test --workspace --locked
sh scripts/cargo.sh run -p ergent-protocol --example export --locked
sh scripts/pnpm.sh build
sh scripts/pnpm.sh exec playwright install chromium
sh scripts/pnpm.sh test:e2e
python3 scripts/package.py --expect-version v1.3.0
```

本机打包当前平台，打包前会检查 Server、Agent、网页及指定标签的版本一致性。

## CI/CD 与发布

- 分支推送和 Pull Request 运行 `Check`，包含发布工具测试、Rust 格式/Clippy/测试、协议导出一致性及网页构建/E2E；Pull Request 另外运行六平台 `Package validation` 和双架构 `Docker validation`。
- 原生包发布使用小写 `v1.3.0` 标签，并确保标签指向包含上述版本和文档修改的提交；大写 `V1.3.0` 不会触发 Release。`Release` 先调用同一套 `Check`，全部通过后才构建、校验并汇总六个平台的包及 `SHA256SUMS`。
- Release 不存在时创建含校验和的草稿，维护者检查附件和发布说明后手动公开；已存在时只补传缺失附件，同名附件须校验一致，不覆盖已有文件。在分支上手动运行 `Release` 只生成 Actions 构建产物；在版本标签上手动运行也会执行 Release 附件上传。
- Docker 镜像通过独立的 `Docker image (manual)` 手动发布到 Docker Hub 的 `erquren/ergent`，默认只验证不上传；推送 `v1.3.0` 标签或公开原生包 Release 均不会自动发布镜像。分支或 Git 标签上的手动发布都使用项目版本作为镜像标签，例如 `1.3.0`；Rust 与网页版本必须一致，从 Git 标签运行时还要求其为 `v<项目版本>`。不再生成 SHA 镜像标签，不自动生成 `latest`；提交 SHA 保留在镜像元数据中。同版本再次发布会更新该标签，具体见 [Docker 发布说明](docker.md#可选手动镜像发布维护者)。
- Docker 验证与正式发布均在原生 AMD64/ARM64 runner 上分别构建，不使用 QEMU。正式发布按 digest 上传各架构镜像，两者成功后合并到版本标签，并检查镜像索引包含两种架构。
