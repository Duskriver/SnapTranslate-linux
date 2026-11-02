# SnapTranslate · 轻译 (Linux 原生版)

轻量、极简的 Linux 原生截屏翻译桌面应用。基于 **Rust + GTK4 / libadwaita** 构建，集成 DeepSeek 视觉模型流式翻译，具备极低待机资源占用与纯原生桌面交互。

* [SnapTranslate macOS 版](https://github.com/Duskriver/SnapTranslate-macos) 独立兄弟项目。

---

## 特性亮点

- **极致轻量**：纯 Rust 编写，无 Electron 或 WebKit 冗余引擎，待机内存仅约 10MB。
- **全桌面环境适配**：
  - 首选 **XDG Desktop Portal** 原生跨桌面安全截屏；
  - 自动探测 Wayland (`grim` + `slurp`) 与 X11 (`maim`) 极速选区工具链；
  - 支持 GNOME、KDE Plasma、Sway、i3、Hyprland 等全部主流桌面。
- **DeepSeek 多模态流式翻译**：
  - 异步 SSE 流式协议直连 DeepSeek Vision 接口；
  - 增量渲染 Markdown 排版（标题、加粗、行内代码、代码块、列表），保留排版美感。
- **安全凭据存储**：
  - 密钥严格保存在系统 Keyring（FreeDesktop Secret Service API）；
  - 无 Keyring 环境自动降级至严苛 `0600` POSIX 权限加密存储，杜绝命令行与日志明文泄露。
- **双模交互形态**：
  - **托盘常驻模式**：支持标准 StatusNotifierItem 系统托盘，点按菜单即可截屏或设置；
  - **CLI 快捷键唤起**：通过 `snaptranslate-linux capture` 命令行直调，可绑定任意系统全局快捷键，随用随起、按 `Esc` 即关。

---

## 快速构建与安装

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

```bash
git clone https://github.com/Duskriver/SnapTranslate-linux.git
cd SnapTranslate-linux
cargo build --release
```

编译出的可执行文件位于 `target/release/snaptranslate-linux`。

---

## 使用指南

### 1. 配置 DeepSeek API Key

首次启动时，应用会自动弹出设置对话框，也可通过命令行快速写入：

```bash
# 交互式安全输入 API Key（终端无明文回显）：
./target/release/snaptranslate-linux config set-key --stdin

# 查看当前配置状态（密钥自动遮罩显示）：
./target/release/snaptranslate-linux config get
```

### 2. 绑定系统全局快捷键（推荐）

在 Linux 桌面（如 GNOME / KDE / Sway / Hyprland）的 **系统设置 → 键盘 → 自定义快捷键** 中新增一条：
- **名称**：`SnapTranslate 截屏翻译`
- **命令**：`/usr/local/bin/snaptranslate-linux capture` （或你的二进制绝对路径）
- **快捷键**：例如 `Super + Shift + T` 或 `Alt + Shift + T`

按下快捷键后即可拖拽选区并立即展示流式译文浮窗，阅读完毕按 `Esc` 键一键退出。

---

## 运行自动化测试

本项目包含 146 项全面的单元测试、安全对抗测试及端到端（E2E）集成测试：

```bash
cargo test
```
