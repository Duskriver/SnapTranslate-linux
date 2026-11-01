# E2E Test Suite Ready: SnapTranslate-linux

## 1. Executive Summary

The opaque-box End-to-End (E2E) integration test suite for `SnapTranslate-linux` has been successfully implemented and verified. All 58 E2E test cases execute against the compiled binary target `snaptranslate-linux` within isolated temporary sandboxes with 100% pass rate.

- **Total E2E Integration Tests**: 58
- **Total Project Tests**: 96 (38 Unit/Grammar/Adversarial + 58 E2E)
- **Pass Rate**: 100% (96 passed, 0 failed, 0 ignored)
- **Zero Plaintext Leakage**: Verified across all stdout, stderr, verbose debug logs, and JSON outputs.
- **POSIX Mode Compliance**: Strict `0600` for credential fallback files, `0700` for configuration directories.

---

## 2. Test Suite Inventory

| Test File | Target Scope | Test Count | Pass / Fail | Execution Time |
|-----------|--------------|------------|-------------|----------------|
| `tests/e2e_cli_test.rs` | CLI Help, Subcommand Routing, Headless Execution, Display Checks, CLI Error Handling | 31 | 31 Passed / 0 Failed | ~0.02s |
| `tests/e2e_config_test.rs` | Config Persistence, Keyring Fallback, Credential Masking, Boundaries, Cross-Feature Interactions, Real-World Scenarios | 27 | 27 Passed / 0 Failed | ~0.04s |
| **Total E2E Tests** | **Comprehensive 4-Tier Opaque-Box Coverage** | **58** | **58 Passed / 0 Failed** | **~0.06s** |

---

## 3. 4-Tier Testing Methodology Coverage

### Tier 1: Feature Coverage (>= 5 test cases per feature)

| Feature | Requirement | Test Cases Implemented | Test Names |
|---------|-------------|------------------------|------------|
| **F-CLI-01: CLI Help & Info** | R5, AC § Functional | 6 | `test_cli_help_long_flag`, `test_cli_help_short_flag`, `test_cli_version_flags`, `test_cli_config_help`, `test_cli_capture_help`, `test_cli_subcommand_specific_help` |
| **F-CLI-02: Subcommands & Routing** | R5, AC § Functional | 7 | `test_subcommand_config_get_default`, `test_subcommand_config_path`, `test_subcommand_config_set_language_execution`, `test_subcommand_config_set_model_execution`, `test_subcommand_config_set_url_execution`, `test_subcommand_capture_invocation_with_display`, `test_subcommand_capture_invocation_with_lang_flag` |
| **F-CFG-01: Config Persistence** | R4, AC § Functional | 6 | `test_config_persistence_initial_write_creates_valid_toml`, `test_config_persistence_field_independence_target_language`, `test_config_persistence_field_independence_model`, `test_config_persistence_missing_file_yields_defaults`, `test_config_persistence_partial_toml_augments_defaults`, `test_config_persistence_atomic_write_no_tmp_leftover` |
| **F-KEY-01: Keyring Storage & Fallback** | R4, AC § Functional | 6 | `test_keyring_set_key_via_stdin_creates_credentials_file`, `test_keyring_fallback_file_permissions_are_strict_0600`, `test_keyring_fallback_directory_permissions_are_strict_0700`, `test_keyring_read_after_write_reports_configured`, `test_keyring_delete_key_force_removes_credentials_file`, `test_keyring_permissive_file_auto_remediated_to_0600` |
| **F-SEC-01: Credential Masking** | R4, AC § Functional | 5 | `test_masking_standard_key_in_stdout`, `test_masking_short_key_in_stdout`, `test_masking_json_output_never_reveals_plaintext`, `test_masking_set_key_stdin_no_echo`, `test_masking_verbose_logging_zero_leakage` |
| **F-HDL-01: Headless Execution** | R5 | 5 | `test_headless_config_get_requires_no_display`, `test_headless_config_path_requires_no_display`, `test_headless_config_set_language_requires_no_display`, `test_headless_config_set_key_via_stdin_requires_no_display`, `test_headless_config_get_json_pipeline_requires_no_display` |
| **F-ERR-01: Robust Error Handling** | R1, R4, R5 | 6 | `test_error_invalid_subcommand`, `test_error_missing_required_language_argument`, `test_error_empty_language_rejected`, `test_error_empty_model_rejected`, `test_error_invalid_url_scheme_rejected`, `test_error_gui_missing_display_detected` |

*Tier 1 Total: 41 test cases (exceeding the >= 35 threshold across all 7 features)*

---

### Tier 2: Boundary & Corner Cases

| Scenario / Edge Case | Test Name | Invariant Verified |
|----------------------|-----------|--------------------|
| Extreme Length Secret | `test_boundary_extreme_length_api_key` | 4096-character key parsed, stored, and masked without truncation bugs |
| Multi-byte Unicode Tag | `test_boundary_unicode_target_language` | Unicode & emoji characters (`zh-Hans-CN-⚡-translation`) preserved in config |
| Non-Standard API Key Prefix | `test_boundary_non_standard_key_prefix_emits_warning` | Warning emitted to stderr, key successfully ingested and masked |
| Corrupt Config File | `test_boundary_corrupt_config_file_refuses_overwrite` | CLI refuses to mutate corrupted `config.toml`, protecting user data |
| Corrupt Credentials File | `test_boundary_corrupt_credentials_file_fails_gracefully` | Graceful parse error handling without unhandled panics |
| Whitespace Ingestion | `test_boundary_whitespace_padding_in_key_trimmed` | Leading/trailing whitespace cleanly stripped before storage |
| Whitespace Language Tag | `test_boundary_whitespace_trimming_language` | Whitespace around language argument cleanly trimmed |
| Complex URL with Port & Path | `test_boundary_url_with_ports_and_paths` | Custom ports (`:8443`) and sub-paths (`/v1/deepseek`) parsed properly |
| Wayland Session Detection | `test_boundary_wayland_display_accepted` | `WAYLAND_DISPLAY` without `DISPLAY` fulfills graphical requirements |
| Multi-Level Verbosity Flags | `test_boundary_logging_verbosity_flags` | `-v`, `-vv`, `-vvv`, and `-q` flags execute cleanly |

*Tier 2 Total: 10 test cases*

---

### Tier 3: Cross-Feature Interactions

| Interaction Workflow | Test Name | Description |
|----------------------|-----------|-------------|
| Full Config Round-trip | `test_interaction_full_config_roundtrip_lifecycle` | Sequentially set key -> language -> model -> URL -> verify all fields via `config get --json` |
| Secret Rotation & Revocation | `test_interaction_key_rotation_and_revocation` | Store key 1 -> verify get -> overwrite with key 2 -> verify get -> delete-key -f -> verify unconfigured |
| Subcommand Alias Parity | `test_subcommand_alias_set_lang` | Confirms `set-lang` alias produces identical persistence to `set-language` |
| Help Parity | `test_help_subcommand_syntax_parity` | Confirms `snaptranslate-linux help config` exactly matches `config --help` |

*Tier 3 Total: 4 test cases*

---

### Tier 4: Real-World Scenarios

| Real-World Scenario | Test Name | Workflow Steps |
|---------------------|-----------|----------------|
| First-Time Onboarding | `test_scenario_first_time_user_onboarding` | Fresh install -> inspect default config -> query paths -> set key via stdin -> set language -> verify ready state |
| Automated DevOps Script | `test_scenario_automated_devops_script_pipeline` | Headless pipeline sets configuration and parses `config get --json` via JSON deserializer |
| Desktop Shortcut Trigger | `test_scenario_shortcut_capture_invocation` | Simulates global desktop keybinding triggering `snaptranslate-linux capture --lang zh-CN` and `capture` under active display |

*Tier 4 Total: 3 test cases*

---

## 4. Test Runner Instructions

### Run All E2E Integration Tests
```bash
cargo test --test e2e_*
```

### Run CLI E2E Tests Only
```bash
cargo test --test e2e_cli_test
```

### Run Config & Keyring E2E Tests Only
```bash
cargo test --test e2e_config_test
```

### Run with Verbose Test Output
```bash
cargo test --test e2e_* -- --nocapture
```

### Run the Complete Project Suite (Unit, Integration, E2E)
```bash
cargo test
```

---

## 5. Security & Privacy Audit Verification

- **Plaintext Secret Leakage Check**: PASSED. Verified that neither normal execution, error handling, nor `-vvv` (trace logging) ever prints plaintext API keys.
- **Key Masking Integrity**: PASSED. Standard keys (>8 chars) always masked as `prefix...suffix` (e.g. `sk-a...2345`). Short keys (<=8 chars) masked as `********`.
- **POSIX Mode Check**: PASSED. Fallback credentials file `credentials.toml` created with mode `0600` (`-rw-------`), and parent folder created with mode `0700` (`drwx------`). Insecure permissions (e.g., `0644`) are automatically remediated to `0600` upon read.
