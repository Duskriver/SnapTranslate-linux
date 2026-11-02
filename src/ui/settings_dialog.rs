use gtk4::prelude::*;
use libadwaita as adw;
use adw::prelude::*;
use tracing::{error, info};

use crate::config::{load_config, save_config};
use crate::keyring::{CredentialStorage, KeyringStore};

pub struct SettingsDialog {
    window: adw::PreferencesWindow,
}

impl SettingsDialog {
    pub fn new(parent: Option<&impl IsA<gtk4::Window>>) -> Self {
        let window = adw::PreferencesWindow::builder()
            .title("SnapTranslate · 设置")
            .default_width(450)
            .default_height(380)
            .modal(true)
            .build();

        if let Some(parent) = parent {
            window.set_transient_for(Some(parent));
        }

        let page = adw::PreferencesPage::new();
        let group = adw::PreferencesGroup::new();
        group.set_title("API 与语言设置");
        group.set_description(Some("配置 DeepSeek 访问凭据及截图翻译默认目标语言"));

        // Load existing config & key status
        let config = load_config().unwrap_or_default();
        let keyring_store = KeyringStore::default_store().ok();
        let has_key = keyring_store
            .as_ref()
            .and_then(|s| s.get_api_key().ok().flatten())
            .map(|k| !k.is_empty())
            .unwrap_or(false);

        // API Key Entry Row
        let key_row = adw::ActionRow::builder()
            .title("DeepSeek API Key")
            .subtitle("用于调用视觉模型翻译")
            .build();
        let key_entry = gtk4::PasswordEntry::builder()
            .hexpand(true)
            .show_peek_icon(true)
            .build();
        if has_key {
            key_entry.set_placeholder_text(Some("●●●●●●●● (已安全保存)"));
        } else {
            key_entry.set_placeholder_text(Some("请输入 sk- 开头的 API Key"));
        }
        key_row.add_suffix(&key_entry);
        group.add(&key_row);

        // Target Language Entry Row
        let lang_row = adw::ActionRow::builder()
            .title("目标语言")
            .subtitle("默认翻译为此语言")
            .build();
        let lang_entry = gtk4::Entry::builder()
            .text(&config.target_language)
            .hexpand(true)
            .build();
        lang_row.add_suffix(&lang_entry);
        group.add(&lang_row);

        // Model Entry Row
        let model_row = adw::ActionRow::builder()
            .title("模型名称")
            .subtitle("默认 deepseek-flash")
            .build();
        let model_entry = gtk4::Entry::builder()
            .text(&config.model)
            .hexpand(true)
            .build();
        model_row.add_suffix(&model_entry);
        group.add(&model_row);

        // Action Buttons Row
        let button_group = adw::PreferencesGroup::new();
        let action_box = gtk4::Box::new(gtk4::Orientation::Horizontal, 12);
        action_box.set_halign(gtk4::Align::End);
        action_box.set_margin_top(12);

        let delete_key_btn = gtk4::Button::builder()
            .label("删除密钥")
            .css_classes(["destructive-action"])
            .build();

        let save_btn = gtk4::Button::builder()
            .label("保存设置")
            .css_classes(["suggested-action"])
            .build();

        action_box.append(&delete_key_btn);
        action_box.append(&save_btn);
        button_group.add(&action_box);

        page.add(&group);
        page.add(&button_group);
        window.add(&page);

        // Connect Save
        let key_entry_clone = key_entry.clone();
        let lang_entry_clone = lang_entry.clone();
        let model_entry_clone = model_entry.clone();
        let win_weak = window.downgrade();

        save_btn.connect_clicked(move |_| {
            let mut cfg = load_config().unwrap_or_default();
            let lang = lang_entry_clone.text().to_string();
            let model = model_entry_clone.text().to_string();

            if !lang.trim().is_empty() {
                cfg.target_language = lang.trim().to_string();
            }
            if !model.trim().is_empty() {
                cfg.model = model.trim().to_string();
            }

            if let Err(e) = save_config(&cfg) {
                error!("Failed to save config: {}", e);
            } else {
                info!("Configuration saved successfully");
            }

            let new_key = key_entry_clone.text().to_string();
            if !new_key.trim().is_empty() {
                if let Ok(store) = KeyringStore::default_store() {
                    if let Err(e) = store.set_api_key(new_key.trim()) {
                        error!("Failed to save API key to Keyring: {}", e);
                    } else {
                        info!("API Key saved to Keyring successfully");
                    }
                }
            }

            if let Some(win) = win_weak.upgrade() {
                win.close();
            }
        });

        // Connect Delete Key
        let key_entry_for_del = key_entry.clone();
        delete_key_btn.connect_clicked(move |_| {
            if let Ok(store) = KeyringStore::default_store() {
                if let Err(e) = store.delete_api_key() {
                    error!("Failed to delete API Key: {}", e);
                } else {
                    info!("API Key deleted");
                    key_entry_for_del.set_text("");
                    key_entry_for_del.set_placeholder_text(Some("密钥已删除"));
                }
            }
        });

        Self { window }
    }

    pub fn present(&self) {
        self.window.present();
    }
}
