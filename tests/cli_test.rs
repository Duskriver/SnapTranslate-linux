//! Integration tests for CLI parsing, subcommands, and security constraints.

use clap::{CommandFactory, Parser};
use snaptranslate_linux::cli::{
    mask_api_key, CaptureArgs, Cli, Commands, ConfigAction, ConfigArgs, ConfigDeleteKeyArgs,
    ConfigSetKeyArgs, ConfigSetLanguageArgs, ConfigSetModelArgs, ConfigSetUrlArgs,
};

#[test]
fn test_clap_grammar_debug_assert() {
    // Validates Clap's internal invariants (no duplicate short flags, proper subcommand tree).
    Cli::command().debug_assert();
}

#[test]
fn test_root_default_parsing() {
    let cli =
        Cli::try_parse_from(["snaptranslate-linux"]).expect("Default root parse should succeed");
    assert!(cli.command.is_none());
    assert_eq!(cli.verbose, 0);
    assert!(!cli.quiet);
}

#[test]
fn test_capture_parsing_default() {
    let cli = Cli::try_parse_from(["snaptranslate-linux", "capture"])
        .expect("Capture parse should succeed");
    match cli.command {
        Some(Commands::Capture(args)) => {
            assert_eq!(args.target_language, None);
        }
        other => panic!("Expected Capture command, got {:?}", other),
    }
}

#[test]
fn test_capture_parsing_with_lang_flags() {
    // Short flag -l
    let cli_short = Cli::try_parse_from(["snaptranslate-linux", "capture", "-l", "ja"])
        .expect("Short flag -l should parse");
    assert_eq!(
        cli_short.command,
        Some(Commands::Capture(CaptureArgs {
            target_language: Some("ja".to_string())
        }))
    );

    // Long flag --lang
    let cli_long = Cli::try_parse_from(["snaptranslate-linux", "capture", "--lang", "zh-CN"])
        .expect("Long flag --lang should parse");
    assert_eq!(
        cli_long.command,
        Some(Commands::Capture(CaptureArgs {
            target_language: Some("zh-CN".to_string())
        }))
    );
}

#[test]
fn test_config_get_parsing() {
    let cli = Cli::try_parse_from(["snaptranslate-linux", "config", "get"])
        .expect("config get should parse");
    match cli.command {
        Some(Commands::Config(ConfigArgs {
            action: Some(ConfigAction::Get(get_args)),
        })) => {
            assert!(!get_args.json);
        }
        other => panic!("Expected ConfigAction::Get, got {:?}", other),
    }

    let cli_json = Cli::try_parse_from(["snaptranslate-linux", "config", "get", "--json"])
        .expect("config get --json should parse");
    match cli_json.command {
        Some(Commands::Config(ConfigArgs {
            action: Some(ConfigAction::Get(get_args)),
        })) => {
            assert!(get_args.json);
        }
        other => panic!("Expected ConfigAction::Get with json=true, got {:?}", other),
    }
}

#[test]
fn test_config_set_key_parsing() {
    // Interactive mode (no flags)
    let cli_interactive = Cli::try_parse_from(["snaptranslate-linux", "config", "set-key"])
        .expect("config set-key interactive should parse");
    assert_eq!(
        cli_interactive.command,
        Some(Commands::Config(ConfigArgs {
            action: Some(ConfigAction::SetKey(ConfigSetKeyArgs { stdin: false }))
        }))
    );

    // Stdin mode
    let cli_stdin = Cli::try_parse_from(["snaptranslate-linux", "config", "set-key", "--stdin"])
        .expect("config set-key --stdin should parse");
    assert_eq!(
        cli_stdin.command,
        Some(Commands::Config(ConfigArgs {
            action: Some(ConfigAction::SetKey(ConfigSetKeyArgs { stdin: true }))
        }))
    );
}

#[test]
fn test_security_invariant_no_key_flag() {
    // Adversarial verification: ensuring passing --key or -k is REJECTED by the CLI parser.
    let res1 = Cli::try_parse_from([
        "snaptranslate-linux",
        "config",
        "set-key",
        "--key",
        "sk-secret123",
    ]);
    assert!(res1.is_err(), "Passing --key must be rejected by parser");

    let res2 = Cli::try_parse_from(["snaptranslate-linux", "config", "-k", "sk-secret123"]);
    assert!(res2.is_err(), "Passing -k must be rejected by parser");
}

#[test]
fn test_config_set_language_parsing() {
    let cli = Cli::try_parse_from(["snaptranslate-linux", "config", "set-language", "en"])
        .expect("config set-language should parse");
    match cli.command {
        Some(Commands::Config(ConfigArgs {
            action: Some(ConfigAction::SetLanguage(args)),
        })) => {
            assert_eq!(
                args,
                ConfigSetLanguageArgs {
                    language: "en".to_string()
                }
            );
        }
        other => panic!("Expected ConfigAction::SetLanguage, got {:?}", other),
    }

    // Test alias set-lang
    let cli_alias = Cli::try_parse_from(["snaptranslate-linux", "config", "set-lang", "fr"])
        .expect("config set-lang alias should parse");
    match cli_alias.command {
        Some(Commands::Config(ConfigArgs {
            action: Some(ConfigAction::SetLanguage(args)),
        })) => {
            assert_eq!(
                args,
                ConfigSetLanguageArgs {
                    language: "fr".to_string()
                }
            );
        }
        other => panic!("Expected ConfigAction::SetLanguage, got {:?}", other),
    }
}

#[test]
fn test_config_set_model_parsing() {
    let cli = Cli::try_parse_from([
        "snaptranslate-linux",
        "config",
        "set-model",
        "deepseek-chat",
    ])
    .expect("config set-model should parse");
    match cli.command {
        Some(Commands::Config(ConfigArgs {
            action: Some(ConfigAction::SetModel(args)),
        })) => {
            assert_eq!(
                args,
                ConfigSetModelArgs {
                    model: "deepseek-chat".to_string()
                }
            );
        }
        other => panic!("Expected ConfigAction::SetModel, got {:?}", other),
    }
}

#[test]
fn test_config_set_url_parsing() {
    let cli = Cli::try_parse_from([
        "snaptranslate-linux",
        "config",
        "set-url",
        "https://api.deepseek.com",
    ])
    .expect("config set-url should parse");
    match cli.command {
        Some(Commands::Config(ConfigArgs {
            action: Some(ConfigAction::SetUrl(args)),
        })) => {
            assert_eq!(
                args,
                ConfigSetUrlArgs {
                    url: "https://api.deepseek.com".to_string()
                }
            );
        }
        other => panic!("Expected ConfigAction::SetUrl, got {:?}", other),
    }
}

#[test]
fn test_config_delete_key_parsing() {
    let cli = Cli::try_parse_from(["snaptranslate-linux", "config", "delete-key"])
        .expect("config delete-key should parse");
    match cli.command {
        Some(Commands::Config(ConfigArgs {
            action: Some(ConfigAction::DeleteKey(args)),
        })) => {
            assert_eq!(args, ConfigDeleteKeyArgs { force: false });
        }
        other => panic!("Expected ConfigAction::DeleteKey, got {:?}", other),
    }

    let cli_force = Cli::try_parse_from(["snaptranslate-linux", "config", "delete-key", "-f"])
        .expect("config delete-key -f should parse");
    match cli_force.command {
        Some(Commands::Config(ConfigArgs {
            action: Some(ConfigAction::DeleteKey(args)),
        })) => {
            assert_eq!(args, ConfigDeleteKeyArgs { force: true });
        }
        other => panic!(
            "Expected ConfigAction::DeleteKey with force=true, got {:?}",
            other
        ),
    }

    let cli_force_long =
        Cli::try_parse_from(["snaptranslate-linux", "config", "delete-key", "--force"])
            .expect("config delete-key --force should parse");
    match cli_force_long.command {
        Some(Commands::Config(ConfigArgs {
            action: Some(ConfigAction::DeleteKey(args)),
        })) => {
            assert_eq!(args, ConfigDeleteKeyArgs { force: true });
        }
        other => panic!(
            "Expected ConfigAction::DeleteKey with force=true, got {:?}",
            other
        ),
    }
}

#[test]
fn test_config_path_parsing() {
    let cli = Cli::try_parse_from(["snaptranslate-linux", "config", "path"])
        .expect("config path should parse");
    match cli.command {
        Some(Commands::Config(ConfigArgs {
            action: Some(ConfigAction::Path),
        })) => {}
        other => panic!("Expected ConfigAction::Path, got {:?}", other),
    }
}

#[test]
fn test_mask_api_key_logic() {
    assert_eq!(mask_api_key("short"), "********");
    assert_eq!(mask_api_key("12345678"), "********");
    assert_eq!(mask_api_key("sk-1234567890abcdef"), "sk-1...cdef");
    assert_eq!(mask_api_key("sk-abcdef1234567890"), "sk-a...7890");
}

#[test]
fn test_help_and_version_flags() {
    let help_err = Cli::try_parse_from(["snaptranslate-linux", "--help"]).unwrap_err();
    assert_eq!(help_err.kind(), clap::error::ErrorKind::DisplayHelp);

    let ver_err = Cli::try_parse_from(["snaptranslate-linux", "--version"]).unwrap_err();
    assert_eq!(ver_err.kind(), clap::error::ErrorKind::DisplayVersion);
}
