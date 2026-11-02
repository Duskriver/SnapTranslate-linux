use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

pub const KEYRING_SERVICE: &str = "snaptranslate";
pub const KEYRING_ACCOUNT: &str = "deepseek-api-key";
pub const CREDENTIALS_FILE_NAME: &str = "credentials.toml";

/// Helper function to mask API keys for safe logging or debug displays.
/// e.g. "sk-1234567890abcdef" -> "sk-1...cdef"
pub fn mask_api_key(key: &str) -> String {
    let chars: Vec<char> = key.chars().collect();
    if chars.len() <= 8 {
        "********".to_string()
    } else {
        let prefix: String = chars[..4].iter().collect();
        let suffix: String = chars[chars.len() - 4..].iter().collect();
        format!("{}...{}", prefix, suffix)
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

        let content =
            std::fs::read_to_string(&self.fallback_path).map_err(|e| KeyringError::Io {
                path: self.fallback_path.clone(),
                source: e,
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
        let keyring_result = self
            .keyring_entry()
            .and_then(|entry| entry.set_password(trimmed));

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
        let result = self
            .keyring_entry()
            .and_then(|entry| entry.delete_credential());
        finish_credential_deletion(result, self.fallback_path.exists(), || {
            self.remove_fallback()
        })
    }
}

fn finish_credential_deletion(
    keyring_result: Result<(), keyring::Error>,
    has_fallback: bool,
    remove_fallback: impl FnOnce() -> Result<(), KeyringError>,
) -> Result<(), KeyringError> {
    match keyring_result {
        Ok(()) | Err(keyring::Error::NoEntry) => remove_fallback(),
        // A system without Secret Service can still remove the file it uses as
        // its credential store. Errors from an initialized keyring must surface.
        Err(keyring::Error::NoDefaultStore) if has_fallback => remove_fallback(),
        Err(error) => Err(KeyringError::Keyring(error)),
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
static KEYRING_TMP_COUNTER: AtomicU64 = AtomicU64::new(0);

#[cfg(unix)]
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
    let seq = KEYRING_TMP_COUNTER.fetch_add(1, Ordering::SeqCst);

    path.with_extension(format!("tmp.{pid}.{thread_id}.{nanos}.{seq}"))
}

#[cfg(unix)]
pub fn write_secure_0600(path: &Path, content: &str) -> std::io::Result<()> {
    use std::io::Write;
    use std::os::unix::fs::OpenOptionsExt;
    use std::os::unix::fs::PermissionsExt;

    let tmp_path = unique_temp_path(path);
    let result = (|| {
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
        std::fs::rename(&tmp_path, path)?;
        Ok(())
    })();

    if result.is_err() {
        let _ = std::fs::remove_file(&tmp_path);
    }

    result
}

#[cfg(test)]
mod deletion_tests {
    use super::*;
    use std::cell::Cell;

    #[test]
    fn locked_keyring_deletion_never_reports_success() {
        let file_deleted = Cell::new(false);
        let result = finish_credential_deletion(
            Err(keyring::Error::NoStorageAccess(Box::new(
                std::io::Error::new(std::io::ErrorKind::PermissionDenied, "locked"),
            ))),
            true,
            || {
                file_deleted.set(true);
                Ok(())
            },
        );
        assert!(result.is_err());
        assert!(!file_deleted.get());
    }

    #[test]
    fn file_only_store_can_be_deleted_without_secret_service() {
        let deleted = Cell::new(false);
        finish_credential_deletion(Err(keyring::Error::NoDefaultStore), true, || {
            deleted.set(true);
            Ok(())
        })
        .unwrap();
        assert!(deleted.get());
        assert!(
            finish_credential_deletion(Err(keyring::Error::NoDefaultStore), false, || Ok(()))
                .is_err()
        );
    }

    #[test]
    fn fallback_removal_error_is_returned_after_system_deletion() {
        let result = finish_credential_deletion(Ok(()), true, || {
            Err(KeyringError::Io {
                path: PathBuf::from("isolated-credentials.toml"),
                source: std::io::Error::new(std::io::ErrorKind::PermissionDenied, "read-only"),
            })
        });
        assert!(matches!(result, Err(KeyringError::Io { .. })));
    }
}
