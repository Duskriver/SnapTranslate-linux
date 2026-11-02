//! End-to-End (E2E) Test Suite for SnapTranslate-linux CLI Subsystem
//!
//! Covers:
//! - Tier 1: CLI Help & Information (F-CLI-01)
//! - Tier 1: Subcommand Invocations & Routing (F-CLI-02)
//! - Tier 1: Headless Execution (F-HDL-01)
//! - Tier 1: Error Handling & Argument Validation (F-ERR-01)
//! - Tier 2: Boundary & Corner Cases (display env, whitespace, verbosity flags)
//! - Tier 3: CLI Subcommand Aliases & Help Consistency
//! Real GTK / desktop shortcut flows are exercised by scripts/verify-gui-flow.py.

use assert_cmd::Command;
use predicates::prelude::*;
use std::path::PathBuf;
use tempfile::TempDir;

/// Ephemeral sandbox environment isolating test execution from host settings and display.
struct CliTestEnv {
    temp_dir: TempDir,
    pub config_dir: PathBuf,
}

impl CliTestEnv {
    fn new() -> Self {
        let temp_dir = TempDir::new().expect("Failed to create temporary sandbox directory");
        let config_dir = temp_dir.path().join(".config").join("snaptranslate");
        Self {
            temp_dir,
            config_dir,
        }
    }

    /// Spawns the binary in an isolated headless sandbox (display variables removed).
    fn bin(&self) -> Command {
        let mut cmd = Command::cargo_bin("snaptranslate-linux")
            .expect("Compiled snaptranslate-linux binary must be present");
        cmd.env("HOME", self.temp_dir.path());
        cmd.env("XDG_CONFIG_HOME", self.temp_dir.path().join(".config"));
        cmd.env_remove("WAYLAND_DISPLAY");
        cmd.env_remove("DISPLAY");
        // Isolate D-Bus session to guarantee fallback isolation
        cmd.env("DBUS_SESSION_BUS_ADDRESS", "unix:path=/dev/null");
        cmd
    }

    /// An unreachable X11 display must fail instead of pretending to capture.
    fn bin_x11(&self) -> Command {
        let mut cmd = self.bin();
        cmd.env("DISPLAY", ":65535");
        cmd.env("GDK_BACKEND", "x11");
        cmd
    }

    /// An isolated directory with no Wayland socket cannot connect to the host.
    fn bin_wayland(&self) -> Command {
        let mut cmd = self.bin();
        cmd.env("WAYLAND_DISPLAY", "wayland-1");
        cmd.env("XDG_RUNTIME_DIR", self.temp_dir.path());
        cmd.env("GDK_BACKEND", "wayland");
        cmd
    }
}

// ============================================================================
// Tier 1: Feature 1 - CLI Help & Information (>= 5 test cases)
// ============================================================================

#[test]
fn test_cli_help_long_flag() {
    let env = CliTestEnv::new();
    env.bin()
        .arg("--help")
        .assert()
        .success()
        .code(0)
        .stdout(predicate::str::contains("SnapTranslate"))
        .stdout(predicate::str::contains("Usage: snaptranslate-linux"))
        .stdout(predicate::str::contains("Commands:"))
        .stdout(predicate::str::contains("capture"))
        .stdout(predicate::str::contains("config"))
        .stdout(predicate::str::contains("Options:"))
        .stdout(predicate::str::contains("--verbose"))
        .stdout(predicate::str::contains("--quiet"));
}

#[test]
fn test_cli_help_short_flag() {
    let env = CliTestEnv::new();
    env.bin()
        .arg("-h")
        .assert()
        .success()
        .code(0)
        .stdout(predicate::str::contains("Usage: snaptranslate-linux"))
        .stdout(predicate::str::contains("capture"))
        .stdout(predicate::str::contains("config"));
}

#[test]
fn test_cli_version_flags() {
    let env = CliTestEnv::new();
    // Long flag
    env.bin()
        .arg("--version")
        .assert()
        .success()
        .code(0)
        .stdout(predicate::str::contains("snaptranslate-linux 0.1.0"));

    // Short flag
    env.bin()
        .arg("-V")
        .assert()
        .success()
        .code(0)
        .stdout(predicate::str::contains("snaptranslate-linux 0.1.0"));
}

#[test]
fn test_cli_config_help() {
    let env = CliTestEnv::new();
    env.bin()
        .args(["config", "--help"])
        .assert()
        .success()
        .code(0)
        .stdout(predicate::str::contains(
            "View or modify application configuration",
        ))
        .stdout(predicate::str::contains("get"))
        .stdout(predicate::str::contains("set-key"))
        .stdout(predicate::str::contains("set-language"))
        .stdout(predicate::str::contains("set-model"))
        .stdout(predicate::str::contains("set-url"))
        .stdout(predicate::str::contains("delete-key"))
        .stdout(predicate::str::contains("path"));
}

#[test]
fn test_cli_capture_help() {
    let env = CliTestEnv::new();
    env.bin()
        .args(["capture", "--help"])
        .assert()
        .success()
        .code(0)
        .stdout(predicate::str::contains("Trigger screen capture"))
        .stdout(predicate::str::contains("--lang"))
        .stdout(predicate::str::contains("-l"));
}

#[test]
fn test_cli_subcommand_specific_help() {
    let env = CliTestEnv::new();

    // config set-key --help
    env.bin()
        .args(["config", "set-key", "--help"])
        .assert()
        .success()
        .code(0)
        .stdout(predicate::str::contains("--stdin"));

    // config get --help
    env.bin()
        .args(["config", "get", "--help"])
        .assert()
        .success()
        .code(0)
        .stdout(predicate::str::contains("--json"));

    // config delete-key --help
    env.bin()
        .args(["config", "delete-key", "--help"])
        .assert()
        .success()
        .code(0)
        .stdout(predicate::str::contains("--force"))
        .stdout(predicate::str::contains("-f"));
}

// ============================================================================
// Tier 1: Feature 2 - Subcommand Invocations & Routing (>= 5 test cases)
// ============================================================================

#[test]
fn test_subcommand_config_get_default() {
    let env = CliTestEnv::new();
    env.bin()
        .args(["config", "get"])
        .assert()
        .success()
        .code(0)
        .stdout(predicate::str::contains("SnapTranslate Configuration:"))
        .stdout(predicate::str::contains("Target Language: zh-CN"))
        .stdout(predicate::str::contains(
            "API Base URL:    https://api.deepseek.com",
        ))
        .stdout(predicate::str::contains("Model:           deepseek-flash"))
        .stdout(predicate::str::contains("API Key Status:  Not Configured"));
}

#[test]
fn test_subcommand_config_path() {
    let env = CliTestEnv::new();
    let expected_cfg = env.config_dir.join("config.toml").display().to_string();
    let expected_cred = env
        .config_dir
        .join("credentials.toml")
        .display()
        .to_string();

    env.bin()
        .args(["config", "path"])
        .assert()
        .success()
        .code(0)
        .stdout(predicate::str::contains(format!(
            "Config File:      {}",
            expected_cfg
        )))
        .stdout(predicate::str::contains(format!(
            "Credentials File: {}",
            expected_cred
        )));
}

#[test]
fn test_subcommand_config_set_language_execution() {
    let env = CliTestEnv::new();
    env.bin()
        .args(["config", "set-language", "en"])
        .assert()
        .success()
        .code(0)
        .stdout(predicate::str::contains("Target language updated to 'en'."));

    // Verify persistence
    env.bin()
        .args(["config", "get"])
        .assert()
        .success()
        .stdout(predicate::str::contains("Target Language: en"));
}

#[test]
fn test_subcommand_config_set_model_execution() {
    let env = CliTestEnv::new();
    env.bin()
        .args(["config", "set-model", "deepseek-chat"])
        .assert()
        .success()
        .code(0)
        .stdout(predicate::str::contains(
            "Model updated to 'deepseek-chat'.",
        ));

    // Verify persistence
    env.bin()
        .args(["config", "get"])
        .assert()
        .success()
        .stdout(predicate::str::contains("Model:           deepseek-chat"));
}

#[test]
fn test_subcommand_config_set_url_execution() {
    let env = CliTestEnv::new();
    env.bin()
        .args(["config", "set-url", "https://custom.deepseek.local/v1"])
        .assert()
        .success()
        .code(0)
        .stdout(predicate::str::contains(
            "API base URL updated to 'https://custom.deepseek.local/v1'.",
        ));

    // Verify persistence
    env.bin()
        .args(["config", "get"])
        .assert()
        .success()
        .stdout(predicate::str::contains(
            "API Base URL:    https://custom.deepseek.local/v1",
        ));
}

#[test]
fn test_capture_with_unreachable_x11_display_fails() {
    let env = CliTestEnv::new();
    env.bin_x11()
        .arg("capture")
        .assert()
        .failure()
        .code(1)
        .stderr(predicate::str::contains(
            "Failed to initialize graphical display",
        ));
}

#[test]
fn test_capture_language_override_does_not_bypass_display_validation() {
    let env = CliTestEnv::new();
    env.bin_x11()
        .args(["capture", "--lang", "es"])
        .assert()
        .failure()
        .code(1)
        .stderr(predicate::str::contains(
            "Failed to initialize graphical display",
        ));
}

// ============================================================================
// Tier 1: Feature 6 - Headless Execution (>= 5 test cases)
// ============================================================================

#[test]
fn test_headless_config_get_requires_no_display() {
    let env = CliTestEnv::new();
    // env.bin() has no DISPLAY and no WAYLAND_DISPLAY
    env.bin()
        .args(["config", "get"])
        .assert()
        .success()
        .code(0)
        .stdout(predicate::str::contains("SnapTranslate Configuration:"));
}

#[test]
fn test_headless_config_path_requires_no_display() {
    let env = CliTestEnv::new();
    env.bin()
        .args(["config", "path"])
        .assert()
        .success()
        .code(0)
        .stdout(predicate::str::contains("Config File:"));
}

#[test]
fn test_headless_config_set_language_requires_no_display() {
    let env = CliTestEnv::new();
    env.bin()
        .args(["config", "set-language", "ja"])
        .assert()
        .success()
        .code(0)
        .stdout(predicate::str::contains("Target language updated to 'ja'."));
}

#[test]
fn test_headless_config_set_key_via_stdin_requires_no_display() {
    let env = CliTestEnv::new();
    env.bin()
        .args(["config", "set-key", "--stdin"])
        .write_stdin("sk-headless-secret-test-key-1234\n")
        .assert()
        .success()
        .code(0)
        .stdout(predicate::str::contains(
            "API key successfully stored in secure credentials store.",
        ));
}

#[test]
fn test_headless_config_get_json_pipeline_requires_no_display() {
    let env = CliTestEnv::new();
    let output = env
        .bin()
        .args(["config", "get", "--json"])
        .assert()
        .success()
        .code(0)
        .get_output()
        .stdout
        .clone();

    let json_val: serde_json::Value = serde_json::from_slice(&output)
        .expect("Headless config get --json must produce valid JSON");
    assert_eq!(json_val["target_language"], "zh-CN");
    assert_eq!(json_val["model"], "deepseek-flash");
}

// ============================================================================
// Tier 1: Feature 7 - Error Handling & Argument Validation (>= 5 test cases)
// ============================================================================

#[test]
fn test_error_invalid_subcommand() {
    let env = CliTestEnv::new();
    env.bin()
        .arg("unknown-subcommand")
        .assert()
        .failure()
        .code(2)
        .stderr(predicate::str::contains(
            "unrecognized subcommand 'unknown-subcommand'",
        ));
}

#[test]
fn test_error_missing_required_language_argument() {
    let env = CliTestEnv::new();
    env.bin()
        .args(["config", "set-language"])
        .assert()
        .failure()
        .code(2)
        .stderr(predicate::str::contains(
            "required arguments were not provided",
        ));
}

#[test]
fn test_error_empty_language_rejected() {
    let env = CliTestEnv::new();
    env.bin()
        .args(["config", "set-language", ""])
        .assert()
        .failure()
        .code(1)
        .stderr(predicate::str::contains(
            "Error: Target language cannot be empty.",
        ));
}

#[test]
fn test_error_empty_model_rejected() {
    let env = CliTestEnv::new();
    env.bin()
        .args(["config", "set-model", "   "])
        .assert()
        .failure()
        .code(1)
        .stderr(predicate::str::contains("Error: Model cannot be empty."));
}

#[test]
fn test_error_invalid_url_scheme_rejected() {
    let env = CliTestEnv::new();
    env.bin()
        .args(["config", "set-url", "ftp://invalid-server.com"])
        .assert()
        .failure()
        .code(1)
        .stderr(predicate::str::contains(
            "api_base_url must start with http:// or https://",
        ));
}

#[test]
fn test_error_gui_missing_display_detected() {
    let env = CliTestEnv::new();
    // Root binary without subcommands triggers GUI tray mode
    env.bin()
        .assert()
        .failure()
        .code(1)
        .stderr(predicate::str::contains(
            "Error: No graphical display detected.",
        ))
        .stderr(predicate::str::contains(
            "SnapTranslate requires Wayland ($WAYLAND_DISPLAY) or X11 ($DISPLAY)",
        ));

    // Capture subcommand also requires GUI display
    env.bin()
        .arg("capture")
        .assert()
        .failure()
        .code(1)
        .stderr(predicate::str::contains(
            "Error: No graphical display detected.",
        ));
}

// ============================================================================
// Tier 2: Boundary & Corner Cases
// ============================================================================

#[test]
fn test_unreachable_wayland_display_reports_failure() {
    let env = CliTestEnv::new();
    env.bin_wayland()
        .arg("capture")
        .assert()
        .failure()
        .code(1)
        .stderr(predicate::str::contains(
            "Failed to initialize graphical display",
        ));
}

#[test]
fn test_boundary_logging_verbosity_flags() {
    let env = CliTestEnv::new();
    // -v
    env.bin().args(["-v", "config", "path"]).assert().success();

    // -vv
    env.bin().args(["-vv", "config", "path"]).assert().success();

    // -vvv
    env.bin()
        .args(["-vvv", "config", "path"])
        .assert()
        .success();

    // -q (quiet)
    env.bin().args(["-q", "config", "path"]).assert().success();
}

#[test]
fn test_boundary_whitespace_trimming_language() {
    let env = CliTestEnv::new();
    env.bin()
        .args(["config", "set-language", "   ko   "])
        .assert()
        .success()
        .code(0)
        .stdout(predicate::str::contains("Target language updated to 'ko'."));

    env.bin()
        .args(["config", "get"])
        .assert()
        .success()
        .stdout(predicate::str::contains("Target Language: ko"));
}

#[test]
fn test_boundary_url_with_ports_and_paths() {
    let env = CliTestEnv::new();
    let complex_url = "https://internal-gateway.corp.net:8443/v1/deepseek";
    env.bin()
        .args(["config", "set-url", complex_url])
        .assert()
        .success()
        .code(0);

    env.bin()
        .args(["config", "get"])
        .assert()
        .success()
        .stdout(predicate::str::contains(complex_url));
}

// ============================================================================
// Tier 3: CLI Subcommand Aliases & Help Consistency
// ============================================================================

#[test]
fn test_subcommand_alias_set_lang() {
    let env = CliTestEnv::new();
    env.bin()
        .args(["config", "set-lang", "fr"])
        .assert()
        .success()
        .code(0)
        .stdout(predicate::str::contains("Target language updated to 'fr'."));

    env.bin()
        .args(["config", "get"])
        .assert()
        .success()
        .stdout(predicate::str::contains("Target Language: fr"));
}

#[test]
fn test_help_subcommand_syntax_parity() {
    let env = CliTestEnv::new();

    // `snaptranslate-linux help config` vs `snaptranslate-linux config --help`
    let out_subcmd = env
        .bin()
        .args(["help", "config"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let out_flag = env
        .bin()
        .args(["config", "--help"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();

    assert_eq!(
        out_subcmd, out_flag,
        "help config should output identical help text to config --help"
    );
}

// ============================================================================
// A null D-Bus test sandbox must not short-circuit GUI startup as success.
// ============================================================================

#[test]
fn test_null_dbus_does_not_fake_success_for_gui_commands() {
    let env = CliTestEnv::new();

    // No real display exists: these invocations must report startup failure.
    env.bin_x11()
        .args(["capture", "--lang", "zh-CN"])
        .assert()
        .failure()
        .code(1)
        .stderr(predicate::str::contains(
            "Failed to initialize graphical display",
        ));

    env.bin_x11()
        .assert()
        .failure()
        .code(1)
        .stderr(predicate::str::contains(
            "Failed to initialize graphical display",
        ));
}
