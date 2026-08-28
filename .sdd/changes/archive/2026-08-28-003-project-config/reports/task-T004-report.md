# Task Report: T004

- Change ID: 003-project-config
- Recorded: 2026-08-28T08:30:16.855578+00:00
- Source: /Users/zhaiqifeng/Dev/agentenv/.sdd/changes/003-project-config/reports/task-T004-report.md
- Status: DONE

## Worker Result (verbatim)

# Implementation Report: T004

Status: DONE
Provider: claude-code
Model: claude-opus-5[1m]
Profile: implementation

## Summary

- The trust-store interface is pinned and its behavior is specified by a test suite that compiles and fails, so T005 implements against a fixed contract rather than inventing one.
- Eight integration tests in `tests/project_trust.rs` and one fault-injection unit test in `src/project/trust.rs` fail through `todo!()`; the store-location logic is implemented and its six unit tests pass.

## Implemented

- `src/project/trust.rs`: the `StoreFs` seam (`read`, `write_temp`, `rename`), the production `RealFs` adapter over `std::fs`, `store_path`, the `TrustStore` type with `load`/`lookup`/`allow`/`revoke`/`save`, and `fingerprint`. Every `TrustStore` method and `fingerprint` carry a `todo!()` body naming what T005 must implement; each carries the doc comment that states its contract (missing store loads empty, corrupt store is exit 2 naming the path, approval binds the passed snapshot, revoke is path-only, save is write-temp plus rename).
- `RealFs` is fully implemented: `write_temp` creates the store directory when missing, opens a uniquely named temporary file with `0600` permissions applied at creation on Unix (so no content is ever world-readable), writes and syncs, and removes the partial file when the write fails rather than leaving it behind. A bounded name-collision retry ends in an explicit error instead of a silent overwrite.
- `store_path` is fully implemented as pure environment logic mirroring `src/config/locate.rs`: `$XDG_STATE_HOME/agentenv/trust.toml` when absolute, else `$HOME/.local/state/agentenv/trust.toml` on Unix, `%LOCALAPPDATA%\agentenv\trust.toml` on Windows with `XDG_STATE_HOME` not consulted. Empty and relative values count as unset. It reuses `config::env_value` so the empty-value rule (SPEC-AS-028) keeps one owner.
- `tests/project_trust.rs`: fingerprint byte sensitivity and hex shape; canonical-path lookup, including a Unix case reaching the file through a symlinked ancestor; missing store loads empty; corrupt store errors with exit 2 naming the store path and a next action; save creates `0600` on Unix; two approvals then revoke-one preserves the other across save and reload, with the second revoke reporting nothing removed; approval binds the snapshot handed to `allow` while the on-disk content diverges.
- Unit tests in `src/project/trust.rs`: a `FaultyFs` adapter that models a single store file and can fail the rename, asserting that a failed commit errors with the store path plus a next action and leaves the previous store bytes byte-intact; plus per-platform `store_path` resolution including the unset-base error naming the variables.
- `Cargo.toml`: `sha2 = "0.10"` added in the dependency list's alphabetical position with a comment explaining what it fingerprints, matching the file's existing annotation style.

## Verification

| Command or check | Result | Notes |
| --- | --- | --- |
| `cargo build` | passed (exit 0) | No warnings. |
| `cargo test --test project_trust` | failed as intended | 8 tests, 0 passed, 8 failed; every failure is `not yet implemented: T005: …` from a `todo!()`. No compilation errors. |
| `cargo test --lib` | failed as intended | 111 tests, 110 passed, 1 failed. The single failure is `project::trust::tests::a_failing_commit_leaves_the_previous_store_intact`, panicking in `TrustStore::load`'s `todo!()`. All 105 pre-existing unit tests still pass. |
| `cargo test --test query_p1` | passed | 13 passed, 0 failed; no collateral damage. |
| `cargo fmt --check` | passed | Clean after formatting. |
| `git status --porcelain` | passed | Only `Cargo.toml`, `Cargo.lock`, `src/project/trust.rs`, and the new `tests/project_trust.rs`. No no-go path touched. |

## Files Changed

| Path | Change |
| --- | --- |
| `src/project/trust.rs` | Interface skeletons with `todo!()` bodies, the implemented `RealFs` adapter and `store_path`, and the unit tests needing the `StoreFs` seam. |
| `tests/project_trust.rs` | New integration suite for the store contract over the real filesystem. |
| `Cargo.toml` | Added `sha2 = "0.10"`. |
| `Cargo.lock` | Lock entries for `sha2` and its transitive dependencies. |

## Acceptance Coverage

| Acceptance ID | Evidence | Status |
| --- | --- | --- |
| AC-003.3 (store half) | `a_fingerprint_changes_with_any_byte_change` — added blank line, dropped trailing newline, and different content all diverge; identical bytes agree. | Test authored, failing |
| AC-003.7 | `lookup_is_keyed_by_the_canonical_path`, `lookup_matches_a_path_reached_through_a_symlinked_ancestor`. | Test authored, failing |
| AC-003.8 | `a_corrupt_store_names_the_path_and_a_next_action` asserts exit 2, the store path, and a next action; `a_missing_store_loads_as_empty` fixes the missing-file case so corruption can never be conflated with absence. | Test authored, failing |
| AC-003.9 | `saving_creates_the_store_with_owner_only_permissions` asserts `mode & 0o777 == 0o600`; `RealFs::write_temp` applies the mode at creation, before content. | Test authored, failing |
| AC-003.10 | `revoking_one_approval_preserves_the_other` across save and reload. | Test authored, failing |
| AC-003.11 | `a_failing_commit_leaves_the_previous_store_intact` via the `FaultyFs` seam: exit-2 error naming the store path plus a next action, and the previous bytes unchanged. | Test authored, failing |
| AC-003.12 (store half) | `approval_binds_the_snapshot_it_was_given`. | Test authored, failing |
| AC-003.13 (store half) | `revoking_one_approval_preserves_the_other` — revoke takes only a path and never reads content; the second revoke reports `false`. | Test authored, failing |
| EDGE-004b (store half) | `an_unset_state_base_names_the_variables_and_a_next_action` (Unix) and `an_unset_localappdata_names_the_variable_and_a_next_action` (Windows), plus the absolute, empty, and relative `XDG_STATE_HOME` cases. | Passing (`store_path` is implemented) |

## Self-Review

- [x] Scope matches the task brief.
- [x] No unrelated files changed; no no-go path touched.
- [x] Acceptance criteria are covered.
- [x] Verification evidence is recorded.
- [x] Concerns are documented.

## Concerns

- The six `store_path` unit tests pass rather than fail, because the task instruction directs `store_path` to be implemented (pure environment logic mirroring `config::locate`). They are unit tests in `src/project/trust.rs`, so `cargo test --test project_trust` remains uniformly failing; `cargo test --lib` shows 5 store-location passes plus 1 `todo!()` failure among the new tests. This is the intended state, recorded here so the mixed result is not read as a partial implementation.
- `store_path` treats a relative `XDG_STATE_HOME` as unset. The spec does not name this case; the rule mirrors the accepted `XDG_CONFIG_HOME` behavior (AC-001.4) so both bases behave the same, and it is covered by `a_relative_xdg_state_home_counts_as_unset`. T005 inherits it as implemented.
- Three diagnostics assert that the message contains `agentenv project allow` as its next action: the corrupt store, the failed commit, and the unset state base. This makes the change-001 next-action contract checkable without pinning wording; T005 must include that command in each of the three messages. The store path is asserted as a full path string, not a substring of it.
- `TrustStore`'s `records` field and the serialized store format are internal and carry `#[allow(dead_code)]`; T005 owns the representation and should remove that attribute along with the `#[allow(unused_variables)]` on the `impl` block and on `fingerprint` once the bodies land.
- The `sha2` dependency is declared but not yet used, since `fingerprint` is `todo!()`. This is expected until T005 lands and does not warn.

## Impact Delta

The impact map predicted no existing call sites. Re-verification with `git grep -n -F -e TrustStore -e StoreFs -e project::trust -e fingerprint -- ':(exclude).sdd/'` found no hits outside the two files this task authored, other than the explanatory comment added beside the new `sha2` entry in `Cargo.toml`. No coupling was discovered and the map needs no correction.
