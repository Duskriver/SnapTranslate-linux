//! Native CLI Fallback Screen Capture Subsystem
//!
//! Provides region capture fallbacks using native command-line utilities:
//! - Wayland: `grim` + `slurp` (wlroots compositors: Sway, Hyprland, Wayfire, River)
//! - X11: `maim -s` (X11 desktop environments and window managers)
//!
//! Features automatic desktop session detection (with strict Wayland-over-X11 precedence),
//! POSIX 0600 secure temporary file lifecycle management, and transparent user cancellation handling.

use std::fs::OpenOptions;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

#[cfg(unix)]
use std::os::unix::fs::OpenOptionsExt;
#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;

use async_trait::async_trait;

use super::{CaptureError, CapturedImage, ScreenCapturer};

static TMP_FILE_COUNTER: AtomicU64 = AtomicU64::new(0);

// Explicitly reap a cancelled selector. kill_on_drop alone can leave the child
// in Tokio's orphan queue until another process or signal wakes that queue.
struct ReapingChild(Option<tokio::process::Child>);
impl Drop for ReapingChild {
    fn drop(&mut self) {
        if let Some(mut child) = self.0.take() {
            let _ = child.start_kill();
            if let Ok(runtime) = tokio::runtime::Handle::try_current() {
                runtime.spawn(async move {
                    let _ = child.wait().await;
                });
            }
        }
    }
}
#[async_trait]
trait ReapedOutput {
    async fn output_reaped(&mut self) -> std::io::Result<std::process::Output>;
}
#[async_trait]
impl ReapedOutput for tokio::process::Command {
    async fn output_reaped(&mut self) -> std::io::Result<std::process::Output> {
        use tokio::io::AsyncReadExt;
        let mut guard = ReapingChild(Some(self.spawn()?));
        let child = guard.0.as_mut().unwrap();
        let mut stdout = child.stdout.take().unwrap();
        let mut stderr = child.stderr.take().unwrap();
        let mut out = Vec::new();
        let mut err = Vec::new();
        let (status, _, _) = tokio::try_join!(
            child.wait(),
            stdout.read_to_end(&mut out),
            stderr.read_to_end(&mut err)
        )?;
        guard.0.take();
        Ok(std::process::Output {
            status,
            stdout: out,
            stderr: err,
        })
    }
}

/// Desktop session display protocol.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SessionType {
    Wayland,
    X11,
    Unknown,
}

/// Detects the active desktop session type using system environment variables.
pub fn detect_session_type() -> SessionType {
    detect_session_type_from_env(|k| std::env::var(k).ok())
}

/// Pure helper allowing injection of environment variables for unit testing.
pub fn detect_session_type_from_env<F>(mut get_env: F) -> SessionType
where
    F: FnMut(&str) -> Option<String>,
{
    let session_type = get_env("XDG_SESSION_TYPE")
        .unwrap_or_default()
        .trim()
        .to_ascii_lowercase();

    let wayland_display = get_env("WAYLAND_DISPLAY")
        .unwrap_or_default()
        .trim()
        .to_string();

    let display = get_env("DISPLAY").unwrap_or_default().trim().to_string();

    // 1. Wayland has strict precedence over X11 (avoids the Xwayland trap)
    if session_type == "wayland" || !wayland_display.is_empty() {
        SessionType::Wayland
    } else if session_type == "x11" || !display.is_empty() {
        SessionType::X11
    } else {
        SessionType::Unknown
    }
}

/// Checks if an executable binary exists in PATH (or as an executable file path) and has Unix execute permissions.
pub fn is_binary_available(name: &str) -> bool {
    let p = Path::new(name);
    if p.is_absolute() || p.components().count() > 1 {
        if p.is_file() {
            #[cfg(unix)]
            {
                if let Ok(meta) = p.metadata() {
                    return meta.permissions().mode() & 0o111 != 0;
                }
            }
            #[cfg(not(unix))]
            return true;
        }
        return false;
    }
    is_binary_available_in_path(name, std::env::var_os("PATH").as_deref())
}

/// Checks if a named binary exists in the provided PATH search variable.
pub fn is_binary_available_in_path(name: &str, path_var: Option<&std::ffi::OsStr>) -> bool {
    let Some(paths) = path_var else { return false };
    for dir in std::env::split_paths(paths) {
        let candidate = dir.join(name);
        if candidate.is_file() {
            #[cfg(unix)]
            {
                if let Ok(meta) = candidate.metadata() {
                    if meta.permissions().mode() & 0o111 != 0 {
                        return true;
                    }
                }
            }
            #[cfg(not(unix))]
            return true;
        }
    }
    false
}

/// Helper to verify Wayland fallback prerequisites: both `slurp` and `grim` must be available.
pub fn is_wayland_capture_available() -> bool {
    is_binary_available("slurp") && is_binary_available("grim")
}

/// Helper to verify X11 fallback prerequisites: `maim` must be available.
pub fn is_x11_capture_available() -> bool {
    is_binary_available("maim")
}

/// RAII Guard managing a secure, strictly permissioned POSIX 0600 temporary screenshot file.
pub struct TempScreenshotFile {
    path: PathBuf,
    cleaned: bool,
}

impl TempScreenshotFile {
    /// Creates a new secure temporary file in the default system temp directory.
    pub fn new() -> std::io::Result<Self> {
        Self::with_dir(std::env::temp_dir())
    }

    /// Creates a new secure temporary file inside a specified directory.
    pub fn with_dir<P: AsRef<Path>>(dir: P) -> std::io::Result<Self> {
        let dir = dir.as_ref();
        let pid = std::process::id();
        let thread_id = format!("{:?}", std::thread::current().id())
            .chars()
            .filter(|c| c.is_ascii_digit())
            .collect::<String>();

        let mut attempts = 0;
        loop {
            attempts += 1;
            if attempts > 100 {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::AlreadyExists,
                    "Exhausted 100 attempts trying to create unique temporary screenshot file",
                ));
            }

            let nanos = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0);
            let seq = TMP_FILE_COUNTER.fetch_add(1, Ordering::SeqCst);
            let file_name = format!("snaptranslate-capture-{pid}-{thread_id}-{nanos}-{seq}.png");
            let path = dir.join(file_name);

            #[cfg(unix)]
            let file_result = OpenOptions::new()
                .write(true)
                .create_new(true) // O_CREAT | O_EXCL
                .mode(0o600) // POSIX 0600
                .open(&path);

            #[cfg(not(unix))]
            let file_result = OpenOptions::new().write(true).create_new(true).open(&path);

            match file_result {
                Ok(file) => {
                    #[cfg(unix)]
                    {
                        // Defense-in-depth: explicitly enforce 0600 in case umask modified permissions
                        if let Ok(meta) = file.metadata() {
                            let mut perms = meta.permissions();
                            if perms.mode() & 0o777 != 0o600 {
                                perms.set_mode(0o600);
                                let _ = file.set_permissions(perms);
                            }
                        }
                    }
                    // Drop file descriptor so CLI utilities (grim, maim) can write to it cleanly
                    drop(file);
                    return Ok(Self {
                        path,
                        cleaned: false,
                    });
                }
                Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
                    continue;
                }
                Err(e) => return Err(e),
            }
        }
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Reads screenshot bytes into memory.
    pub fn read_bytes(&self) -> std::io::Result<Vec<u8>> {
        std::fs::read(&self.path)
    }

    /// Immediately cleans up the file on disk. Idempotent.
    pub fn cleanup(&mut self) -> std::io::Result<()> {
        if !self.cleaned {
            self.cleaned = true;
            if self.path.exists() {
                std::fs::remove_file(&self.path)?;
            }
        }
        Ok(())
    }
}

impl Drop for TempScreenshotFile {
    fn drop(&mut self) {
        let _ = self.cleanup();
    }
}

/// Wayland native CLI capturer using `slurp` for interactive region selection and `grim` for capture.
#[derive(Debug, Clone)]
pub struct WaylandCliCapturer {
    pub slurp_bin: String,
    pub grim_bin: String,
}

impl Default for WaylandCliCapturer {
    fn default() -> Self {
        Self {
            slurp_bin: "slurp".to_string(),
            grim_bin: "grim".to_string(),
        }
    }
}

impl WaylandCliCapturer {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn with_binaries(slurp_bin: impl Into<String>, grim_bin: impl Into<String>) -> Self {
        Self {
            slurp_bin: slurp_bin.into(),
            grim_bin: grim_bin.into(),
        }
    }

    pub fn is_available(&self) -> bool {
        is_binary_available(&self.slurp_bin) && is_binary_available(&self.grim_bin)
    }

    pub async fn capture_region_internal(&self) -> Result<Option<CapturedImage>, CaptureError> {
        // 1. Verify binaries
        if !is_binary_available(&self.slurp_bin) {
            return Err(CaptureError::CommandNotFound(format!(
                "Required tool '{}' not found in PATH",
                self.slurp_bin
            )));
        }
        if !is_binary_available(&self.grim_bin) {
            return Err(CaptureError::CommandNotFound(format!(
                "Required tool '{}' not found in PATH",
                self.grim_bin
            )));
        }

        // 2. Interactive region selection with slurp
        let slurp_res = tokio::process::Command::new(&self.slurp_bin)
            .kill_on_drop(true)
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .output_reaped()
            .await
            .map_err(|e| {
                CaptureError::IoError(format!("Failed to spawn '{}': {e}", self.slurp_bin))
            })?;

        if !slurp_res.status.success() {
            let stderr = String::from_utf8_lossy(&slurp_res.stderr);
            tracing::info!(
                tool = %self.slurp_bin,
                status = ?slurp_res.status.code(),
                stderr = %stderr.trim(),
                "Region selection was canceled by user"
            );
            return Ok(None);
        }

        let geometry = String::from_utf8_lossy(&slurp_res.stdout)
            .trim()
            .to_string();
        if geometry.is_empty() {
            tracing::info!("Slurp returned empty geometry; treating as canceled");
            return Ok(None);
        }

        // Validate geometry syntax (safe characters only)
        if !geometry.chars().all(|c| {
            c.is_ascii_digit()
                || c.is_ascii_alphanumeric()
                || matches!(c, ' ' | ',' | 'x' | '+' | '-')
        }) {
            return Err(CaptureError::InvalidGeometry(format!(
                "Invalid geometry string returned by slurp: '{geometry}'"
            )));
        }

        // 3. Prepare secure temporary file with POSIX 0600 mode
        let mut temp_file = TempScreenshotFile::new()?;

        // 4. Capture screenshot via grim
        let grim_res = tokio::process::Command::new(&self.grim_bin)
            .kill_on_drop(true)
            .arg("-g")
            .arg(&geometry)
            .arg(temp_file.path())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .output_reaped()
            .await
            .map_err(|e| {
                CaptureError::IoError(format!("Failed to spawn '{}': {e}", self.grim_bin))
            })?;

        if !grim_res.status.success() {
            let stderr = String::from_utf8_lossy(&grim_res.stderr);
            return Err(CaptureError::CommandFailed {
                command: self.grim_bin.clone(),
                exit_code: grim_res.status.code(),
                stderr: stderr.trim().to_string(),
            });
        }

        // 5. Ingest into memory and clean up immediately
        let raw_bytes = temp_file.read_bytes()?;
        let _ = temp_file.cleanup();

        // 6. Validate PNG and encode Base64 Data URI
        let captured = CapturedImage::from_raw_png(raw_bytes)?;
        Ok(Some(captured))
    }
}

#[async_trait]
impl ScreenCapturer for WaylandCliCapturer {
    fn name(&self) -> &'static str {
        "WaylandCliCapturer"
    }

    async fn capture_region(&self) -> Result<Option<CapturedImage>, CaptureError> {
        self.capture_region_internal().await
    }
}

/// X11 native CLI capturer using `maim -s` for interactive region selection and capture.
#[derive(Debug, Clone)]
pub struct X11CliCapturer {
    pub maim_bin: String,
}

impl Default for X11CliCapturer {
    fn default() -> Self {
        Self {
            maim_bin: "maim".to_string(),
        }
    }
}

impl X11CliCapturer {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn with_binary(maim_bin: impl Into<String>) -> Self {
        Self {
            maim_bin: maim_bin.into(),
        }
    }

    pub fn is_available(&self) -> bool {
        is_binary_available(&self.maim_bin)
    }

    pub async fn capture_region_internal(&self) -> Result<Option<CapturedImage>, CaptureError> {
        // 1. Verify maim exists
        if !is_binary_available(&self.maim_bin) {
            return Err(CaptureError::CommandNotFound(format!(
                "Required tool '{}' not found in PATH",
                self.maim_bin
            )));
        }

        // 2. Create secure temporary file with mode 0600
        let mut temp_file = TempScreenshotFile::new()?;

        // 3. Execute maim -s <path>
        let maim_res = tokio::process::Command::new(&self.maim_bin)
            .kill_on_drop(true)
            .arg("-s")
            .arg(temp_file.path())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .output_reaped()
            .await
            .map_err(|e| {
                CaptureError::IoError(format!("Failed to spawn '{}': {e}", self.maim_bin))
            })?;

        if !maim_res.status.success() {
            let stderr = String::from_utf8_lossy(&maim_res.stderr);
            tracing::info!(
                tool = %self.maim_bin,
                status = ?maim_res.status.code(),
                stderr = %stderr.trim(),
                "Maim selection was canceled by user"
            );
            return Ok(None);
        }

        // 4. Ingest raw bytes and clean up disk file
        let raw_bytes = temp_file.read_bytes()?;
        let _ = temp_file.cleanup();

        if raw_bytes.is_empty() {
            tracing::info!("Maim generated an empty file; treating as canceled");
            return Ok(None);
        }

        // 5. Validate PNG and build CapturedImage
        let captured = CapturedImage::from_raw_png(raw_bytes)?;
        Ok(Some(captured))
    }
}

#[async_trait]
impl ScreenCapturer for X11CliCapturer {
    fn name(&self) -> &'static str {
        "X11CliCapturer"
    }

    async fn capture_region(&self) -> Result<Option<CapturedImage>, CaptureError> {
        self.capture_region_internal().await
    }
}

/// Fallback capturer coordinating environment detection and appropriate native CLI tools.
pub struct NativeFallbackCapturer {
    session_type: SessionType,
    wayland_capturer: WaylandCliCapturer,
    x11_capturer: X11CliCapturer,
}

impl Default for NativeFallbackCapturer {
    fn default() -> Self {
        Self::new()
    }
}

impl NativeFallbackCapturer {
    pub fn new() -> Self {
        Self {
            session_type: detect_session_type(),
            wayland_capturer: WaylandCliCapturer::new(),
            x11_capturer: X11CliCapturer::new(),
        }
    }

    pub fn with_session(session_type: SessionType) -> Self {
        Self {
            session_type,
            wayland_capturer: WaylandCliCapturer::new(),
            x11_capturer: X11CliCapturer::new(),
        }
    }

    pub fn with_capturers(
        session_type: SessionType,
        wayland_capturer: WaylandCliCapturer,
        x11_capturer: X11CliCapturer,
    ) -> Self {
        Self {
            session_type,
            wayland_capturer,
            x11_capturer,
        }
    }

    pub async fn capture_region_internal(&self) -> Result<Option<CapturedImage>, CaptureError> {
        match self.session_type {
            SessionType::Wayland => {
                if self.wayland_capturer.is_available() {
                    tracing::info!("Executing Wayland CLI fallback (grim + slurp)");
                    self.wayland_capturer.capture_region().await
                } else if self.x11_capturer.is_available() {
                    tracing::warn!("grim/slurp not found on Wayland; attempting maim via Xwayland");
                    self.x11_capturer.capture_region().await
                } else {
                    Err(CaptureError::CommandNotFound(
                        "Neither Wayland tools (grim/slurp) nor X11 tool (maim) found in PATH"
                            .into(),
                    ))
                }
            }
            SessionType::X11 => {
                if self.x11_capturer.is_available() {
                    tracing::info!("Executing X11 CLI fallback (maim -s)");
                    self.x11_capturer.capture_region().await
                } else {
                    Err(CaptureError::CommandNotFound(
                        "X11 capture tool 'maim' not found in PATH".into(),
                    ))
                }
            }
            SessionType::Unknown => {
                if self.wayland_capturer.is_available() {
                    tracing::info!("Session type unknown; falling back to grim + slurp");
                    self.wayland_capturer.capture_region().await
                } else if self.x11_capturer.is_available() {
                    tracing::info!("Session type unknown; falling back to maim");
                    self.x11_capturer.capture_region().await
                } else {
                    Err(CaptureError::NoBackendAvailable)
                }
            }
        }
    }
}

#[async_trait]
impl ScreenCapturer for NativeFallbackCapturer {
    fn name(&self) -> &'static str {
        "NativeFallbackCapturer"
    }

    async fn capture_region(&self) -> Result<Option<CapturedImage>, CaptureError> {
        self.capture_region_internal().await
    }
}
