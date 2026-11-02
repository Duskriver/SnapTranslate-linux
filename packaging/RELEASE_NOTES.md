## 下载与安装

普通用户不需要克隆源码、安装 Rust 或执行安装脚本。请下载本页附件中的 `.deb`，不要选择 GitHub 自动生成的 **Source code** 压缩包。

- **amd64.deb**：常见 Intel / AMD 64 位电脑。
- **arm64.deb**：运行 64 位 ARM Linux 的设备。

发行构建基于 Ubuntu 24.04。通过系统软件安装器打开 `.deb`，完成后从应用菜单启动 **SnapTranslate · 轻译**。若桌面没有关联安装器，可以在下载目录使用 `sudo apt install ./下载的文件.deb`。

首次打开填写自己的 API Key，在设置中确认截图快捷键。截图权限和原生快捷键支持取决于桌面环境。

## 更新与卸载

更新前退出正在运行的旧版，再安装新版 `.deb`；已保存的设置和密钥保留。本版本使用手动下载更新。

卸载可使用系统软件管理器或 `sudo apt remove snaptranslate-linux`。卸载不会清除个人数据；如需删除密钥，请先从应用设置中删除并保存。

## 校验

`SHA256SUMS` 包含安装包校验和，可在下载目录运行 `sha256sum --ignore-missing -c SHA256SUMS`。文件名以本页实际附件为准。
