pub mod capture;
pub mod client;
pub mod cli;
pub mod config;
pub mod keyring;
pub mod ui;

// Ergonomic re-exports
pub use capture::{
    CaptureError, CapturedImage, CompositeCapturer, MockCapturer, NativeFallbackCapturer,
    PortalCapturer, ScreenCapturer,
};
pub use client::{ClientError, DeepSeekClient};
pub use config::AppConfig;
pub use keyring::CredentialStorage;
