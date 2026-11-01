# Test Infrastructure & Specification: SnapTranslate-linux

## 1. Test Philosophy

SnapTranslate-linux adopts an **opaque-box, process-isolated End-to-End (E2E)** testing methodology.
The application interacts directly with Linux desktop subsystems:
- Standard FreeDesktop XDG specifications (Base Directory, Desktop Portal, Secret Service).
- Wayland / X11 display compositors.
- Command-line interfaces bound to desktop shortcut managers.
- Network endpoints streaming Server-Sent Events (SSE).

### Core Principles

1. **True Process Boundaries**: E2E tests spawn the compiled `snaptranslate-linux` binary as an external subprocess via `assert_cmd`. No internal in-memory shortcuts; tests observe only exit codes, standard streams (`stdout`, `stderr`), file artifacts, and environment mutations.
2. **Deterministic Isolation**: Every test creates a disposable, ephemeral sandbox (`tempfile::TempDir`) that isolates `$HOME`, `$XDG_CONFIG_HOME`, and D-Bus session variables. Tests never read, modify, or pollute the host system's `~/.config/snaptranslate/` or host system Keyring.
3. **Defense-in-Depth Key Security**: Plaintext credentials must never be leaked across any observable channel:
   - Zero plaintext in CLI arguments (no `--api-key` flag allowed).
   - Zero plaintext in stdout or stderr during normal or verbose (`-vvv`) logging.
   - Zero plaintext in JSON exports (`config get --json`).
   - Strict POSIX file permissions (`0600` file, `0700` directory) for unencrypted fallbacks.
4. **4-Tier Verification Hierarchy**:
   - **Tier 1 (Feature Coverage)**: >= 5 test cases per inventoried feature verifying the primary happy path and direct functionality.
   - **Tier 2 (Boundary & Corner Cases)**: Empty values, extreme lengths, multi-byte UTF-8 / emoji strings, corrupt configs, missing display environments.
   - **Tier 3 (Cross-Feature Interactions)**: Multi-step lifecycle operations (set-key via stdin -> config get -> set-language -> verify roundtrip).
   - **Tier 4 (Real-World Scenarios)**: Complete end-to-end user workflows (first-time onboarding, key rotation, script automation, shortcut binding).

---

## 2. Feature Inventory & Mapping

| Feature ID | Feature Name | Description | Acceptance Criteria Source | Test Suite Target |
|------------|--------------|-------------|----------------------------|-------------------|
| **F-CLI-01** | CLI Help & Information | Root `--help`, `-h`, `--version`, `-V`, and subcommand help | R5, AC § Functional | `e2e_cli_test.rs` |
| **F-CLI-02** | Subcommand Routing | Dispatching to `capture`, `config` (get, path, set-*, delete-key) | R5, AC § Functional | `e2e_cli_test.rs` |
| **F-CFG-01** | Config Persistence | Atomic reading & writing of `config.toml` under `$XDG_CONFIG_HOME` | R4, AC § Functional | `e2e_config_test.rs` |
| **F-KEY-01** | Keyring & Fallback Storage | Credential storage via Secret Service with fallback to `0600` file | R4, AC § Functional | `e2e_config_test.rs` |
| **F-SEC-01** | Credential Masking | Safe masking (`sk-1...cdef` / `********`) and zero plaintext leakage | R4, AC § Functional | `e2e_config_test.rs` |
| **F-HDL-01** | Headless Execution | Config commands execute cleanly without graphical display environment | R5 | `e2e_cli_test.rs` |
| **F-ERR-01** | Robust Error Handling | Input validation, corrupt config rejection, display check failure | R1, R4, R5 | `e2e_cli_test.rs`, `e2e_config_test.rs` |

---

## 3. Test Architecture & Harness

### 3.1 Test Harness (`E2eTestEnv`)

The integration test suites use an isolated environment harness:

```rust
pub struct E2eTestEnv {
    pub temp_dir: TempDir,
    pub home_dir: PathBuf,
    pub config_dir: PathBuf,
    pub config_file: PathBuf,
    pub credentials_file: PathBuf,
}
```

#### Environment Variables Configured per Process:
- `HOME`: Set to `temp_dir.path()`.
- `XDG_CONFIG_HOME`: Set to `temp_dir.path().join(".config")`.
- `DBUS_SESSION_BUS_ADDRESS`: Controlled to test Secret Service fallback (`unix:path=/dev/null` forces fallback).
- `WAYLAND_DISPLAY` / `DISPLAY`: Controlled to test headless execution vs. graphical display detection.
- `RUST_LOG` / CLI verbosity flags: Controlled to test logging isolation.

### 3.2 Directory Structure of Test Suites

```
tests/
├── e2e_cli_test.rs             # Tier 1-4 tests for CLI, Help, Subcommands, Headless, Display checks
├── e2e_config_test.rs          # Tier 1-4 tests for Config, Keyring, Masking, Boundaries, Scenarios
├── cli_test.rs                 # Unit/grammar tests for Clap parser
├── config_test.rs              # Unit tests for Config struct and serialization
└── adversarial_storage_test.rs # Adversarial file corruption and POSIX permission tests
```

---

## 4. Test Scenarios Matrix

### 4.1 Tier 1: Feature Coverage (>=5 test cases per feature)

#### Feature 1: CLI Help & Information (`F-CLI-01`)
1. `test_cli_root_help_long`: Run `--help`, verify exit 0, usage format, subcommands list.
2. `test_cli_root_help_short`: Run `-h`, verify exit 0, summary format.
3. `test_cli_version_flags`: Run `--version` and `-V`, verify exit 0, outputs `snaptranslate-linux 0.1.0`.
4. `test_cli_config_help`: Run `config --help`, verify exit 0, lists `get`, `set-key`, `set-language`, `set-model`, `set-url`, `delete-key`, `path`.
5. `test_cli_capture_help`: Run `capture --help`, verify exit 0, describes `--lang` / `-l` flag.
6. `test_cli_subcommand_help_variants`: Run `config set-key --help`, `config get --help`, `config set-language --help`, verify exit 0 and option descriptions.

#### Feature 2: Subcommands Invocations (`F-CLI-02`)
1. `test_subcommand_config_get_default`: Run `snaptranslate-linux config get` without prior config, verify defaults.
2. `test_subcommand_config_path`: Run `snaptranslate-linux config path`, verify output contains config and credentials paths under sandbox.
3. `test_subcommand_config_set_language`: Run `snaptranslate-linux config set-language en`, verify exit 0 and confirmation message.
4. `test_subcommand_config_set_language_alias`: Run `snaptranslate-linux config set-lang ja`, verify alias works identically.
5. `test_subcommand_config_set_model`: Run `snaptranslate-linux config set-model deepseek-chat`, verify exit 0 and confirmation.
6. `test_subcommand_config_set_url`: Run `snaptranslate-linux config set-url https://custom.api.com`, verify exit 0 and confirmation.
7. `test_subcommand_capture_with_lang_flag`: Run `snaptranslate-linux capture -l de` with simulated display, verify clean invocation.

#### Feature 3: Config Persistence (`F-CFG-01`)
1. `test_config_persistence_initial_write`: Writing target language creates valid TOML at expected path.
2. `test_config_persistence_field_independence`: Updating `model` preserves previously set `target_language` and `api_base_url`.
3. `test_config_persistence_missing_file_falls_back_to_defaults`: Reading config when file does not exist yields defaults.
4. `test_config_persistence_partial_toml`: When config file contains only a subset of fields, missing fields adopt defaults.
5. `test_config_persistence_atomic_write`: Verifies that file modifications occur cleanly without leaving dangling temporary files.

#### Feature 4: Keyring & Fallback Storage (`F-KEY-01`)
1. `test_keyring_set_key_via_stdin`: Piping key via stdin stores key successfully.
2. `test_keyring_fallback_file_permissions_0600`: Stored credentials file has strict POSIX mode `0600`.
3. `test_keyring_fallback_directory_permissions_0700`: Parent directory of credentials file has strict POSIX mode `0700`.
4. `test_keyring_delete_key_force`: `config delete-key -f` deletes credentials file cleanly.
5. `test_keyring_read_after_write`: Stored key can be read back and verified via `config get`.

#### Feature 5: Credential Masking & Zero Plaintext Leakage (`F-SEC-01`)
1. `test_masking_standard_key_in_stdout`: Standard key `sk-1234567890abcdef` is masked as `sk-1...cdef` in `config get`.
2. `test_masking_short_key_in_stdout`: Short key `sk-short` is masked as `********` in `config get`.
3. `test_masking_json_output`: `config get --json` returns masked key and never reveals plaintext.
4. `test_masking_set_key_stdin_no_echo`: `config set-key --stdin` output contains no plaintext key characters.
5. `test_masking_verbose_logging`: Running with `-vvv` does not log plaintext credentials to stderr or stdout.

#### Feature 6: Headless Execution (`F-HDL-01`)
1. `test_headless_config_get`: `config get` succeeds with exit 0 when `WAYLAND_DISPLAY` and `DISPLAY` are unset.
2. `test_headless_config_set_key`: `config set-key --stdin` succeeds with exit 0 in headless environment.
3. `test_headless_config_set_language`: `config set-language` succeeds with exit 0 in headless environment.
4. `test_headless_config_path`: `config path` outputs valid paths with exit 0 in headless environment.
5. `test_headless_config_get_json_pipeline`: `config get --json` outputs parseable JSON to stdout in headless environment.

#### Feature 7: Error Handling & Input Validation (`F-ERR-01`)
1. `test_error_invalid_subcommand`: Unknown subcommand outputs error and exits with code 2.
2. `test_error_set_key_empty_stdin`: Passing empty string to `set-key --stdin` fails with exit 1 and stderr message.
3. `test_error_set_language_empty`: Passing empty language fails with exit 1 and error message.
4. `test_error_set_model_empty`: Passing empty model fails with exit 1 and error message.
5. `test_error_set_url_invalid_scheme`: Setting URL with invalid scheme (`ftp://...`) fails validation with exit 1.
6. `test_error_gui_missing_display`: Invoking root or `capture` without display env vars fails with exit 1 and helpful error message.

---

### 4.2 Tier 2: Boundary & Corner Cases

1. `test_boundary_extreme_length_api_key`: 4096-character key is safely stored, masked, and read without buffer overflow.
2. `test_boundary_unicode_target_language`: Complex Unicode language tags (e.g., `zh-Hans-CN`, `ja-JP-u-ca-japanese`) are preserved verbatim.
3. `test_boundary_whitespace_trimming`: Leading and trailing whitespace around keys, languages, URLs are cleanly trimmed.
4. `test_boundary_non_standard_key_prefix`: Key without `sk-` prefix emits warning on stderr but succeeds.
5. `test_boundary_corrupt_config_file_refuses_overwrite`: Syntax-corrupted `config.toml` causes mutator to fail with exit 1 rather than silently overwriting.
6. `test_boundary_corrupt_credentials_file_fails_gracefully`: Syntax-corrupted `credentials.toml` causes `config get` to report parse failure gracefully.
7. `test_boundary_url_with_ports_and_paths`: `https://my-proxy.internal:8080/v1/deepseek` is accepted as a valid URL.

---

### 4.3 Tier 3: Cross-Feature Interactions

1. `test_interaction_full_config_roundtrip`: Set key via stdin -> set language -> set model -> set URL -> verify `config get --json` returns all updated values.
2. `test_interaction_key_rotation_and_clear`: Set key A -> verify get -> set key B -> verify get -> delete key -> verify get shows Not Configured.
3. `test_interaction_quiet_vs_verbose`: Compare `config get -q` (suppresses non-essential text) vs `config get -vvv` (adds trace output).
4. `test_interaction_capture_flag_override_vs_persisted_config`: Setting default language to `ja` in config, then running `capture -l fr` overrides language for the session without mutating `config.toml`.

---

### 4.4 Tier 4: Real-World Scenarios

1. `test_scenario_first_time_user_onboarding`:
   - Step 1: User runs `config get` (sees unconfigured defaults).
   - Step 2: User runs `config set-key --stdin` with their API key.
   - Step 3: User configures target language to `zh-CN`.
   - Step 4: User inspects `config get` to confirm ready status.
2. `test_scenario_automated_script_pipeline`:
   - Automates setup via shell-style pipeline: pipes stdin key, runs `config get --json`, parses JSON, verifies `api_key_configured: true`.
3. `test_scenario_desktop_shortcut_invocation`:
   - Simulates window manager shortcut trigger (`snaptranslate-linux capture`) under graphical display (`DISPLAY=:0`), ensuring clean exit code 0.

---

## 5. Coverage & Verification Thresholds

| Metric | Target | Verification Method |
|--------|--------|---------------------|
| Tier 1 Feature Coverage | >= 5 tests per feature (7 features = >= 35 tests) | `cargo test --test e2e_*` |
| Tier 2 Boundary Coverage | >= 7 boundary cases | `cargo test --test e2e_*` |
| Tier 3 Cross-Feature Interactions | >= 4 interaction workflows | `cargo test --test e2e_*` |
| Tier 4 Real-World Scenarios | >= 3 real-world scenarios | `cargo test --test e2e_*` |
| Total E2E Tests | >= 45 tests | `cargo test --test e2e_*` |
| Pass Rate | 100% (zero failures, zero flakes) | `cargo test --test e2e_*` |
| Process Security | Zero plaintext API key leaks across stdout/stderr | String matching assertions |
| POSIX Compliance | `0600` on credentials file, `0700` on directory | `std::os::unix::fs::PermissionsExt` |

---

## 6. How to Run the Test Suite

```bash
# Run all E2E test suites:
cargo test --test e2e_*

# Run E2E CLI tests specifically:
cargo test --test e2e_cli_test

# Run E2E Config & Keyring tests specifically:
cargo test --test e2e_config_test

# Run with full output logging:
cargo test --test e2e_* -- --nocapture
```
