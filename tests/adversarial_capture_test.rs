//! Adversarial & Empirical Stress Test Suite for Milestone 2 (Screen Capture Subsystem)
//!
//! Objectives:
//! 1. POSIX Permission Verification: Verify strict mode 0600 (-rw-------) on temp files under standard and adverse umasks.
//! 2. RAII Cleanup Verification: Verify immediate unlinking on drop, panic unwinding resilience, and idempotent cleanup.
//! 3. High-Concurrency Stress Testing: Massive multi-threaded creation with Barrier synchronization to prove collision safety.
//! 4. Desktop Session Environment Detection Precedence & Fallback Matrix.
//! 5. Unlinking guarantees on corrupt or truncated images.

use std::collections::HashSet;
use std::fs::File;
use std::io::Write;
use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;
use std::sync::{Arc, Barrier, Mutex};
use std::thread;
use tempfile::tempdir;

use snaptranslate_linux::capture::fallback::{
    detect_session_type, detect_session_type_from_env, SessionType, TempScreenshotFile,
    WaylandCliCapturer,
};
use snaptranslate_linux::capture::portal::read_and_unlink_file;
use snaptranslate_linux::capture::{
    read_and_cleanup_temp_image, CaptureError, ScreenCapturer,
};

#[cfg(unix)]
extern "C" {
    fn umask(mask: u32) -> u32;
}

/// Minimal valid 1x1 RGBA PNG bytes fixture.
pub const VALID_PNG: &[u8] = &[
    0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A, 0x00, 0x00, 0x00, 0x0D, 0x49, 0x48, 0x44,
    0x52, 0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x01, 0x08, 0x06, 0x00, 0x00, 0x00, 0x1F,
    0x15, 0xC4, 0x89, 0x00, 0x00, 0x00, 0x0B, 0x49, 0x44, 0x41, 0x54, 0x78, 0x9C, 0x63, 0x60,
    0x00, 0x02, 0x00, 0x00, 0x05, 0x00, 0x01, 0x7A, 0x5E, 0xAB, 0x3F, 0x00, 0x00, 0x00, 0x00,
    0x49, 0x45, 0x4E, 0x44, 0xAE, 0x42, 0x60, 0x82,
];

// ============================================================================
// 1. POSIX Permission Verification (Mode 0600 / -rw-------)
// ============================================================================

#[test]
fn test_temp_screenshot_file_posix_mode_0600_strict() {
    let dir = tempdir().expect("Failed to create tempdir");
    let temp_file = TempScreenshotFile::with_dir(dir.path()).expect("Create temp screenshot file");
    let path = temp_file.path();

    assert!(path.exists(), "Temp screenshot file must exist");

    let meta = std::fs::metadata(path).expect("Failed to fetch metadata");
    let mode = meta.permissions().mode() & 0o777;

    assert_eq!(
        mode, 0o600,
        "Temp screenshot file MUST have exact POSIX mode 0600 (-rw-------), found 0{:o}",
        mode
    );

    // Group and Other permissions must be strictly zero
    assert_eq!(
        mode & 0o077,
        0,
        "Group and Other must have 0 permissions on screenshot file"
    );

    // Owner read and write must be set
    assert_eq!(
        mode & 0o600,
        0o600,
        "Owner read and write must be present on screenshot file"
    );

    // Owner execute must NOT be set
    assert_eq!(
        mode & 0o100,
        0,
        "Owner execute bit must not be set on screenshot file"
    );
}

#[test]
fn test_temp_screenshot_file_mode_0600_under_adverse_umask() {
    let dir = tempdir().expect("Failed to create tempdir");

    // Test under permissive umask 0000 (which would leak group/other rw if not defended)
    unsafe {
        let old_umask = umask(0o000);
        let res = TempScreenshotFile::with_dir(dir.path());
        umask(old_umask); // Always restore immediately

        let temp_file = res.expect("Create temp file under umask 0000");
        let meta = std::fs::metadata(temp_file.path()).expect("Metadata");
        let mode = meta.permissions().mode() & 0o777;
        assert_eq!(
            mode, 0o600,
            "Permissive umask 0000 must NOT loosen file permissions from 0600; found 0{:o}",
            mode
        );
    }

    // Test under restrictive umask 0077
    unsafe {
        let old_umask = umask(0o077);
        let res = TempScreenshotFile::with_dir(dir.path());
        umask(old_umask);

        let temp_file = res.expect("Create temp file under umask 0077");
        let meta = std::fs::metadata(temp_file.path()).expect("Metadata");
        let mode = meta.permissions().mode() & 0o777;
        assert_eq!(
            mode, 0o600,
            "Restrictive umask 0077 must result in exact 0600 permissions; found 0{:o}",
            mode
        );
    }
}

// ============================================================================
// 2. RAII Cleanup and Immediate Unlinking
// ============================================================================

#[test]
fn test_temp_screenshot_file_raii_immediate_unlink_on_drop() {
    let dir = tempdir().expect("Failed to create tempdir");
    let path: PathBuf;

    {
        let temp_file = TempScreenshotFile::with_dir(dir.path()).expect("Create temp file");
        path = temp_file.path().to_path_buf();
        assert!(path.exists(), "File exists before drop");

        // Write screenshot payload
        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .open(&path)
            .expect("Open file");
        file.write_all(VALID_PNG).expect("Write PNG bytes");
        file.sync_all().expect("Sync file");

        let read_back = temp_file.read_bytes().expect("Read bytes");
        assert_eq!(read_back, VALID_PNG);
    } // temp_file goes out of scope here -> Drop called

    assert!(
        !path.exists(),
        "Temp screenshot file MUST be immediately unlinked from disk upon RAII drop"
    );
}

#[test]
fn test_temp_screenshot_file_raii_unwind_panic_safety() {
    let dir = tempdir().expect("Failed to create tempdir");
    let path_cell = Arc::new(Mutex::new(None));
    let path_clone = Arc::clone(&path_cell);
    let target_dir = dir.path().to_path_buf();

    let handle = thread::spawn(move || {
        let temp_file = TempScreenshotFile::with_dir(&target_dir).expect("Create temp file");
        let p = temp_file.path().to_path_buf();
        *path_clone.lock().unwrap() = Some(p);

        // Deliberately trigger panic during active capture lifecycle
        panic!("Simulated worker panic during capture processing");
    });

    let join_res = handle.join();
    assert!(join_res.is_err(), "Worker thread must have panicked");

    let created_path = path_cell.lock().unwrap().take().expect("Path was recorded");
    assert!(
        !created_path.exists(),
        "Temp screenshot file MUST be unlinked during stack unwinding on panic"
    );
}

#[test]
fn test_temp_screenshot_file_idempotent_cleanup() {
    let dir = tempdir().expect("Failed to create tempdir");
    let mut temp_file = TempScreenshotFile::with_dir(dir.path()).expect("Create temp file");
    let path = temp_file.path().to_path_buf();

    assert!(path.exists());

    // First cleanup: removes file
    assert!(temp_file.cleanup().is_ok());
    assert!(!path.exists());

    // Second cleanup: idempotent, must not error
    assert!(temp_file.cleanup().is_ok());

    // External file removal before cleanup / drop
    let mut temp_file2 = TempScreenshotFile::with_dir(dir.path()).expect("Create temp file 2");
    let path2 = temp_file2.path().to_path_buf();
    std::fs::remove_file(&path2).expect("External removal");
    assert!(!path2.exists());

    // cleanup on already deleted file should not error
    assert!(temp_file2.cleanup().is_ok());
    drop(temp_file2); // Drop on already unlinked file must not panic
}

// ============================================================================
// 3. Concurrency and Multi-Threaded Stress Testing
// ============================================================================

#[test]
fn test_high_concurrency_temp_file_creation_no_collisions() {
    let dir = tempdir().expect("Failed to create tempdir");
    let num_threads = 64;
    let barrier = Arc::new(Barrier::new(num_threads));
    let created_paths = Arc::new(Mutex::new(Vec::new()));

    let mut handles = Vec::new();

    for _ in 0..num_threads {
        let b = Arc::clone(&barrier);
        let paths = Arc::clone(&created_paths);
        let d = dir.path().to_path_buf();

        handles.push(thread::spawn(move || {
            b.wait(); // Synchronize all 64 threads to strike at the exact same instant
            let temp_file = TempScreenshotFile::with_dir(&d).expect("Must not collide");
            let p = temp_file.path().to_path_buf();
            paths.lock().unwrap().push(p);

            // Keep the temp_file alive briefly so names overlap in time
            thread::sleep(std::time::Duration::from_millis(10));
            drop(temp_file);
        }));
    }

    for h in handles {
        h.join().expect("Thread should not panic");
    }

    let all_paths = created_paths.lock().unwrap().clone();
    assert_eq!(all_paths.len(), num_threads);

    // Verify all 64 paths are unique (zero collisions)
    let unique_paths: HashSet<_> = all_paths.into_iter().collect();
    assert_eq!(
        unique_paths.len(),
        num_threads,
        "Every concurrently created temp file must have a unique path without collision"
    );

    // Verify all files were unlinked after threads finished
    let remaining_entries = std::fs::read_dir(dir.path())
        .expect("Read dir")
        .collect::<Result<Vec<_>, _>>()
        .expect("Collect entries");
    assert_eq!(
        remaining_entries.len(),
        0,
        "All temporary files must be cleaned up; found {} leaked files",
        remaining_entries.len()
    );
}

#[test]
fn test_high_throughput_burst_temp_file_concurrency() {
    let dir = tempdir().expect("Failed to create tempdir");
    let num_threads = 16;
    let iterations_per_thread = 50;
    let barrier = Arc::new(Barrier::new(num_threads));
    let all_paths = Arc::new(Mutex::new(HashSet::new()));

    let mut handles = Vec::new();

    for t in 0..num_threads {
        let b = Arc::clone(&barrier);
        let paths_set = Arc::clone(&all_paths);
        let d = dir.path().to_path_buf();

        handles.push(thread::spawn(move || {
            b.wait();
            for i in 0..iterations_per_thread {
                let mut temp_file = TempScreenshotFile::with_dir(&d)
                    .unwrap_or_else(|e| panic!("Thread {t} iter {i} creation failed: {e}"));
                let p = temp_file.path().to_path_buf();

                // Check uniqueness
                let mut guard = paths_set.lock().unwrap();
                assert!(
                    guard.insert(p.clone()),
                    "Collision detected! File path {:?} was already generated",
                    p
                );
                drop(guard);

                // Write and read
                let payload = format!("test-payload-thread-{t}-iter-{i}");
                std::fs::write(&p, payload.as_bytes()).expect("Write");
                let read = temp_file.read_bytes().expect("Read");
                assert_eq!(read, payload.as_bytes());

                // Cleanup immediately
                temp_file.cleanup().expect("Cleanup");
                assert!(!p.exists());
            }
        }));
    }

    for h in handles {
        h.join().expect("Thread finished cleanly");
    }

    let total_created = all_paths.lock().unwrap().len();
    assert_eq!(
        total_created,
        num_threads * iterations_per_thread,
        "Total 800 burst files must all have unique paths"
    );

    let remaining = std::fs::read_dir(dir.path())
        .unwrap()
        .collect::<Result<Vec<_>, _>>()
        .unwrap();
    assert_eq!(remaining.len(), 0, "Zero leaked temporary files");
}

// ============================================================================
// 4. Desktop Session Environment Detection Precedence & Fallback Matrix
// ============================================================================

#[test]
fn test_detect_session_type_from_env_comprehensive_matrix() {
    // 4.1 Strict Wayland Precedence over X11
    assert_eq!(
        detect_session_type_from_env(|k| match k {
            "XDG_SESSION_TYPE" => Some("wayland".into()),
            "WAYLAND_DISPLAY" => Some("wayland-0".into()),
            "DISPLAY" => Some(":0".into()),
            _ => None,
        }),
        SessionType::Wayland,
        "Wayland session type + socket wins over DISPLAY"
    );

    assert_eq!(
        detect_session_type_from_env(|k| match k {
            "XDG_SESSION_TYPE" => Some("wayland".into()),
            "WAYLAND_DISPLAY" => None,
            "DISPLAY" => Some(":0".into()),
            _ => None,
        }),
        SessionType::Wayland,
        "XDG_SESSION_TYPE=wayland alone wins over DISPLAY"
    );

    assert_eq!(
        detect_session_type_from_env(|k| match k {
            "XDG_SESSION_TYPE" => None,
            "WAYLAND_DISPLAY" => Some("wayland-1".into()),
            "DISPLAY" => Some(":0".into()),
            _ => None,
        }),
        SessionType::Wayland,
        "WAYLAND_DISPLAY alone wins over DISPLAY"
    );

    assert_eq!(
        detect_session_type_from_env(|k| match k {
            "XDG_SESSION_TYPE" => Some("x11".into()),
            "WAYLAND_DISPLAY" => Some("wayland-0".into()),
            "DISPLAY" => Some(":0".into()),
            _ => None,
        }),
        SessionType::Wayland,
        "WAYLAND_DISPLAY socket presence overrides stale XDG_SESSION_TYPE=x11"
    );

    // 4.2 Case-Insensitivity & Whitespace Resilient Normalization
    assert_eq!(
        detect_session_type_from_env(|k| match k {
            "XDG_SESSION_TYPE" => Some("  WAYLAND  ".into()),
            _ => None,
        }),
        SessionType::Wayland,
        "Uppercase and whitespace trimmed for WAYLAND"
    );

    assert_eq!(
        detect_session_type_from_env(|k| match k {
            "XDG_SESSION_TYPE" => Some("  WaYlAnD\t\n".into()),
            _ => None,
        }),
        SessionType::Wayland,
        "Mixed case and whitespace trimmed for WaYlAnD"
    );

    assert_eq!(
        detect_session_type_from_env(|k| match k {
            "XDG_SESSION_TYPE" => Some("  X11  ".into()),
            _ => None,
        }),
        SessionType::X11,
        "Uppercase and whitespace trimmed for X11"
    );

    // 4.3 Legitimate X11 Detections
    assert_eq!(
        detect_session_type_from_env(|k| match k {
            "XDG_SESSION_TYPE" => Some("x11".into()),
            "WAYLAND_DISPLAY" => None,
            "DISPLAY" => None,
            _ => None,
        }),
        SessionType::X11,
        "XDG_SESSION_TYPE=x11 with no displays set"
    );

    assert_eq!(
        detect_session_type_from_env(|k| match k {
            "XDG_SESSION_TYPE" => None,
            "WAYLAND_DISPLAY" => None,
            "DISPLAY" => Some(":0".into()),
            _ => None,
        }),
        SessionType::X11,
        "DISPLAY set with no wayland indicators"
    );

    assert_eq!(
        detect_session_type_from_env(|k| match k {
            "XDG_SESSION_TYPE" => None,
            "WAYLAND_DISPLAY" => None,
            "DISPLAY" => Some("localhost:10.0".into()),
            _ => None,
        }),
        SessionType::X11,
        "Remote / SSH forwarding DISPLAY"
    );

    // 4.4 Fallback to Unknown
    assert_eq!(
        detect_session_type_from_env(|_| None),
        SessionType::Unknown,
        "All env vars unset must yield Unknown"
    );

    assert_eq!(
        detect_session_type_from_env(|k| match k {
            "XDG_SESSION_TYPE" => Some("".into()),
            "WAYLAND_DISPLAY" => Some("".into()),
            "DISPLAY" => Some("".into()),
            _ => None,
        }),
        SessionType::Unknown,
        "All env vars empty strings must yield Unknown"
    );

    assert_eq!(
        detect_session_type_from_env(|k| match k {
            "XDG_SESSION_TYPE" => Some("   ".into()),
            "WAYLAND_DISPLAY" => Some("   ".into()),
            "DISPLAY" => Some("   ".into()),
            _ => None,
        }),
        SessionType::Unknown,
        "All env vars whitespace must yield Unknown"
    );

    assert_eq!(
        detect_session_type_from_env(|k| match k {
            "XDG_SESSION_TYPE" => Some("tty".into()),
            "WAYLAND_DISPLAY" => None,
            "DISPLAY" => None,
            _ => None,
        }),
        SessionType::Unknown,
        "Non-graphical session type 'tty' must yield Unknown"
    );

    assert_eq!(
        detect_session_type_from_env(|k| match k {
            "XDG_SESSION_TYPE" => Some("unsupported_custom_protocol".into()),
            "WAYLAND_DISPLAY" => None,
            "DISPLAY" => None,
            _ => None,
        }),
        SessionType::Unknown,
        "Arbitrary unrecognized session type must yield Unknown"
    );

    // 4.5 Live System Session Detection Test
    let live_session = detect_session_type();
    assert!(
        matches!(
            live_session,
            SessionType::Wayland | SessionType::X11 | SessionType::Unknown
        ),
        "Live detect_session_type() returned valid variant: {:?}",
        live_session
    );
}

// ============================================================================
// 5. Unlinking and Privacy Guarantees on Corrupt or Truncated Images
// ============================================================================

#[test]
fn test_read_and_cleanup_temp_image_removes_corrupted_png() {
    let dir = tempdir().expect("tempdir");
    let corrupt_file = dir.path().join("corrupted.png");

    // Write invalid bytes (not a valid PNG)
    std::fs::write(&corrupt_file, b"NOT_A_VALID_PNG_SIGNATURE").expect("write corrupt");
    assert!(corrupt_file.exists());

    let res = read_and_cleanup_temp_image(&corrupt_file);
    assert!(
        matches!(res, Err(CaptureError::InvalidImage(_))),
        "Must return InvalidImage error"
    );

    // File MUST be removed regardless of validation error!
    assert!(
        !corrupt_file.exists(),
        "Corrupted screenshot file MUST be deleted from disk even when validation fails"
    );
}

#[tokio::test]
async fn test_portal_read_and_unlink_file_removes_corrupted_png() {
    let dir = tempdir().expect("tempdir");
    let corrupt_portal_file = dir.path().join("corrupted_portal.png");

    std::fs::write(&corrupt_portal_file, b"TRUNCATED").expect("write");
    assert!(corrupt_portal_file.exists());

    let res = read_and_unlink_file(&corrupt_portal_file).await;
    assert!(
        matches!(res, Err(CaptureError::InvalidImage(_))),
        "Must return InvalidImage error"
    );

    // File MUST be removed via TempFileGuard
    assert!(
        !corrupt_portal_file.exists(),
        "Portal temporary file MUST be unlinked by RAII guard even when PNG validation fails"
    );
}

// ============================================================================
// 6. Adversarial Geometry Validation
// ============================================================================

#[tokio::test]
async fn test_wayland_cli_capturer_rejects_malicious_geometry_injection() {
    let dir = tempdir().expect("tempdir");

    // Mock slurp returning a command injection payload: "10,20 100x100; touch /tmp/pwned"
    let slurp_evil = dir.path().join("mock_slurp_evil.sh");
    {
        let mut f = File::create(&slurp_evil).expect("create");
        f.write_all(b"#!/bin/sh\necho '10,20 100x100; touch /tmp/pwned'\nexit 0\n")
            .expect("write");
        let mut perms = f.metadata().unwrap().permissions();
        perms.set_mode(0o755);
        f.set_permissions(perms).unwrap();
    }

    let grim_dummy = dir.path().join("mock_grim_dummy.sh");
    {
        let mut f = File::create(&grim_dummy).expect("create");
        f.write_all(b"#!/bin/sh\nexit 0\n").expect("write");
        let mut perms = f.metadata().unwrap().permissions();
        perms.set_mode(0o755);
        f.set_permissions(perms).unwrap();
    }

    tokio::time::sleep(std::time::Duration::from_millis(10)).await;

    let capturer = WaylandCliCapturer::with_binaries(
        slurp_evil.to_str().unwrap(),
        grim_dummy.to_str().unwrap(),
    );

    let res = capturer.capture_region().await;
    assert!(
        matches!(res, Err(CaptureError::InvalidGeometry(_))),
        "Command injection character in geometry must trigger InvalidGeometry error"
    );
}
