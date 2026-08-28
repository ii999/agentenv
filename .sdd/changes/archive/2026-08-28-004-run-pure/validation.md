# Validation: 004-run-pure

Date: 2026-08-28. Validated on macOS (Darwin 25.6.0) against checkpoint `6fb95a4` on branch `sdd/004-run-pure`.

## Verification Runs

- `cargo build`: clean, no warnings.
- `cargo fmt --check`: clean.
- `cargo test`: 242 passed, 0 failed (223 pre-existing unmodified, 4 new seam unit tests, 15 new integration tests in `tests/run_pure.rs`).
- Checkpoint review: high-capability native lane over the full diff — verdict "Approve with fixes", no correctness or security defects; findings CR-01..CR-08 applied in the same checkpoint, CR-09 recorded as an accepted residual (below). The reviewer independently confirmed both base lists match the spec name-for-name in code, README, and spec, and that no new path can write a parent value or credential to agentenv's own output.
- Manual walkthrough (scratch config, fake command-provider token): stray parent variable absent under `--pure` while base, `inject` value, and credential injection arrive; missing-keep line reported with next action and the target's exit status (3) preserved; `--keep` without `--pure` exits 1 with the documented diagnostic.

## Acceptance Matrix Results

| Acceptance ID | Result | Evidence |
| --- | --- | --- |
| AC-001.1 | Pass | `run_pure.rs::pure_carries_base_keeps_and_injections_and_nothing_else` |
| AC-001.2 | Pass | `run_pure.rs::pure_excludes_unlisted_lc_names` (LC_ALL and LC_CTYPE asserted) |
| AC-001.3 | Pass | `run_pure.rs::pure_base_names_absent_from_parent_stay_absent` |
| AC-001.4 | Pass | `run_pure.rs::injection_overrides_an_inherited_value_under_pure` |
| AC-001.5 | Pass | `run_pure.rs::default_run_still_inherits_every_non_overridden_variable` |
| AC-001.6 | Pass | `run_pure.rs::pure_excludes_unlisted_lc_names` + seam test `unlisted_lc_names_are_excluded` |
| AC-001.7 | Deferred | `run_pure.rs::windows_carries_the_parent_path_spelling_and_value_once` (cfg(windows); see DEF-001) |
| AC-001.8 | Pass | `run_pure.rs::nested_agentenv_resolves_the_same_config_and_profile_under_pure` (AGENTENV_FILE and AGENTENV_PROFILE both exercised) |
| AC-002.1 | Pass | `run_pure.rs::keep_carries_a_parent_value_and_duplicates_collapse` |
| AC-002.2 | Pass | `run_pure.rs::missing_keep_is_reported_once_and_the_run_continues` |
| AC-002.3 | Pass | `run_pure.rs::invalid_keep_names_are_usage_errors_before_launch` (exit 1, grammar named, no launch) |
| AC-002.4 | Pass | `run_pure.rs::keep_without_pure_is_a_usage_error` |
| AC-002.5 | Pass | duplicate-keep carriage and single-report-line assertions in the two tests above |
| AC-002.6 | Pass | `run_pure.rs::missing_keep_line_survives_an_injection_conflict` (report precedes conflict detection; exit 4) |
| AC-003.1 | Pass | full suite green with zero pre-existing test files modified |
| AC-003.2 | Pass | `run_pure.rs::pure_conflict_diagnostic_matches_the_default_one` (byte-identical stderr) |
| AC-003.3 | Pass | exit-7 propagation in `missing_keep_is_reported_once_and_the_run_continues`; manual exit-3 walkthrough |
| AC-004.1 | Pass | `run_pure.rs::new_diagnostic_paths_never_echo_parent_values` + harness-wide sentinel gate in `run_ac` |
| AC-004.2 | Pass | `run_pure.rs::invalid_keep_argument_value_is_never_echoed` (`--keep API_KEY=<sentinel>` echoes only `'API_KEY'`) |
| AC-004.3 | Pass | next-action assertions across the diagnostic tests; reviewer-confirmed wording |
| AC-005.1 | Pass | README "Pure runs" section: both flags, both base lists (reviewer-verified name-for-name), layering order, TLS/proxy omission with `--keep` recipe |
| AC-005.2 | Pass | `agentenv run --help` renders both flags with accurate one-line descriptions |
| AC-005.3 | Pass | README threat-model paragraph and SKILL.md both state filter-not-sandbox and the unchanged `command`-provider environment (CR-05 fix) |

Edge cases: EDGE-001 (`unfindable_target_exits_127_and_absolute_targets_need_no_path`), EDGE-002/003 (keep tests), EDGE-004 (inheritance-only report with injected supply, CR-02 fix), EDGE-005 (seam unit test with non-UTF-8 name and value), EDGE-006 (Windows test, DEF-001), EDGE-007 (README recipe, manual).

## Deviations and Accepted Residuals

| Item | Detail | Disposition |
| --- | --- | --- |
| DEV-001 (from CR-09) | An invalid `--keep` token containing no `=` is echoed verbatim in the usage diagnostic. Spec-conformant — SPEC-004 scopes redaction to text at or after the first `=`, and the token is a user-typed name — but a secret pasted as a bare token reaches stderr. | Accepted as a known residual surface; tighten in a later change by reporting the first offending character instead of the token. |

## Deferred Items

| Item | Reason | Follow-up |
| --- | --- | --- |
| DEF-001: AC-001.7 / EDGE-006 Windows execution | The Windows-marked test compiles only under `cfg(windows)`; this machine is macOS. The test establishes its own `Path`-spelling precondition (CR-01 fix) and asserts the surviving spelling and value exactly. | Confirm the Windows CI job passes on the next push of `main`. |

Resolution (2026-08-28, post-archive): DEF-001 is resolved. CI run 33169616459 on `main` passed the full matrix, including `Test (windows-latest)` executing `windows_carries_the_parent_path_spelling_and_value_once`. AC-001.7 and EDGE-006 therefore stand fully accepted (23/23). Getting the matrix green surfaced test-only Windows portability defects — none in this change's product code: two seam unit tests and two integration assertions in this change's suites asserted Unix-only base names (fixed by platform-selecting the asserted names and scoping the locale test to Unix per AC-001.2), and the change-003 test fixtures hardcoded `XDG_STATE_HOME` where Windows derives the trust store from `LOCALAPPDATA` (fixed via a shared `STATE_BASE_ENV` helper constant, canonical-path notice assertions, and Unix-scoping of platform-specific diagnostic wording and the unavailable-state snapshot).

## Final Decision

Decision: Accepted

Rationale: all 23 acceptance criteria are resolved — 22 accepted with evidence, 1 (AC-001.7) deferred to the Windows CI job with the test in place. Build, format, and full suite are green with zero pre-existing tests modified. The checkpoint review found no correctness or security defect, and all eight of its actionable findings were applied before the implementation checkpoint. The end-to-end walkthrough matches the documented behavior exactly.
