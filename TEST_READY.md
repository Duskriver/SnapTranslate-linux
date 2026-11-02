# 核心流程与日常交互验证

## 已完成的验证

本轮以当前工作区构建运行，不调用真实 DeepSeek API，不读取用户密钥，不截图实际桌面。

- `cargo test --locked`：174 项测试通过，覆盖配置、CLI、截图后端、凭据文件、Markdown、设置校验、恢复提示、截图释放与网络协议。
- `cargo build --locked --release`：Release 构建通过。
- `scripts/verify-gui-flow.py`：启动真实 GTK 应用，并通过 AT-SPI 读取窗口中的内容、通过 D-Bus 操作托盘、通过本地 HTTP 服务观察请求和断连。
- 最终 Release 全流程通过，共 23 次本地 HTTP 请求，包含核心操作、日常交互、错误恢复，以及本轮新增的排队、框选取消和系统快捷键场景。
- 真实 X11 选区使用系统 `maim` 与真实鼠标拖选，在独立 Xvfb 上框选生成的测试窗口；检查上传 PNG 的尺寸和实际结果窗口。用户桌面、真实密钥和真实 API 均不参与。
- 安装脚本在临时目录验证，包含中文、空格及 shell 特殊字符路径；生成的应用入口通过 `desktop-file-validate`，复制的程序可实际启动。


核心场景包括：

| 场景 | 验收行为 |
| --- | --- |
| 冷启动 `capture` | 截图任务不会因尚无窗口提前退出；结果窗口显示译文 |
| 关闭窗口后再次截图 | 主进程仍然存活；后续命令转发到同一个实例 |
| `capture --lang en` | 本次请求携带覆盖后的语言 |
| 401 等 API 失败 | 窗口显示密钥错误，不以空白窗口冒充完成 |
| 流在中途结束 | 已有译文保留，状态明确提示失败 |
| 两个独立结果 | 关闭 A 会断开 A 的网络请求，B 仍可完成 |
| 用户取消截图 | 不发出翻译请求，不创建错误窗口 |
| 托盘菜单 | 设置、截图、退出均能操作 |
| 缺少密钥 | 打开设置引导配置，不发出网络请求 |
| Esc 关闭结果 | 真实键盘事件关闭当前结果并取消请求 |
| 翻译期间退出 | 应用正常退出，服务端观察到连接关闭 |
| 截图选区期间退出 | 截图子进程被终止并回收 |
| 桌面没有托盘 | 显示提示，关闭设置后仍可重新打开，并可从设置退出 |

日常交互通过真实 GTK 控件、键鼠事件和系统剪贴板验证：

| 场景 | 验收行为 |
| --- | --- |
| 快捷键引导 | 复制的命令包含当前程序的绝对路径，并能正确解析 |
| 语言与设置 | 常用和自定义语言可保存；非法输入不落盘；取消与放弃修改恢复保存值 |
| 密钥删除 | 删除先暂存，撤销和取消不删除已存凭据 |
| 未保存时截图 | 结果按钮和转发命令均提示先保存或取消，不发出 HTTP 请求 |
| 连续截图 | 修改默认语言后，旧结果的“再次截图”仍使用该结果的语言 |
| 复制译文 | 按钮复制排版后的文字；键盘与鼠标选区可复制；Ctrl+Shift+C 复制全部 |
| 长译文 | 自动跟随新内容，上翻后保留阅读位置，回到底部恢复跟随，完成排版不跳走 |
| 窗口与退出 | Ctrl+W 关闭结果；关闭确认期间的退出请求仍生效，“继续编辑”清除退出意图 |

失败恢复场景包括：

| 场景 | 验收行为 |
| --- | --- |
| 401 与 404 | 分别定位密钥、模型与接口设置；相关字段可直接通过键盘编辑 |
| 修改配置后原图重试 | 请求实际携带新密钥、模型和接口路径，保留原窗口编号与原截图，截图工具调用次数不增加 |
| 未保存时重试 | 显示先保存或取消的提示，不发请求，不丢失现有结果 |
| 快速重复点击 | 同一窗口仅发起一个请求 |
| 停止翻译 | 服务端观察到断连，窗口和部分译文保留，仍可复制 |
| 停止后重试 | 在同一个窗口以新译文替换旧内容 |
| 关闭重试窗口 | 仅取消该请求，另一窗口继续完成；关闭会话释放重试闭包持有的截图 |

本轮对标新增场景：

| 场景 | 验收行为 |
| --- | --- |
| 网络排队 | 同时最多两条 HTTP 请求；后续窗口等待；取消或关闭等待窗口不上传；释放槽位后继续 |
| 再次触发截图 | 取消当前选区，原生子进程被终止并回收；Portal 请求收到 `Request.Close` |
| 本应用窗口 | 框选期间隐藏已有结果和设置，成功或取消后恢复，不终止原有翻译 |
| 系统快捷键 | 授权取消不假报成功，显示系统实际组合，修改后同步更新，激活触发截图，重复事件去重 |
| 快捷键恢复 | 重启恢复已授权绑定；会话关闭显示未启用；退出关闭系统会话 |
| 截图错误 | 权限拒绝与取消分别处理；权限拒绝不会静默转向其他截图工具 |
| 阅读 | 普通换行、表格行列及空单元格保留；Ctrl+单击打开链接；测试用私有 URL 处理器，未访问外网 |

## 复现方式

```bash
cargo test --locked
cargo build --locked --release
/usr/bin/python3 scripts/verify-gui-flow.py --binary target/release/snaptranslate-linux --keep-artifacts
# 只复查日常交互：
/usr/bin/python3 scripts/verify-gui-flow.py --binary target/release/snaptranslate-linux --ux-only --keep-artifacts
# 只复查本轮新增路径：
/usr/bin/python3 scripts/verify-gui-flow.py --binary target/release/snaptranslate-linux --parity-only --keep-artifacts
# 仅调用真实 X11 工具（需安装 maim）：
/usr/bin/python3 scripts/verify-gui-flow.py --binary target/release/snaptranslate-linux --native-only --keep-artifacts
# 只复查停止、设置定位和原图重试：
/usr/bin/python3 scripts/verify-gui-flow.py --binary target/release/snaptranslate-linux --recovery-only --keep-artifacts
```

Ubuntu / Debian 的 GUI 测试依赖：`xvfb xauth dbus-daemon python3-gi python3-dbus gir1.2-atspi-2.0 gir1.2-gtk-3.0 gir1.2-gtk-4.0 at-spi2-core libxtst6`。

`--keep-artifacts` 会输出临时日志目录，包含设置与结果窗口截图；脚本结束时清理它启动的应用、Xvfb 和 D-Bus 进程。未指定时自动清理日志。

## 验证边界

这是完整应用在隔离桌面、固定截图、真实 X11 选区和模拟服务上的验证。全局快捷键与截图权限的 Portal 接线由隔离 D-Bus 服务验证。真实 GNOME/KDE/Wayland 的 Portal 权限、选区交互、多显示器以及真实 API 的速度与翻译质量，仍需在对应桌面人工验收。

命令行测试中的不可用显示器应当返回失败。原先通过 `/dev/null` D-Bus 地址直接跳过 GUI 并返回成功的分支已删除；仅有命令行测试通过不能证明截图体验可用。

## 发行包验证

本轮生成 `dist/snaptranslate-linux_0.1.0-1_amd64.deb`，本地 Ubuntu 26.04 amd64 验证通过：

- 实际安装进私有 dpkg 根目录，应用菜单和图标存在，程序可启动。
- 包内可执行文件跑完整 GTK 套件，23 次本地 HTTP 请求通过，包含真实 maim 选区。
- 实际覆盖升级、更高 Debian 修订号、移除和 purge 均通过；个人配置和合成密钥保留。
- 软件包所有文件为 root:root，文件模式与安装后 MD5 检查通过；SHA-256 与成品一致。
- 宿主 APT 模拟安装依赖解析通过，没有修改真实安装状态。
- README 改为用户下载安装优先，源码构建/开发脚本单列。

GitHub CI 的 Ubuntu 24.04 amd64/arm64 构建与 Release 草稿流程已配置，尚未在远端执行；正式仓库已确定为 `Duskriver/SnapTranslate-linux`，并补入与 macOS 版一致的 MIT 许可证。详见 [发行说明](docs/RELEASING.md)。
