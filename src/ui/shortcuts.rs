//! Desktop-specific setup guidance. Binding a global shortcut stays under the
//! user's control; simply opening settings never changes desktop bindings.

use std::path::Path;
use std::process::{Command, Stdio};

/// A copyable command that works even when the executable path contains spaces
/// or shell metacharacters. Never depend on a source build being on PATH.
pub fn capture_command() -> Result<String, String> {
    let executable = std::env::current_exe()
        .map_err(|_| "无法获取程序位置，请使用可执行文件的绝对路径加上 capture。".to_owned())?;
    command_for_path(&executable)
}

fn command_for_path(path: &Path) -> Result<String, String> {
    let value = path
        .to_str()
        .filter(|value| !value.contains(['\n', '\r']))
        .ok_or_else(|| "程序路径无法显示为单行命令，请将程序移到普通目录。".to_owned())?;
    Ok(format!("'{}' capture", value.replace('\'', "'\\''")))
}

fn desktop() -> String {
    std::env::var("XDG_CURRENT_DESKTOP")
        .or_else(|_| std::env::var("XDG_SESSION_DESKTOP"))
        .unwrap_or_default()
        .to_ascii_lowercase()
}

pub fn desktop_shortcut_help() -> String {
    guidance(&desktop()).to_owned()
}

fn guidance(desktop: &str) -> &'static str {
    if desktop.contains("gnome") || desktop.contains("ubuntu") {
        "1. 复制下方命令。\n2. 打开键盘设置 → 查看及自定义快捷键 → 自定义快捷键，新增“截屏翻译”并粘贴命令。\n3. 录入 Alt+Shift+T；若组合已占用，请改选其他组合。"
    } else if desktop.contains("kde") || desktop.contains("plasma") {
        "1. 复制下方命令。\n2. 在系统设置 → 键盘 → 快捷键中添加命令或脚本，粘贴命令。\n3. 录入 Alt+Shift+T；若组合已占用，请改选其他组合。"
    } else if desktop.contains("sway") || desktop.contains("i3") {
        "复制下方命令，在窗口管理器配置中添加：bindsym $mod+Shift+t exec 后接该命令。重新加载配置后，用 Mod+Shift+T 截图。请避开已占用的组合。"
    } else if desktop.contains("hyprland") {
        "复制下方命令，按所用 Hyprland 版本的快捷键配置方式，将 Super+Shift+T 绑定到此命令。保存并重新加载配置后即可截图；请避开已占用的组合。"
    } else {
        "1. 复制下方命令。\n2. 在桌面系统设置 → 键盘 → 自定义快捷键中新增“截屏翻译”并粘贴命令。\n3. 录入 Alt+Shift+T；若组合已占用，请改选其他组合。"
    }
}

fn settings_commands(desktop: &str) -> &'static [(&'static str, &'static [&'static str])] {
    if desktop.contains("gnome") || desktop.contains("ubuntu") {
        &[("gnome-control-center", &["keyboard"])]
    } else if desktop.contains("kde") || desktop.contains("plasma") {
        &[
            ("systemsettings", &["kcm_keys"]),
            ("systemsettings5", &["kcm_keys"]),
        ]
    } else if desktop.contains("xfce") {
        &[("xfce4-keyboard-settings", &[])]
    } else if desktop.contains("cinnamon") {
        &[("cinnamon-settings", &["keyboard"])]
    } else if desktop.contains("mate") {
        &[("mate-keybinding-properties", &[])]
    } else if desktop.contains("lxqt") {
        &[("lxqt-config-globalkeyshortcuts", &[])]
    } else {
        &[]
    }
}

pub fn open_keyboard_settings() -> Result<(), String> {
    for (program, args) in settings_commands(&desktop()) {
        if let Ok(mut child) = Command::new(program)
            .args(*args)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
        {
            // Reap the external settings process without blocking GTK.
            std::thread::spawn(move || {
                let _ = child.wait();
            });
            return Ok(());
        }
    }
    Err("无法自动打开键盘设置，请按上方说明手动配置。".to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn copied_command_preserves_paths_with_shell_characters() {
        for name in [
            "/opt/Snap Translate/轻译",
            "/opt/user's app/translator",
            "/opt/$(echo hello)`echo hi`/translate",
        ] {
            let command = command_for_path(Path::new(name)).unwrap();
            let parsed = gtk4::glib::shell_parse_argv(command).unwrap();
            assert_eq!(
                parsed,
                vec![
                    std::ffi::OsString::from(name),
                    std::ffi::OsString::from("capture")
                ]
            );
        }
    }

    #[test]
    fn unknown_desktop_does_not_launch_unrelated_settings() {
        assert!(settings_commands("sway").is_empty());
        assert!(settings_commands("unknown").is_empty());
        assert_eq!(
            settings_commands("ubuntu:gnome")[0].0,
            "gnome-control-center"
        );
    }
}
