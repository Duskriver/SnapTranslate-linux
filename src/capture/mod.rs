//! Screen Capture Subsystem
//!
//! Provides a unified, cross-desktop screen capture interface supporting:
//! - Primary: XDG Desktop Portal via `ashpd` (GNOME, KDE Plasma, wlroots portals)
//! - Fallback: Native CLI tools (`grim` + `slurp` on Wayland, `maim` on X11)
//! - Composite Capturer with strict short-circuiting on user cancellation
//! - In-memory Base64 Data URI formatting for DeepSeek Multimodal Vision API

pub mod fallback;
pub mod portal;

use async_trait::async_trait;
use base64::Engine;
use std::path::Path;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

pub use fallback::{
    detect_session_type, detect_session_type_from_env, is_binary_available,
    is_wayland_capture_available, is_x11_capture_available, NativeFallbackCapturer, SessionType,
    TempScreenshotFile, WaylandCliCapturer, X11CliCapturer,
};
pub use portal::PortalCapturer;

/// Magic header bytes identifying a valid Portable Network Graphics (PNG) stream.
pub const PNG_MAGIC_BYTES: &[u8; 8] = b"\x89PNG\r\n\x1a\n";

/// Structured error type for the screen capture subsystem.
#[derive(Debug, thiserror::Error, Clone, PartialEq, Eq)]
pub enum CaptureError {
    #[error("Screen capture permission was denied")]
    PermissionDenied,
    #[error("Portal unavailable: {0}")]
    PortalUnavailable(String),

    #[error("Capture backend unavailable: {0}")]
    BackendUnavailable(String),

    #[error("Portal screenshot error: {0}")]
    PortalError(String),

    #[error("CLI tool not found: {0}")]
    CommandNotFound(String),

    #[error("Capture command failed ({command}, exit code: {exit_code:?}): {stderr}")]
    CommandFailed {
        command: String,
        exit_code: Option<i32>,
        stderr: String,
    },

    #[error("Invalid image: {0}")]
    InvalidImage(String),

    #[error("Failed to parse URI: {0}")]
    UriParseError(String),

    #[error("Capture I/O error: {0}")]
    IoError(String),

    #[error("No capture backend available. Ensure XDG Desktop Portal is running or install native tools (grim+slurp on Wayland, maim on X11).")]
    NoBackendAvailable,

    #[error("Invalid geometry: {0}")]
    InvalidGeometry(String),
}

impl From<std::io::Error> for CaptureError {
    fn from(err: std::io::Error) -> Self {
        Self::IoError(err.to_string())
    }
}

/// An in-memory representation of a captured screen region.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CapturedImage {
    /// Raw image bytes in PNG format.
    pub raw_bytes: Vec<u8>,
    /// RFC 2397 Data URI formatted as `data:image/png;base64,<payload>`.
    pub base64_data_uri: String,
}

impl CapturedImage {
    /// Validates raw PNG bytes and formats an in-memory `CapturedImage`.
    pub fn from_raw_png(raw_bytes: Vec<u8>) -> Result<Self, CaptureError> {
        validate_png_header(&raw_bytes)?;
        let base64_data_uri = encode_png_data_uri(&raw_bytes);
        Ok(Self {
            raw_bytes,
            base64_data_uri,
        })
    }

    /// Direct constructor useful for testing fixtures.
    pub fn from_parts(raw_bytes: Vec<u8>, base64_data_uri: String) -> Self {
        Self {
            raw_bytes,
            base64_data_uri,
        }
    }

    #[inline]
    pub fn len(&self) -> usize {
        self.raw_bytes.len()
    }

    #[inline]
    pub fn is_empty(&self) -> bool {
        self.raw_bytes.is_empty()
    }

    #[inline]
    pub fn as_slice(&self) -> &[u8] {
        &self.raw_bytes
    }
}

/// Validates that the provided byte slice starts with standard PNG magic bytes.
pub fn validate_png_header(bytes: &[u8]) -> Result<(), CaptureError> {
    if bytes.len() < 8 {
        return Err(CaptureError::InvalidImage(format!(
            "Image payload too short: expected at least 8 bytes for PNG header, got {} bytes",
            bytes.len()
        )));
    }
    if !bytes.starts_with(PNG_MAGIC_BYTES) {
        return Err(CaptureError::InvalidImage(format!(
            "Invalid image format: missing PNG magic header bytes (got {:?})",
            &bytes[..bytes.len().min(8)]
        )));
    }
    Ok(())
}

/// Alias for `validate_png_header`.
pub fn validate_png(bytes: &[u8]) -> Result<(), CaptureError> {
    validate_png_header(bytes)
}

/// Encodes raw PNG bytes into an RFC 2397 Data URI (`data:image/png;base64,<encoded>`).
pub fn encode_png_data_uri(bytes: &[u8]) -> String {
    let encoded = base64::engine::general_purpose::STANDARD.encode(bytes);
    format!("data:image/png;base64,{}", encoded)
}

/// Formats raw PNG bytes into a Base64 Data URI (alias for `encode_png_data_uri`).
pub fn format_base64_data_uri(bytes: &[u8]) -> String {
    encode_png_data_uri(bytes)
}

/// Reads a PNG from disk, removes the temporary file immediately, and validates image data.
pub fn read_and_cleanup_temp_image<P: AsRef<Path>>(path: P) -> Result<CapturedImage, CaptureError> {
    let p = path.as_ref();
    let bytes = std::fs::read(p).map_err(|e| {
        CaptureError::IoError(format!(
            "Failed to read screenshot from '{}': {}",
            p.display(),
            e
        ))
    })?;

    if let Err(e) = std::fs::remove_file(p) {
        tracing::warn!(
            "Failed to delete temporary screenshot file '{}': {}",
            p.display(),
            e
        );
    }

    CapturedImage::from_raw_png(bytes)
}

/// Core asynchronous abstraction for screen region capturing backends.
#[async_trait]
pub trait ScreenCapturer: Send + Sync {
    /// Returns the diagnostic name of the backend.
    fn name(&self) -> &'static str {
        "UnknownCapturer"
    }

    /// Triggers region capture.
    ///
    /// Returns:
    /// - `Ok(Some(img))`: Region captured successfully.
    /// - `Ok(None)`: User cancelled region selection (e.g. Escape or Cancel button).
    /// - `Err(e)`: Backend encountered a failure.
    async fn capture_region(&self) -> Result<Option<CapturedImage>, CaptureError>;
}

/// Composite capturer orchestrating Portal -> NativeFallbackCapturer cascade.
///
/// CRITICAL: If the primary capturer returns `Ok(None)` (user canceled screenshot selection),
/// `CompositeCapturer` MUST return `Ok(None)` immediately, NEVER cascading to fallbacks!
/// Only cascades if primary returns `Err(CaptureError::PortalUnavailable(_))` or
/// `Err(CaptureError::BackendUnavailable(_))`.
pub struct CompositeCapturer {
    primary: Box<dyn ScreenCapturer>,
    fallback: Box<dyn ScreenCapturer>,
}

impl Default for CompositeCapturer {
    fn default() -> Self {
        Self::new()
    }
}

impl CompositeCapturer {
    /// Constructs default production `CompositeCapturer` with PortalCapturer and NativeFallbackCapturer.
    pub fn new() -> Self {
        Self {
            primary: Box::new(PortalCapturer::new()),
            fallback: Box::new(NativeFallbackCapturer::new()),
        }
    }

    /// Constructs `CompositeCapturer` with explicit primary and fallback capturers.
    pub fn with_capturers(
        primary: Box<dyn ScreenCapturer>,
        fallback: Box<dyn ScreenCapturer>,
    ) -> Self {
        Self { primary, fallback }
    }

    /// Constructs `CompositeCapturer` with an arbitrary vector of backends (useful in automated tests).
    pub fn with_backends(mut backends: Vec<Box<dyn ScreenCapturer>>) -> Self {
        if backends.is_empty() {
            Self {
                primary: Box::new(MockCapturer::failing(CaptureError::NoBackendAvailable)),
                fallback: Box::new(MockCapturer::failing(CaptureError::NoBackendAvailable)),
            }
        } else if backends.len() == 1 {
            Self {
                primary: backends.remove(0),
                fallback: Box::new(MockCapturer::failing(CaptureError::NoBackendAvailable)),
            }
        } else if backends.len() == 2 {
            let primary = backends.remove(0);
            let fallback = backends.remove(0);
            Self { primary, fallback }
        } else {
            let primary = backends.remove(0);
            let fallback = Box::new(CompositeCapturer::with_backends(backends));
            Self { primary, fallback }
        }
    }
}

#[async_trait]
impl ScreenCapturer for CompositeCapturer {
    fn name(&self) -> &'static str {
        "CompositeCapturer"
    }

    async fn capture_region(&self) -> Result<Option<CapturedImage>, CaptureError> {
        match self.primary.capture_region().await {
            Ok(Some(img)) => {
                tracing::info!(
                    "Screen capture succeeded with primary capturer ({})",
                    self.primary.name()
                );
                Ok(Some(img))
            }
            Ok(None) => {
                tracing::info!(
                    "Primary capturer ({}) cancelled by user; short-circuiting capture cascade.",
                    self.primary.name()
                );
                Ok(None)
            }
            Err(CaptureError::PortalUnavailable(reason)) => {
                tracing::warn!(
                    reason = %reason,
                    "Primary capturer portal unavailable; cascading to fallback capturer ({})",
                    self.fallback.name()
                );
                self.fallback.capture_region().await
            }
            Err(CaptureError::BackendUnavailable(reason)) => {
                tracing::warn!(
                    reason = %reason,
                    "Primary capturer backend unavailable; cascading to fallback capturer ({})",
                    self.fallback.name()
                );
                self.fallback.capture_region().await
            }
            Err(other) => {
                tracing::error!(
                    error = %other,
                    "Primary capturer ({}) failed with non-cascading error",
                    self.primary.name()
                );
                Err(other)
            }
        }
    }
}

/// Mock capturer for deterministic, headless testing without interactive GUI prompts.
#[derive(Clone)]
pub struct MockCapturer {
    pub name: &'static str,
    outcome: Arc<tokio::sync::Mutex<MockOutcome>>,
    call_count: Arc<AtomicUsize>,
}

pub enum MockOutcome {
    Success(CapturedImage),
    Cancelled,
    Failed(CaptureError),
    Sequential(Vec<Result<Option<CapturedImage>, CaptureError>>),
}

impl MockCapturer {
    pub fn succeeding(image: CapturedImage) -> Self {
        Self::with_outcome("MockSuccess", MockOutcome::Success(image))
    }

    pub fn succeeding_with_png(png_bytes: Vec<u8>) -> Self {
        let image = CapturedImage::from_raw_png(png_bytes).expect("Valid test PNG bytes");
        Self::succeeding(image)
    }

    pub fn cancelling() -> Self {
        Self::with_outcome("MockCancelling", MockOutcome::Cancelled)
    }

    pub fn failing(err: CaptureError) -> Self {
        Self::with_outcome("MockFailing", MockOutcome::Failed(err))
    }

    pub fn sequential(outcomes: Vec<Result<Option<CapturedImage>, CaptureError>>) -> Self {
        Self::with_outcome("MockSequential", MockOutcome::Sequential(outcomes))
    }

    pub fn with_outcome(name: &'static str, outcome: MockOutcome) -> Self {
        Self {
            name,
            outcome: Arc::new(tokio::sync::Mutex::new(outcome)),
            call_count: Arc::new(AtomicUsize::new(0)),
        }
    }

    pub fn call_count(&self) -> usize {
        self.call_count.load(Ordering::SeqCst)
    }
}

#[async_trait]
impl ScreenCapturer for MockCapturer {
    fn name(&self) -> &'static str {
        self.name
    }

    async fn capture_region(&self) -> Result<Option<CapturedImage>, CaptureError> {
        self.call_count.fetch_add(1, Ordering::SeqCst);
        let mut guard = self.outcome.lock().await;
        match &mut *guard {
            MockOutcome::Success(img) => Ok(Some(img.clone())),
            MockOutcome::Cancelled => Ok(None),
            MockOutcome::Failed(err) => Err(err.clone()),
            MockOutcome::Sequential(list) => {
                if list.is_empty() {
                    Err(CaptureError::BackendUnavailable(
                        "Mock sequence exhausted".into(),
                    ))
                } else {
                    list.remove(0)
                }
            }
        }
    }
}
