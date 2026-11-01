use std::path::{Path, PathBuf};
use serde::{Deserialize, Serialize};

pub const DEFAULT_TARGET_LANGUAGE: &str = "zh-CN";
pub const DEFAULT_API_BASE_URL: &str = "https://api.deepseek.com";
pub const DEFAULT_MODEL: &str = "deepseek-flash";
pub const APP_CONFIG_DIR_NAME: &str = "snaptranslate";
pub const CONFIG_FILE_NAME: &str = "config.toml";

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(default)]
pub struct AppConfig {
    pub target_language: String,
    pub api_base_url: String,
    pub model: String,
}

impl Default for AppConfig {
    fn default() -> Self {
        Self {
            target_language: DEFAULT_TARGET_LANGUAGE.to_string(),
            api_base_url: DEFAULT_API_BASE_URL.to_string(),
            model: DEFAULT_MODEL.to_string(),
        }
    }
}

impl AppConfig {
    /// Validates the configuration values.
    pub fn validate(&self) -> Result<(), ConfigError> {
        if self.target_language.trim().is_empty() {
            return Err(ConfigError::Validation(
                "target_language cannot be empty".to_string(),
            ));
        }
        if self.model.trim().is_empty() {
            return Err(ConfigError::Validation(
                "model cannot be empty".to_string(),
            ));
        }
        let url = self.api_base_url.trim();
        if !url.starts_with("http://") && !url.starts_with("https://") {
            return Err(ConfigError::Validation(
                "api_base_url must start with http:// or https://".to_string(),
            ));
        }
        Ok(())
    }
}

#[derive(Debug, thiserror::Error)]
pub enum ConfigError {
    #[error("Standard configuration directory could not be located")]
    ConfigDirNotFound,

    #[error("I/O error at '{path}': {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },

    #[error("TOML deserialization error in '{path}': {source}")]
    Parse {
        path: PathBuf,
        #[source]
        source: toml::de::Error,
    },

    #[error("TOML serialization error: {0}")]
    Serialize(#[from] toml::ser::Error),

    #[error("Config validation error: {0}")]
    Validation(String),
}

/// Returns standard configuration directory: `~/.config/snaptranslate`
pub fn default_config_dir() -> Result<PathBuf, ConfigError> {
    dirs::config_dir()
        .map(|p| p.join(APP_CONFIG_DIR_NAME))
        .ok_or(ConfigError::ConfigDirNotFound)
}

/// Returns standard configuration file path: `~/.config/snaptranslate/config.toml`
pub fn default_config_path() -> Result<PathBuf, ConfigError> {
    Ok(default_config_dir()?.join(CONFIG_FILE_NAME))
}

/// Convenience function to get configuration file path without returning Result.
pub fn get_config_path() -> PathBuf {
    default_config_path().unwrap_or_else(|_| PathBuf::from(CONFIG_FILE_NAME))
}

/// Loads configuration from standard path (~/.config/snaptranslate/config.toml).
/// If the file does not exist, returns `AppConfig::default()`.
pub fn load_config() -> Result<AppConfig, ConfigError> {
    let path = default_config_path()?;
    load_config_from_path(&path)
}

/// Saves configuration to standard path (~/.config/snaptranslate/config.toml).
pub fn save_config(config: &AppConfig) -> Result<(), ConfigError> {
    let path = default_config_path()?;
    save_config_to_path(config, &path)
}

/// Loads configuration from an explicit path.
/// If file does not exist, returns `AppConfig::default()`.
pub fn load_config_from_path(path: &Path) -> Result<AppConfig, ConfigError> {
    if !path.exists() {
        return Ok(AppConfig::default());
    }

    let content = std::fs::read_to_string(path).map_err(|e| ConfigError::Io {
        path: path.to_path_buf(),
        source: e,
    })?;

    let config: AppConfig = toml::from_str(&content).map_err(|e| ConfigError::Parse {
        path: path.to_path_buf(),
        source: e,
    })?;

    config.validate()?;
    Ok(config)
}

/// Atomically saves configuration to the specified path.
pub fn save_config_to_path(config: &AppConfig, path: &Path) -> Result<(), ConfigError> {
    config.validate()?;

    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| ConfigError::Io {
            path: parent.to_path_buf(),
            source: e,
        })?;
    }

    let toml_str = toml::to_string_pretty(config)?;
    let tmp_path = path.with_extension(format!("tmp.{}", std::process::id()));

    std::fs::write(&tmp_path, toml_str).map_err(|e| ConfigError::Io {
        path: tmp_path.clone(),
        source: e,
    })?;

    std::fs::rename(&tmp_path, path).map_err(|e| ConfigError::Io {
        path: path.to_path_buf(),
        source: e,
    })?;

    Ok(())
}
