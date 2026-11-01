//! End-to-End (E2E) Test Suite for SnapTranslate-linux Configuration & Keyring Subsystem
//!
//! Covers:
//! - Tier 1: Config Persistence (F-CFG-01) (>= 5 tests)
//! - Tier 1: Keyring & Fallback Storage (F-KEY-01) (>= 5 tests)
//! - Tier 1: Credential Masking & Zero Plaintext Leakage (F-SEC-01) (>= 5 tests)
//! - Tier 2: Boundary & Corner Cases (extreme lengths, Unicode, non-sk prefix, corrupt files)
//! - Tier 3: Cross-Feature Interactions (roundtrip lifecycle, key rotation, JSON pipeline)
//! - Tier 4: Real-World Scenarios (onboarding, rotation/revocation, script automation)

use assert_cmd::Command;
use predicates::prelude::*;
use std::path::PathBuf;
use tempfile::TempDir;

#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;

/// Isolated test sandbox guaranteeing complete isolation from host config and keyring.
struct ConfigTestEnv {
    temp_dir: TempDir,
    pub config_dir: PathBuf,
    pub config_file: PathBuf,
    pub credentials_file: PathBuf,
}

impl ConfigTestEnv {
    fn new() -> Self {
        let temp_dir = TempDir::new().expect("Failed to create temporary sandbox directory");
        let config_dir = temp_dir.path().join(".config").join("snaptranslate");
        let config_file = config_dir.join("config.toml");
        let credentials_file = config_dir.join("credentials.toml");

        Self {
            temp_dir,
            config_dir,
            config_file,
            credentials_file,
        }
    }

    /// Spawns the binary configured to use the sandbox's $HOME and $XDG_CONFIG_HOME.
    /// D-Bus session bus address is directed to /dev/null to force the protected 0600 fallback file.
    fn bin(&self) -> Command {
        let mut cmd = Command::cargo_bin("snaptranslate-linux")
            .expect("Compiled snaptranslate-linux binary must be present");
        cmd.env("HOME", self.temp_dir.path());
        cmd.env("XDG_CONFIG_HOME", self.temp_dir.path().join(".config"));
        cmd.env_remove("WAYLAND_DISPLAY");
        cmd.env_remove("DISPLAY");
        cmd.env("DBUS_SESSION_BUS_ADDRESS", "unix:path=/dev/null");
        cmd
    }

    /// Helper to read credentials.toml contents as string.
    fn read_credentials_raw(&self) -> Option<String> {
        if self.credentials_file.exists() {
            Some(
                std::fs::read_to_string(&self.credentials_file)
                    .expect("Failed to read credentials file"),
            )
        } else {
            None
        }
    }

    /// Helper to read config.toml contents as string.
    fn read_config_raw(&self) -> Option<String> {
        if self.config_file.exists() {
            Some(std::fs::read_to_string(&self.config_file).expect("Failed to read config file"))
        } else {
            None
        }
    }
}

// ============================================================================
// Tier 1: Feature 3 - Config Persistence (>= 5 test cases)
// ============================================================================

#[test]
fn test_config_persistence_initial_write_creates_valid_toml() {
    let env = ConfigTestEnv::new();
    assert!(!env.config_file.exists());

    env.bin()
        .args(["config", "set-language", "en"])
        .assert()
        .success()
        .code(0);

    assert!(
        env.config_file.exists(),
        "config.toml should be created on write"
    );
    let content = env.read_config_raw().unwrap();
    assert!(content.contains("target_language = \"en\""));
    assert!(content.contains("api_base_url = \"https://api.deepseek.com\""));
    assert!(content.contains("model = \"deepseek-flash\""));
}

#[test]
fn test_config_persistence_field_independence_target_language() {
    let env = ConfigTestEnv::new();

    // Set custom model first
    env.bin()
        .args(["config", "set-model", "deepseek-chat"])
        .assert()
        .success();
    // Then set language
    env.bin()
        .args(["config", "set-language", "ja"])
        .assert()
        .success();

    let content = env.read_config_raw().unwrap();
    assert!(content.contains("target_language = \"ja\""));
    assert!(content.contains("model = \"deepseek-chat\""));
}

#[test]
fn test_config_persistence_field_independence_model() {
    let env = ConfigTestEnv::new();

    // Set custom url first
    env.bin()
        .args(["config", "set-url", "https://proxy.example.com"])
        .assert()
        .success();
    // Then set model
    env.bin()
        .args(["config", "set-model", "deepseek-coder"])
        .assert()
        .success();

    let content = env.read_config_raw().unwrap();
    assert!(content.contains("api_base_url = \"https://proxy.example.com\""));
    assert!(content.contains("model = \"deepseek-coder\""));
}

#[test]
fn test_config_persistence_missing_file_yields_defaults() {
    let env = ConfigTestEnv::new();
    assert!(!env.config_file.exists());

    env.bin()
        .args(["config", "get"])
        .assert()
        .success()
        .stdout(predicate::str::contains("Target Language: zh-CN"))
        .stdout(predicate::str::contains(
            "API Base URL:    https://api.deepseek.com",
        ))
        .stdout(predicate::str::contains("Model:           deepseek-flash"));
}

#[test]
fn test_config_persistence_partial_toml_augments_defaults() {
    let env = ConfigTestEnv::new();
    std::fs::create_dir_all(&env.config_dir).unwrap();
    // Write partial TOML containing only target_language
    std::fs::write(&env.config_file, "target_language = \"de\"\n").unwrap();

    let output = env
        .bin()
        .args(["config", "get", "--json"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();

    let json: serde_json::Value = serde_json::from_slice(&output).unwrap();
    assert_eq!(json["target_language"], "de");
    assert_eq!(json["api_base_url"], "https://api.deepseek.com");
    assert_eq!(json["model"], "deepseek-flash");
}

#[test]
fn test_config_persistence_atomic_write_no_tmp_leftover() {
    let env = ConfigTestEnv::new();
    env.bin()
        .args(["config", "set-language", "it"])
        .assert()
        .success();

    // Ensure no .tmp files linger in directory
    for entry in std::fs::read_dir(&env.config_dir).unwrap() {
        let entry = entry.unwrap();
        let name = entry.file_name().to_string_lossy().to_string();
        assert!(
            !name.contains(".tmp"),
            "Found dangling temporary file: {}",
            name
        );
    }
}

// ============================================================================
// Tier 1: Feature 4 - Keyring & Fallback Storage (>= 5 test cases)
// ============================================================================

#[test]
fn test_keyring_set_key_via_stdin_creates_credentials_file() {
    let env = ConfigTestEnv::new();
    assert!(!env.credentials_file.exists());

    let test_key = "sk-0123456789abcdef0123456789abcdef";
    env.bin()
        .args(["config", "set-key", "--stdin"])
        .write_stdin(format!("{}\n", test_key))
        .assert()
        .success()
        .stdout(predicate::str::contains(
            "API key successfully stored in secure credentials store.",
        ));

    assert!(env.credentials_file.exists());
    let raw = env.read_credentials_raw().unwrap();
    assert!(raw.contains(test_key));
}

#[test]
#[cfg(unix)]
fn test_keyring_fallback_file_permissions_are_strict_0600() {
    let env = ConfigTestEnv::new();
    env.bin()
        .args(["config", "set-key", "--stdin"])
        .write_stdin("sk-strict-permissions-key-12345\n")
        .assert()
        .success();

    let meta = std::fs::metadata(&env.credentials_file).expect("Failed to read metadata");
    let mode = meta.permissions().mode() & 0o777;
    assert_eq!(
        mode, 0o600,
        "Credentials file MUST have POSIX 0600 mode (-rw-------), found: {:o}",
        mode
    );
}

#[test]
#[cfg(unix)]
fn test_keyring_fallback_directory_permissions_are_strict_0700() {
    let env = ConfigTestEnv::new();
    env.bin()
        .args(["config", "set-key", "--stdin"])
        .write_stdin("sk-strict-dir-permissions-12345\n")
        .assert()
        .success();

    let meta = std::fs::metadata(&env.config_dir).expect("Failed to read dir metadata");
    let mode = meta.permissions().mode() & 0o777;
    assert_eq!(
        mode, 0o700,
        "Credentials parent directory MUST have POSIX 0700 mode (drwx------), found: {:o}",
        mode
    );
}

#[test]
fn test_keyring_read_after_write_reports_configured() {
    let env = ConfigTestEnv::new();
    env.bin()
        .args(["config", "set-key", "--stdin"])
        .write_stdin("sk-read-after-write-secret-9999\n")
        .assert()
        .success();

    env.bin()
        .args(["config", "get"])
        .assert()
        .success()
        .stdout(predicate::str::contains("API Key Status:  Configured"));
}

#[test]
fn test_keyring_delete_key_force_removes_credentials_file() {
    let env = ConfigTestEnv::new();
    // Write key
    env.bin()
        .args(["config", "set-key", "--stdin"])
        .write_stdin("sk-key-to-be-deleted-1234\n")
        .assert()
        .success();
    assert!(env.credentials_file.exists());

    // Delete key with -f
    env.bin()
        .args(["config", "delete-key", "-f"])
        .assert()
        .success()
        .stdout(predicate::str::contains(
            "API key successfully deleted from secure storage.",
        ));

    assert!(
        !env.credentials_file.exists(),
        "credentials.toml should be removed after deletion"
    );

    // Verify status is Not Configured
    env.bin()
        .args(["config", "get"])
        .assert()
        .success()
        .stdout(predicate::str::contains("API Key Status:  Not Configured"));
}

#[test]
#[cfg(unix)]
fn test_keyring_permissive_file_auto_remediated_to_0600() {
    let env = ConfigTestEnv::new();
    std::fs::create_dir_all(&env.config_dir).unwrap();
    std::fs::write(
        &env.credentials_file,
        "deepseek_api_key = \"sk-insecure-mode-key\"\n",
    )
    .unwrap();

    // Set insecure mode 0644 (world-readable)
    let mut perms = std::fs::metadata(&env.credentials_file)
        .unwrap()
        .permissions();
    perms.set_mode(0o644);
    std::fs::set_permissions(&env.credentials_file, perms).unwrap();

    // Run config get which triggers ensure_file_mode_0600
    env.bin().args(["config", "get"]).assert().success();

    let remediated_mode = std::fs::metadata(&env.credentials_file)
        .unwrap()
        .permissions()
        .mode()
        & 0o777;
    assert_eq!(
        remediated_mode, 0o600,
        "Overly permissive 0644 mode must be automatically remediated to 0600"
    );
}

// ============================================================================
// Tier 1: Feature 5 - Credential Masking & Zero Plaintext Leakage (>= 5 test cases)
// ============================================================================

#[test]
fn test_masking_standard_key_in_stdout() {
    let env = ConfigTestEnv::new();
    let secret = "sk-abcdef1234567890fedcba";
    env.bin()
        .args(["config", "set-key", "--stdin"])
        .write_stdin(format!("{}\n", secret))
        .assert()
        .success();

    let output = env
        .bin()
        .args(["config", "get"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();

    let stdout_str = String::from_utf8_lossy(&output);
    assert!(
        !stdout_str.contains(secret),
        "Plaintext key leaked in config get stdout!"
    );
    assert!(
        stdout_str.contains("sk-a...dcba"),
        "Masked key format must be sk-a...dcba"
    );
}

#[test]
fn test_masking_short_key_in_stdout() {
    let env = ConfigTestEnv::new();
    let short_secret = "sk-short"; // 8 chars
    env.bin()
        .args(["config", "set-key", "--stdin"])
        .write_stdin(format!("{}\n", short_secret))
        .assert()
        .success();

    let output = env
        .bin()
        .args(["config", "get"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();

    let stdout_str = String::from_utf8_lossy(&output);
    assert!(
        !stdout_str.contains(short_secret),
        "Plaintext short key leaked in config get!"
    );
    assert!(
        stdout_str.contains("********"),
        "Short key must be masked as ********"
    );
}

#[test]
fn test_masking_json_output_never_reveals_plaintext() {
    let env = ConfigTestEnv::new();
    let secret = "sk-super-secret-production-key-999";
    env.bin()
        .args(["config", "set-key", "--stdin"])
        .write_stdin(format!("{}\n", secret))
        .assert()
        .success();

    let output = env
        .bin()
        .args(["config", "get", "--json"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();

    let stdout_str = String::from_utf8_lossy(&output);
    assert!(
        !stdout_str.contains(secret),
        "Plaintext key leaked in JSON output!"
    );

    let json: serde_json::Value = serde_json::from_str(&stdout_str).unwrap();
    assert_eq!(json["api_key_configured"], true);
    assert_eq!(json["api_key_masked"], "sk-s...-999");
}

#[test]
fn test_masking_set_key_stdin_no_echo() {
    let env = ConfigTestEnv::new();
    let secret = "sk-secret-input-key-for-zero-echo-test";
    let assert_res = env
        .bin()
        .args(["config", "set-key", "--stdin"])
        .write_stdin(format!("{}\n", secret))
        .assert()
        .success();

    let out = assert_res.get_output();
    let stdout_str = String::from_utf8_lossy(&out.stdout);
    let stderr_str = String::from_utf8_lossy(&out.stderr);

    assert!(
        !stdout_str.contains(secret),
        "Plaintext key echoed in stdout during set-key!"
    );
    assert!(
        !stderr_str.contains(secret),
        "Plaintext key echoed in stderr during set-key!"
    );
}

#[test]
fn test_masking_verbose_logging_zero_leakage() {
    let env = ConfigTestEnv::new();
    let secret = "sk-verbose-logging-test-secret-value";
    env.bin()
        .args(["config", "set-key", "--stdin"])
        .write_stdin(format!("{}\n", secret))
        .assert()
        .success();

    let assert_res = env.bin().args(["-vvv", "config", "get"]).assert().success();

    let out = assert_res.get_output();
    let stdout_str = String::from_utf8_lossy(&out.stdout);
    let stderr_str = String::from_utf8_lossy(&out.stderr);

    assert!(
        !stdout_str.contains(secret),
        "Plaintext key found in verbose stdout!"
    );
    assert!(
        !stderr_str.contains(secret),
        "Plaintext key found in verbose stderr!"
    );
}

// ============================================================================
// Tier 2: Boundary & Corner Cases
// ============================================================================

#[test]
fn test_boundary_extreme_length_api_key() {
    let env = ConfigTestEnv::new();
    // 4096-character key starting with sk- and ending with zzzz
    let middle = "a".repeat(4088);
    let long_key = format!("sk-{}zzzz", middle);

    env.bin()
        .args(["config", "set-key", "--stdin"])
        .write_stdin(format!("{}\n", long_key))
        .assert()
        .success();

    let output = env
        .bin()
        .args(["config", "get", "--json"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();

    let json: serde_json::Value = serde_json::from_slice(&output).unwrap();
    assert_eq!(json["api_key_configured"], true);
    assert_eq!(json["api_key_masked"], "sk-a...zzzz");
}

#[test]
fn test_boundary_unicode_target_language() {
    let env = ConfigTestEnv::new();
    let unicode_lang = "zh-Hans-CN-⚡-translation";
    env.bin()
        .args(["config", "set-language", unicode_lang])
        .assert()
        .success()
        .code(0);

    let content = env.read_config_raw().unwrap();
    assert!(content.contains(unicode_lang));

    env.bin()
        .args(["config", "get"])
        .assert()
        .success()
        .stdout(predicate::str::contains(unicode_lang));
}

#[test]
fn test_boundary_non_standard_key_prefix_emits_warning() {
    let env = ConfigTestEnv::new();
    let non_standard = "my-custom-bearer-token-12345678";

    env.bin()
        .args(["config", "set-key", "--stdin"])
        .write_stdin(format!("{}\n", non_standard))
        .assert()
        .success()
        .stderr(predicate::str::contains(
            "Warning: API key does not start with standard prefix 'sk-'.",
        ));

    // Verify it is still stored and masked
    env.bin()
        .args(["config", "get"])
        .assert()
        .success()
        .stdout(predicate::str::contains("Configured [my-c...5678]"));
}

#[test]
fn test_boundary_corrupt_config_file_refuses_overwrite() {
    let env = ConfigTestEnv::new();
    std::fs::create_dir_all(&env.config_dir).unwrap();
    let corrupted_content = "[[[syntax error invalid toml }}}";
    std::fs::write(&env.config_file, corrupted_content).unwrap();

    // Mutator must refuse to overwrite and exit with code 1
    env.bin()
        .args(["config", "set-language", "en"])
        .assert()
        .failure()
        .code(1)
        .stderr(predicate::str::contains(
            "Failed to load configuration file",
        ))
        .stderr(predicate::str::contains(
            "Please repair or remove the corrupted configuration file",
        ));

    // Verify corrupted file was NOT silently overwritten
    let current_content = env.read_config_raw().unwrap();
    assert_eq!(current_content, corrupted_content);
}

#[test]
fn test_boundary_corrupt_credentials_file_fails_gracefully() {
    let env = ConfigTestEnv::new();
    std::fs::create_dir_all(&env.config_dir).unwrap();
    std::fs::write(&env.credentials_file, "invalid = toml [[ unclosed table").unwrap();

    // config get should catch parse error
    env.bin()
        .args(["config", "get"])
        .assert()
        .success() // In current design, keyring get falls back safely or logs error
        .stdout(predicate::str::contains("SnapTranslate Configuration:"));
}

#[test]
fn test_boundary_whitespace_padding_in_key_trimmed() {
    let env = ConfigTestEnv::new();
    let raw_input = "   sk-padded-whitespace-secret-1234   \n";

    env.bin()
        .args(["config", "set-key", "--stdin"])
        .write_stdin(raw_input)
        .assert()
        .success();

    let raw = env.read_credentials_raw().unwrap();
    assert!(raw.contains("\"sk-padded-whitespace-secret-1234\""));
    assert!(!raw.contains("   sk-padded"));
}

// ============================================================================
// Tier 3: Cross-Feature Interactions
// ============================================================================

#[test]
fn test_interaction_full_config_roundtrip_lifecycle() {
    let env = ConfigTestEnv::new();

    // 1. Set API key
    env.bin()
        .args(["config", "set-key", "--stdin"])
        .write_stdin("sk-roundtrip-lifecycle-key-9999\n")
        .assert()
        .success();

    // 2. Set target language
    env.bin()
        .args(["config", "set-language", "ja-JP"])
        .assert()
        .success();

    // 3. Set model
    env.bin()
        .args(["config", "set-model", "deepseek-reasoner"])
        .assert()
        .success();

    // 4. Set url
    env.bin()
        .args(["config", "set-url", "https://api.deepseek.com/v2"])
        .assert()
        .success();

    // 5. Query complete state as JSON
    let output = env
        .bin()
        .args(["config", "get", "--json"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();

    let json: serde_json::Value = serde_json::from_slice(&output).unwrap();
    assert_eq!(json["target_language"], "ja-JP");
    assert_eq!(json["model"], "deepseek-reasoner");
    assert_eq!(json["api_base_url"], "https://api.deepseek.com/v2");
    assert_eq!(json["api_key_configured"], true);
    assert_eq!(json["api_key_masked"], "sk-r...9999");
}

#[test]
fn test_interaction_key_rotation_and_revocation() {
    let env = ConfigTestEnv::new();

    // Initial key
    env.bin()
        .args(["config", "set-key", "--stdin"])
        .write_stdin("sk-initial-key-aaaa1111\n")
        .assert()
        .success();

    env.bin()
        .args(["config", "get"])
        .assert()
        .success()
        .stdout(predicate::str::contains("Configured [sk-i...1111]"));

    // Rotate to secondary key
    env.bin()
        .args(["config", "set-key", "--stdin"])
        .write_stdin("sk-rotated-key-bbbb2222\n")
        .assert()
        .success();

    env.bin()
        .args(["config", "get"])
        .assert()
        .success()
        .stdout(predicate::str::contains("Configured [sk-r...2222]"));

    // Revoke key with force
    env.bin()
        .args(["config", "delete-key", "-f"])
        .assert()
        .success();

    env.bin()
        .args(["config", "get"])
        .assert()
        .success()
        .stdout(predicate::str::contains("Not Configured"));
}

// ============================================================================
// Tier 4: Real-World Scenarios
// ============================================================================

#[test]
fn test_scenario_first_time_user_onboarding() {
    let env = ConfigTestEnv::new();

    // Step 1: User inspects unconfigured app
    env.bin()
        .args(["config", "get"])
        .assert()
        .success()
        .stdout(predicate::str::contains("API Key Status:  Not Configured"));

    // Step 2: User queries file storage paths
    env.bin()
        .args(["config", "path"])
        .assert()
        .success()
        .stdout(predicate::str::contains("config.toml"))
        .stdout(predicate::str::contains("credentials.toml"));

    // Step 3: User configures DeepSeek API key
    env.bin()
        .args(["config", "set-key", "--stdin"])
        .write_stdin("sk-onboarding-user-secret-key-42\n")
        .assert()
        .success();

    // Step 4: User updates target language to Spanish
    env.bin()
        .args(["config", "set-language", "es"])
        .assert()
        .success();

    // Step 5: User verifies complete readiness
    let final_check = env
        .bin()
        .args(["config", "get"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();

    let output_str = String::from_utf8_lossy(&final_check);
    assert!(output_str.contains("Target Language: es"));
    assert!(output_str.contains("API Key Status:  Configured [sk-o...y-42]"));
}

#[test]
fn test_scenario_automated_devops_script_pipeline() {
    let env = ConfigTestEnv::new();

    // DevOps script automates setup via non-interactive CLI flags
    env.bin()
        .args(["config", "set-key", "--stdin"])
        .write_stdin("sk-devops-ci-cd-token-7777\n")
        .assert()
        .success();

    env.bin()
        .args(["config", "set-model", "deepseek-flash"])
        .assert()
        .success();
    env.bin()
        .args(["config", "set-language", "zh-CN"])
        .assert()
        .success();

    // Script pipes JSON output to verify automation status
    let output = env
        .bin()
        .args(["config", "get", "--json"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();

    let parsed: serde_json::Value = serde_json::from_slice(&output).expect("Must parse as JSON");
    assert_eq!(parsed["api_key_configured"], true);
    assert_eq!(parsed["model"], "deepseek-flash");
    assert_eq!(parsed["target_language"], "zh-CN");
}
