# Task T007 quality retry — apply base patch, then fix review findings

## Task

Task T007 (injection planner + `run`) of SDD change `001-agent-context-cli` was implemented and reviewed REVISE (no Critical findings; the security core is verified sound — conflict-before-resolution, dedup semantics, secret flow, exit codes, and diagnostics are all correct). Reconstruct it and apply the targeted fixes below.

Steps:

1. Your worktree HEAD already contains the T007 base implementation (`src/runner.rs`, `run` wiring in `src/cli/query_cmds.rs`, `tests/run_p3.rs`) — do not re-implement it.
2. Read `.sdd/changes/001-agent-context-cli/briefs/task-T007-brief.md` and `.sdd/changes/001-agent-context-cli/briefs/task-T007-dispatch.md` (both in your worktree) for the full task contract.
3. Apply every fix below, then run the full verification and write your report.

## Fix List (I = Important, blocking; M = Minor, blocking unless marked optional)

- I1 (AC-017.1): no test exercises non-zero exit propagation — `TEST_PROBE_EXIT` is never set anywhere in `tests/`. In `tests/run_p3.rs`'s injection test, add `("TEST_PROBE_EXIT", "7")` to the probe invocation's env and assert exit 7 instead of 0 (the stdout/stderr byte assertions still hold).
- I2 (AC-016.7 / EDGE-018): the array-element non-scan behavior has no discriminating test — the current `ci` entry references the same credential both directly and inside an array, so the test passes regardless of whether arrays are scanned. Add a separate entry whose ONLY `credential://` reference sits inside an array value, backed by a `command` credential using `tests/fixtures/canary_provider.sh`; assert exit 0, the canary file absent, and the probe report containing no record for that credential's target env name.
- I3 (duplication on a security seam): `src/runner.rs` re-implements two traversals that must stay semantically identical to the validator — the reference walk (`collect_references`/`collect_value_references` mirroring `validate.rs`'s `scan_entry_references`/`scan_value_references`, including the `description`/`inject` skips and non-recursion into arrays) and a byte-identical private copy of `resolve_in_entry`. Five `expect`s in `InjectionPlan::build` rest on the two staying in sync; drift would turn them into user-reachable panics. Make one source of truth: expose `validate::resolve_in_entry` as `pub(crate)` and extract the reference walk into a single `pub(crate)` visitor in `validate.rs` (e.g. `fn walk_entry_references(entry: &Table, visit: &mut impl FnMut(&str, &CredentialRef))`) driven by BOTH the validator and the runner. No observable behavior change; all suites stay green.
- M4: `src/runner.rs:81-86` duplicates `query::entry`'s unknown-entry lookup but drops the `available entries: …` clause, so the same failure prints two different messages. Fold the lookup into one shared `pub(crate)` helper used by both paths (keep the richer message).
- M5: an injection conflict is currently reported via `AppError::Credential`, so an inject-vs-inject conflict prints under the "credential error" family with no credential involved. Add a dedicated `AppError` variant (e.g. `#[error("injection conflict: {0}")] Injection(String)`) mapped to exit code 4 and use it for conflicts. `tests/security_p3.rs` is read-only and must stay green — its assertions grep source names and exit 4, not the prefix.
- M6: `tests/run_p3.rs`'s signal test bypasses `run_ac` (hand-built command, `.status()`, inherited stdio, no sentinel scan). Rework it to capture output (`.output()` exposes `status.signal()`), plant `helpers::SENTINEL_PLAIN` as the credential value, and assert the sentinel absent from both captured channels.
- M7: the injection test's credential value is `"resolved-value"`, making `run_ac`'s leak scan vacuous. Use `helpers::SENTINEL_PLAIN` as the injected value and compare the probe report without printing either side (see `tests/security_p3.rs` around its report-comparison helper for the pattern).
- M8: `src/runner.rs`'s non-Unix path `status.code().unwrap_or(1)` silently substitutes 1 for an unreportable status; on Windows `code()` is always `Some`, so make the impossibility explicit: `.expect("a Windows process always reports an exit code")`.
- R1 (report, required): the previous run never wrote `.sdd/changes/001-agent-context-cli/reports/task-T007-report.md`. Write it this time per the brief's Report Contract, and include: an Impact Delta section re-verifying the brief's seeds (note the `InjectionPlan` 0-call-site seed is expected — it is the symbol you introduce), a note that CLI wiring lives in `src/cli/query_cmds.rs` rather than a new `src/cli/run.rs` (reviewed and accepted: `QueryCommand` is the single clap enum and already hosts `credential set`; do NOT rename the module in this task), and a note that an explicitly empty program string exits 1 as a usage error by deliberate choice (reviewer finding 9, no change required).

## Constraints

- READ-ONLY: `tests/security_p3.rs`, the whole `tests/helpers/` directory, `tests/security_p1.rs`, `tests/query_p1.rs`, `tests/credential_p2.rs`, all pre-existing fixtures (you may add fixture files). All global constraints from the T007 brief bind verbatim. English only.
- Scope: only the files the base patch touches plus `src/config/validate.rs` (I3), `src/error.rs` (M5), `src/query.rs` or a shared location (M4), new fixtures for I2, and the report. Nothing else.

## Verification (record actual output tails in your report)

- `cargo test --features test-keychain --all-targets` — ALL suites green including `security_p3` (3) and the strengthened `run_p3`.
- `cargo clippy --all-targets -- -D warnings` AND `cargo clippy --all-targets --features test-keychain -- -D warnings` — both clean.
- `cargo fmt --check` — clean.
- `cargo build --release` — succeeds; `cargo build --release --features test-keychain` — fails on the `compile_error!` guard (record as "failed as required", never "pass").

## Worker Execution Boundary

You are a leaf executor: complete this task directly; never spawn subagents or delegate.
