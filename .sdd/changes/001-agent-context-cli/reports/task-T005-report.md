# Task Report: T005

- Change ID: 001-agent-context-cli
- Recorded: 2026-08-21T20:39:45.046232+00:00
- Source: /Users/zhaiqifeng/Dev/agent-context/.sdd/changes/001-agent-context-cli/reports/task-T005-report.md
- Status: DONE

## Worker Result (verbatim)

# Implementation Report: T005

Status: DONE
Provider: codex
Model: gpt-5.6-terra
Profile: implementation

## Summary

Implemented credential providers and `credential check`/`set` with a secret-safe capture boundary, interactive no-echo coverage, and test-keychain isolation.

## Implemented

- Added redacting `CapturedSecret` and `Secret` types, checked conversion, compile-fail contracts for `Display`/`Serialize`, and env, keychain, command, and test-store adapters.
- Absorbed shallow status handling into `credential`; free status calculation and provider adapters now share the same env, keychain, and command status logic.
- Added `credential check`/`set`, JSON rejection for resolving actions, test-store round trips, no-echo PTY coverage, sentinel checks for piped input, and line-ending scope coverage.
- Keychain reads now retain bytes through `get_secret`; command output and `credential set` input strip one terminal line ending while env and keychain values retain newlines.

## Verification

| Command or check | Result | Output tail |
| --- | --- | --- |
| `cargo test --features test-keychain --all-targets` | passed | 98 unit tests, 8 credential integration tests, 13 query integration tests, and 8 security integration tests passed. |
| `cargo test --doc --features test-keychain` | passed | 4 compile-fail doctests passed. |
| `cargo clippy --all-targets -- -D warnings` | passed | `Finished dev profile [unoptimized + debuginfo]`. |
| `cargo clippy --all-targets --features test-keychain -- -D warnings` | passed | `Finished dev profile [unoptimized + debuginfo]`. |
| `cargo fmt --check` | passed | No output; formatting is clean. |
| `cargo build --release` | passed | `Finished release profile [optimized]`. |
| `cargo build --release --features test-keychain` | failed as required (compile_error guard) | `the test-keychain feature is restricted to debug builds`. |

## Files Changed

| Path | Change |
| --- | --- |
| `Cargo.toml`, `Cargo.lock` | Added the test-keychain feature and Unix-only `portable-pty` test dependency. |
| `src/credential/` | Added provider adapters, secret capture types, test store, and the absorbed shallow-status module. |
| `src/cli/query_cmds.rs` | Added credential action dispatch, input handling, and JSON rejection. |
| `src/config/mod.rs`, `src/lib.rs`, `src/query.rs` | Reused generic empty-environment handling and exposed credential status use. |
| `src/shallow.rs` | Removed after absorption into `src/credential/shallow.rs`. |
| `tests/credential_p2.rs` | Added credential provider, PTY, strip-scope, JSON, and sentinel-guard coverage. |

## Acceptance Coverage

| Acceptance ID | Evidence | Status |
| --- | --- | --- |
| AC-014.1 | Unset env provider integration case. | Covered |
| AC-014.2–AC-014.5 | Command success, direct argv, keychain absence, and redacted failure cases. | Covered |
| AC-015.2–AC-015.5 | Piped and interactive test-store round trips, external-provider rejection, and unknown-name cases. | Covered |
| AC-018.1 | Credential failure integration cases assert exit codes 1, 3, and 4. | Covered |
| AC-019.2 | Unit and compile-fail doctest coverage verifies redacted `Debug` output and no `Display`/`Serialize` implementations. | Covered |
| EDGE-006, EDGE-011–EDGE-013 | Empty input, invalid UTF-8/NUL, retained environment newlines, and test-store write failures are covered. | Covered |

## Self-Review

- [x] Scope matches the task brief.
- [x] No unrelated files changed.
- [x] Acceptance criteria are covered.
- [x] Verification evidence is recorded.
- [x] Concerns are documented.

## Concerns

`Secret::as_str` remains `pub(crate)` for the Phase-3 runner seam; narrowing it further would require that phase's call-site design.

## Impact Delta

`shallow_status` moved from `src/shallow.rs` to `src/credential/shallow.rs`; its call site is now `src/query.rs`, with no observable Phase-1 behavior change. `run_ac` remained read-only; `run_with_input` now applies the same sentinel and environment rules locally. `AppError::Credential` is used by provider resolution and store failures. The credential CLI actions remain in `src/cli/query_cmds.rs`, which differs from the task-text path suggestion but matches the existing command-dispatch structure.
