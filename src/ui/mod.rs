pub mod markdown;
pub mod result_panel;
pub mod settings_dialog;
pub mod shortcuts;
pub mod translation_phase;
pub mod tray;

pub use markdown::markdown_to_pango;
pub use result_panel::ResultPanel;
pub use settings_dialog::SettingsDialog;
pub use translation_phase::TranslationPhase;
pub use tray::{AppTray, TrayAction};

pub mod global_shortcuts;
