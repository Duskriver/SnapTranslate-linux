pub mod capture;
pub mod cli;
pub mod config;
pub mod keyring;

// Ergonomic re-exports
pub use capture::{
    CaptureError, CapturedImage, CompositeCapturer, MockCapturer, NativeFallbackCapturer,
    PortalCapturer, ScreenCapturer,
};
pub use config::AppConfig;
pub use keyring::CredentialStorage;
