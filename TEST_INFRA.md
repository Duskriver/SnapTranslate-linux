# Test infrastructure

## Automated Rust tests

Run `cargo test --locked`.

- `client_test.rs` uses local TCP HTTP/SSE servers. It checks HTTP status mapping, UTF-8 split across network chunks, CRLF and multiline SSE, explicit completion, early EOF, empty output, truncated output, malformed response handling, and cancellation while waiting for headers or stream data.
- `capture_test.rs` and `adversarial_capture_test.rs` use synthetic images and mock screenshot commands. They verify backend selection, cancellation, file cleanup, and invalid data handling.
- Configuration, CLI, and credential tests use temporary configuration directories. Headless tests deliberately use an unavailable D-Bus address to isolate credential access. That address is not a production GUI bypass.
- Invalid X11 / Wayland displays must fail initialization with a nonzero exit code. Real GUI success is checked separately.
- UI unit tests check native Pango output, command quoting for executable paths, settings validation and rollback, credential-deletion failures, and screenshot ownership release on window close. Client tests also check typed recovery hints and safe HTTP-status diagnostics.

## GUI integration harness

Run after building:

```bash
cargo build --locked
/usr/bin/python3 scripts/verify-gui-flow.py --keep-artifacts
```

Use `--binary target/release/snaptranslate-linux` to test the release executable.
Use `--ux-only` to run daily settings, clipboard, window, and reading interactions; `--reading-only` isolates streaming scroll behavior, `--recovery-only` checks stopping and retrying a screenshot, and `--edge-only` selects lifecycle edge cases. `--parity-only` covers queue admission, capture cancellation and GlobalShortcuts; `--native-only` runs actual maim selection on generated pixels. The default runs all groups.

The harness starts:

1. A dedicated D-Bus session with no desktop-service activation directories.
2. Xvfb and an isolated accessibility bus.
3. A mock StatusNotifierWatcher exposing real D-Bus tray registration and menu calls.
4. The actual application binary with temporary XDG config/data/cache/state/runtime directories.
5. A fake `maim` returning a fixed PNG or a cancellation result.
6. A localhost HTTP server that records each translation request and supplies successful, unauthorized, interrupted, or slow SSE responses.

Assertions inspect the real GTK accessibility tree, process lifetime, HTTP request content, and client disconnections. UX checks use real XTest keyboard/mouse events, GDK clipboard reads, and AT-SPI text positions to check selection copying and streaming scroll behavior. They also verify language selection, save validation, cancel/discard behavior, and staged credential deletion. No application-only test mode or fake-success startup branch is used.

Recovery checks compare the actual image payload and screenshot-command count across retries, verify saved credentials/model/API-path changes reach the local server, count all incoming requests to detect duplicate activation, and verify stopping preserves copyable text while disconnecting HTTP. They also check that a retried window keeps its identity, replaces its previous translation, and cancels independently of other windows.

The script requires `Xvfb`, `xvfb-run`, `xauth`, `dbus-run-session`, Python GI/D-Bus bindings, AT-SPI introspection data, `at-spi-bus-launcher`, and libXtst for real Escape key events. On Ubuntu / Debian these are provided by `xvfb xauth dbus-daemon python3-gi python3-dbus gir1.2-atspi-2.0 gir1.2-gtk-3.0 gir1.2-gtk-4.0 at-spi2-core libxtst6`.

With `--keep-artifacts`, application logs, settings/result screenshots, and failure-time accessible window text are retained in a printed temporary directory. Test child processes are cleaned up on success or failure. Test API credentials and screenshot fixtures are synthetic.

## Manual desktop acceptance

The harness does not verify the user's real compositor or Portal permission UI. On each supported desktop, separately verify selection, cancellation, repeated shortcut invocation, permissions, mixed-scale monitors, and tray availability. Real model quality, latency, and service-account billing are outside the isolated test suite.

## Desktop parity checks

The additional D-Bus portal fixture implements GlobalShortcuts sessions, requests, binding/configuration, activation, binding changes, session closure, and cancellable Screenshot requests. These verify that actual backend trigger descriptions are shown, authorization cancellation is visible, saved authorization restores, and permission denial is not silently treated as cancellation. HTTP active/peak counts verify two-slot admission; cancelled queued results never upload.

When `maim` is installed, the suite uses the real binary to capture a generated GTK window via XTest drag events inside Xvfb, checks PNG dimensions, verifies multiline/table rendering, and opens an HTTPS link through a private temporary desktop handler. No user desktop pixels or external website are used. Capture hides and restores the real application windows, so saved accessibility proxies are refreshed by window title after remapping.

## Native package lifecycle

`python3 scripts/package.py` builds a .deb and hashes. `python3 scripts/verify-package.py --gui dist/FILE.deb` uses a temporary dpkg root for real install/upgrade/remove/purge, validates owner/mode and installed payload hashes, preserves synthetic user config/credentials, and runs the full GTK suite against the executable installed from the archive. Unprivileged runs use bubblewrap; root CI runs still target only the private dpkg root. The host dependency solver can be checked separately with `apt-get -s install ./dist/FILE.deb`.

The packaging workflow builds natively on Ubuntu 24.04 amd64/arm64. YAML/local package checks do not constitute a successful remote Actions run. A tag matching Cargo.toml creates only a reviewable draft after both architectures pass and LICENSE exists.
