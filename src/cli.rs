//! Command-line interface definitions and subcommand handlers for SnapTranslate-linux.

use clap::{Args, Parser, Subcommand};
use std::io::{self, IsTerminal, Write};
use crate::config::{get_config_path, load_config, save_config};
use crate::keyring::{delete_api_key, get_api_key, get_credentials_path, set_api_key};

pub use crate::keyring::mask_api_key;

/// SnapTranslate-linux: Native Linux screen capture translation desktop tool.
#[derive(Debug, Parser)]
#[command(
    name = "snaptranslate-linux",
    author,
    version,
    about = "Lightweight native Linux screenshot translation tool with DeepSeek streaming",
    long_about = "SnapTranslate is a lightweight, native Linux desktop application written in Rust and GTK4.\n\
                  It captures screen regions via XDG Desktop Portal (or native fallbacks), streams OCR and\n\
                  translations via DeepSeek multimodal AI, and displays results in a floating dismissable window.\n\n\
                  When executed without subcommands, it starts the background system tray application.\n\
                  Subcommands such as 'capture' can be bound to desktop keyboard shortcuts."
)]
pub struct Cli {
    #[command(subcommand)]
    pub command: Option<Commands>,

    /// Increase logging verbosity (-v, -vv, -vvv)
    #[arg(short = 'v', long = "verbose", action = clap::ArgAction::Count, global = true)]
    pub verbose: u8,

    /// Suppress non-error informational messages
    #[arg(short = 'q', long = "quiet", global = true)]
    pub quiet: bool,
}

#[derive(Debug, Subcommand, Clone, PartialEq, Eq)]
pub enum Commands {
    /// Trigger screen capture and launch the floating translation window
    #[command(name = "capture")]
    Capture(CaptureArgs),

    /// View or modify application configuration and API credentials
    #[command(name = "config")]
    Config(ConfigArgs),
}

#[derive(Debug, Args, Clone, PartialEq, Eq, Default)]
pub struct CaptureArgs {
    /// Override the default target language for this capture session (e.g. 'zh-CN', 'en', 'ja', 'es')
    #[arg(short = 'l', long = "lang", value_name = "LANG")]
    pub target_language: Option<String>,
}

#[derive(Debug, Args, Clone, PartialEq, Eq, Default)]
pub struct ConfigArgs {
    #[command(subcommand)]
    pub action: Option<ConfigAction>,
}

#[derive(Debug, Subcommand, Clone, PartialEq, Eq)]
pub enum ConfigAction {
    /// Display active configuration and API key status (masked)
    #[command(name = "get")]
    Get(ConfigGetArgs),

    /// Securely set the DeepSeek API key (via interactive prompt or stdin)
    #[command(name = "set-key")]
    SetKey(ConfigSetKeyArgs),

    /// Update the default target language (e.g. 'zh-CN', 'en', 'ja')
    #[command(name = "set-language", alias = "set-lang")]
    SetLanguage(ConfigSetLanguageArgs),

    /// Update the DeepSeek model (e.g. 'deepseek-flash', 'deepseek-chat')
    #[command(name = "set-model")]
    SetModel(ConfigSetModelArgs),

    /// Update the API base URL (e.g. 'https://api.deepseek.com')
    #[command(name = "set-url")]
    SetUrl(ConfigSetUrlArgs),

    /// Delete stored API key from Keyring and fallback credentials
    #[command(name = "delete-key")]
    DeleteKey(ConfigDeleteKeyArgs),

    /// Print configuration and credentials file paths
    #[command(name = "path")]
    Path,
}

#[derive(Debug, Args, Clone, PartialEq, Eq, Default)]
pub struct ConfigGetArgs {
    /// Output active configuration as JSON (convenient for scripts)
    #[arg(long = "json")]
    pub json: bool,
}

#[derive(Debug, Args, Clone, PartialEq, Eq, Default)]
pub struct ConfigSetKeyArgs {
    /// Read the API key from standard input instead of prompting interactively
    #[arg(long = "stdin")]
    pub stdin: bool,
}

#[derive(Debug, Args, Clone, PartialEq, Eq)]
pub struct ConfigSetLanguageArgs {
    /// Target language code or name (e.g. 'zh-CN', 'en', 'ja', 'es', 'de')
    #[arg(value_name = "LANG")]
    pub language: String,
}

#[derive(Debug, Args, Clone, PartialEq, Eq)]
pub struct ConfigSetModelArgs {
    /// Model name (e.g. 'deepseek-flash', 'deepseek-chat')
    #[arg(value_name = "MODEL")]
    pub model: String,
}

#[derive(Debug, Args, Clone, PartialEq, Eq)]
pub struct ConfigSetUrlArgs {
    /// Base URL for DeepSeek API (e.g. 'https://api.deepseek.com')
    #[arg(value_name = "URL")]
    pub url: String,
}

#[derive(Debug, Args, Clone, PartialEq, Eq, Default)]
pub struct ConfigDeleteKeyArgs {
    /// Force deletion without interactive confirmation
    #[arg(short = 'f', long = "force")]
    pub force: bool,
}

// ============================================================================
// Execution Handlers for Headless CLI Operations
// ============================================================================

/// Execute configuration commands completely headlessly without GUI initialization.
pub fn handle_config_command(args: ConfigArgs) -> Result<(), Box<dyn std::error::Error>> {
    match args.action.unwrap_or(ConfigAction::Get(ConfigGetArgs::default())) {
        ConfigAction::Get(get_args) => handle_config_get(get_args),
        ConfigAction::SetKey(key_args) => handle_config_set_key(key_args),
        ConfigAction::SetLanguage(lang_args) => handle_config_set_language(lang_args),
        ConfigAction::SetModel(model_args) => handle_config_set_model(model_args),
        ConfigAction::SetUrl(url_args) => handle_config_set_url(url_args),
        ConfigAction::DeleteKey(del_args) => handle_config_delete_key(del_args),
        ConfigAction::Path => handle_config_path(),
    }
}

fn handle_config_get(args: ConfigGetArgs) -> Result<(), Box<dyn std::error::Error>> {
    let cfg = load_config().unwrap_or_default();
    let maybe_key = get_api_key().ok().flatten();

    if args.json {
        let json_obj = serde_json::json!({
            "target_language": cfg.target_language,
            "api_base_url": cfg.api_base_url,
            "model": cfg.model,
            "api_key_configured": maybe_key.is_some(),
            "api_key_masked": maybe_key.as_deref().map(mask_api_key),
            "config_path": get_config_path().to_string_lossy(),
            "credentials_path": get_credentials_path().to_string_lossy(),
        });
        println!("{}", serde_json::to_string_pretty(&json_obj)?);
    } else {
        println!("SnapTranslate Configuration:");
        println!("  Target Language: {}", cfg.target_language);
        println!("  API Base URL:    {}", cfg.api_base_url);
        println!("  Model:           {}", cfg.model);
        match maybe_key {
            Some(key) => println!("  API Key Status:  Configured [{}]", mask_api_key(&key)),
            None => println!("  API Key Status:  Not Configured (use 'snaptranslate-linux config set-key' to set)"),
        }
        println!();
        println!("Storage Paths:");
        println!("  Config File:      {}", get_config_path().display());
        println!("  Credentials File: {}", get_credentials_path().display());
    }
    Ok(())
}

fn handle_config_set_key(args: ConfigSetKeyArgs) -> Result<(), Box<dyn std::error::Error>> {
    let key = if args.stdin || !io::stdin().is_terminal() {
        let mut line = String::new();
        io::stdin().read_line(&mut line)?;
        line.trim().to_string()
    } else {
        rpassword::prompt_password("Enter DeepSeek API Key: ")?
            .trim()
            .to_string()
    };

    if key.is_empty() {
        eprintln!("Error: API key cannot be empty.");
        std::process::exit(1);
    }

    if !key.starts_with("sk-") {
        eprintln!("Warning: API key does not start with standard prefix 'sk-'.");
    }

    set_api_key(&key)?;
    println!("API key successfully stored in secure credentials store.");
    Ok(())
}

fn handle_config_set_language(args: ConfigSetLanguageArgs) -> Result<(), Box<dyn std::error::Error>> {
    let lang = args.language.trim();
    if lang.is_empty() {
        eprintln!("Error: Target language cannot be empty.");
        std::process::exit(1);
    }
    let mut cfg = load_config().unwrap_or_default();
    cfg.target_language = lang.to_string();
    save_config(&cfg)?;
    println!("Target language updated to '{}'.", cfg.target_language);
    Ok(())
}

fn handle_config_set_model(args: ConfigSetModelArgs) -> Result<(), Box<dyn std::error::Error>> {
    let model = args.model.trim();
    if model.is_empty() {
        eprintln!("Error: Model cannot be empty.");
        std::process::exit(1);
    }
    let mut cfg = load_config().unwrap_or_default();
    cfg.model = model.to_string();
    save_config(&cfg)?;
    println!("Model updated to '{}'.", cfg.model);
    Ok(())
}

fn handle_config_set_url(args: ConfigSetUrlArgs) -> Result<(), Box<dyn std::error::Error>> {
    let url = args.url.trim();
    if url.is_empty() {
        eprintln!("Error: API base URL cannot be empty.");
        std::process::exit(1);
    }
    let mut cfg = load_config().unwrap_or_default();
    cfg.api_base_url = url.to_string();
    save_config(&cfg)?;
    println!("API base URL updated to '{}'.", cfg.api_base_url);
    Ok(())
}

fn handle_config_delete_key(args: ConfigDeleteKeyArgs) -> Result<(), Box<dyn std::error::Error>> {
    if !args.force && io::stdin().is_terminal() {
        print!("Are you sure you want to delete the stored DeepSeek API key? [y/N]: ");
        io::stdout().flush()?;
        let mut answer = String::new();
        io::stdin().read_line(&mut answer)?;
        if !answer.trim().eq_ignore_ascii_case("y") && !answer.trim().eq_ignore_ascii_case("yes") {
            println!("Aborted.");
            return Ok(());
        }
    }
    delete_api_key()?;
    println!("API key successfully deleted from secure storage.");
    Ok(())
}

fn handle_config_path() -> Result<(), Box<dyn std::error::Error>> {
    println!("Config File:      {}", get_config_path().display());
    println!("Credentials File: {}", get_credentials_path().display());
    Ok(())
}
