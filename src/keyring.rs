use std::path::{Path, PathBuf};
use serde::{Deserialize, Serialize};

pub const KEYRING_SERVICE: &str = "snaptranslate";
pub const KEYRING_ACCOUNT: &str = "deepseek-api-key";
pub const CREDENTIALS_FILE_NAME: &str = "credentials.toml";

/// Helper function to mask API keys for safe logging or debug displays.
/// e.g. "sk-1234567890abcdef" -> "sk-1...cdef"
pub fn mask_api_key(key: &str) -> String {
    let len = key.len();
    if len <= 8 {
        "********".to_string()
    } else {
        format!("{}...{}", &key[..4], &key[len - 4..])
    }
}

/// Fallback file schema for credential storage when system Secret Service is unavailable.
#[derive(Clone, Default, Serialize, Deserialize)]
pub struct FallbackCredentials {
    #[serde(default)]
    pub deepseek_api_key: Option<String>,
}

impl std::fmt::Debug for FallbackCredentials {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("FallbackCredentials")
            .field(
                "deepseek_api_key",
                &self.deepseek_api_key.as_deref().map(mask_api_key),
            )
            .finish()
    }
}

#[derive(Debug, thiserror::Error)]
pub enum KeyringError {
    #[error("Keyring error: {0}")]
    Keyring(#[from] keyring::Error),

    #[error("I/O error at '{path}': {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },

    #[error("Credentials parse error in '{path}': {source}")]
    Parse {
        path: PathBuf,
        #[source]
        source: toml::de::Error,
    },

    #[error("Credentials serialization error: {0}")]
    Serialize(#[from] toml::ser::Error),

    #[error("Standard configuration directory could not be located")]
    ConfigDirNotFound,
}

pub trait CredentialStorage: Send + Sync {
    fn get_api_key(&self) -> Result<Option<String>, KeyringError>;
    fn set_api_key(&self, key: &str) -> Result<(), KeyringError>;
    fn delete_api_key(&self) -> Result<(), KeyringError>;
}

/// Unified credential store combining Secret Service and 0600 file fallback.
pub struct KeyringStore {
    service: String,
    account: String,
    fallback_path: PathBuf,
}

impl KeyringStore {
    /// Creates the standard production KeyringStore.
    pub fn default_store() -> Result<Self, KeyringError> {
        let base_dir = dirs::config_dir()
            .map(|p| p.join("snaptranslate"))
            .ok_or(KeyringError::ConfigDirNotFound)?;
        let fallback_path = base_dir.join(CREDENTIALS_FILE_NAME);

        Ok(Self {
            service: KEYRING_SERVICE.to_string(),
            account: KEYRING_ACCOUNT.to_string(),
            fallback_path,
        })
    }

    /// Creates a store with custom service, account, and fallback path (useful for testing).
    pub fn with_details(
        service: impl Into<String>,
        account: impl Into<String>,
        fallback_path: PathBuf,
    ) -> Self {
        Self {
            service: service.into(),
            account: account.into(),
            fallback_path,
        }
    }

    fn keyring_entry(&self) -> Result<keyring::Entry, keyring::Error> {
        keyring::Entry::new(&self.service, &self.account)
    }

    // --- Fallback file I/O operations ---

    fn read_fallback(&self) -> Result<Option<String>, KeyringError> {
        if !self.fallback_path.exists() {
            return Ok(None);
        }

        #[cfg(unix)]
        ensure_file_mode_0600(&self.fallback_path);

        let content = std::fs::read_to_string(&self.fallback_path).map_err(|e| {
            KeyringError::Io {
                path: self.fallback_path.clone(),
                source: e,
            }
        })?;

        let creds: FallbackCredentials =
            toml::from_str(&content).map_err(|e| KeyringError::Parse {
                path: self.fallback_path.clone(),
                source: e,
            })?;

        Ok(creds.deepseek_api_key.filter(|k| !k.trim().is_empty()))
    }

    fn write_fallback(&self, key: &str) -> Result<(), KeyringError> {
        if let Some(parent) = self.fallback_path.parent() {
            #[cfg(unix)]
            ensure_dir_mode_0700(parent).map_err(|e| KeyringError::Io {
                path: parent.to_path_buf(),
                source: e,
            })?;
            #[cfg(not(unix))]
            std::fs::create_dir_all(parent).map_err(|e| KeyringError::Io {
                path: parent.to_path_buf(),
                source: e,
            })?;
        }

        let creds = FallbackCredentials {
            deepseek_api_key: Some(key.to_string()),
        };
        let toml_str = toml::to_string_pretty(&creds)?;

        #[cfg(unix)]
        write_secure_0600(&self.fallback_path, &toml_str).map_err(|e| KeyringError::Io {
            path: self.fallback_path.clone(),
            source: e,
        })?;

        #[cfg(not(unix))]
        std::fs::write(&self.fallback_path, toml_str).map_err(|e| KeyringError::Io {
            path: self.fallback_path.clone(),
            source: e,
        })?;

        Ok(())
    }

    fn remove_fallback(&self) -> Result<(), KeyringError> {
        if self.fallback_path.exists() {
            std::fs::remove_file(&self.fallback_path).map_err(|e| KeyringError::Io {
                path: self.fallback_path.clone(),
                source: e,
            })?;
        }
        Ok(())
    }
}

impl CredentialStorage for KeyringStore {
    fn get_api_key(&self) -> Result<Option<String>, KeyringError> {
        match self.keyring_entry() {
            Ok(entry) => match entry.get_password() {
                Ok(password) if !password.trim().is_empty() => Ok(Some(password)),
                Ok(_) => self.read_fallback(),
                Err(keyring::Error::NoEntry) => self.read_fallback(),
                Err(err) => {
                    tracing::warn!(
                        "System keyring read failed ({err}); attempting fallback credential file"
                    );
                    self.read_fallback()
                }
            },
            Err(err) => {
                tracing::warn!(
                    "System keyring entry initialization failed ({err}); attempting fallback"
                );
                self.read_fallback()
            }
        }
    }

    fn set_api_key(&self, key: &str) -> Result<(), KeyringError> {
        let trimmed = key.trim();
        let keyring_result = self.keyring_entry().and_then(|entry| entry.set_password(trimmed));

        match keyring_result {
            Ok(()) => {
                // Key successfully saved to Secret Service; remove unencrypted fallback file
                let _ = self.remove_fallback();
                Ok(())
            }
            Err(err) => {
                tracing::warn!(
                    "Failed to save to system keyring ({err}); saving to secure fallback store with mode 0600"
                );
                self.write_fallback(trimmed)
            }
        }
    }

    fn delete_api_key(&self) -> Result<(), KeyringError> {
        if let Ok(entry) = self.keyring_entry() {
            // In keyring 4.2, the deletion method is delete_credential()
            if let Err(e) = entry.delete_credential() {
                if !matches!(e, keyring::Error::NoEntry) {
                    tracing::warn!("System keyring delete_credential returned: {e}");
                }
            }
        }

        self.remove_fallback()?;
        Ok(())
    }
}

// --- Top-Level Convenience Functions ---

pub fn get_api_key() -> Result<Option<String>, KeyringError> {
    KeyringStore::default_store()?.get_api_key()
}

pub fn set_api_key(key: &str) -> Result<(), KeyringError> {
    KeyringStore::default_store()?.set_api_key(key)
}

pub fn delete_api_key() -> Result<(), KeyringError> {
    KeyringStore::default_store()?.delete_api_key()
}

pub fn default_credentials_path() -> Result<PathBuf, KeyringError> {
    let base_dir = dirs::config_dir()
        .map(|p| p.join("snaptranslate"))
        .ok_or(KeyringError::ConfigDirNotFound)?;
    Ok(base_dir.join(CREDENTIALS_FILE_NAME))
}

pub fn get_credentials_path() -> PathBuf {
    default_credentials_path().unwrap_or_else(|_| PathBuf::from(CREDENTIALS_FILE_NAME))
}

// --- POSIX Security Helpers ---

#[cfg(unix)]
fn ensure_dir_mode_0700(dir: &Path) -> std::io::Result<()> {
    use std::os::unix::fs::PermissionsExt;
    if !dir.exists() {
        std::fs::create_dir_all(dir)?;
    }
    let mut perms = std::fs::metadata(dir)?.permissions();
    if perms.mode() & 0o777 != 0o700 {
        perms.set_mode(0o700);
        std::fs::set_permissions(dir, perms)?;
    }
    Ok(())
}

#[cfg(unix)]
pub fn ensure_file_mode_0600(file: &Path) {
    use std::os::unix::fs::PermissionsExt;
    if let Ok(meta) = std::fs::metadata(file) {
        let current_mode = meta.permissions().mode() & 0o777;
        if current_mode != 0o600 {
            tracing::warn!(
                path = %file.display(),
                mode = format!("{current_mode:o}"),
                "Correcting credential file permissions to 0600"
            );
            let mut perms = meta.permissions();
            perms.set_mode(0o600);
            let _ = std::fs::set_permissions(file, perms);
        }
    }
}

#[cfg(unix)]
fn write_secure_0600(path: &Path, content: &str) -> std::io::Result<()> {
    use std::io::Write;
    use std::os::unix::fs::OpenOptionsExt;
    use std::os::unix::fs::PermissionsExt;

    let tmp_path = path.with_extension(format!("tmp.{}", std::process::id()));
    {
        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .mode(0o600)
            .open(&tmp_path)?;

        let mut perms = file.metadata()?.permissions();
        perms.set_mode(0o600);
        file.set_permissions(perms)?;

        file.write_all(content.as_bytes())?;
        file.sync_all()?;
    }

    std::fs::rename(&tmp_path, path)?;
    Ok(())
}
