use adw::prelude::*;
use gtk4::prelude::*;
use libadwaita as adw;
use std::cell::{Cell, RefCell};
use std::rc::Rc;

use crate::client::RecoveryHint;
use crate::config::{load_config, save_config, AppConfig, ConfigError};
use crate::keyring::{CredentialStorage, KeyringStore};
use crate::ui::global_shortcuts::{ShortcutCommand, ShortcutState};
use crate::ui::shortcuts;

const LANGUAGES: &[(&str, &str)] = &[
    ("简体中文", "zh-CN"),
    ("繁體中文", "zh-TW"),
    ("English", "en"),
    ("日本語", "ja"),
    ("한국어", "ko"),
    ("自定义", ""),
];

pub struct SettingsDialog {
    inner: Rc<SettingsContents>,
}

struct SettingsContents {
    window: adw::PreferencesWindow,
    status: gtk4::Label,
    shortcut_status: gtk4::Label,
    portal_status: gtk4::Label,
    portal_button: gtk4::Button,
    portal_configure: Cell<bool>,
    portal_action: RefCell<Option<Rc<dyn Fn(ShortcutCommand)>>>,
    tray_notice: gtk4::Label,
    key_row: adw::ActionRow,
    key: gtk4::PasswordEntry,
    language: adw::ComboRow,
    custom_row: adw::ActionRow,
    custom_language: gtk4::Entry,
    advanced: adw::ExpanderRow,
    model: gtk4::Entry,
    url: gtk4::Entry,
    delete_key: gtk4::Button,
    save: gtk4::Button,
    snapshot: RefCell<Option<AppConfig>>,
    load_error: RefCell<Option<String>>,
    has_key: Cell<bool>,
    remove_key: Cell<bool>,
    dirty: Cell<bool>,
    loading: Cell<bool>,
    notice: RefCell<Option<String>>,
    confirmation: RefCell<Option<gtk4::MessageDialog>>,
    pending_after_close: RefCell<Option<Rc<dyn Fn()>>>,
    recovery_context: Cell<Option<RecoveryHint>>,
    active_toast: RefCell<Option<adw::Toast>>,
}

fn entry(name: &str, label: &str) -> gtk4::Entry {
    let builder = gtk4::Builder::from_string(&format!(
        "<interface><object class=\"GtkEntry\" id=\"entry\"><accessibility><property name=\"label\">{}</property></accessibility></object></interface>",
        gtk4::glib::markup_escape_text(label)
    ));
    let entry: gtk4::Entry = builder.object("entry").expect("static settings entry");
    entry.set_hexpand(true);
    entry.set_width_chars(22);
    entry.set_widget_name(name);
    entry
}

fn entry_row(title: &str, subtitle: &str, entry: &gtk4::Entry) -> adw::ActionRow {
    let row = adw::ActionRow::builder()
        .title(title)
        .subtitle(subtitle)
        .build();
    row.set_activatable_widget(Some(entry));
    row.add_suffix(entry);
    row
}

impl SettingsDialog {
    pub fn new(app: &adw::Application, on_capture: Option<Rc<dyn Fn() + 'static>>) -> Self {
        let window = adw::PreferencesWindow::builder()
            .application(app)
            .title("SnapTranslate · 轻译")
            .default_width(640)
            .default_height(720)
            .hide_on_close(true)
            .build();
        let page = adw::PreferencesPage::new();

        let quick = adw::PreferencesGroup::new();
        quick.set_title("截图翻译");
        quick.set_description(Some(
            "保存设置后开始框选。截图只在你主动翻译时发送给所配置的服务。",
        ));
        let capture = gtk4::Button::with_label("立即截屏翻译");
        capture.set_widget_name("settings-capture");
        capture.add_css_class("suggested-action");
        capture.set_valign(gtk4::Align::Center);
        let capture_row = adw::ActionRow::builder()
            .title("截屏翻译")
            .subtitle("拖拽选择区域，Esc 取消框选")
            .build();
        capture_row.add_suffix(&capture);
        quick.add(&capture_row);
        let tray_notice = gtk4::Label::builder()
            .label("当前桌面未显示系统托盘。关闭设置后仍可使用已绑定的系统快捷键；重新运行应用可打开设置。")
            .wrap(true).xalign(0.0).visible(false).css_classes(["dim-label"]).build();
        quick.add(&tray_notice);
        page.add(&quick);

        let credentials = adw::PreferencesGroup::new();
        credentials.set_title("API Key");
        credentials.set_description(Some(
            "留空保留已存密钥；保存只写入本机，不会发起验证或计费请求。",
        ));
        let key_builder = gtk4::Builder::from_string("<interface><object class=\"GtkPasswordEntry\" id=\"key\"><accessibility><property name=\"label\">API Key</property></accessibility></object></interface>");
        let key: gtk4::PasswordEntry = key_builder.object("key").expect("static key entry");
        key.set_hexpand(true);
        key.set_show_peek_icon(true);
        key.set_widget_name("settings-api-key");
        let key_row = adw::ActionRow::builder().title("DeepSeek API Key").build();
        key_row.add_suffix(&key);
        credentials.add(&key_row);
        let key_actions = gtk4::Box::new(gtk4::Orientation::Horizontal, 12);
        let get_key = gtk4::LinkButton::with_label(
            "https://platform.deepseek.com/api_keys",
            "获取官方 API Key ↗",
        );
        get_key.set_hexpand(true);
        get_key.set_halign(gtk4::Align::Start);
        let delete_key = gtk4::Button::with_label("删除密钥");
        delete_key.set_widget_name("settings-delete-key");
        key_actions.append(&get_key);
        key_actions.append(&delete_key);
        credentials.add(&key_actions);
        page.add(&credentials);

        let translation = adw::PreferencesGroup::new();
        translation.set_title("翻译偏好");
        let labels: Vec<_> = LANGUAGES.iter().map(|(label, _)| *label).collect();
        let language = adw::ComboRow::builder()
            .title("目标语言")
            .model(&gtk4::StringList::new(&labels))
            .build();
        language.set_widget_name("settings-language");
        let custom_language = entry("settings-custom-language", "自定义目标语言");
        custom_language.set_placeholder_text(Some("例如 fr、Deutsch"));
        let custom_row = entry_row("自定义目标语言", "语言名称或代码均可", &custom_language);
        translation.add(&language);
        translation.add(&custom_row);
        page.add(&translation);

        let advanced = adw::PreferencesGroup::new();
        let advanced_row = adw::ExpanderRow::builder()
            .title("高级设置")
            .subtitle("模型与 API 基础地址")
            .expanded(false)
            .build();
        advanced_row.set_widget_name("settings-advanced");
        let model = entry("settings-model", "模型名称");
        let url = entry("settings-api-url", "API 基础地址");
        advanced_row.add_row(&entry_row(
            "模型名称",
            "需要支持图片输入，例如 deepseek-flash",
            &model,
        ));
        advanced_row.add_row(&entry_row(
            "API 基础地址",
            "不含 /chat/completions；密钥与选区图片会发送至此地址",
            &url,
        ));
        advanced.add(&advanced_row);
        page.add(&advanced);

        let actions = adw::PreferencesGroup::new();
        let status = gtk4::Label::builder()
            .wrap(true)
            .xalign(0.0)
            .selectable(true)
            .build();
        status.set_widget_name("settings-status");
        actions.add(&status);
        let buttons = gtk4::Box::new(gtk4::Orientation::Horizontal, 12);
        buttons.set_halign(gtk4::Align::End);
        let reset = gtk4::Button::with_label("取消修改");
        reset.set_widget_name("settings-reset");
        reset.set_tooltip_text(Some("放弃未保存修改，并重新读取当前保存的配置"));
        let save = gtk4::Button::with_label("保存设置");
        save.set_widget_name("settings-save");
        save.add_css_class("suggested-action");
        buttons.append(&reset);
        buttons.append(&save);
        actions.add(&buttons);
        page.add(&actions);

        let shortcut_group = adw::PreferencesGroup::new();
        shortcut_group.set_title("全局快捷键");
        shortcut_group.set_description(Some(
            "系统授权确认后快捷键立即生效，无需点击本页保存。也可手动绑定 Alt + Shift + T。再次触发可取消框选。",
        ));
        let portal_status = gtk4::Label::builder()
            .label("正在检查系统快捷键支持…")
            .wrap(true)
            .xalign(0.0)
            .build();
        let portal_button = gtk4::Button::with_label("设置系统快捷键");
        portal_button.set_sensitive(false);
        shortcut_group.add(&portal_status);
        shortcut_group.add(&portal_button);
        let command = shortcuts::capture_command();
        let command_label = gtk4::Label::builder()
            .wrap(true)
            .selectable(true)
            .xalign(0.0)
            .build();
        command_label.set_label(command.as_deref().unwrap_or("无法生成截屏命令"));
        shortcut_group.add(&command_label);
        let shortcut_help = gtk4::Label::builder()
            .label(shortcuts::desktop_shortcut_help())
            .wrap(true)
            .xalign(0.0)
            .css_classes(["dim-label"])
            .build();
        shortcut_group.add(&shortcut_help);
        let shortcut_buttons = gtk4::Box::new(gtk4::Orientation::Horizontal, 12);
        let copy_command = gtk4::Button::with_label("复制命令");
        copy_command.set_widget_name("settings-copy-command");
        copy_command.set_sensitive(command.is_ok());
        let keyboard = gtk4::Button::with_label("打开键盘设置");
        keyboard.set_widget_name("settings-keyboard-settings");
        shortcut_buttons.append(&copy_command);
        shortcut_buttons.append(&keyboard);
        shortcut_group.add(&shortcut_buttons);
        let shortcut_status = gtk4::Label::builder()
            .wrap(true)
            .xalign(0.0)
            .visible(false)
            .build();
        shortcut_status.set_widget_name("settings-shortcut-status");
        shortcut_group.add(&shortcut_status);
        page.add(&shortcut_group);
        let quit = gtk4::Button::with_label("退出应用");
        quit.set_action_name(Some("app.quit"));
        quit.set_halign(gtk4::Align::End);
        let quit_group = adw::PreferencesGroup::new();
        quit_group.add(&quit);
        page.add(&quit_group);
        window.add(&page);

        let inner = Rc::new(SettingsContents {
            window,
            status,
            shortcut_status,
            portal_status,
            portal_button,
            portal_configure: Cell::new(false),
            portal_action: RefCell::new(None),
            tray_notice,
            key_row,
            key,
            language,
            custom_row,
            custom_language,
            advanced: advanced_row,
            model,
            url,
            delete_key,
            save,
            snapshot: RefCell::new(None),
            load_error: RefCell::new(None),
            has_key: Cell::new(false),
            remove_key: Cell::new(false),
            dirty: Cell::new(false),
            loading: Cell::new(false),
            notice: RefCell::new(None),
            confirmation: RefCell::new(None),
            pending_after_close: RefCell::new(None),
            recovery_context: Cell::new(None),
            active_toast: RefCell::new(None),
        });
        let weak = Rc::downgrade(&inner);
        inner.portal_button.connect_clicked(move |_| {
            if let Some(inner) = weak.upgrade() {
                let action = inner.portal_action.borrow().clone();
                if let Some(action) = action {
                    action(if inner.portal_configure.get() {
                        ShortcutCommand::Configure
                    } else {
                        ShortcutCommand::Bind
                    });
                }
            }
        });
        for entry in [&inner.custom_language, &inner.model, &inner.url] {
            let weak = Rc::downgrade(&inner);
            entry.connect_changed(move |_| {
                if let Some(inner) = weak.upgrade() {
                    inner.changed();
                }
            });
        }
        let weak = Rc::downgrade(&inner);
        inner.key.connect_changed(move |key| {
            if let Some(inner) = weak.upgrade() {
                if !inner.loading.get() && !key.text().is_empty() {
                    inner.remove_key.set(false);
                }
                inner.update_key_state();
                inner.changed();
            }
        });
        let weak = Rc::downgrade(&inner);
        inner.language.connect_selected_notify(move |_| {
            if let Some(inner) = weak.upgrade() {
                inner
                    .custom_row
                    .set_visible(inner.language.selected() as usize == LANGUAGES.len() - 1);
                inner.changed();
            }
        });
        let weak = Rc::downgrade(&inner);
        inner.delete_key.connect_clicked(move |_| {
            if let Some(inner) = weak.upgrade() {
                inner.loading.set(true);
                inner.key.set_text("");
                inner.remove_key.set(!inner.remove_key.get());
                inner.loading.set(false);
                inner.update_key_state();
                inner.changed();
            }
        });
        let weak = Rc::downgrade(&inner);
        inner.save.connect_clicked(move |_| {
            if let Some(inner) = weak.upgrade() {
                inner.save_settings();
            }
        });
        let weak = Rc::downgrade(&inner);
        reset.connect_clicked(move |_| {
            if let Some(inner) = weak.upgrade() {
                inner.reload();
                if inner.snapshot.borrow().is_some() {
                    inner.status.set_label("已取消修改，并重新载入已保存设置。");
                }
            }
        });
        let weak = Rc::downgrade(&inner);
        capture.connect_clicked(move |_| {
            if let Some(inner) = weak.upgrade() {
                if inner.prepare_capture() {
                    if let Some(callback) = &on_capture {
                        callback();
                    }
                }
            }
        });
        let weak = Rc::downgrade(&inner);
        inner.window.connect_close_request(move |_| {
            if let Some(inner) = weak.upgrade() {
                inner.request_close(None);
            }
            gtk4::glib::Propagation::Stop
        });
        let weak = Rc::downgrade(&inner);
        copy_command.connect_clicked(move |_| {
            if let Some(inner) = weak.upgrade() {
                if let Ok(command) = &command {
                    inner.window.clipboard().set_text(command);
                    inner.shortcut_status.set_label("截屏命令已复制");
                    inner.shortcut_status.set_visible(true);
                }
            }
        });
        let weak = Rc::downgrade(&inner);
        keyboard.connect_clicked(move |_| {
            if let Some(inner) = weak.upgrade() {
                match shortcuts::open_keyboard_settings() {
                    Ok(()) => inner
                        .shortcut_status
                        .set_label("已请求打开键盘设置；请添加自定义快捷键并粘贴命令。"),
                    Err(message) => inner.shortcut_status.set_label(&message),
                }
                inner.shortcut_status.set_visible(true);
            }
        });
        Self { inner }
    }

    pub fn set_shortcut_action(&self, action: Rc<dyn Fn(ShortcutCommand)>) {
        *self.inner.portal_action.borrow_mut() = Some(action);
    }
    pub fn set_shortcut_state(&self, state: &ShortcutState) {
        self.inner.portal_configure.set(false);
        self.inner.portal_button.set_label("设置系统快捷键");
        self.inner.portal_button.set_sensitive(true);
        let message = match state {
            ShortcutState::Checking => {
                self.inner.portal_button.set_sensitive(false);
                "正在检查系统快捷键支持…".into()
            }
            ShortcutState::Available => {
                "尚未启用系统快捷键。点击按钮后，在系统弹窗中确认组合键。".into()
            }
            ShortcutState::Binding => {
                self.inner.portal_button.set_sensitive(false);
                "请在系统弹窗中选择并确认快捷键…".into()
            }
            ShortcutState::Active {
                trigger,
                configurable,
            } => {
                self.inner.portal_configure.set(*configurable);
                self.inner.portal_button.set_sensitive(*configurable);
                self.inner.portal_button.set_label(if *configurable {
                    "更改系统快捷键"
                } else {
                    "请在系统键盘设置中修改"
                });
                format!("系统快捷键已启用：{trigger}")
            }
            ShortcutState::Inactive { message } => message.clone(),
            ShortcutState::Unavailable { message } => {
                self.inner.portal_button.set_label("重新检测系统快捷键");
                message.clone()
            }
        };
        self.inner.portal_status.set_label(&message);
    }
    pub fn present(&self) {
        if !self.inner.window.is_visible() {
            self.inner.reload();
        }
        if let Some(message) = self.inner.notice.borrow_mut().take() {
            self.inner.status.set_label(&message);
            self.inner.show_toast(&message);
        }
        self.inner.window.present();
    }
    pub fn hide(&self) {
        self.inner.hide_and_clear();
    }
    pub fn prepare_capture(&self) -> bool {
        self.inner.prepare_capture()
    }
    pub fn prepare_retry(&self) -> bool {
        self.inner.prepare_action("请先保存或取消修改后再重试。")
    }
    pub fn clear_recovery_context(&self) {
        self.inner.clear_toast();
        self.inner.recovery_context.set(None);
        if let Some(error) = self.inner.load_error.borrow().as_ref() {
            self.inner.status.set_label(error);
            return;
        }
        self.inner.status.set_label(if self.inner.dirty.get() {
            "有未保存的修改，点击保存后生效。"
        } else {
            "修改后请点击保存；取消修改会重新载入已保存设置。"
        });
    }
    /// Called after presenting the settings window. Existing unsaved input is
    /// preserved; only focus, guidance, and the post-save instruction change.
    pub fn focus_recovery(&self, hint: RecoveryHint, message: &str) {
        if self.inner.snapshot.borrow().is_none() {
            self.inner.recovery_context.set(None);
            let message = self.inner.load_error.borrow().clone().unwrap_or_else(|| {
                "配置尚未成功载入，请点击“取消修改”重新载入后再修改。".to_owned()
            });
            self.inner.status.set_label(&message);
            self.inner.show_toast(&message);
            return;
        }
        self.inner.recovery_context.set(Some(hint));
        self.inner.status.set_label(message);
        self.inner.show_toast(message);
        let target: gtk4::Widget = match hint {
            RecoveryHint::Credentials => self.inner.key.clone().upcast(),
            RecoveryHint::Service => {
                self.inner.advanced.set_expanded(true);
                self.inner.url.clone().upcast()
            }
            RecoveryHint::Retry | RecoveryHint::Recapture => return,
        };
        let target = target.downgrade();
        let window = self.inner.window.downgrade();
        // Expanding a row maps its children on the next main-loop iteration.
        gtk4::glib::idle_add_local_once(move || {
            if let (Some(window), Some(target)) = (window.upgrade(), target.upgrade()) {
                if window.is_visible() {
                    target.grab_focus();
                    if hint == RecoveryHint::Service {
                        // Leave room for the short guidance toast below the API
                        // field instead of placing the focused editor behind it.
                        if let Some(scrolled) = target
                            .ancestor(gtk4::ScrolledWindow::static_type())
                            .and_then(|widget| widget.downcast::<gtk4::ScrolledWindow>().ok())
                        {
                            if let Some(bounds) = target.compute_bounds(&scrolled) {
                                let safe_bottom = (scrolled.height() - 96).max(0) as f32;
                                let overflow = bounds.y() + bounds.height() - safe_bottom;
                                if overflow > 0.0 {
                                    let adjustment = scrolled.vadjustment();
                                    adjustment.set_value(
                                        (adjustment.value() + f64::from(overflow)).min(
                                            (adjustment.upper() - adjustment.page_size())
                                                .max(adjustment.lower()),
                                        ),
                                    );
                                }
                            }
                        }
                    }
                }
            }
        });
    }
    pub fn request_quit(&self, on_quit: Rc<dyn Fn()>) {
        self.inner.request_close(Some(on_quit));
    }
    pub fn set_notice(&self, message: &str) {
        self.inner.status.set_label(message);
        *self.inner.notice.borrow_mut() = Some(message.to_owned());
    }
    pub fn set_tray_available(&self, available: bool) {
        self.inner.tray_notice.set_visible(!available);
    }
}

impl SettingsContents {
    fn clear_toast(&self) {
        // Dismiss outside the RefCell borrow: dismissal can emit UI signals.
        let toast = self.active_toast.borrow_mut().take();
        if let Some(toast) = toast {
            toast.dismiss();
        }
    }

    fn show_toast(&self, message: &str) {
        self.clear_toast();
        let toast = adw::Toast::new(message);
        toast.set_timeout(2);
        *self.active_toast.borrow_mut() = Some(toast.clone());
        self.window.add_toast(toast);
    }

    fn selected_language(&self) -> String {
        LANGUAGES
            .get(self.language.selected() as usize)
            .filter(|(_, code)| !code.is_empty())
            .map(|(_, code)| (*code).to_owned())
            .unwrap_or_else(|| self.custom_language.text().to_string())
    }
    fn form_config(&self) -> AppConfig {
        AppConfig {
            target_language: self.selected_language(),
            model: self.model.text().to_string(),
            api_base_url: self.url.text().to_string(),
        }
    }
    fn changed(&self) {
        if self.loading.get() {
            return;
        }
        let dirty = !self.key.text().is_empty()
            || self.remove_key.get()
            || self.snapshot.borrow().as_ref() != Some(&self.form_config());
        self.dirty.set(dirty);
        self.status.set_label(if dirty {
            "有未保存的修改，点击保存后生效。"
        } else {
            "当前设置没有未保存的修改。"
        });
    }
    fn update_key_state(&self) {
        let pending = !self.key.text().is_empty();
        self.key_row.set_subtitle(if self.remove_key.get() {
            "待删除；点击保存后生效，可撤销"
        } else if pending {
            "新密钥待保存（未验证）"
        } else if self.has_key.get() {
            "已保存（未验证）"
        } else {
            "尚未保存 API Key"
        });
        self.key
            .set_placeholder_text(Some(if self.remove_key.get() {
                "保存后删除现有密钥"
            } else if self.has_key.get() {
                "留空保留已保存密钥"
            } else {
                "请输入 API Key"
            }));
        self.delete_key.set_label(if self.remove_key.get() {
            "撤销删除"
        } else {
            "删除密钥"
        });
        self.delete_key
            .set_sensitive(self.has_key.get() || self.remove_key.get());
    }
    fn reload(&self) {
        self.clear_toast();
        self.loading.set(true);
        self.load_error.borrow_mut().take();
        self.shortcut_status.set_label("");
        self.shortcut_status.set_visible(false);
        self.key.set_text("");
        self.remove_key.set(false);
        self.dirty.set(false);
        match load_config() {
            Ok(config) => {
                let selected = LANGUAGES
                    .iter()
                    .position(|(_, code)| *code == config.target_language)
                    .unwrap_or(LANGUAGES.len() - 1);
                self.language.set_selected(selected as u32);
                self.custom_row.set_visible(selected == LANGUAGES.len() - 1);
                self.custom_language
                    .set_text(if selected == LANGUAGES.len() - 1 {
                        &config.target_language
                    } else {
                        ""
                    });
                self.model.set_text(&config.model);
                self.url.set_text(&config.api_base_url);
                *self.snapshot.borrow_mut() = Some(config);
                self.save.set_sensitive(true);
                self.status
                    .set_label("修改后请点击保存；取消修改会重新载入已保存设置。");
            }
            Err(error) => {
                let message = config_error_message(&error);
                *self.load_error.borrow_mut() = Some(message.clone());
                *self.snapshot.borrow_mut() = None;
                self.model.set_text("");
                self.url.set_text("");
                self.custom_language.set_text("");
                self.save.set_sensitive(false);
                self.status.set_label(&message);
            }
        }
        match KeyringStore::default_store().and_then(|store| store.get_api_key()) {
            Ok(key) => self.has_key.set(key.is_some()),
            Err(_) => {
                self.has_key.set(false);
                if self.snapshot.borrow().is_some() {
                    self.status.set_label(
                        "无法读取密钥状态，请检查系统密钥环或凭据文件；不会显示已有密钥。",
                    );
                }
            }
        }
        self.update_key_state();
        self.loading.set(false);
    }
    fn save_settings(&self) -> bool {
        let config = match validated_settings(self.form_config(), &self.key.text()) {
            Ok(config) => config,
            Err(message) => {
                self.status.set_label(&message);
                return false;
            }
        };
        let current = match load_config() {
            Ok(config) => config,
            Err(error) => {
                self.status.set_label(&config_error_message(&error));
                return false;
            }
        };
        if self.snapshot.borrow().as_ref() != Some(&current) {
            self.status.set_label(
                "配置已被其他操作修改。请点击“取消修改”载入最新设置，再重新修改；本次没有保存。",
            );
            return false;
        }
        let replacement = self.key.text();
        let remove_key = self.remove_key.get();
        let result = persist_settings(
            &current,
            &config,
            |config| save_config(config).map_err(|error| config_error_message(&error)),
            || {
                if !remove_key && replacement.trim().is_empty() {
                    return Ok(());
                }
                let store = KeyringStore::default_store()
                    .map_err(|_| "无法打开密钥存储。请检查系统密钥环和配置目录。".to_string())?;
                if remove_key {
                    store.delete_api_key().map_err(|_| "删除密钥失败，可能仍有凭据未删除。请检查密钥环是否已解锁，并取消修改后重新确认状态。".to_string())
                } else {
                    store.set_api_key(replacement.trim()).map_err(|_| {
                        "密钥保存失败。请检查系统密钥环及凭据目录的写入权限。".to_string()
                    })
                }
            },
        );
        match result {
            Ok(()) => {
                let recovery = self.recovery_context.take();
                self.reload();
                let message = if recovery.is_some() && self.has_key.get() {
                    "设置已保存，请返回译文窗口点击原图重试。"
                } else if recovery.is_some() {
                    "设置已保存；请先填入并保存 API Key，再返回译文窗口点击原图重试。"
                } else if remove_key {
                    "设置已保存，密钥已删除。"
                } else {
                    "设置已保存；API Key 尚未联网验证。"
                };
                self.status.set_label(message);
                if recovery.is_some() {
                    self.show_toast(message);
                }
                true
            }
            Err(message) => {
                self.status.set_label(&message);
                false
            }
        }
    }
    fn prepare_capture(&self) -> bool {
        self.prepare_action("请先保存或取消修改后再截图。")
    }
    fn prepare_action(&self, message: &str) -> bool {
        if self.dirty.get() {
            self.status.set_label(message);
            self.window.present();
            self.show_toast(message);
            false
        } else {
            true
        }
    }
    fn hide_and_clear(&self) {
        self.clear_toast();
        self.recovery_context.set(None);
        let confirmation = self.confirmation.borrow_mut().take();
        self.pending_after_close.borrow_mut().take();
        if let Some(dialog) = confirmation {
            dialog.destroy();
        }
        self.loading.set(true);
        self.key.set_text("");
        self.remove_key.set(false);
        self.dirty.set(false);
        self.loading.set(false);
        self.window.set_visible(false);
    }
    fn request_close(self: &Rc<Self>, after_close: Option<Rc<dyn Fn()>>) {
        if !self.dirty.get() {
            self.hide_and_clear();
            if let Some(callback) = after_close {
                callback();
            }
            return;
        }
        // A quit request upgrades an already-open discard dialog. A later
        // ordinary close must not replace that intent with a no-op.
        if let Some(callback) = after_close {
            *self.pending_after_close.borrow_mut() = Some(callback);
        }
        if let Some(dialog) = self.confirmation.borrow().as_ref() {
            dialog.present();
            return;
        }
        let dialog = gtk4::MessageDialog::builder()
            .transient_for(&self.window)
            .modal(true)
            .message_type(gtk4::MessageType::Question)
            .text("放弃未保存的修改？")
            .secondary_text("已保存的设置保持不变。未保存的密钥输入会被清空。")
            .build();
        dialog.add_button("继续编辑", gtk4::ResponseType::Cancel);
        dialog.add_button("放弃修改", gtk4::ResponseType::Reject);
        dialog.set_default_response(gtk4::ResponseType::Cancel);
        let weak = Rc::downgrade(self);
        dialog.connect_response(move |dialog, response| {
            if let Some(inner) = weak.upgrade() {
                inner.confirmation.borrow_mut().take();
                let after_close = inner.pending_after_close.borrow_mut().take();
                dialog.destroy();
                if response == gtk4::ResponseType::Reject {
                    inner.hide_and_clear();
                    if let Some(callback) = &after_close {
                        callback();
                    }
                }
            }
        });
        *self.confirmation.borrow_mut() = Some(dialog.clone());
        dialog.present();
    }
}

fn validated_settings(mut config: AppConfig, key: &str) -> Result<AppConfig, String> {
    config.target_language = config.target_language.trim().to_owned();
    config.model = config.model.trim().to_owned();
    config.api_base_url = config.api_base_url.trim().trim_end_matches('/').to_owned();
    if config.target_language.is_empty() {
        return Err("目标语言不能为空。".into());
    }
    if config.target_language.chars().any(char::is_control) {
        return Err("目标语言不能包含换行或控制字符。".into());
    }
    if config.model.is_empty() {
        return Err("模型名称不能为空。".into());
    }
    if config.model.chars().any(char::is_whitespace) || config.model.chars().any(char::is_control) {
        return Err("模型名称不能包含空格或换行。".into());
    }
    if config.api_base_url.chars().any(char::is_whitespace) {
        return Err("API 基础地址不能包含空格或换行。".into());
    }
    let url = reqwest::Url::parse(&config.api_base_url).map_err(|_| {
        "API 基础地址格式不正确，请填写完整的 http:// 或 https:// 地址。".to_string()
    })?;
    if !matches!(url.scheme(), "http" | "https") || url.host_str().is_none() {
        return Err("API 基础地址必须是有效的 http:// 或 https:// 地址。".into());
    }
    if !url.username().is_empty()
        || url.password().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
    {
        return Err(
            "API 基础地址不能包含账号、密码、查询参数或 # 片段；API Key 请填写在密钥栏。".into(),
        );
    }
    if url
        .path()
        .trim_end_matches('/')
        .ends_with("/chat/completions")
    {
        return Err("请填写 API 基础地址，不要包含末尾的 /chat/completions。".into());
    }
    let key = key.trim();
    if !key.is_empty() && (key.len() > 1024 || !key.bytes().all(|byte| (33..127).contains(&byte))) {
        return Err("API Key 格式不正确，请重新粘贴完整密钥，不要包含空格或换行。".into());
    }
    Ok(config)
}

fn config_error_message(error: &ConfigError) -> String {
    match error {
        ConfigError::Parse { .. } | ConfigError::Validation(_) => {
            "配置文件格式损坏或内容无效，未覆盖原文件。请修复配置文件后点击“取消修改”重新载入。"
        }
        ConfigError::Io { .. } => "无法读写配置文件，请检查目录权限和可用磁盘空间。",
        ConfigError::ConfigDirNotFound => {
            "无法定位配置目录，请检查当前用户的 HOME / XDG_CONFIG_HOME。"
        }
        ConfigError::Serialize(_) => "无法生成配置文件，设置尚未保存。",
    }
    .to_owned()
}

/// The stores cannot share a transaction. Restore config on credential failure;
/// if rollback also fails, explicitly tell the user what may already be saved.
fn persist_settings(
    current: &AppConfig,
    next: &AppConfig,
    mut write_config: impl FnMut(&AppConfig) -> Result<(), String>,
    write_key: impl FnOnce() -> Result<(), String>,
) -> Result<(), String> {
    let changed = current != next;
    if changed {
        write_config(next)?;
    }
    if let Err(error) = write_key() {
        if changed {
            if write_config(current).is_err() {
                return Err(format!("{error}\n其他设置已经写入，恢复旧配置也失败。请取消修改并重新载入，检查当前实际保存的状态。"));
            }
            return Err(format!("{error}\n其他设置已恢复，本次未全部保存。"));
        }
        return Err(error);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn settings_validation_preserves_custom_languages_and_api_paths() {
        let config = AppConfig {
            target_language: " français ".into(),
            model: " custom/vision ".into(),
            api_base_url: " https://example.com/v1/ ".into(),
        };
        let validated = validated_settings(config, "").unwrap();
        assert_eq!(validated.target_language, "français");
        assert_eq!(validated.model, "custom/vision");
        assert_eq!(validated.api_base_url, "https://example.com/v1");
    }
    #[test]
    fn settings_reject_invalid_values_before_writing() {
        let config = AppConfig {
            model: " ".into(),
            ..AppConfig::default()
        };
        assert_eq!(
            validated_settings(config, "").unwrap_err(),
            "模型名称不能为空。"
        );
        for url in [
            "https://",
            "https://example.com/white space",
            "file:///tmp/test",
            "https://user:pass@example.com",
            "https://example.com?key=secret",
            "https://example.com/chat/completions",
        ] {
            let config = AppConfig {
                api_base_url: url.into(),
                ..AppConfig::default()
            };
            assert!(validated_settings(config, "").is_err());
        }
        assert!(validated_settings(AppConfig::default(), "sk-bad\nkey").is_err());
    }
    #[test]
    fn credential_failure_restores_previous_config() {
        let previous = AppConfig::default();
        let next = AppConfig {
            target_language: "en".into(),
            ..previous.clone()
        };
        let mut writes = Vec::new();
        let result = persist_settings(
            &previous,
            &next,
            |config| {
                writes.push(config.clone());
                Ok(())
            },
            || Err("密钥保存失败".into()),
        );
        assert!(result.unwrap_err().contains("其他设置已恢复"));
        assert_eq!(writes, vec![next, previous]);
    }
    #[test]
    fn config_failure_does_not_touch_credentials() {
        let previous = AppConfig::default();
        let next = AppConfig {
            target_language: "en".into(),
            ..previous.clone()
        };
        let called = Cell::new(false);
        let result = persist_settings(
            &previous,
            &next,
            |_| Err("配置写入失败".into()),
            || {
                called.set(true);
                Ok(())
            },
        );
        assert!(result.is_err());
        assert!(!called.get());
    }
    #[test]
    fn rollback_failure_reports_partial_save() {
        let previous = AppConfig::default();
        let next = AppConfig {
            target_language: "en".into(),
            ..previous.clone()
        };
        let mut count = 0;
        let result = persist_settings(
            &previous,
            &next,
            |_| {
                count += 1;
                if count == 1 {
                    Ok(())
                } else {
                    Err("磁盘错误".into())
                }
            },
            || Err("密钥操作失败".into()),
        );
        assert!(result.unwrap_err().contains("恢复旧配置也失败"));
    }
}
