use ksni::{menu::StandardItem, MenuItem, Tray, TrayMethods};
use std::sync::mpsc::Sender;
use tokio::runtime::Runtime;
use tracing::info;

pub enum TrayAction {
    Capture,
    OpenSettings,
    Quit,
}

pub struct AppTray {
    sender: Sender<TrayAction>,
}

impl AppTray {
    pub fn new(sender: Sender<TrayAction>) -> Self {
        Self { sender }
    }

    pub fn spawn_with_runtime(runtime: &Runtime, sender: Sender<TrayAction>) {
        runtime.spawn(async move {
            let tray = Self::new(sender);
            match tray.spawn().await {
                Ok(_) => {
                    info!("StatusNotifierItem tray spawned successfully via DBus");
                }
                Err(e) => {
                    tracing::warn!("Failed to spawn DBus system tray: {}. Proceeding without tray.", e);
                }
            }
        });
    }
}

impl Tray for AppTray {
    fn id(&self) -> String {
        "snaptranslate-linux".to_string()
    }

    fn title(&self) -> String {
        "SnapTranslate · 轻译".to_string()
    }

    fn icon_name(&self) -> String {
        "accessories-character-map-symbolic".to_string()
    }

    fn menu(&self) -> Vec<MenuItem<Self>> {
        let tx_capture = self.sender.clone();
        let tx_settings = self.sender.clone();
        let tx_quit = self.sender.clone();

        vec![
            StandardItem {
                label: "截屏翻译".into(),
                icon_name: "edit-find-symbolic".into(),
                activate: Box::new(move |_| {
                    let _ = tx_capture.send(TrayAction::Capture);
                }),
                ..Default::default()
            }
            .into(),
            StandardItem {
                label: "设置...".into(),
                icon_name: "preferences-system-symbolic".into(),
                activate: Box::new(move |_| {
                    let _ = tx_settings.send(TrayAction::OpenSettings);
                }),
                ..Default::default()
            }
            .into(),
            MenuItem::Separator,
            StandardItem {
                label: "退出 SnapTranslate".into(),
                icon_name: "application-exit-symbolic".into(),
                activate: Box::new(move |_| {
                    let _ = tx_quit.send(TrayAction::Quit);
                }),
                ..Default::default()
            }
            .into(),
        ]
    }
}
