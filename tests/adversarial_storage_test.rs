//! Empirical Challenger Adversarial Test Suite for Milestone 1
//! Tests Keyring & Storage Fallback Subsystem:
//! 1. POSIX permission enforcement (0600 file, 0700 dir)
//! 2. Atomic save under concurrency and rapid updates
//! 3. Corrupt TOML recovery and failure modes
//! 4. Plaintext secret leakage across CLI stdout/stderr and logging

use assert_cmd::Command;
use snaptranslate_linux::config::{
    load_config_from_path, save_config_to_path, AppConfig, ConfigError,
};
use snaptranslate_linux::keyring::{
    ensure_file_mode_0600, mask_api_key, write_secure_0600, CredentialStorage, KeyringError,
    KeyringStore,
};
use std::os::unix::fs::PermissionsExt;
use std::sync::{Arc, Barrier};
use std::thread;
use tempfile::tempdir;

// ============================================================================
// 1. POSIX Permission Enforcement Tests
// ============================================================================

#[test]
fn test_posix_dir_0700_and_file_0600_on_fallback_write() {
    let xdg_dir = tempdir().expect("Failed to create tempdir");
    let fallback_dir = xdg_dir.path().join("snaptranslate");
    let fallback_file = fallback_dir.join("credentials.toml");

    // Run isolated CLI command with invalid DBus to trigger fallback write
    let mut cmd = Command::cargo_bin("snaptranslate-linux").expect("Binary must exist");
    cmd.env("XDG_CONFIG_HOME", xdg_dir.path())
        .env(
            "DBUS_SESSION_BUS_ADDRESS",
            "unix:path=/nonexistent/bus.sock",
        )
        .args(["config", "set-key", "--stdin"])
        .write_stdin("sk-fallback-posix-test-12345678\n");
    cmd.assert().success();

    let dir_meta = std::fs::metadata(&fallback_dir).expect("Directory was not created");
    let dir_mode = dir_meta.permissions().mode() & 0o777;
    assert_eq!(
        dir_mode, 0o700,
        "Credentials parent directory must have POSIX mode 0700, found: 0{:o}",
        dir_mode
    );

    let file_meta = std::fs::metadata(&fallback_file).expect("Credentials file was not created");
    let file_mode = file_meta.permissions().mode() & 0o777;
    assert_eq!(
        file_mode, 0o600,
        "Credentials fallback file must have POSIX mode 0600, found: 0{:o}",
        file_mode
    );

    // Read back via isolated custom store
    let custom_store = KeyringStore::with_details(
        "snaptranslate_adversarial_test_invalid_svc",
        "deepseek-api-key",
        fallback_file,
    );
    let key = custom_store.get_api_key().expect("Failed to read");
    assert_eq!(key.as_deref(), Some("sk-fallback-posix-test-12345678"));
}

#[test]
fn test_posix_dir_0755_remediated_to_0700_on_fallback() {
    let xdg_dir = tempdir().expect("Failed to create tempdir");
    let fallback_dir = xdg_dir.path().join("snaptranslate");
    std::fs::create_dir_all(&fallback_dir).unwrap();

    // Set directory to open permissions: 0755
    let mut perms = std::fs::metadata(&fallback_dir).unwrap().permissions();
    perms.set_mode(0o755);
    std::fs::set_permissions(&fallback_dir, perms).unwrap();

    // Trigger fallback write
    let mut cmd = Command::cargo_bin("snaptranslate-linux").expect("Binary must exist");
    cmd.env("XDG_CONFIG_HOME", xdg_dir.path())
        .env(
            "DBUS_SESSION_BUS_ADDRESS",
            "unix:path=/nonexistent/bus.sock",
        )
        .args(["config", "set-key", "--stdin"])
        .write_stdin("sk-remediate-test-1234\n");
    cmd.assert().success();

    let dir_mode = std::fs::metadata(&fallback_dir)
        .unwrap()
        .permissions()
        .mode()
        & 0o777;
    assert_eq!(
        dir_mode, 0o700,
        "Pre-existing 0755 directory must be corrected to 0700"
    );
}

#[test]
fn test_posix_file_0644_remediated_to_0600_on_read() {
    let dir = tempdir().expect("Failed to create tempdir");
    let fallback_path = dir.path().join("credentials.toml");

    let content = "deepseek_api_key = \"sk-test-insecure-perms-1234\"\n";
    std::fs::write(&fallback_path, content).unwrap();

    let mut perms = std::fs::metadata(&fallback_path).unwrap().permissions();
    perms.set_mode(0o644);
    std::fs::set_permissions(&fallback_path, perms).unwrap();

    let mode_before = std::fs::metadata(&fallback_path)
        .unwrap()
        .permissions()
        .mode()
        & 0o777;
    assert_eq!(mode_before, 0o644);

    // Call ensure_file_mode_0600
    ensure_file_mode_0600(&fallback_path);

    let mode_after = std::fs::metadata(&fallback_path)
        .unwrap()
        .permissions()
        .mode()
        & 0o777;
    assert_eq!(
        mode_after, 0o600,
        "Insecure file permissions 0644 must be automatically corrected to 0600"
    );
}

// ============================================================================
// 2. Atomic Save Under Concurrency & Rapid Sequential Updates
// ============================================================================

#[test]
fn test_rapid_sequential_config_updates() {
    let dir = tempdir().expect("Failed to create tempdir");
    let config_path = dir.path().join("config.toml");

    for i in 0..100 {
        let cfg = AppConfig {
            target_language: format!("lang-{}", i),
            api_base_url: format!("https://api.example.com/v{}", i),
            model: format!("model-{}", i),
        };
        save_config_to_path(&cfg, &config_path).expect("Sequential save failed");

        let loaded = load_config_from_path(&config_path).expect("Sequential load failed");
        assert_eq!(loaded, cfg, "Sequential update mismatch at iteration {}", i);
    }
}

#[test]
fn test_in_process_concurrent_config_save_stress() {
    let dir = tempdir().expect("Failed to create tempdir");
    let config_path = dir.path().join("config.toml");
    let num_threads = 64;
    let barrier = Arc::new(Barrier::new(num_threads));

    let mut handles = Vec::new();

    for t in 0..num_threads {
        let path_clone = config_path.clone();
        let barrier_clone = Arc::clone(&barrier);

        handles.push(thread::spawn(move || {
            let cfg = AppConfig {
                target_language: format!("lang-{}", t),
                api_base_url: "https://api.deepseek.com".to_string(),
                model: format!("model-{}", t),
            };

            barrier_clone.wait();
            save_config_to_path(&cfg, &path_clone)
        }));
    }

    let mut successes = 0;
    let mut failures = 0;

    for h in handles {
        match h.join().unwrap() {
            Ok(()) => successes += 1,
            Err(e) => {
                eprintln!("Save error: {e}");
                failures += 1;
            }
        }
    }

    assert_eq!(failures, 0, "No concurrent save operations should fail");
    assert_eq!(
        successes, num_threads,
        "All concurrent save operations should succeed"
    );

    // Verify final file is valid TOML and readable
    let loaded = load_config_from_path(&config_path)
        .expect("Target config file must be valid after concurrent writes");
    assert!(loaded.target_language.starts_with("lang-"));

    // Verify no orphaned temporary files exist
    let entries = std::fs::read_dir(dir.path())
        .unwrap()
        .collect::<Result<Vec<_>, _>>()
        .unwrap();
    assert_eq!(
        entries.len(),
        1,
        "Only config.toml should exist; no orphaned tmp files"
    );
}

#[test]
fn test_in_process_concurrent_keyring_fallback_save_stress() {
    let dir = tempdir().expect("Failed to create tempdir");
    let creds_path = dir.path().join("credentials.toml");
    let num_threads = 64;
    let barrier = Arc::new(Barrier::new(num_threads));

    let mut handles = Vec::new();

    for t in 0..num_threads {
        let path_clone = creds_path.clone();
        let barrier_clone = Arc::clone(&barrier);

        handles.push(thread::spawn(move || {
            let content = format!("deepseek_api_key = \"sk-concurrent-test-{}\"\n", t);
            barrier_clone.wait();
            write_secure_0600(&path_clone, &content)
        }));
    }

    let mut successes = 0;
    let mut failures = 0;

    for h in handles {
        match h.join().unwrap() {
            Ok(()) => successes += 1,
            Err(e) => {
                eprintln!("Keyring save error: {e}");
                failures += 1;
            }
        }
    }

    assert_eq!(failures, 0, "No concurrent keyring saves should fail");
    assert_eq!(successes, num_threads);

    let meta = std::fs::metadata(&creds_path).unwrap();
    let mode = meta.permissions().mode() & 0o777;
    assert_eq!(mode, 0o600, "Credentials fallback file must have mode 0600");

    let entries = std::fs::read_dir(dir.path())
        .unwrap()
        .collect::<Result<Vec<_>, _>>()
        .unwrap();
    assert_eq!(
        entries.len(),
        1,
        "Only credentials.toml should exist; no orphaned tmp files"
    );
}

// ============================================================================
// 3. Corrupt TOML Recovery & Error Handling Tests
// ============================================================================

#[test]
fn test_corrupt_config_toml_returns_parse_error() {
    let dir = tempdir().expect("Failed to create tempdir");
    let config_path = dir.path().join("config.toml");

    std::fs::write(&config_path, "[[[[invalid [[[ toml syntax === 1234").unwrap();

    let res = load_config_from_path(&config_path);
    assert!(res.is_err(), "Loading corrupt config must fail with error");
    match res.unwrap_err() {
        ConfigError::Parse { path, .. } => {
            assert_eq!(path, config_path);
        }
        other => panic!("Expected ConfigError::Parse, got {:?}", other),
    }
}

#[test]
fn test_partial_or_unknown_fields_in_config_toml() {
    let dir = tempdir().expect("Failed to create tempdir");
    let config_path = dir.path().join("config.toml");

    let toml_content = r#"
        unknown_future_field = "some_value"
        another_extra = 42
        target_language = "es"
    "#;
    std::fs::write(&config_path, toml_content).unwrap();

    let loaded = load_config_from_path(&config_path).expect("Should parse forward-compatibly");
    assert_eq!(loaded.target_language, "es");
    assert_eq!(loaded.model, "deepseek-flash");
    assert_eq!(loaded.api_base_url, "https://api.deepseek.com");
}

#[test]
fn test_corrupt_credentials_toml_returns_parse_error() {
    let dir = tempdir().expect("Failed to create tempdir");
    let fallback_path = dir.path().join("credentials.toml");

    std::fs::write(&fallback_path, "deepseek_api_key = \"unclosed string").unwrap();

    let store = KeyringStore::with_details(
        "snaptranslate_test_corrupt_creds",
        "deepseek-api-key",
        fallback_path.clone(),
    );

    let res = store.get_api_key();
    assert!(res.is_err(), "Corrupt credentials.toml must return error");
    match res.unwrap_err() {
        KeyringError::Parse { path, .. } => {
            assert_eq!(path, fallback_path);
        }
        other => panic!("Expected KeyringError::Parse, got {:?}", other),
    }
}

// ============================================================================
// 4. Sensitive Data Masking & Plaintext Leakage Tests
// ============================================================================

#[test]
fn test_mask_api_key_ascii_boundaries() {
    assert_eq!(mask_api_key(""), "********");
    assert_eq!(mask_api_key("1"), "********");
    assert_eq!(mask_api_key("12345678"), "********");
    assert_eq!(mask_api_key("123456789"), "1234...6789");
    assert_eq!(
        mask_api_key("sk-antigravity-secret-key-abcdef-123456789"),
        "sk-a...6789"
    );
}

#[test]
fn test_mask_api_key_utf8_char_boundary_safety() {
    let unicode_key = "你好世界abcdef1234";
    let panic_result = std::panic::catch_unwind(|| mask_api_key(unicode_key));

    assert!(
        panic_result.is_ok(),
        "mask_api_key must handle non-ASCII char boundaries safely without panic"
    );
    assert_eq!(panic_result.unwrap(), "你好世界...1234");

    // Unicode boundary tests
    assert_eq!(mask_api_key("你好世界"), "********"); // 4 chars <= 8
    assert_eq!(mask_api_key("你好世界你好世界"), "********"); // 8 chars <= 8
    assert_eq!(mask_api_key("你好世界你好世界!"), "你好世界...好世界!"); // 9 chars > 8
    assert_eq!(mask_api_key("🦀🚀🌍💡🎉✨🔥🌟🍎"), "🦀🚀🌍💡...✨🔥🌟🍎"); // 9 emojis > 8
}

#[test]
fn test_cli_set_key_via_stdin_masks_plaintext_output() {
    let xdg_dir = tempdir().expect("Failed to create tempdir");
    let secret = "sk-adversarial-secret-leakage-check-99887766";

    let mut cmd = Command::cargo_bin("snaptranslate-linux").expect("Binary must be built");
    cmd.env("XDG_CONFIG_HOME", xdg_dir.path())
        .args(["config", "set-key", "--stdin"])
        .write_stdin(format!("{}\n", secret));

    let assert_res = cmd.assert().success();
    let stdout = String::from_utf8_lossy(&assert_res.get_output().stdout);
    let stderr = String::from_utf8_lossy(&assert_res.get_output().stderr);

    assert!(
        !stdout.contains(secret),
        "Plaintext API key leaked in stdout on set-key! Output: {}",
        stdout
    );
    assert!(
        !stderr.contains(secret),
        "Plaintext API key leaked in stderr on set-key! Output: {}",
        stderr
    );
}

#[test]
fn test_cli_rejects_key_as_command_line_argument() {
    let secret = "sk-should-never-be-on-argv-12345";
    let mut cmd = Command::cargo_bin("snaptranslate-linux").expect("Binary must be built");
    cmd.args(["config", "set-key", "--key", secret]);
    cmd.assert().failure();

    let mut cmd2 = Command::cargo_bin("snaptranslate-linux").expect("Binary must be built");
    cmd2.args(["config", "-k", secret]);
    cmd2.assert().failure();
}

#[test]
fn test_cli_config_get_masks_key_in_stdout_and_json() {
    let xdg_dir = tempdir().expect("Failed to create tempdir");
    let mut cmd = Command::cargo_bin("snaptranslate-linux").expect("Binary must exist");
    cmd.env("XDG_CONFIG_HOME", xdg_dir.path())
        .env(
            "DBUS_SESSION_BUS_ADDRESS",
            "unix:path=/nonexistent/bus.sock",
        )
        .args(["config", "get", "--json"]);

    let output = cmd.output().expect("Command failed");
    let stdout_str = String::from_utf8_lossy(&output.stdout);

    let json_val: serde_json::Value = serde_json::from_str(&stdout_str).unwrap_or_else(|e| {
        panic!(
            "config get --json output must be valid JSON: {}\nOutput: {}",
            e, stdout_str
        )
    });
    assert!(json_val.is_object());
    assert_eq!(json_val["model"], "deepseek-flash");
}

#[test]
fn test_cli_config_mutators_refuse_to_overwrite_corrupt_config() {
    let xdg_dir = tempdir().expect("Failed to create tempdir");
    let config_dir = xdg_dir.path().join("snaptranslate");
    std::fs::create_dir_all(&config_dir).unwrap();
    let config_file = config_dir.join("config.toml");
    let corrupt_content = "[[[corrupted = toml syntax !!! invalid\n";
    std::fs::write(&config_file, corrupt_content).unwrap();

    // 1. Verify set-language fails and preserves file
    let mut cmd = Command::cargo_bin("snaptranslate-linux").expect("Binary must exist");
    cmd.env("XDG_CONFIG_HOME", xdg_dir.path())
        .args(["config", "set-language", "ja"]);
    let output = cmd.output().expect("Execution failed");
    assert!(
        !output.status.success(),
        "set-language must fail on corrupt config"
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("Failed to load configuration file"),
        "Error message must clearly explain configuration load failure. Stderr: {}",
        stderr
    );

    let content_after = std::fs::read_to_string(&config_file).unwrap();
    assert_eq!(
        content_after, corrupt_content,
        "Corrupt config file must NOT be overwritten or wiped"
    );

    // 2. Verify config get fails
    let mut cmd_get = Command::cargo_bin("snaptranslate-linux").expect("Binary must exist");
    cmd_get
        .env("XDG_CONFIG_HOME", xdg_dir.path())
        .args(["config", "get"]);
    assert!(!cmd_get.output().unwrap().status.success());

    // 3. Verify config get --json fails
    let mut cmd_json = Command::cargo_bin("snaptranslate-linux").expect("Binary must exist");
    cmd_json
        .env("XDG_CONFIG_HOME", xdg_dir.path())
        .args(["config", "get", "--json"]);
    assert!(!cmd_json.output().unwrap().status.success());
}
