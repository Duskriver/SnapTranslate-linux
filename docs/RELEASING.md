# 面向用户的发行

普通用户的交付物是预编译 `.deb`，不是仓库源码或开发安装脚本。

## 当前验证范围

- 本地已构建 amd64 `.deb`，用私有 dpkg 根目录验证安装、升级、移除和 purge，保留用户设置与合成密钥。
- 使用从安装包实际安装出的程序运行隔离 GTK 回归。
- APT 模拟安装检查本机依赖；依赖版本由 `dpkg-shlibdeps` 分析实际 ELF 生成，不根据发行版名称猜测。
- 当前本机环境为 Ubuntu 26.04 amd64。CI 配置为 Ubuntu 24.04 amd64/arm64，接入 GitHub 后须实际通过两个任务才能出发行草稿；本地测试不代替这两项 CI。
- 正式仓库为 `Duskriver/SnapTranslate-linux`，沿用 macOS 版本的 MIT 许可证；发行工作流仍要求 LICENSE 存在。公开 Release 以 GitHub 页面实际状态为准。

## 维护者生成安装包

需要 Rust 和构建依赖，以及 `python3 dpkg-dev binutils desktop-file-utils`。

```bash
cargo build --locked --release
python3 scripts/package.py
```

输出目录 `dist/` 包含 `.deb`、单包 SHA-256 和构建信息。安装包包含 `/usr/bin/snaptranslate-linux`、应用菜单、图标、用户说明和存在的项目许可证；不会携带构建目录、测试凭据或个人配置。

`Cargo.lock` 作为应用发行的一部分提交。`.deb` 的架构从 ELF 读取并与打包主机核对；版本从实际可执行文件核对；维护者不能把 amd64 文件改名成 arm64，或把旧二进制贴成新版本。

## 本地验证

安装生命周期测试不写宿主系统的 dpkg 数据库，也不安装到用户桌面：

```bash
python3 scripts/verify-package.py --gui dist/snaptranslate-linux_0.1.0-1_amd64.deb
```

普通用户运行测试需要 `bubblewrap`，通过用户命名空间在临时根目录执行真实 dpkg；CI 可以以 root 执行，仍强制使用临时 `--root`。测试允许私有 dpkg 数据库缺少系统依赖，实际依赖由软件包元数据、宿主 APT 模拟安装及包内程序 GUI 启动共同检查。不会跳过 GUI 的动态库加载。

升级测试使用同一应用版本、更高 Debian 修订号，验证覆盖安装和个人数据保留；不能据此声称未来任意版本的数据迁移已验证。软件包没有安装/卸载脚本，不会启动、终止旧实例或清除用户主目录。

## GitHub 工作流

`.github/workflows/release.yml` 的 pull request、主分支 push 和手动运行会：

1. 在 Ubuntu 24.04 的 amd64 和 arm64 原生 runner 编译、测试。
2. 构建 `.deb`，验证实际安装/升级/卸载和包内程序图形流程。
3. 保存可下载的 Actions artifacts。

推送与 `Cargo.toml` 版本一致的 `v版本` 标签还会在两个任务通过、LICENSE 存在后创建 **Release 草稿**，附上两个安装包、`SHA256SUMS`、构建依赖信息和用户安装说明。工作流不自动公开发布；维护者检查草稿后再点击 Publish。

首次正式发布前需完成：

- 仓库与 MIT 许可证已确定，发布时确认代码与 LICENSE 均已提交。
- 提交 Cargo.lock、应用代码、资源、打包与验证脚本，确保发布来自同一提交。
- 在 GitHub 实际通过两个架构的任务，检查安装包中的许可证、架构和依赖。
- 在对应桌面验收系统权限、原生快捷键、升级后重启行为。
- 从 README 移除“正式下载版本尚未发布”的状态说明，然后公开 Release。

已有公开 Release 的附件不会被工作流覆盖；修改已有版本请重新发布新的版本号/修订，而不是替换用户已下载的文件。

## 用户更新与卸载

用户下载新的 `.deb` 后由软件管理器升级，保留个人配置。正在运行的旧实例不会被自动终止，用户需先退出并在更新后重开。

本项目当前使用手动下载更新，不宣称已实现自动更新或全 Linux 发行版通用包。

## 从开发安装切换到发行包

如果此前运行过 `scripts/install.py`，用户级同名 `.desktop` 会优先于系统安装包的入口。切换前检查 `~/.local/share/applications/com.github.duskriver.snaptranslate.desktop` 的 `Exec`，确认它属于旧开发安装后移除该菜单文件，再从系统应用菜单打开发行版。可一并移除旧的 `~/.local/libexec/snaptranslate-linux`。不要删除保存设置和密钥的目录。

## 参考

- [GitHub runner 架构与环境](https://docs.github.com/en/actions/reference/runners/github-hosted-runners)
- [GitHub Release 草稿与附件](https://docs.github.com/en/repositories/releasing-projects-on-github/managing-releases-in-a-repository)
- [dpkg-deb 构建与所有权](https://manpages.debian.org/unstable/dpkg/dpkg-deb.1.en.html)
- [dpkg-shlibdeps 依赖计算](https://manpages.debian.org/unstable/dpkg-dev/dpkg-shlibdeps.1.en.html)
