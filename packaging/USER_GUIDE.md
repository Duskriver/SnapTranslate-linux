# SnapTranslate · 轻译

从 GitHub Releases 下载适合 CPU 架构的 `.deb` 文件，通过系统的软件安装器打开并安装，然后在应用菜单搜索 SnapTranslate。

首次打开：填写你自己的 API Key，保存后点击“立即截屏翻译”。可在设置中授权全局快捷键；桌面不支持时按页面说明手动绑定。

- 框选区域后开始翻译；再次触发截图可取消当前框选。
- Esc 关闭当前结果，应用继续常驻；退出请使用托盘或设置中的“退出应用”。
- 失败后可检查设置并“原图重试”，无需再次框选。
- 普通用户不需要 Git、Rust、Cargo 或安装脚本。

## 更新

退出正在运行的 SnapTranslate，下载并安装新版本 `.deb`，再从应用菜单打开。软件包覆盖程序文件，保留用户设置和密钥。

本应用没有自动更新；GitHub Releases 是检查新版和下载安装包的入口。

## 卸载

在系统软件管理器中移除 SnapTranslate；命令行也可执行 `sudo apt remove snaptranslate-linux`。

卸载不会删除个人设置和 API Key。若希望清除密钥，请在卸载前从应用设置中删除并保存。配置文件位于 `$XDG_CONFIG_HOME/snaptranslate`（默认 `~/.config/snaptranslate`），界面状态默认位于 `~/.local/state/snaptranslate`。

## 命令行备用安装方式

在下载目录运行 `sudo apt install ./snaptranslate-linux_版本_架构.deb`，让 APT 一并处理依赖。

当前发行工作流面向 Ubuntu 24.04 及更新版本，提供 amd64、arm64 原生构建。其他 Debian 系桌面由软件包的实际依赖决定能否安装，未完成对应系统验收前不承诺兼容。首次截屏的权限和快捷键授权由桌面系统管理。
