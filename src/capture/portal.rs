//! XDG Desktop Portal Screen Capture Implementation
//!
//! Provides region and screen capture functionality using the FreeDesktop
//! XDG Desktop Portal (`org.freedesktop.portal.Screenshot`) via `ashpd` 0.13.

use std::path::{Path, PathBuf};
use ashpd::desktop::screenshot::Screenshot;
use ashpd::desktop::ResponseError;
use ashpd::PortalError;
use async_trait::async_trait;

use super::{CaptureError, CapturedImage, ScreenCapturer, PNG_MAGIC_BYTES};

/// Screen capturer using XDG Desktop Portal via `ashpd`.
#[derive(Debug, Default, Clone)]
pub struct PortalCapturer;

impl PortalCapturer {
    /// Creates a new `PortalCapturer` instance.
    pub fn new() -> Self {
        Self
    }

    /// Internal capture logic performing the D-Bus portal request, URI extraction,
    /// in-memory byte reading, and immediate disk cleanup.
    async fn capture_region_internal(&self) -> Result<Option<CapturedImage>, CaptureError> {
        tracing::info!("Initiating interactive screenshot capture via XDG Desktop Portal (ashpd)");

        // Step 1: Dispatch interactive screenshot request to org.freedesktop.portal.Screenshot
        let request_handle = match Screenshot::request()
            .interactive(true)
            .modal(true)
            .send()
            .await
        {
            Ok(handle) => handle,
            Err(err) => return Self::classify_ashpd_error(err),
        };

        // Step 2: Await user interaction completion and fetch response
        let screenshot = match request_handle.response() {
            Ok(res) => res,
            Err(err) => return Self::classify_ashpd_error(err),
        };

        let uri_str = screenshot.uri().as_str();
        tracing::debug!(uri = %uri_str, "Portal screenshot request completed successfully");

        // Step 3: Extract file path from URI
        let file_path = parse_file_uri(uri_str)?;
        tracing::debug!(path = %file_path.display(), "Extracted local filesystem path from portal URI");

        // Step 4: Read raw bytes into memory and immediately unlink the file
        let raw_bytes = read_and_unlink_file(&file_path).await?;

        // Step 5: Validate PNG signature and construct CapturedImage
        let captured = CapturedImage::from_raw_png(raw_bytes)?;
        tracing::info!(
            byte_count = captured.raw_bytes.len(),
            "Screenshot successfully captured and loaded into memory"
        );

        Ok(Some(captured))
    }

    /// Classifies an `ashpd::Error` into user cancellation, portal unavailability, or runtime failure.
    pub fn classify_ashpd_error<T>(err: ashpd::Error) -> Result<Option<T>, CaptureError> {
        match err {
            // User Cancellation: user pressed Escape, clicked Cancel, or closed the portal window
            ashpd::Error::Response(ResponseError::Cancelled) => {
                tracing::info!("Portal screenshot interaction was cancelled by the user");
                Ok(None)
            }
            ashpd::Error::Response(ResponseError::Other) => {
                tracing::info!("Portal screenshot interaction ended without capture (dismissed)");
                Ok(None)
            }
            ashpd::Error::Portal(PortalError::Cancelled(_)) => {
                tracing::info!("Portal backend reported user cancellation");
                Ok(None)
            }
            ashpd::Error::Portal(PortalError::WindowDestroyed(_)) => {
                tracing::info!("Portal dialog window was closed/destroyed");
                Ok(None)
            }

            // Portal Unavailability: interface missing, bus disconnected, or service absent
            ashpd::Error::PortalNotFound(iface) => {
                tracing::warn!(interface = %iface, "XDG Desktop Portal screenshot interface not found");
                Err(CaptureError::PortalUnavailable(format!(
                    "Portal interface not found: {iface}"
                )))
            }
            ashpd::Error::Zbus(zbus_err) => {
                tracing::warn!(error = %zbus_err, "Failed to communicate with portal via D-Bus");
                Err(CaptureError::PortalUnavailable(format!(
                    "Portal D-Bus connection failed: {zbus_err}"
                )))
            }
            ashpd::Error::NoResponse => {
                tracing::warn!("No response received from XDG Desktop Portal daemon");
                Err(CaptureError::PortalUnavailable(
                    "No response signal received from portal daemon".into(),
                ))
            }
            ashpd::Error::Portal(PortalError::NotFound(msg)) => {
                tracing::warn!(message = %msg, "Portal screenshot backend not found");
                Err(CaptureError::PortalUnavailable(format!(
                    "Portal backend not found: {msg}"
                )))
            }
            ashpd::Error::Portal(PortalError::NotAllowed(msg)) => {
                tracing::warn!(message = %msg, "Portal screenshot action not permitted");
                Err(CaptureError::PortalUnavailable(format!(
                    "Portal screenshot action not permitted by system policy: {msg}"
                )))
            }

            // General or internal portal errors
            ashpd::Error::Portal(other_portal_err) => {
                tracing::error!(error = %other_portal_err, "Portal request failed with backend error");
                Err(CaptureError::PortalUnavailable(format!(
                    "Portal backend error: {other_portal_err}"
                )))
            }
            other => {
                tracing::error!(error = %other, "Unexpected ashpd error during screenshot request");
                Err(CaptureError::PortalUnavailable(format!(
                    "Portal request failed: {other}"
                )))
            }
        }
    }
}

#[async_trait]
impl ScreenCapturer for PortalCapturer {
    fn name(&self) -> &'static str {
        "PortalCapturer"
    }

    async fn capture_region(&self) -> Result<Option<CapturedImage>, CaptureError> {
        self.capture_region_internal().await
    }
}

/// Parses a `file://` URI into an absolute `PathBuf`, handling percent-decoding.
pub fn parse_file_uri(uri: &str) -> Result<PathBuf, CaptureError> {
    if !uri.starts_with("file://") {
        return Err(CaptureError::UriParseError(format!(
            "Unsupported URI scheme (expected 'file://'): {uri}"
        )));
    }

    let without_scheme = &uri["file://".len()..];

    // Strip optional 'localhost' authority prefix: file://localhost/path
    let path_part = if let Some(stripped) = without_scheme.strip_prefix("localhost") {
        stripped
    } else {
        without_scheme
    };

    if path_part.is_empty() || !path_part.starts_with('/') {
        return Err(CaptureError::UriParseError(format!(
            "URI does not contain a valid absolute path: {uri}"
        )));
    }

    let decoded_path_str = percent_decode(path_part)?;
    let path = PathBuf::from(decoded_path_str);

    if !path.is_absolute() {
        return Err(CaptureError::UriParseError(format!(
            "Resolved path is not absolute: {uri}"
        )));
    }

    Ok(path)
}

/// Decodes percent-encoded UTF-8 octets (`%XX`) in a URI string.
pub fn percent_decode(s: &str) -> Result<String, CaptureError> {
    let mut bytes = Vec::with_capacity(s.len());
    let mut chars = s.as_bytes().iter();

    while let Some(&byte) = chars.next() {
        if byte == b'%' {
            let h1 = chars.next().ok_or_else(|| {
                CaptureError::UriParseError("Incomplete percent-encoding escape in URI".into())
            })?;
            let h2 = chars.next().ok_or_else(|| {
                CaptureError::UriParseError("Incomplete percent-encoding escape in URI".into())
            })?;

            let hex_bytes = [*h1, *h2];
            let hex_str = std::str::from_utf8(&hex_bytes).map_err(|_| {
                CaptureError::UriParseError("Non-ASCII hex character in percent-encoding".into())
            })?;

            let val = u8::from_str_radix(hex_str, 16).map_err(|_| {
                CaptureError::UriParseError(format!("Invalid hex digits in %{hex_str}"))
            })?;

            bytes.push(val);
        } else {
            bytes.push(byte);
        }
    }

    String::from_utf8(bytes).map_err(|err| {
        CaptureError::UriParseError(format!("Percent-decoded bytes are not valid UTF-8: {err}"))
    })
}

/// RAII guard to guarantee temporary file removal on drop even upon early return or panic.
pub struct TempFileGuard<'a>(pub &'a Path);

impl<'a> Drop for TempFileGuard<'a> {
    fn drop(&mut self) {
        if self.0.exists() {
            let _ = std::fs::remove_file(self.0);
        }
    }
}

/// Reads image bytes into memory and immediately unlinks the temporary file from disk.
pub async fn read_and_unlink_file(path: &Path) -> Result<Vec<u8>, CaptureError> {
    if !path.exists() {
        return Err(CaptureError::IoError(format!(
            "Portal screenshot file does not exist: {}",
            path.display()
        )));
    }

    // Deploy RAII guard
    let _guard = TempFileGuard(path);

    // Read bytes into memory
    let bytes = tokio::fs::read(path)
        .await
        .map_err(|e| CaptureError::IoError(e.to_string()))?;

    // Validate PNG signature before returning
    if bytes.len() < 8 || &bytes[..8] != PNG_MAGIC_BYTES {
        return Err(CaptureError::InvalidImage(
            "Captured portal file does not contain a valid PNG magic header".into(),
        ));
    }

    // Explicit immediate unlink
    if let Err(e) = tokio::fs::remove_file(path).await {
        tracing::debug!(path = %path.display(), error = %e, "Explicit removal completed or file already unlinked");
    }

    Ok(bytes)
}
