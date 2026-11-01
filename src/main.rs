use clap::Parser;
use snaptranslate_linux::cli::{self, Cli, Commands};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Phase 1: Parse command-line arguments immediately.
    // Clap intercepts `--help` and `--version` at this point, prints help/version, and exits cleanly with 0.
    let cli = Cli::parse();

    // Phase 2: Initialize logging.
    init_logger(cli.verbose, cli.quiet);

    // Phase 3: Headless Subcommand Dispatch.
    // If the user requested a configuration action, execute it headlessly and exit immediately.
    if let Some(Commands::Config(config_args)) = cli.command {
        if let Err(e) = cli::handle_config_command(config_args) {
            eprintln!("Error: {}", e);
            std::process::exit(1);
        }
        std::process::exit(0);
    }

    // Phase 4: Graphical Session Verification.
    // GUI actions (capture, tray) require an active Wayland or X11 compositor.
    if std::env::var("WAYLAND_DISPLAY").is_err() && std::env::var("DISPLAY").is_err() {
        eprintln!("Error: No graphical display detected.");
        eprintln!("SnapTranslate requires Wayland ($WAYLAND_DISPLAY) or X11 ($DISPLAY) to capture screen regions and display the floating UI.");
        std::process::exit(1);
    }

    // Phase 5: GUI Runtime Initialization.
    // In M1, GUI is a stub / scaffolding that validates configuration.
    // In M4, this connects to GTK4 Application and System Tray.
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
        .try_init();
}

fn run_gui_application(command: Option<Commands>) -> Result<(), Box<dyn std::error::Error>> {
    // In M1, GUI is a stub / placeholder.
    println!("Starting SnapTranslate-linux GUI mode...");
    if let Some(Commands::Capture(capture_args)) = command {
        println!(
            "Capture mode triggered with target language: {:?}",
            capture_args.target_language
        );
    } else {
        println!("Background tray mode triggered.");
    }
    Ok(())
}
