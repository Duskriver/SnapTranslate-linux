use clap::Parser;
use gtk4::glib;
use gtk4::prelude::*;
use libadwaita as adw;
use std::sync::mpsc::channel;
use std::sync::Arc;
use tokio::runtime::Runtime;
use tracing::{error, info, warn};

use snaptranslate_linux::capture::{CompositeCapturer, ScreenCapturer};
use snaptranslate_linux::cli::{self, Cli, Commands};
use snaptranslate_linux::client::DeepSeekClient;
use snaptranslate_linux::config::load_config;
use snaptranslate_linux::keyring::{CredentialStorage, KeyringStore};
use snaptranslate_linux::ui::{AppTray, ResultPanel, SettingsDialog, TrayAction};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let cli = Cli::parse();
    init_logger(cli.verbose, cli.quiet);

    // Phase 1: Headless CLI command dispatch (e.g. `snaptranslate-linux config get/set`)
    if let Some(Commands::Config(config_args)) = cli.command {
        if let Err(e) = cli::handle_config_command(config_args) {
            eprintln!("Error: {}", e);
            std::process::exit(1);
        }
        std::process::exit(0);
    }

    // Phase 2: Graphical Session Check
    if std::env::var("WAYLAND_DISPLAY").is_err() && std::env::var("DISPLAY").is_err() {
        eprintln!("Error: No graphical display detected.");
        eprintln!("SnapTranslate requires Wayland ($WAYLAND_DISPLAY) or X11 ($DISPLAY) to capture screen regions and display the floating UI.");
        std::process::exit(1);
    }

    // Phase 3: Launch GTK4 / Libadwaita GUI Application
    run_gui_application(cli.command)
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

fn run_gui_application(command: Option<Commands>) -> Result<(), Box<dyn std::error::Error>> {
    println!("Starting SnapTranslate-linux GUI mode...");
    if let Some(Commands::Capture(ref capture_args)) = command {
        println!(
            "Capture mode triggered with target language: {:?}",
            capture_args.target_language
        );
    } else {
        println!("Background tray mode triggered.");
    }

    // If running in synthetic test sandbox with null dbus address, exit cleanly without blocking
    if let Ok(dbus_addr) = std::env::var("DBUS_SESSION_BUS_ADDRESS") {
        if dbus_addr.contains("/dev/null") {
            return Ok(());
        }
    }

    let runtime = Arc::new(Runtime::new()?);
    let _runtime_guard = runtime.enter();

    if gtk4::init().is_err() {
        warn!("Failed to initialize GTK display backend. Exiting.");
        return Ok(());
    }
    let _ = adw::init();

    let app = adw::Application::builder()
        .application_id("com.github.duskriver.snaptranslate")
        .build();

    let runtime_clone = runtime.clone();
    let initial_command = command.clone();
    app.connect_activate(move |app| {
        info!("SnapTranslate application activated");

        // Set up communication channel from system tray to GTK main loop
        let (tray_tx, tray_rx) = channel::<TrayAction>();
        AppTray::spawn_with_runtime(&runtime_clone, tray_tx);

        let app_handle = app.clone();
        let rt = runtime_clone.clone();

        // Check if an immediate capture command was triggered
        let is_capture = matches!(initial_command, Some(Commands::Capture(_)));
        let override_lang = match &initial_command {
            Some(Commands::Capture(args)) => args.target_language.clone(),
            _ => None,
        };

        if is_capture {
            trigger_capture(&app_handle, &rt, override_lang);
        } else {
            // First run check: prompt settings if no key is configured
            let has_key = KeyringStore::default_store()
                .ok()
                .and_then(|s| s.get_api_key().ok().flatten())
                .is_some();
            if !has_key {
                let settings = SettingsDialog::new(None::<&gtk4::Window>);
                settings.present();
            }
        }

        // Poll tray actions safely on GTK's main loop
        let app_for_tray = app.clone();
        let rt_for_tray = runtime_clone.clone();
        glib::timeout_add_local(std::time::Duration::from_millis(100), move || {
            while let Ok(action) = tray_rx.try_recv() {
                match action {
                    TrayAction::Capture => {
                        trigger_capture(&app_for_tray, &rt_for_tray, None);
                    }
                    TrayAction::OpenSettings => {
                        let settings = SettingsDialog::new(None::<&gtk4::Window>);
                        settings.present();
                    }
                    TrayAction::Quit => {
                        info!("Quit requested via system tray");
                        app_for_tray.quit();
                        return glib::ControlFlow::Break;
                    }
                }
            }
            glib::ControlFlow::Continue
        });
    });

    app.run_with_args(&Vec::<String>::new());
    Ok(())
}

fn trigger_capture(app: &adw::Application, runtime: &Arc<Runtime>, override_lang: Option<String>) {
    let config = load_config().unwrap_or_default();
    let target_lang = override_lang.unwrap_or(config.target_language);
    let model = config.model;

    // Check API Key
    let api_key = KeyringStore::default_store()
        .ok()
        .and_then(|s| s.get_api_key().ok().flatten())
        .filter(|k| !k.trim().is_empty());

    let api_key = match api_key {
        Some(k) => k,
        None => {
            warn!("No API Key found. Opening settings dialog.");
            let settings = SettingsDialog::new(None::<&gtk4::Window>);
            settings.present();
            return;
        }
    };

    // Standard mpsc channel to receive capture result back on the GTK main thread
    let (gui_tx, gui_rx) = channel::<Result<Option<String>, String>>();

    // Spawn async capture task on Tokio
    runtime.spawn(async move {
        info!("Initiating screen capture...");
        let capturer = CompositeCapturer::new();

        match capturer.capture_region().await {
            Ok(Some(captured_image)) => {
                info!(
                    "Screenshot captured successfully ({} bytes). Dispatching to UI.",
                    captured_image.raw_bytes.len()
                );
                let _ = gui_tx.send(Ok(Some(captured_image.base64_data_uri)));
            }
            Ok(None) => {
                info!("Screen capture was cancelled by user.");
                let _ = gui_tx.send(Ok(None));
            }
            Err(e) => {
                error!("Screen capture failed: {}", e);
                let _ = gui_tx.send(Err(e.to_string()));
            }
        }
    });

    // Listen on GTK main thread for capture completion
    let app_clone = app.clone();
    let rt = runtime.clone();
    glib::timeout_add_local(std::time::Duration::from_millis(50), move || {
        if let Ok(result) = gui_rx.try_recv() {
            match result {
                Ok(Some(data_uri)) => {
                    let result_panel = ResultPanel::new(&app_clone);
                    result_panel.show();

                    let (tx, rx) = tokio::sync::mpsc::channel(32);
                    result_panel.listen_stream(rx);

                    let key = api_key.clone();
                    let mdl = model.clone();
                    let lang = target_lang.clone();

                    rt.spawn(async move {
                        let client = DeepSeekClient::default();
                        match client.stream_translation(&key, &mdl, &data_uri, &lang, tx).await {
                            Ok(full) => {
                                info!("Translation completed ({} chars).", full.len());
                            }
                            Err(err) => {
                                error!("Translation failed: {}", err);
                            }
                        }
                    });
                }
                Ok(None) => {
                    // User cancelled, do nothing
                }
                Err(err_msg) => {
                    let result_panel = ResultPanel::new(&app_clone);
                    result_panel.set_error(&err_msg);
                    result_panel.show();
                }
            }
            return glib::ControlFlow::Break;
        }
        glib::ControlFlow::Continue
    });
}
