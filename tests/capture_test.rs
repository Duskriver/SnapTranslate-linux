//! Automated Test Suite for Screen Capture Subsystem (Milestone 2)
//!
//! Tests include:
//! - PNG magic byte validation & corrupted/truncated detection
//! - RFC 2397 Base64 Data URI formatting and roundtrip decoding
//! - User cancellation semantics (mapping to `Ok(None)`)
//! - CompositeCapturer cascading on `PortalUnavailable`
//! - CompositeCapturer strict short-circuiting on user cancellation (NEVER cascade)
//! - Desktop session environment detection with Wayland precedence
//! - Secure POSIX 0600 temporary file lifecycle & RAII cleanup
//! - Portal URI parsing & percent-decoding
//! - Native CLI mock scripts for Wayland (`slurp`+`grim`) and X11 (`maim`)

use std::fs::File;
use std::io::Write;
use std::path::Path;

#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;

use base64::Engine;
use snaptranslate_linux::capture::fallback::{
    detect_session_type_from_env, is_binary_available, SessionType, TempScreenshotFile,
    WaylandCliCapturer, X11CliCapturer,
};
use snaptranslate_linux::capture::portal::{parse_file_uri, percent_decode, PortalCapturer};
use snaptranslate_linux::capture::{
    encode_png_data_uri, format_base64_data_uri, read_and_cleanup_temp_image,
    validate_png_header, CaptureError, CapturedImage, CompositeCapturer, MockCapturer,
    ScreenCapturer, PNG_MAGIC_BYTES,
};
use tempfile::tempdir;

/// Minimal 68-byte 1x1 RGBA non-interlaced valid PNG fixture.
pub const MINIMAL_PNG: &[u8] = &[
    0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A, // Magic header: \x89PNG\r\n\x1a\n
    0x00, 0x00, 0x00, 0x0D, // IHDR chunk length: 13
    0x49, 0x48, 0x44, 0x52, // Chunk type: IHDR
    0x00, 0x00, 0x00, 0x01, // Width: 1
    0x00, 0x00, 0x00, 0x01, // Height: 1
    0x08, // Bit depth: 8
    0x06, // Color type: 6 (RGBA)
    0x00, // Compression: 0
    0x00, // Filter: 0
    0x00, // Interlace: 0
    0x1F, 0x15, 0xC4, 0x89, // CRC
    0x00, 0x00, 0x00, 0x0B, // IDAT chunk length: 11
    0x49, 0x44, 0x41, 0x54, // Chunk type: IDAT
    0x78, 0x9C, 0x63, 0x60, 0x00, 0x02, 0x00, 0x00, 0x05, 0x00, 0x01, // Compressed zlib stream
    0x7A, 0x5E, 0xAB, 0x3F, // CRC
    0x00, 0x00, 0x00, 0x00, // IEND chunk length: 0
    0x49, 0x45, 0x4E, 0x44, // Chunk type: IEND
    0xAE, 0x42, 0x60, 0x82, // CRC
];

// =========================================================================
// 1. PNG Header Validation Tests
// =========================================================================

#[test]
fn test_png_magic_header_validation_success() {
    assert!(validate_png_header(PNG_MAGIC_BYTES).is_ok());
    assert!(validate_png_header(MINIMAL_PNG).is_ok());
}

#[test]
fn test_png_magic_header_validation_truncated() {
    // Empty payload
    let err_empty = validate_png_header(&[]).unwrap_err();
    assert!(matches!(err_empty, CaptureError::InvalidImage(_)));

    // Less than 8 bytes
    let err_short = validate_png_header(b"\x89PNG").unwrap_err();
    assert!(matches!(err_short, CaptureError::InvalidImage(_)));

    // 7 bytes
    let err_7 = validate_png_header(b"\x89PNG\r\n\x1a").unwrap_err();
    assert!(matches!(err_7, CaptureError::InvalidImage(_)));
}

#[test]
fn test_png_magic_header_validation_corrupted() {
    // JPEG signature: \xFF\xD8\xFF\xE0...
    let jpeg_header = b"\xFF\xD8\xFF\xE0\x00\x10JFIF";
    let err_jpeg = validate_png_header(jpeg_header).unwrap_err();
    assert!(matches!(err_jpeg, CaptureError::InvalidImage(_)));

    // Arbitrary text data
    let text_header = b"NOT_A_PNG_FILE!!";
    let err_text = validate_png_header(text_header).unwrap_err();
    assert!(matches!(err_text, CaptureError::InvalidImage(_)));
}

// =========================================================================
// 2. Base64 Data URI Formatting & Roundtrip Decoding Tests
// =========================================================================

#[test]
fn test_captured_image_from_raw_png_and_base64_roundtrip() {
    let captured = CapturedImage::from_raw_png(MINIMAL_PNG.to_vec()).expect("Valid PNG bytes");
    assert_eq!(captured.len(), MINIMAL_PNG.len());
    assert!(!captured.is_empty());
    assert_eq!(captured.as_slice(), MINIMAL_PNG);

    // Verify URI format
    assert!(
        captured.base64_data_uri.starts_with("data:image/png;base64,"),
        "URI must start with data:image/png;base64,"
    );

    // Decode and verify roundtrip byte equality
    let base64_payload = captured
        .base64_data_uri
        .strip_prefix("data:image/png;base64,")
        .expect("Prefix exists");
    let decoded_bytes = base64::engine::general_purpose::STANDARD
        .decode(base64_payload)
        .expect("Valid base64 payload");

    assert_eq!(decoded_bytes, MINIMAL_PNG);
}

#[test]
fn test_encode_png_data_uri_helpers() {
    let uri1 = encode_png_data_uri(MINIMAL_PNG);
    let uri2 = format_base64_data_uri(MINIMAL_PNG);
    assert_eq!(uri1, uri2);
    assert!(uri1.starts_with("data:image/png;base64,"));

    let payload = uri1.strip_prefix("data:image/png;base64,").unwrap();
    let decoded = base64::engine::general_purpose::STANDARD
        .decode(payload)
        .unwrap();
    assert_eq!(decoded, MINIMAL_PNG);
}

// =========================================================================
// 3. Cancellation Semantics Tests
// =========================================================================

#[tokio::test]
async fn test_cancellation_handling_returns_ok_none() {
    let mock = MockCapturer::cancelling();
    let res = mock.capture_region().await.expect("Cancellation should not return Err");
    assert!(res.is_none(), "User cancellation must return Ok(None)");
    assert_eq!(mock.call_count(), 1);
}

// =========================================================================
// 4. CompositeCapturer Cascading and Short-Circuiting Tests
// =========================================================================

#[tokio::test]
async fn test_composite_capturer_primary_succeeds_no_cascade() {
    let primary = MockCapturer::succeeding_with_png(MINIMAL_PNG.to_vec());
    let fallback = MockCapturer::cancelling();

    let composite = CompositeCapturer::with_capturers(
        Box::new(primary.clone()),
        Box::new(fallback.clone()),
    );

    let res = composite.capture_region().await.expect("Capture succeeds");
    assert!(res.is_some());
    assert_eq!(primary.call_count(), 1);
    assert_eq!(
        fallback.call_count(),
        0,
        "Fallback must NOT be called when primary succeeds"
    );
}

#[tokio::test]
async fn test_composite_capturer_user_cancels_short_circuits_never_cascades() {
    // CRITICAL REQUIREMENT:
    // If primary returns Ok(None), CompositeCapturer MUST return Ok(None) immediately,
    // NEVER cascading to fallbacks!
    let primary = MockCapturer::cancelling();
    let fallback = MockCapturer::succeeding_with_png(MINIMAL_PNG.to_vec());

    let composite = CompositeCapturer::with_capturers(
        Box::new(primary.clone()),
        Box::new(fallback.clone()),
    );

    let res = composite.capture_region().await.expect("Cancellation is not an error");
    assert!(res.is_none(), "Result must be None when user cancels");
    assert_eq!(primary.call_count(), 1);
    assert_eq!(
        fallback.call_count(),
        0,
        "CRITICAL: Fallback MUST NEVER be called when user cancels primary selection"
    );
}

#[tokio::test]
async fn test_composite_capturer_portal_unavailable_cascades_to_fallback() {
    let primary = MockCapturer::failing(CaptureError::PortalUnavailable(
        "org.freedesktop.portal.Screenshot not found on D-Bus".into(),
    ));
    let fallback = MockCapturer::succeeding_with_png(MINIMAL_PNG.to_vec());

    let composite = CompositeCapturer::with_capturers(
        Box::new(primary.clone()),
        Box::new(fallback.clone()),
    );

    let res = composite.capture_region().await.expect("Fallback capture succeeds");
    assert!(res.is_some());
    assert_eq!(primary.call_count(), 1, "Primary portal attempted first");
    assert_eq!(fallback.call_count(), 1, "Fallback invoked after PortalUnavailable");
}

#[tokio::test]
async fn test_composite_capturer_backend_unavailable_cascades_to_fallback() {
    let primary = MockCapturer::failing(CaptureError::BackendUnavailable("Service down".into()));
    let fallback = MockCapturer::succeeding_with_png(MINIMAL_PNG.to_vec());

    let composite = CompositeCapturer::with_capturers(
        Box::new(primary.clone()),
        Box::new(fallback.clone()),
    );

    let res = composite.capture_region().await.expect("Fallback succeeds");
    assert!(res.is_some());
    assert_eq!(primary.call_count(), 1);
    assert_eq!(fallback.call_count(), 1);
}

#[tokio::test]
async fn test_composite_capturer_non_cascading_error_fails_immediately() {
    // An error that is NOT PortalUnavailable / BackendUnavailable does not cascade
    let primary = MockCapturer::failing(CaptureError::InvalidImage("Corrupt PNG".into()));
    let fallback = MockCapturer::succeeding_with_png(MINIMAL_PNG.to_vec());

    let composite = CompositeCapturer::with_capturers(
        Box::new(primary.clone()),
        Box::new(fallback.clone()),
    );

    let err = composite.capture_region().await.unwrap_err();
    assert!(matches!(err, CaptureError::InvalidImage(_)));
    assert_eq!(primary.call_count(), 1);
    assert_eq!(fallback.call_count(), 0, "Fallback must not run on non-cascading error");
}

#[tokio::test]
async fn test_composite_capturer_with_backends_helper() {
    let portal = MockCapturer::failing(CaptureError::PortalUnavailable("No portal".into()));
    let wayland = MockCapturer::failing(CaptureError::BackendUnavailable("No grim".into()));
    let x11 = MockCapturer::succeeding_with_png(MINIMAL_PNG.to_vec());

    let composite = CompositeCapturer::with_backends(vec![
        Box::new(portal.clone()),
        Box::new(wayland.clone()),
        Box::new(x11.clone()),
    ]);

    let res = composite.capture_region().await.expect("Cascade to X11 succeeds");
    assert!(res.is_some());
    assert_eq!(portal.call_count(), 1);
    assert_eq!(wayland.call_count(), 1);
    assert_eq!(x11.call_count(), 1);
}

// =========================================================================
// 5. Desktop Session Detection Tests (Wayland Precedence)
// =========================================================================

#[test]
fn test_environment_session_detection_precedence() {
    // 1. Wayland session type takes precedence even when DISPLAY is present
    let s1 = detect_session_type_from_env(|k| match k {
        "XDG_SESSION_TYPE" => Some("wayland".into()),
        "DISPLAY" => Some(":0".into()),
        _ => None,
    });
    assert_eq!(s1, SessionType::Wayland, "XDG_SESSION_TYPE=wayland has precedence");

    // 2. WAYLAND_DISPLAY set takes precedence over DISPLAY (Xwayland environment)
    let s2 = detect_session_type_from_env(|k| match k {
        "WAYLAND_DISPLAY" => Some("wayland-0".into()),
        "DISPLAY" => Some(":0".into()),
        _ => None,
    });
    assert_eq!(s2, SessionType::Wayland, "WAYLAND_DISPLAY has precedence over DISPLAY");

    // 3. X11 session type
    let s3 = detect_session_type_from_env(|k| match k {
        "XDG_SESSION_TYPE" => Some("x11".into()),
        "DISPLAY" => Some(":1".into()),
        _ => None,
    });
    assert_eq!(s3, SessionType::X11);

    // 4. DISPLAY only (legacy X11)
    let s4 = detect_session_type_from_env(|k| match k {
        "DISPLAY" => Some(":0".into()),
        _ => None,
    });
    assert_eq!(s4, SessionType::X11);

    // 5. Unknown / Headless environment
    let s5 = detect_session_type_from_env(|_| None);
    assert_eq!(s5, SessionType::Unknown);
}

// =========================================================================
// 6. Secure Temporary File Management & Cleanup Tests
// =========================================================================

#[test]
fn test_temp_screenshot_file_mode_0600_and_cleanup() {
    let dir = tempdir().expect("create tempdir");
    let mut temp_file = TempScreenshotFile::with_dir(dir.path()).expect("create temp file");
    let path = temp_file.path().to_path_buf();

    assert!(path.exists(), "Temporary file must exist upon creation");

    #[cfg(unix)]
    {
        let meta = std::fs::metadata(&path).expect("read metadata");
        let mode = meta.permissions().mode() & 0o777;
        assert_eq!(
            mode, 0o600,
            "Temporary screenshot file MUST have POSIX 0600 permissions"
        );
    }

    // Explicit cleanup
    temp_file.cleanup().expect("cleanup succeeds");
    assert!(!path.exists(), "File must be removed after cleanup");
}

#[test]
fn test_temp_screenshot_file_raii_drop_cleanup() {
    let dir = tempdir().expect("create tempdir");
    let path;
    {
        let temp_file = TempScreenshotFile::with_dir(dir.path()).expect("create temp file");
        path = temp_file.path().to_path_buf();
        assert!(path.exists());
    } // temp_file dropped here

    assert!(
        !path.exists(),
        "Temporary screenshot file must be unlinked on drop via RAII"
    );
}

#[test]
fn test_read_and_cleanup_temp_image_lifecycle() {
    let dir = tempdir().expect("create tempdir");
    let file_path = dir.path().join("shot.png");
    std::fs::write(&file_path, MINIMAL_PNG).expect("write temp png");

    assert!(file_path.exists());
    let captured = read_and_cleanup_temp_image(&file_path).expect("read temp image");
    assert_eq!(captured.raw_bytes, MINIMAL_PNG);
    assert!(!file_path.exists(), "File must be deleted immediately after reading");
}

// =========================================================================
// 7. Portal URI Parsing & Percent-Decoding Tests
// =========================================================================

#[test]
fn test_portal_uri_parsing_and_percent_decoding() {
    // Simple file URI
    let p1 = parse_file_uri("file:///tmp/snap.png").expect("parse standard URI");
    assert_eq!(p1, Path::new("/tmp/snap.png"));

    // localhost authority
    let p2 = parse_file_uri("file://localhost/tmp/snap.png").expect("parse localhost URI");
    assert_eq!(p2, Path::new("/tmp/snap.png"));

    // Percent-encoded spaces (%20)
    let p3 = parse_file_uri("file:///tmp/My%20Screenshot.png").expect("parse encoded spaces");
    assert_eq!(p3, Path::new("/tmp/My Screenshot.png"));

    // Percent-encoded UTF-8 (%E6%A1%8C%E9%9D%A2 -> 桌面)
    let p4 = parse_file_uri("file:///home/user/%E6%A1%8C%E9%9D%A2/test.png").expect("parse UTF-8");
    assert_eq!(p4, Path::new("/home/user/桌面/test.png"));

    // Rejection of non-file scheme
    assert!(matches!(
        parse_file_uri("http://example.com/shot.png"),
        Err(CaptureError::UriParseError(_))
    ));

    // Rejection of relative path
    assert!(matches!(
        parse_file_uri("file://shot.png"),
        Err(CaptureError::UriParseError(_))
    ));
}

#[test]
fn test_percent_decode_direct() {
    assert_eq!(percent_decode("hello%20world").unwrap(), "hello world");
    assert_eq!(percent_decode("%E6%B5%8B%E8%AF%95").unwrap(), "测试");
    assert!(percent_decode("%ZZ").is_err());
    assert!(percent_decode("%2").is_err());
}

#[test]
fn test_portal_error_classification() {
    // User cancellations
    let res_cancel = PortalCapturer::classify_ashpd_error::<()>(
        ashpd::desktop::ResponseError::Cancelled.into(),
    );
    assert_eq!(res_cancel, Ok(None));

    let res_other = PortalCapturer::classify_ashpd_error::<()>(
        ashpd::desktop::ResponseError::Other.into(),
    );
    assert!(matches!(res_other, Err(CaptureError::PortalError(_))));

    // Portal daemon unavailable
    let res_no_response = PortalCapturer::classify_ashpd_error::<()>(ashpd::Error::NoResponse);
    assert!(matches!(res_no_response, Err(CaptureError::PortalUnavailable(_))));
}

// =========================================================================
// 8. Native CLI Mock Execution Tests (Wayland & X11)
// =========================================================================

#[cfg(unix)]
fn make_executable_script(path: &Path, content: &str) {
    let mut file = File::create(path).expect("create script file");
    file.write_all(content.as_bytes()).expect("write script content");
    file.sync_all().expect("sync script");
    let mut perms = file.metadata().expect("meta").permissions();
    perms.set_mode(0o755);
    file.set_permissions(perms).expect("set 0755 permissions");
    drop(file);
}

#[tokio::test]
#[cfg(unix)]
async fn test_wayland_cli_mock_success() {
    let dir = tempdir().expect("tempdir");

    // Pre-create binary PNG fixture
    let fixture_path = dir.path().join("fixture.png");
    std::fs::write(&fixture_path, MINIMAL_PNG).expect("write fixture");

    // Mock slurp: outputs geometry and exits 0
    let slurp_script = dir.path().join("mock_slurp.sh");
    make_executable_script(&slurp_script, "#!/bin/sh\necho '10,20 200x150'\nexit 0\n");

    // Mock grim: copies binary fixture to target path ($3) and exits 0
    let grim_script = dir.path().join("mock_grim.sh");
    let grim_content = format!("#!/bin/sh\ncp \"{}\" \"$3\"\nexit 0\n", fixture_path.display());
    make_executable_script(&grim_script, &grim_content);

    // Yield briefly to ensure Linux inode write-close is fully observed by kernel
    tokio::time::sleep(std::time::Duration::from_millis(10)).await;

    assert!(is_binary_available(slurp_script.to_str().unwrap()));
    assert!(is_binary_available(grim_script.to_str().unwrap()));

    let capturer = WaylandCliCapturer::with_binaries(
        slurp_script.to_str().unwrap(),
        grim_script.to_str().unwrap(),
    );

    let res = capturer.capture_region().await.expect("Capture succeeds");
    let img = res.expect("Image exists");
    assert_eq!(img.raw_bytes, MINIMAL_PNG);
    assert!(img.base64_data_uri.starts_with("data:image/png;base64,"));
}

#[tokio::test]
#[cfg(unix)]
async fn test_wayland_cli_mock_cancellation() {
    let dir = tempdir().expect("tempdir");

    // Mock slurp: exits 1 (user pressed Escape)
    let slurp_script = dir.path().join("mock_slurp_cancel.sh");
    make_executable_script(&slurp_script, "#!/bin/sh\nexit 1\n");

    let grim_script = dir.path().join("mock_grim.sh");
    make_executable_script(&grim_script, "#!/bin/sh\nexit 0\n");

    tokio::time::sleep(std::time::Duration::from_millis(10)).await;

    let capturer = WaylandCliCapturer::with_binaries(
        slurp_script.to_str().unwrap(),
        grim_script.to_str().unwrap(),
    );

    let res = capturer.capture_region().await.expect("Cancellation succeeds");
    assert!(res.is_none(), "Must return Ok(None) when slurp exits non-zero");
}

#[tokio::test]
#[cfg(unix)]
async fn test_x11_cli_mock_success() {
    let dir = tempdir().expect("tempdir");

    let fixture_path = dir.path().join("fixture_x11.png");
    std::fs::write(&fixture_path, MINIMAL_PNG).expect("write fixture");

    // Mock maim: copies binary fixture to target path ($2) and exits 0
    let maim_script = dir.path().join("mock_maim.sh");
    let maim_content = format!("#!/bin/sh\ncp \"{}\" \"$2\"\nexit 0\n", fixture_path.display());
    make_executable_script(&maim_script, &maim_content);

    tokio::time::sleep(std::time::Duration::from_millis(10)).await;

    let capturer = X11CliCapturer::with_binary(maim_script.to_str().unwrap());

    let res = capturer.capture_region().await.expect("Capture succeeds");
    let img = res.expect("Image exists");
    assert_eq!(img.raw_bytes, MINIMAL_PNG);
}

#[tokio::test]
#[cfg(unix)]
async fn test_x11_cli_mock_cancellation() {
    let dir = tempdir().expect("tempdir");

    // Mock maim: exits 1 (user cancelled)
    let maim_script = dir.path().join("mock_maim_cancel.sh");
    make_executable_script(&maim_script, "#!/bin/sh\nexit 1\n");

    tokio::time::sleep(std::time::Duration::from_millis(10)).await;

    let capturer = X11CliCapturer::with_binary(maim_script.to_str().unwrap());

    let res = capturer.capture_region().await.expect("Cancellation succeeds");
    assert!(res.is_none(), "Must return Ok(None) when maim exits non-zero");
}

// =========================================================================
// 9. Empirical Adversarial Challenge Tests
// =========================================================================

#[tokio::test]
async fn test_composite_capturer_all_non_retryable_errors_short_circuit() {
    let non_retryable_errors = vec![
        CaptureError::InvalidImage("Corrupt PNG header".into()),
        CaptureError::CommandNotFound("slurp not found".into()),
        CaptureError::CommandFailed {
            command: "grim".into(),
            exit_code: Some(127),
            stderr: "Fatal capture error".into(),
        },
        CaptureError::UriParseError("Invalid URI".into()),
        CaptureError::IoError("Disk full".into()),
        CaptureError::NoBackendAvailable,
        CaptureError::InvalidGeometry("bad-geom".into()),
        CaptureError::PortalError("Portal error".into()),
    ];

    for expected_err in non_retryable_errors {
        let primary = MockCapturer::failing(expected_err.clone());
        let fallback = MockCapturer::succeeding_with_png(MINIMAL_PNG.to_vec());

        let composite = CompositeCapturer::with_capturers(
            Box::new(primary.clone()),
            Box::new(fallback.clone()),
        );

        let result = composite.capture_region().await;
        assert!(result.is_err(), "Expected error for {:?}", expected_err);
        let actual_err = result.unwrap_err();
        assert_eq!(
            actual_err, expected_err,
            "Error variant must be preserved without distortion"
        );
        assert_eq!(primary.call_count(), 1);
        assert_eq!(
            fallback.call_count(),
            0,
            "Fallback MUST NEVER be called on non-retryable error: {:?}",
            expected_err
        );
    }
}

#[tokio::test]
async fn test_composite_capturer_cancellation_repeated_never_cascades() {
    for _ in 0..5 {
        let primary = MockCapturer::cancelling();
        let fallback = MockCapturer::succeeding_with_png(MINIMAL_PNG.to_vec());

        let composite = CompositeCapturer::with_capturers(
            Box::new(primary.clone()),
            Box::new(fallback.clone()),
        );

        let res = composite.capture_region().await.expect("Must succeed with Ok(None)");
        assert!(res.is_none(), "Result must be None");
        assert_eq!(primary.call_count(), 1);
        assert_eq!(fallback.call_count(), 0, "Fallback call count must remain 0");
    }
}

#[tokio::test]
async fn test_composite_capturer_portal_unavailable_triggers_fallback() {
    let primary = MockCapturer::failing(CaptureError::PortalUnavailable("ashpd error".into()));
    let fallback = MockCapturer::succeeding_with_png(MINIMAL_PNG.to_vec());

    let composite = CompositeCapturer::with_capturers(
        Box::new(primary.clone()),
        Box::new(fallback.clone()),
    );

    let res = composite.capture_region().await.expect("Must succeed via fallback");
    assert!(res.is_some());
    assert_eq!(primary.call_count(), 1, "Primary portal was invoked");
    assert_eq!(fallback.call_count(), 1, "Fallback capturer was invoked");
}

#[tokio::test]
async fn test_composite_capturer_portal_unavailable_fallback_cancels() {
    let primary = MockCapturer::failing(CaptureError::PortalUnavailable("ashpd error".into()));
    let fallback = MockCapturer::cancelling();

    let composite = CompositeCapturer::with_capturers(
        Box::new(primary.clone()),
        Box::new(fallback.clone()),
    );

    let res = composite.capture_region().await.expect("Cancellation is Ok(None)");
    assert!(res.is_none(), "When fallback cancels, final result is Ok(None)");
    assert_eq!(primary.call_count(), 1);
    assert_eq!(fallback.call_count(), 1);
}

#[tokio::test]
async fn test_composite_capturer_cascade_stop_on_second_backend_cancel() {
    let b1 = MockCapturer::failing(CaptureError::PortalUnavailable("ashpd down".into()));
    let b2 = MockCapturer::cancelling();
    let b3 = MockCapturer::succeeding_with_png(MINIMAL_PNG.to_vec());

    let composite = CompositeCapturer::with_backends(vec![
        Box::new(b1.clone()),
        Box::new(b2.clone()),
        Box::new(b3.clone()),
    ]);

    let res = composite.capture_region().await.expect("Must succeed with Ok(None)");
    assert!(res.is_none());
    assert_eq!(b1.call_count(), 1);
    assert_eq!(b2.call_count(), 1);
    assert_eq!(
        b3.call_count(),
        0,
        "Third backend must NEVER be called once second backend cancels"
    );
}

#[test]
fn test_png_validation_adversarial_matrix() {
    // 1. Valid cases
    assert!(validate_png_header(PNG_MAGIC_BYTES).is_ok());
    assert!(validate_png_header(MINIMAL_PNG).is_ok());

    let mut extended_png = MINIMAL_PNG.to_vec();
    extended_png.extend_from_slice(&[0x00; 100]);
    assert!(validate_png_header(&extended_png).is_ok());

    // 2. Truncated cases (0 to 7 bytes)
    for len in 0..8 {
        let truncated = &PNG_MAGIC_BYTES[..len];
        let err = validate_png_header(truncated).unwrap_err();
        assert!(matches!(err, CaptureError::InvalidImage(_)));
        assert!(CapturedImage::from_raw_png(truncated.to_vec()).is_err());
    }

    // 3. Bit-flip mutations across each byte of the 8-byte magic header
    for byte_idx in 0..8 {
        let mut mutated = *PNG_MAGIC_BYTES;
        mutated[byte_idx] ^= 0x01; // flip single bit
        let err = validate_png_header(&mutated).unwrap_err();
        assert!(
            matches!(err, CaptureError::InvalidImage(_)),
            "Bit flip at byte {} must fail validation",
            byte_idx
        );
        assert!(CapturedImage::from_raw_png(mutated.to_vec()).is_err());
    }

    // 4. Non-PNG image formats and arbitrary data formats
    let invalid_fixtures: Vec<(&'static str, &[u8])> = vec![
        ("JPEG", &[0xFF, 0xD8, 0xFF, 0xE0, 0x00, 0x10, 0x4A, 0x46, 0x49, 0x46]),
        ("GIF87a", b"GIF87a\x01\x00\x01\x00"),
        ("GIF89a", b"GIF89a\x01\x00\x01\x00"),
        ("BMP", b"BM\x36\x00\x00\x00\x00\x00\x00\x00"),
        ("WebP", b"RIFF\x1a\x00\x00\x00WEBP"),
        ("TIFF-LE", b"II*\x00\x08\x00\x00\x00"),
        ("TIFF-BE", b"MM\x00*\x00\x00\x00\x08"),
        ("PDF", b"%PDF-1.7\n%obj\n"),
        ("HTML", b"<!DOCTYPE html><html><body>Error</body></html>"),
        ("JSON", b"{\"error\": \"not an image\"}"),
        ("ASCII-Text", b"This is plain text and not a screenshot."),
        ("Zeroes-8", &[0x00; 8]),
        ("Zeroes-64", &[0x00; 64]),
        ("All-FF", &[0xFF; 16]),
    ];

    for (label, fixture) in invalid_fixtures {
        let err = validate_png_header(fixture).unwrap_err();
        assert!(
            matches!(err, CaptureError::InvalidImage(_)),
            "Format '{}' must fail with InvalidImage",
            label
        );
        let res = CapturedImage::from_raw_png(fixture.to_vec());
        assert!(
            matches!(res, Err(CaptureError::InvalidImage(_))),
            "CapturedImage::from_raw_png for '{}' must fail with InvalidImage",
            label
        );
    }
}

#[test]
fn test_rfc2397_data_uri_compliance_and_stress() {
    let base_header = PNG_MAGIC_BYTES;

    // Test different lengths to verify base64 padding invariants (=, ==, none)
    for padding_len in [0, 1, 2, 3, 7, 15, 64, 1024] {
        let mut data = base_header.to_vec();
        data.extend(vec![0x42; padding_len]);

        let captured = CapturedImage::from_raw_png(data.clone()).expect("Valid PNG header");

        // 1. Verify scheme and media type compliance (RFC 2397)
        assert!(
            captured.base64_data_uri.starts_with("data:image/png;base64,"),
            "URI must strictly match 'data:image/png;base64,' prefix"
        );

        let encoded_payload = captured
            .base64_data_uri
            .strip_prefix("data:image/png;base64,")
            .unwrap();

        // 2. Verify payload contains only standard base64 characters (RFC 4648)
        assert!(
            encoded_payload
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '+' || c == '/' || c == '='),
            "Payload must contain only standard Base64 characters"
        );

        // 3. Ensure no whitespace or newline injection
        assert!(
            !encoded_payload.contains(|c: char| c.is_ascii_whitespace()),
            "Base64 data URI must contain no whitespace or newline characters"
        );

        // 4. Verify roundtrip decoding byte equality
        let decoded = base64::engine::general_purpose::STANDARD
            .decode(encoded_payload)
            .expect("Base64 decode succeeds");
        assert_eq!(decoded, data, "Roundtrip decoded bytes must match input exactly");
    }
}

#[test]
fn test_temp_screenshot_file_high_concurrency_race() {
    use std::sync::Arc;
    use std::thread;

    let dir = tempdir().expect("tempdir");
    let dir_path = Arc::new(dir.path().to_path_buf());

    let mut handles = vec![];
    for _ in 0..8 {
        let dir_clone = Arc::clone(&dir_path);
        handles.push(thread::spawn(move || {
            let mut created_paths = vec![];
            for _ in 0..10 {
                let temp_file = TempScreenshotFile::with_dir(&*dir_clone)
                    .expect("Concurrent TempScreenshotFile creation must succeed");
                let path = temp_file.path().to_path_buf();

                assert!(path.exists());

                #[cfg(unix)]
                {
                    let meta = std::fs::metadata(&path).expect("read metadata");
                    let mode = meta.permissions().mode() & 0o777;
                    assert_eq!(mode, 0o600, "Permission must strictly be 0600");
                }

                created_paths.push(temp_file);
            }

            // Drop all files
            drop(created_paths);
        }));
    }

    for handle in handles {
        handle.join().expect("thread finished cleanly");
    }

    // Verify tempdir is clean
    let entries: Vec<_> = std::fs::read_dir(dir.path())
        .expect("read_dir")
        .collect();
    assert!(
        entries.is_empty(),
        "All temporary files must be cleaned up after dropping"
    );
}

#[test]
fn test_portal_uri_parsing_adversarial_matrix() {
    // Valid complex URIs
    let valid_uris = [
        ("file:///var/tmp/shot.png", "/var/tmp/shot.png"),
        ("file://localhost/var/tmp/shot.png", "/var/tmp/shot.png"),
        ("file:///path/with%20spaces/shot.png", "/path/with spaces/shot.png"),
        ("file:///path/with%2Bplus/shot.png", "/path/with+plus/shot.png"),
        ("file:///path/%E4%B8%AD%E6%96%87/shot.png", "/path/中文/shot.png"),
        ("file:///path/%F0%9F%94%A5/fire.png", "/path/🔥/fire.png"),
    ];

    for (uri, expected_path) in valid_uris {
        let parsed = parse_file_uri(uri).unwrap_or_else(|e| panic!("Failed to parse {}: {:?}", uri, e));
        assert_eq!(parsed, Path::new(expected_path));
    }

    // Invalid URIs
    let invalid_uris = [
        "",
        "http://example.com/shot.png",
        "file://",
        "file://localhost",
        "file://relative/path.png",
        "file:///tmp/truncated%",
        "file:///tmp/truncated%2",
        "file:///tmp/nonhex%ZZ.png",
        "file:///tmp/invalid_utf8_%FF%FE.png",
    ];

    for uri in invalid_uris {
        let res = parse_file_uri(uri);
        assert!(
            res.is_err(),
            "URI '{}' must fail parse_file_uri",
            uri
        );
    }
}

