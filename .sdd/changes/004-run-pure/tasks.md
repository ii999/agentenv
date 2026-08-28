# Tasks: 004-run-pure

## Strategy

Approach: implement pure-environment selection as a unit-testable seam inside `src/runner.rs` (the module that already owns child-environment construction), wire it through a small `EnvironmentMode` value passed from the CLI, and keep every diagnostic on the already-established channels. All tasks run inline (light-tier default); one Checkpoint review gates the whole group before checkpointing.

Key constraints from the spec:

- `--keep` validation runs in `src/main.rs` immediately after argument parsing and before the project prelude, because clap's native `invalid value` error echoes the raw token — which AC-004.2 forbids when the token contains `=` — and because SPEC-002 orders validation before project discovery. Errors surface as `AppError::Usage` (exit 1) with next-action diagnostics.
- The missing-keep report is computed and flushed to stderr by the `EnvironmentMode::pure` constructor, which the run handler invokes before `InjectionPlan::build`. (Adjusted during T003: the report was originally placed inside `resolve_and_launch`, but conflict detection runs in `build`, so the exit-4 path would have skipped it — SPEC-002 requires the report on every outcome, and AC-002.6 now locks the ordering in.) This also orders it after the untrusted-project notice and guarantees it survives Unix `exec`.
- Environment selection operates on `OsString` pairs end to end. A parent name that is not valid Unicode cannot equal any base/keep name and is dropped; values are never converted. The selection seam is a pure function over an iterator of pairs so non-UTF-8 cases are unit-testable without touching the process environment.
- The base lists are `const` name tables (`cfg`-selected per platform) matching the spec exactly; membership, keep matching, dedup, and injection override all go through the existing `same_environment_name` equivalence.
- `is_valid_env_name` must become reachable from the binary crate (promote to `pub` with a re-export on the library's config surface).

Verification plan: `cargo build`, `cargo fmt --check`, `cargo test` (full suite; 223 pre-existing tests must pass unmodified), plus the new unit tests at the selection seam and a new `tests/run_pure.rs` integration suite covering every automated AC. Checkpoint review: one high-capability native code-review lane over the complete diff before the group checkpoint. Manual ACs (AC-005.1..3) verified by comparison during validation.

## Tasks

### T001: Pure environment selection seam in the library

Dispatch: inline
Status: complete

- Add per-platform `const` base-name tables to `src/runner.rs` matching SPEC-001 exactly (Unix list including the 13 locale names and `AGENTENV_*`/`XDG_*` controls; Windows list including `CommonProgramFiles(x86)` and `AGENTENV_*`).
- Add an `EnvironmentMode` value (`Inherit` | `Pure { keep: Vec<String> }`) to the library's run surface.
- Implement the selection seam: a pure function taking the parent pairs and the mode, returning the selected pairs plus the ordered list of missing keep names; base/keep matching via `same_environment_name` semantics, parent spelling and `OsString` value preserved, duplicate keeps collapsed.
- Thread the mode through `resolve_and_launch`: compute selection and missing keeps first, write and flush one stderr line per missing keep (naming the variable and the corrective action) before any credential resolution, then apply the existing injection-target filter and injection append unchanged.
- Promote `is_valid_env_name` to the library's public config surface.
- Unit tests at the seam: non-UTF-8 value carried unchanged, non-UTF-8 name dropped, unlisted `LC_` name excluded, keep dedup, injection-target override.

Acceptance: AC-001.2, AC-001.3, AC-001.6 (seam level), EDGE-005; compiles with the CLI still passing `Inherit`.

### T002: CLI flags, parse-time validation, wiring

Dispatch: inline
Status: complete
Depends on: T001

- Extend `RunArgs` with `--pure` and repeatable `--keep <NAME>` (help text per SPEC-005).
- Post-parse validation in `src/main.rs` before the prelude: `--keep` without `--pure`, invalid name (via the promoted validity helper), or empty name → `AppError::Usage` (exit 1); when a token contains `=`, the diagnostic reproduces nothing at or after the first `=`; all diagnostics carry a next action.
- Pass the mode into `resolve_and_launch` from the run handler.

Acceptance: AC-002.3, AC-002.4, AC-004.2, AC-004.3 (behavioral paths in place).

### T003: Integration tests

Dispatch: inline
Status: complete
Depends on: T002

- New `tests/run_pure.rs` using the existing probe fixture: AC-001.1, AC-001.4, AC-001.5, AC-001.8 (nested `agentenv` via the cargo-built binary as target), AC-002.1, AC-002.2, AC-002.5, AC-002.6, AC-003.2, AC-003.3, EDGE-001 (unfindable target), EDGE-002, EDGE-003, EDGE-004.
- Sentinel security tests for AC-004.1 and AC-004.2 following `tests/project_security.rs` patterns.
- Windows-marked coverage for AC-001.7/EDGE-006 (`cfg(windows)` test; CI Windows job is the arbiter).
- Pre-existing suite untouched (AC-003.1).

Acceptance: every automated AC in the matrix has a named test.

### T004: Documentation

Dispatch: inline
Status: complete
Depends on: T002

- README "Running with injected values": document `--pure`, `--keep`, both base lists, layering order, and the TLS/proxy omission with the `--keep` recipe (AC-005.1).
- README "Safety and threat model": filter-not-sandbox statement and unchanged provider-subprocess environment (AC-005.3).
- `skills/agentenv/SKILL.md`: mention `--pure` and the boundary where `run` is described (AC-005.3).
- Help text verified against AC-005.2.

Acceptance: AC-005.1..3 ready for manual validation.

## Checkpoint

Group checkpoint after T001-T004: high-capability native code review of the full diff, full verification suite green, then `sdd.py checkpoint` with label `tasks T001-T004 - run --pure implementation`.
