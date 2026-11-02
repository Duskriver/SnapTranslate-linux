use clap::Parser;
use gtk4::{gio, glib, prelude::*};
use libadwaita as adw;
use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::sync::Arc;
use tokio::runtime::Runtime;
use tokio::sync::{mpsc, oneshot, watch, Semaphore};
use tracing::{error, info, warn};

use snaptranslate_linux::capture::{CaptureError, CompositeCapturer, ScreenCapturer};
use snaptranslate_linux::cli::{self, Cli, Commands};
use snaptranslate_linux::client::{DeepSeekClient, RecoveryHint, TranslationFailure};
use snaptranslate_linux::config::{load_config, AppConfig};
use snaptranslate_linux::keyring::{CredentialStorage, KeyringStore};
use snaptranslate_linux::ui::global_shortcuts::{
    self, ShortcutCommand, ShortcutEvent, ShortcutState,
};
use snaptranslate_linux::ui::{AppTray, ResultPanel, SettingsDialog, TranslationPhase, TrayAction};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let cli = Cli::parse();
    init_logger(cli.verbose, cli.quiet);

    // Configuration commands remain local and never initialize the desktop.
    if let Some(Commands::Config(config_args)) = cli.command {
        return cli::handle_config_command(config_args);
    }

    if std::env::var_os("WAYLAND_DISPLAY").is_none() && std::env::var_os("DISPLAY").is_none() {
        eprintln!("Error: No graphical display detected.");
        eprintln!("SnapTranslate requires Wayland ($WAYLAND_DISPLAY) or X11 ($DISPLAY).");
        std::process::exit(1);
    }
    run_gui_application()
}

fn init_logger(verbose: u8, quiet: bool) {
    if quiet {
        return;
    }
    let filter = match verbose {
        0 => "snaptranslate_linux=info",
        1 => "snaptranslate_linux=debug",
        _ => "snaptranslate_linux=trace",
    };
    let _ = tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_writer(std::io::stderr)
        .try_init();
}

struct AppState {
    runtime: Arc<Runtime>,
    settings: RefCell<Option<SettingsDialog>>,
    capturing: Cell<bool>,
    capture_task: RefCell<Option<tokio::task::AbortHandle>>,
    capture_cancelled: Cell<bool>,
    network_slots: Arc<Semaphore>,
    shortcut_commands: mpsc::UnboundedSender<ShortcutCommand>,
    shortcut_state: RefCell<ShortcutState>,
    activation_token: RefCell<Option<String>>,
    shortcut_task: RefCell<Option<tokio::task::JoinHandle<()>>>,
    settings_requested: Cell<Option<SettingsIntent>>,
    tray_available: Cell<bool>,
    next_result: Cell<u64>,
}

// A result retains only its encoded screenshot while its window remains open.
// Credentials and service settings are loaded afresh for each explicit retry.
struct TranslationSource {
    image: Arc<str>,
    language: String,
}

#[derive(Clone, Copy)]
enum SettingsIntent {
    General,
    Recovery(RecoveryHint),
}

fn run_gui_application() -> Result<(), Box<dyn std::error::Error>> {
    let runtime = Arc::new(Runtime::new()?);
    let _runtime_guard = runtime.enter();
    gtk4::init()
        .map_err(|_| "Failed to initialize graphical display. Check DISPLAY / WAYLAND_DISPLAY.")?;
    adw::init()?;

    let app = adw::Application::builder()
        .application_id("com.github.duskriver.snaptranslate")
        .flags(gio::ApplicationFlags::HANDLES_COMMAND_LINE)
        .build();
    let (shortcut_commands, shortcut_rx) = mpsc::unbounded_channel();
    let (shortcut_events, shortcut_events_rx) = mpsc::unbounded_channel();
    let state = Rc::new(AppState {
        runtime: runtime.clone(),
        settings: RefCell::new(None),
        capturing: Cell::new(false),
        capture_task: RefCell::new(None),
        capture_cancelled: Cell::new(false),
        network_slots: Arc::new(Semaphore::new(2)),
        shortcut_commands,
        shortcut_state: RefCell::new(ShortcutState::Checking),
        activation_token: RefCell::new(None),
        shortcut_task: RefCell::new(None),
        settings_requested: Cell::new(None),
        tray_available: Cell::new(true),
        next_result: Cell::new(1),
    });
    let shortcut_receivers = RefCell::new(Some((shortcut_rx, shortcut_events, shortcut_events_rx)));

    // Only the primary instance starts a tray and owns the resident hold.
    // Window count must not control lifetime while a screenshot is pending.
    let resident_hold = Rc::new(RefCell::new(None));
    let hold = resident_hold.clone();
    let tray_source = Rc::new(RefCell::new(None::<glib::SourceId>));
    let source = tray_source.clone();
    let startup_state = state.clone();
    app.connect_startup(move |app| {
        *hold.borrow_mut() = Some(app.hold());
        let (shortcut_rx, shortcut_events, mut shortcut_events_rx) = shortcut_receivers
            .borrow_mut()
            .take()
            .expect("primary startup once");
        *startup_state.shortcut_task.borrow_mut() = Some(
            startup_state
                .runtime
                .spawn(global_shortcuts::run(shortcut_rx, shortcut_events)),
        );
        let weak_state = Rc::downgrade(&startup_state);
        let weak_app = app.downgrade();
        glib::MainContext::default().spawn_local(async move {
            while let Some(event) = shortcut_events_rx.recv().await {
                let (Some(state), Some(app)) = (weak_state.upgrade(), weak_app.upgrade()) else {
                    break;
                };
                match event {
                    ShortcutEvent::Activated { activation_token } => {
                        *state.activation_token.borrow_mut() = activation_token;
                        state.capture(&app, None);
                    }
                    ShortcutEvent::State(status) => {
                        if let Some(settings) = state.settings.borrow().as_ref() {
                            settings.set_shortcut_state(&status);
                        }
                        *state.shortcut_state.borrow_mut() = status;
                    }
                }
            }
        });

        let quit = gio::SimpleAction::new("quit", None);
        let weak_app = app.downgrade();
        let weak_state = Rc::downgrade(&startup_state);
        quit.connect_activate(move |_, _| {
            if let (Some(app), Some(state)) = (weak_app.upgrade(), weak_state.upgrade()) {
                state.request_quit(&app);
            }
        });
        app.add_action(&quit);
        app.set_accels_for_action("app.quit", &["<Primary>q"]);

        let settings = gio::SimpleAction::new("settings", None);
        let weak_app = app.downgrade();
        let weak_state = Rc::downgrade(&startup_state);
        settings.connect_activate(move |_, _| {
            if let (Some(app), Some(state)) = (weak_app.upgrade(), weak_state.upgrade()) {
                state.show_settings(&app, None);
            }
        });
        app.add_action(&settings);
        app.set_accels_for_action("app.settings", &["<Primary>comma"]);

        let capture = gio::SimpleAction::new("capture", None);
        let weak_app = app.downgrade();
        let weak_state = Rc::downgrade(&startup_state);
        capture.connect_activate(move |_, _| {
            if let (Some(app), Some(state)) = (weak_app.upgrade(), weak_state.upgrade()) {
                state.capture(&app, None);
            }
        });
        app.add_action(&capture);
        app.set_accels_for_action("app.capture", &["<Primary><Shift>t"]);

        let (tray_tx, tray_rx) = std::sync::mpsc::channel();
        AppTray::spawn_with_runtime(&startup_state.runtime, tray_tx);
        let weak_app = app.downgrade();
        let state = startup_state.clone();
        *source.borrow_mut() = Some(glib::timeout_add_local(
            std::time::Duration::from_millis(100),
            move || {
                let Some(app) = weak_app.upgrade() else {
                    return glib::ControlFlow::Break;
                };
                while let Ok(action) = tray_rx.try_recv() {
                    match action {
                        TrayAction::Capture => state.capture(&app, None),
                        TrayAction::OpenSettings => state.show_settings(&app, None),
                        TrayAction::Availability(available) => {
                            state.tray_available.set(available);
                            if let Some(settings) = state.settings.borrow().as_ref() {
                                settings.set_tray_available(available);
                            }
                        }
                        TrayAction::Quit => state.request_quit(&app),
                    }
                }
                glib::ControlFlow::Continue
            },
        ));
    });

    let activate_state = state.clone();
    app.connect_activate(move |app| activate_state.show_settings(app, None));

    // Parse each invocation in the primary process, including remote --lang.
    // Capturing the first process's CLI arguments here loses later commands.
    let command_state = state.clone();
    app.connect_command_line(move |app, command_line| {
        match Cli::try_parse_from(command_line.arguments()) {
            Ok(Cli {
                command: Some(Commands::Capture(args)),
                ..
            }) => {
                command_state.capture(app, args.target_language);
                glib::ExitCode::SUCCESS
            }
            Ok(Cli { command: None, .. }) => {
                app.activate();
                glib::ExitCode::SUCCESS
            }
            Ok(_) => {
                error!("Configuration commands must be run locally");
                glib::ExitCode::FAILURE
            }
            Err(err) => {
                error!("Invalid forwarded command: {err}");
                glib::ExitCode::FAILURE
            }
        }
    });

    app.connect_shutdown(move |app| {
        let _ = state.shortcut_commands.send(ShortcutCommand::Shutdown);
        if let Some(mut task) = state.shortcut_task.borrow_mut().take() {
            if state
                .runtime
                .block_on(tokio::time::timeout(
                    std::time::Duration::from_secs(1),
                    &mut task,
                ))
                .is_err()
            {
                task.abort();
            }
        }
        if let Some(source) = tray_source.borrow_mut().take() {
            source.remove();
        }
        // Destroy signals cancel each window's request before runtime teardown.
        for window in app.windows() {
            window.destroy();
        }
        if let Some(task) = state.capture_task.borrow_mut().take() {
            task.abort();
        }
        state.settings.borrow_mut().take();
        resident_hold.borrow_mut().take();
    });

    let code = app.run();
    if code != glib::ExitCode::SUCCESS {
        return Err("Failed to start or activate SnapTranslate".into());
    }
    Ok(())
}

impl AppState {
    fn request_quit(self: &Rc<Self>, app: &adw::Application) {
        if let Some(settings) = self.settings.borrow().as_ref() {
            let weak_app = app.downgrade();
            settings.request_quit(Rc::new(move || {
                if let Some(app) = weak_app.upgrade() {
                    app.quit();
                }
            }));
        } else {
            app.quit();
        }
    }

    fn result_panel(
        self: &Rc<Self>,
        app: &adw::Application,
        language: Option<&str>,
    ) -> ResultPanel {
        let panel = ResultPanel::new(app);
        if let Some(token) = self.activation_token.borrow_mut().take() {
            panel.set_activation_token(&token);
        }
        let sequence = self.next_result.get();
        self.next_result.set(sequence.saturating_add(1));
        panel.set_context(sequence, language.unwrap_or(""));
        let weak_app = app.downgrade();
        let weak_state = Rc::downgrade(self);
        let language = language.map(str::to_owned);
        let on_capture = Rc::new(move || {
            if let (Some(app), Some(state)) = (weak_app.upgrade(), weak_state.upgrade()) {
                state.capture(&app, language.clone());
            }
        });
        let weak_app = app.downgrade();
        let weak_state = Rc::downgrade(self);
        let on_settings = Rc::new(move || {
            if let (Some(app), Some(state)) = (weak_app.upgrade(), weak_state.upgrade()) {
                state.show_settings(&app, None);
            }
        });
        panel.set_actions(on_capture, on_settings);
        let weak_app = app.downgrade();
        let weak_state = Rc::downgrade(self);
        panel.set_recovery_action(Rc::new(move |hint| {
            if let (Some(app), Some(state)) = (weak_app.upgrade(), weak_state.upgrade()) {
                state.show_recovery_settings(&app, hint);
            }
        }));
        panel
    }

    fn show_error(self: &Rc<Self>, app: &adw::Application, message: &str) {
        let panel = self.result_panel(app, None);
        panel.set_error(message);
        panel.show();
    }

    fn show_settings(self: &Rc<Self>, app: &adw::Application, notice: Option<&str>) {
        // Do not expose a key entry while a screenshot is reading pixels.
        if self.capturing.get() {
            self.settings_requested.set(Some(SettingsIntent::General));
            return;
        }
        if self.settings.borrow().is_none() {
            let weak_state = Rc::downgrade(self);
            let weak_app = app.downgrade();
            let settings = SettingsDialog::new(
                app,
                Some(Rc::new(move || {
                    if let (Some(state), Some(app)) = (weak_state.upgrade(), weak_app.upgrade()) {
                        state.capture(&app, None);
                    }
                })),
            );
            let commands = self.shortcut_commands.clone();
            settings.set_shortcut_action(Rc::new(move |command| {
                let _ = commands.send(command);
            }));
            *self.settings.borrow_mut() = Some(settings);
        }
        if let Some(settings) = self.settings.borrow().as_ref() {
            settings.set_shortcut_state(&self.shortcut_state.borrow());
            settings.clear_recovery_context();
            settings.set_tray_available(self.tray_available.get());
            if let Some(notice) = notice {
                settings.set_notice(notice);
            }
            settings.present();
        }
    }

    fn show_recovery_settings(self: &Rc<Self>, app: &adw::Application, hint: RecoveryHint) {
        if self.capturing.get() {
            self.settings_requested
                .set(Some(SettingsIntent::Recovery(hint)));
            return;
        }
        self.show_settings(app, None);
        if let Some(settings) = self.settings.borrow().as_ref() {
            let message = match hint {
                RecoveryHint::Credentials => "检查并保存 API Key 后，返回译文窗口点击“原图重试”。",
                RecoveryHint::Service => "检查并保存模型与接口地址后，返回译文窗口点击“原图重试”。",
                _ => "保存设置后，返回译文窗口点击“原图重试”。",
            };
            settings.focus_recovery(hint, message);
        }
    }

    fn retry_translation(
        self: &Rc<Self>,
        app: &adw::Application,
        panel: &ResultPanel,
        source: &TranslationSource,
    ) {
        if panel.is_running() {
            return;
        }
        if let Some(settings) = self.settings.borrow().as_ref() {
            if !settings.prepare_retry() {
                return;
            }
        }
        let config = match load_config() {
            Ok(config) => config,
            Err(err) => {
                warn!("Cannot load retry configuration: {err}");
                panel.set_failure(&TranslationFailure {
                    message: "无法读取配置。请检查设置并修复配置文件后再重试；当前译文已保留。"
                        .into(),
                    recovery: RecoveryHint::Service,
                });
                self.show_recovery_settings(app, RecoveryHint::Service);
                return;
            }
        };
        let api_key = match KeyringStore::default_store().and_then(|s| s.get_api_key()) {
            Ok(Some(key)) if !key.trim().is_empty() => key,
            Ok(_) => {
                panel.set_failure(&TranslationFailure {
                    message: "请先在设置中保存 API Key，再点击“原图重试”。".into(),
                    recovery: RecoveryHint::Credentials,
                });
                self.show_recovery_settings(app, RecoveryHint::Credentials);
                return;
            }
            Err(err) => {
                warn!("Cannot read API key for retry: {err}");
                panel.set_failure(&TranslationFailure {
                    message: "无法读取 API Key，请解锁系统密钥环或重新保存密钥后重试。".into(),
                    recovery: RecoveryHint::Credentials,
                });
                self.show_recovery_settings(app, RecoveryHint::Credentials);
                return;
            }
        };
        self.start_translation(panel, source, config, api_key);
        panel.show();
    }

    fn start_translation(
        &self,
        panel: &ResultPanel,
        source: &TranslationSource,
        config: AppConfig,
        api_key: String,
    ) {
        let (tx, rx) = mpsc::channel(32);
        let (completion_tx, completion_rx) = oneshot::channel();
        let (phase_tx, phase_rx) = watch::channel(TranslationPhase::Queued);
        let slots = self.network_slots.clone();
        let api_url = format!(
            "{}/chat/completions",
            config.api_base_url.trim().trim_end_matches('/')
        );
        let image = source.image.clone();
        let language = source.language.clone();
        let task = self.runtime.spawn(async move {
            // Tokio's semaphore admits requests in FIFO order. Cancelling this
            // future removes its waiter; no upload begins before admission.
            let Ok(_permit) = slots.acquire_owned().await else {
                return;
            };
            let _ = phase_tx.send(TranslationPhase::Running);
            let client = DeepSeekClient::new(api_url);
            let result = client
                .stream_translation(&api_key, &config.model, &image, &language, tx)
                .await;
            let completion = match result {
                Ok(_) => {
                    info!("Translation completed");
                    Ok(())
                }
                Err(err) => {
                    let failure = err.failure();
                    warn!("Translation failed: {}", failure.message);
                    Err(failure)
                }
            };
            let _ = completion_tx.send(completion);
        });
        panel.listen_stream_with_state(rx, completion_rx, task.abort_handle(), phase_rx);
    }

    fn capture(self: &Rc<Self>, app: &adw::Application, override_lang: Option<String>) {
        if self.capturing.get() {
            self.activation_token.borrow_mut().take();
            self.capture_cancelled.set(true);
            if let Some(task) = self.capture_task.borrow_mut().take() {
                task.abort();
            }
            info!("Cancelling the active screen selection");
            return;
        }
        if let Some(settings) = self.settings.borrow().as_ref() {
            if !settings.prepare_capture() {
                return;
            }
        }
        let config = match load_config() {
            Ok(config) => config,
            Err(err) => {
                error!("Cannot load capture configuration: {err}");
                self.show_error(app, "无法读取配置，请修复配置文件后重试。");
                return;
            }
        };
        let target_lang = override_lang.unwrap_or_else(|| config.target_language.clone());
        if target_lang.trim().is_empty() {
            self.show_error(
                app,
                "目标语言不能为空，请在设置中选择语言或检查 --lang 参数。",
            );
            return;
        }
        let api_key = match KeyringStore::default_store().and_then(|s| s.get_api_key()) {
            Ok(Some(key)) if !key.trim().is_empty() => key,
            Ok(_) => {
                self.show_settings(app, Some("请先输入 API Key 并保存，再点击截屏翻译。"));
                return;
            }
            Err(err) => {
                warn!("Cannot read API key: {err}");
                self.show_settings(
                    app,
                    Some("无法读取 API Key，请解锁系统密钥环或重新保存密钥。"),
                );
                return;
            }
        };

        self.capturing.set(true);
        self.capture_cancelled.set(false);
        let hidden_windows: Vec<_> = app
            .windows()
            .into_iter()
            .filter(|window| window.is_visible())
            .map(|window| window.downgrade())
            .collect();
        let (capture_tx, capture_rx) = oneshot::channel();
        let (hidden_tx, hidden_rx) = oneshot::channel();
        let task = self.runtime.spawn(async move {
            if hidden_rx.await.is_err() {
                return;
            }
            // Give the compositor time to remove the settings window.
            tokio::time::sleep(std::time::Duration::from_millis(200)).await;
            info!("Initiating screen capture...");
            let result = CompositeCapturer::new().capture_region().await;
            let _ = capture_tx.send(result);
        });
        *self.capture_task.borrow_mut() = Some(task.abort_handle());

        let state = self.clone();
        let weak_app = app.downgrade();
        glib::MainContext::default().spawn_local(async move {
            // Finish the activating GTK/accessibility action before unmapping
            // its window. Otherwise GTK may export a now-invalid object path.
            glib::timeout_future(std::time::Duration::from_millis(40)).await;
            if !state.capture_cancelled.get() {
                for window in hidden_windows.iter().filter_map(|window| window.upgrade()) {
                    if window.application().is_some() {
                        window.set_visible(false);
                    }
                }
                if let Some(settings) = state.settings.borrow().as_ref() {
                    settings.hide();
                }
            }
            let _ = hidden_tx.send(());
            let result = capture_rx.await;
            state.capturing.set(false);
            let cancelled = state.capture_cancelled.replace(false);
            state.capture_task.borrow_mut().take();
            let Some(app) = weak_app.upgrade() else {
                return;
            };
            for window in hidden_windows.iter().filter_map(|window| window.upgrade()) {
                if window.application().is_some() {
                    window.set_visible(true);
                }
            }
            match result {
                Ok(Ok(Some(image))) => {
                    info!(
                        "Screenshot captured successfully ({} bytes)",
                        image.raw_bytes.len()
                    );
                    let panel = state.result_panel(&app, Some(&target_lang));
                    let source = Rc::new(TranslationSource {
                        image: Arc::from(image.base64_data_uri),
                        language: target_lang,
                    });
                    let weak_state = Rc::downgrade(&state);
                    let weak_app = app.downgrade();
                    let retry_source = source.clone();
                    panel.set_retry_action(Rc::new(move |panel| {
                        if let (Some(state), Some(app)) = (weak_state.upgrade(), weak_app.upgrade())
                        {
                            state.retry_translation(&app, panel, &retry_source);
                        }
                    }));
                    state.start_translation(&panel, &source, config, api_key);
                    panel.show();
                }
                Ok(Ok(None)) => info!("Screen capture was cancelled by user"),
                Ok(Err(err)) => {
                    error!("Screen capture failed: {err}");
                    state.show_error(&app, capture_error_message(&err));
                }
                Err(_) if cancelled => info!("Screen selection cancelled by repeated invocation"),
                Err(_) => state.show_error(&app, "截图任务意外结束，请重新截图。"),
            }
            match state.settings_requested.take() {
                Some(SettingsIntent::General) => state.show_settings(&app, None),
                Some(SettingsIntent::Recovery(hint)) => state.show_recovery_settings(&app, hint),
                None => {}
            }
        });
    }
}

fn capture_error_message(error: &CaptureError) -> &'static str {
    match error {
        CaptureError::PermissionDenied => "截图权限被拒绝。请在系统隐私或应用权限设置中允许截屏后重试。",
        CaptureError::PortalError(_) => "桌面截图服务未能完成截图，请检查系统权限和截图服务后重试。",
        CaptureError::NoBackendAvailable
        | CaptureError::BackendUnavailable(_)
        | CaptureError::PortalUnavailable(_)
        | CaptureError::CommandNotFound(_) => "无法启动截图。请检查桌面的截图权限与 XDG Desktop Portal；X11 可安装 maim，支持的 Wayland 桌面可使用 grim 和 slurp。",
        CaptureError::InvalidImage(_) => "截图数据无效，请重新选择区域。",
        _ => "截图失败。请检查桌面的截图权限和截图工具后重试。",
    }
}
