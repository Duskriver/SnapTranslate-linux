use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

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
            return Err(ConfigError::Validation("model cannot be empty".to_string()));
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

static CONFIG_TMP_COUNTER: AtomicU64 = AtomicU64::new(0);

fn unique_temp_path(path: &Path) -> PathBuf {
    let pid = std::process::id();
    let thread_id = format!("{:?}", std::thread::current().id())
        .chars()
        .filter(|c| c.is_ascii_digit())
        .collect::<String>();
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let seq = CONFIG_TMP_COUNTER.fetch_add(1, Ordering::SeqCst);

    path.with_extension(format!("tmp.{pid}.{thread_id}.{nanos}.{seq}"))
}

/// Atomically saves configuration to the specified path.
pub fn save_config_to_path(config: &AppConfig, path: &Path) -> Result<(), ConfigError> {
    config.validate()?;

    if let Some(parent) = path.parent() {
        if !parent.as_os_str().is_empty() {
            std::fs::create_dir_all(parent).map_err(|e| ConfigError::Io {
                path: parent.to_path_buf(),
                source: e,
            })?;
        }
    }

    let toml_str = toml::to_string_pretty(config)?;
    let tmp_path = unique_temp_path(path);

    let write_res = (|| -> Result<(), std::io::Error> {
        use std::io::Write;
        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .open(&tmp_path)?;
        file.write_all(toml_str.as_bytes())?;
        file.sync_all()?;
        std::fs::rename(&tmp_path, path)?;
        Ok(())
    })();

    if let Err(e) = write_res {
        let _ = std::fs::remove_file(&tmp_path);
        return Err(ConfigError::Io {
            path: path.to_path_buf(),
            source: e,
        });
    }

    Ok(())
}
