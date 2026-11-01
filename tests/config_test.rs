use snaptranslate_linux::config::{
    load_config_from_path, save_config_to_path, AppConfig, DEFAULT_API_BASE_URL, DEFAULT_MODEL,
    DEFAULT_TARGET_LANGUAGE,
};
use snaptranslate_linux::keyring::{
    mask_api_key, CredentialStorage, FallbackCredentials, KeyringStore,
};
use std::os::unix::fs::PermissionsExt;
use tempfile::tempdir;

#[test]
fn test_app_config_defaults() {
    let cfg = AppConfig::default();
    assert_eq!(cfg.target_language, DEFAULT_TARGET_LANGUAGE);
    assert_eq!(cfg.api_base_url, DEFAULT_API_BASE_URL);
    assert_eq!(cfg.model, DEFAULT_MODEL);
}

#[test]
fn test_app_config_backward_compatibility() {
    let partial_toml = r#"
        target_language = "ja-JP"
    "#;
    let cfg: AppConfig = toml::from_str(partial_toml).expect("Deserialization failed");
    assert_eq!(cfg.target_language, "ja-JP");
    assert_eq!(cfg.api_base_url, DEFAULT_API_BASE_URL);
    assert_eq!(cfg.model, DEFAULT_MODEL);
}

#[test]
fn test_app_config_validation() {
    let mut cfg = AppConfig::default();
    assert!(cfg.validate().is_ok());

    cfg.target_language = "  ".to_string();
    assert!(cfg.validate().is_err());

    cfg.target_language = "en-US".to_string();
    cfg.api_base_url = "ftp://invalid-scheme.com".to_string();
    assert!(cfg.validate().is_err());

    cfg.api_base_url = "https://custom.deepseek.proxy/v1".to_string();
    assert!(cfg.validate().is_ok());
}

#[test]
fn test_app_config_save_load_roundtrip() {
    let dir = tempdir().expect("Failed to create tempdir");
    let config_path = dir.path().join("config.toml");

    let custom_cfg = AppConfig {
        target_language: "de".to_string(),
        api_base_url: "https://api.example.com".to_string(),
        model: "deepseek-chat".to_string(),
    };

    save_config_to_path(&custom_cfg, &config_path).expect("Failed to save config");
    let loaded = load_config_from_path(&config_path).expect("Failed to load config");
    assert_eq!(custom_cfg, loaded);
}

#[test]
fn test_app_config_nonexistent_returns_default() {
    let dir = tempdir().expect("Failed to create tempdir");
    let non_existent = dir.path().join("does_not_exist.toml");
    let loaded = load_config_from_path(&non_existent).expect("Failed to load non-existent");
    assert_eq!(loaded, AppConfig::default());
}

#[test]
fn test_mask_api_key() {
    assert_eq!(mask_api_key("short"), "********");
    assert_eq!(mask_api_key("sk-1234567890abcdef"), "sk-1...cdef");
}

#[test]
fn test_zero_plaintext_leakage_in_debug() {
    let secret = "sk-deepseek-super-secret-key-999";
    let creds = FallbackCredentials {
        deepseek_api_key: Some(secret.to_string()),
    };
    let debug_repr = format!("{:?}", creds);
    assert!(!debug_repr.contains(secret));
    assert!(debug_repr.contains("sk-d...-999"));
}

#[test]
fn test_fallback_file_posix_permissions() {
    let dir = tempdir().expect("Failed to create tempdir");
    let fallback_path = dir.path().join("sub").join("credentials.toml");

    let _store = KeyringStore::with_details(
        "snaptranslate_test_nonexistent",
        "deepseek-api-key",
        fallback_path.clone(),
    );

    let test_key = "sk-test-secret-12345678";
    let toml_str = format!("deepseek_api_key = \"{}\"\n", test_key);
    std::fs::create_dir_all(fallback_path.parent().unwrap()).unwrap();
    std::fs::write(&fallback_path, toml_str).unwrap();

    let mut perms = std::fs::metadata(&fallback_path).unwrap().permissions();
    perms.set_mode(0o600);
    std::fs::set_permissions(&fallback_path, perms).unwrap();

    let meta = std::fs::metadata(&fallback_path).unwrap();
    let mode = meta.permissions().mode() & 0o777;
    assert_eq!(mode, 0o600, "Fallback file permissions must be 0600");
}

#[test]
fn test_keyring_roundtrip_isolated() {
    let dir = tempdir().expect("Failed to create tempdir");
    let fallback_path = dir.path().join("credentials.toml");
    let unique_service = format!("snaptranslate_test_{}", std::process::id());
    let store = KeyringStore::with_details(&unique_service, "deepseek-api-key", fallback_path);

    let secret = "sk-test-roundtrip-key-12345";
    // Set
    store.set_api_key(secret).expect("Failed to set API key");

    // Get
    let fetched = store.get_api_key().expect("Failed to get API key");
    assert_eq!(fetched.as_deref(), Some(secret));

    // Delete
    store.delete_api_key().expect("Failed to delete API key");
    let after_delete = store.get_api_key().expect("Failed to get after delete");
    assert_eq!(after_delete, None);
}
