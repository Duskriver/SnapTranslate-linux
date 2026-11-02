# SnapTranslate · 轻译 (Linux 原生版)

轻量、极简的 Linux 原生截屏翻译桌面应用。基于 **Rust + GTK4 / libadwaita** 构建，集成 DeepSeek 视觉模型流式翻译，具备极低待机资源占用与纯原生桌面交互。

使用流程参考 [SnapTranslate](https://github.com/Duskriver/SnapTranslate-macos)，本项目面向 Linux 原生桌面。

---

## 下载与安装（普通用户）

**当前状态：v0.1.0 双架构安装包已通过 CI，正式 GitHub 下载版本尚未公开发布。**

正式版本发布后，普通用户只需：

1. 打开本仓库的 **[Releases](https://github.com/Duskriver/SnapTranslate-linux/releases)**，下载附件中的 `.deb` 安装包。
2. 用系统的软件安装器打开并安装，然后在应用菜单搜索 **SnapTranslate · 轻译**。
3. 首次填写 API Key，并在设置中确认截图快捷键。

| 安装包架构 | 适用电脑 |
| --- | --- |
| `amd64.deb` | Intel / AMD 64 位电脑 |
| `arm64.deb` | 64 位 ARM Linux 设备 |

请选择 `.deb` 附件。GitHub 自动生成的 **Source code (zip / tar.gz)** 是源码，不是运行安装包。普通用户不需要 Git、Rust、Cargo，也不需要执行 Python 安装脚本。

发行工作流以 Ubuntu 24.04 为构建基线；其他 Debian 系发行版是否可安装由实际系统依赖决定。本地 Ubuntu 26.04 amd64 及 [Ubuntu 24.04 amd64 / arm64 CI](https://github.com/Duskriver/SnapTranslate-linux/actions/runs/41464015443) 均已完成安装生命周期和隔离图形流程验证。

没有图形安装器时，可在下载目录运行 `sudo apt install ./下载的文件.deb`，由 APT 处理依赖。

### 更新与卸载

更新前从托盘或设置中退出旧版，下载安装新版 `.deb`，再从应用菜单打开。更新保留设置与密钥；当前通过 Releases 手动下载更新。

卸载请使用系统软件管理器，或 `sudo apt remove snaptranslate-linux`。卸载保留个人数据；如需删除 API Key，请先在应用设置中删除并保存。

维护者构建、校验和发布方式见 [发行说明](docs/RELEASING.md)。

---

## 特性亮点

- **极致轻量**：纯 Rust 与 GTK4 原生界面，无 Electron 或 WebKit 运行时；资源占用需按实际桌面测量。
- **全桌面环境适配**：
  - 首选 **XDG Desktop Portal** 原生跨桌面安全截屏；
  - 自动探测 Wayland (`grim` + `slurp`) 与 X11 (`maim`) 极速选区工具链；
  - 实际可用后端与桌面的 Portal、截图权限及已安装工具有关。
- **DeepSeek 多模态流式翻译**：
  - 异步 SSE 流式协议直连 DeepSeek Vision 接口；
  - 流式追加译文，完成后统一进行 Markdown 排版，阅读前文时保留滚动位置。
- **安全凭据存储**：
  - 密钥严格保存在系统 Keyring（FreeDesktop Secret Service API）；
  - 无 Keyring 环境自动降级至受 `0600` 文件权限保护的本地凭据文件。
- **双模交互形态**：
  - **托盘常驻模式**：支持标准 StatusNotifierItem 系统托盘，点按菜单即可截屏或设置；
  - **CLI 快捷键唤起**：通过 `snaptranslate-linux capture` 命令行直调，可绑定系统全局快捷键；已有实例会接收新的截图命令，按 `Esc` 关闭当前结果并取消它的请求。

---

## 使用指南

### 1. 首次使用

从应用菜单打开 SnapTranslate，在设置中填入自己的 API Key 并保存，再点击「立即截屏翻译」。按桌面提示允许截屏权限，框选需要翻译的区域。服务调用使用自己的账户计费。

### 2. 设置全局快捷键

在应用设置的「全局快捷键」中点击「设置系统快捷键」。支持 GlobalShortcuts Portal 的桌面会打开系统授权界面；确认后，应用显示系统实际分配的组合键。取消授权不会显示已启用。系统支持原生修改时可点击「更改系统快捷键」。首次设置仅由此按钮触发；重启后尝试恢复已经授权的绑定。

未提供此接口的桌面可以手动绑定：

在 Linux 桌面（如 GNOME / KDE / Sway / Hyprland）的 **系统设置 → 键盘 → 自定义快捷键** 中新增一条：
- **名称**：`SnapTranslate 截屏翻译`
- **命令**：`/usr/bin/snaptranslate-linux capture` （或你的二进制绝对路径）
- **快捷键**：例如 `Super + Shift + T` 或 `Alt + Shift + T`

也可在应用设置的快捷键引导中点击「复制命令」，得到当前程序的完整路径；包含空格的路径也会正确引用。引导根据当前桌面提供配置步骤，并可打开受支持桌面的键盘设置。全局组合键由桌面管理，绑定时需处理系统提示的冲突。后续启动了 Portal 服务时可点击「重新检测系统快捷键」。

按下快捷键后进入桌面的截图选择流程，再次按快捷键可取消当前框选。截图期间暂时隐藏设置和已有结果，结束或取消后恢复，避免把本应用窗口截入图片。选区完成后展示流式译文。阅读完毕按 `Esc` 关闭当前结果，应用继续常驻；可以再次截图并保留多个结果窗口。关闭一个结果只取消它对应的请求，不影响其他结果。

临时覆盖本次目标语言可用 `snaptranslate-linux capture --lang en`，不修改保存的默认语言。翻译失败会在结果窗口说明原因，中途断流会保留已收到的译文并提示不完整。

失败后可点击「检查 API Key」或「检查模型与接口」，直接进入对应设置。保存后回到原结果点击「原图重试」，使用最新保存的密钥、模型和接口重新翻译同一截图，沿用该结果的目标语言。重试不会再次框选，也不会新开结果窗口；未保存的设置会阻止重试。

关闭设置窗口也会保留常驻进程。退出请使用托盘的「退出 SnapTranslate」、设置中的「退出应用」，或在应用窗口内按 `Ctrl+Q`。桌面不显示托盘时，重新运行程序可打开设置。

### 3. 阅读、复制与连续截图

结果窗口显示截图序号和目标语言，支持拖动边缘调整大小。点击「再次截图」会沿用该结果的目标语言；「设置」打开同一个设置窗口。

- 同时最多运行两个翻译请求，其余结果显示等待状态；可点击「取消等待」或关窗取消，等候期间不会上传截图。
- 译文保留普通换行和表格的行列关系；Markdown 链接保留网址，可用 `Ctrl+单击` 打开 HTTP、HTTPS 或邮件链接。
- 译文持续追加时，位于末尾会自动跟随；向上滚动后保持阅读位置，点击「回到底部」可继续跟随。
- 「复制译文」或 `Ctrl+Shift+C` 复制当前显示的全部文字；选中文本后按 `Ctrl+C` 复制选区。完成排版后复制的文字不含 Markdown 标记。
- `Esc` 或 `Ctrl+W` 关闭当前结果，`Ctrl+,` 打开设置，`Ctrl+Shift+T` 按保存的默认语言再次截图。这些组合在应用窗口内生效。
- 翻译期间可点击「停止翻译」，结束当前请求并保留已收到的文字，继续阅读或复制；之后可点「原图重试」。重试开始后替换旧译文，连续点击不会同时发起多个请求。

用于重试的截图只在结果窗口打开期间保留在内存中，关闭该窗口后释放，不保存为截图历史。重试由你主动触发，会再次向当前配置的服务发送该截图。

### 4. 设置反馈

目标语言可从常用语言中选择，也支持自定义。展开高级设置可调整模型与接口地址。

修改后点击「保存设置」才生效；「取消修改」重新载入已保存的设置。删除密钥会先标记为待删除，可以撤销，保存后才实际删除。关闭窗口或退出应用时，未保存的修改会提示处理；有未保存修改时请先保存或取消，再开始截图。

密钥状态「已保存（未验证）」表示已写入本机存储，认证结果由实际请求反馈。配置文件损坏、输入无效、存储失败或外部配置发生变化都会显示原因，避免覆盖尚未核对的设置。

---

## 从源码构建（开发者）

### 1. 安装系统依赖

在 Ubuntu / Debian 系统：
```bash
sudo apt update
sudo apt install -y build-essential libgtk-4-dev libadwaita-1-dev libsecret-1-dev
```

在 Arch Linux / Manjaro：
```bash
sudo pacman -S base-devel gtk4 libadwaita libsecret
```

在 Fedora：
```bash
sudo dnf install -y gcc gtk4-devel libadwaita-devel libsecret-devel
```

### 2. 编译 Release 二进制文件

克隆本仓库后，在项目目录执行：

```bash
cargo build --locked --release
```

编译出的可执行文件位于 `target/release/snaptranslate-linux`。

源码开发时，可选安装到当前用户的应用菜单（需要 Python 3；发行包用户无需执行）：

```bash
python3 scripts/install.py
```

安装器将程序复制到 `~/.local/libexec`，写入应用菜单入口和图标，不需要 root。菜单支持直接启动「截屏翻译」。安装后的稳定应用标识也用于系统快捷键授权；更新后退出旧实例，再从应用菜单打开。


---

## 运行自动化测试

运行底层逻辑、协议和命令行回归测试：

```bash
cargo test --locked
```

真实 GTK 应用流程另用隔离图形会话验证。Ubuntu / Debian 测试依赖为 `xvfb xauth dbus-daemon python3-gi python3-dbus gir1.2-atspi-2.0 gir1.2-gtk-3.0 gir1.2-gtk-4.0 at-spi2-core libxtst6`：

```bash
cargo build --locked
/usr/bin/python3 scripts/verify-gui-flow.py
```

脚本使用独立 Xvfb、D-Bus、临时配置、截图样本和本地模拟翻译服务，检查窗口中的实际文字、托盘操作、命令转发、关窗后的网络取消，以及设置、剪贴板、滚动和失败恢复交互；不使用真实密钥或实际桌面截图。可用 `--binary target/release/snaptranslate-linux` 验证 Release 构建，`--ux-only` 只检查日常交互，`--recovery-only` 检查停止与原图重试，`--parity-only` 检查快捷键、排队与框选取消，`--native-only` 调用真实 maim 在隔离桌面框选生成的画面，`--keep-artifacts` 保留日志和窗口截图。验证范围见 [TEST_READY.md](TEST_READY.md)。

## 许可证

本项目与 macOS 版本一致，采用 [MIT License](LICENSE)。
