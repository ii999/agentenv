# Task T007 — Injection planner and `run` (impl-standard, TDD)

Read the full brief first: `.sdd/changes/001-agent-context-cli/briefs/task-T007-brief.md` (in this repo/worktree). Everything there binds: task text, global constraints (verbatim), impact map re-verification, no-go list, report contract.

## Addendum (binding, from the T006 red suite)

The paired TDD suite `tests/security_p3.rs` was authored red-first and is a READ-ONLY contract — your implementation must turn it green without modifying it (the read-only set is `tests/security_p3.rs`, the whole `tests/helpers/` directory, `tests/security_p1.rs`, `tests/query_p1.rs`, `tests/credential_p2.rs`, and all pre-existing fixtures; you may add files, and `tests/run_p3.rs` is yours to create for the non-conflict ACs).

Contract details the suite fixes (documented in its module docs — read them):

1. **Conflict diagnostic naming**: exit 4, stderr names BOTH conflicting sources. A credential source is named by its credential name; an `inject`-table source is named as the dotted `<entry>.inject.<KEY>` path (same rendering `list <entry>` uses for inject members). Emit exactly these forms.
2. **Conflict strictly before resolution**: `InjectionPlan::build` performs collect → dedup by effective (ENV, credential) pair → conflict check under platform name identity, touching NO provider. The suite proves this with a canary command provider (`tests/fixtures/canary_provider.sh` creates a file when executed; conflict rows assert the file is absent).
3. **Dedup**: one credential referenced multiple times resolves exactly once — counted via `tests/fixtures/counting_provider.sh` (appends one line per execution).
4. **Child-env observation**: the `test-probe` binary (`tests/fixtures/bin/probe.rs`, `[[bin]] name = "test-probe"`, already in Cargo.toml) writes `argv\t<value>` and `env\t<NAME>=<VALUE>` records to the file named by `TEST_PROBE_OUT`, prints `out`/`err` markers, exits per `TEST_PROBE_EXIT`. The suite launches it as the `run` target.
5. Two AC-018.1 exit-1 rows currently go red on their message token (an unknown `run` subcommand already exits 1); with `run` implemented they must exit 1 with the real usage messages the suite greps for.

Implementation constraints from the spec/architecture (in addition to the brief):

- Unix launch is `exec` (process replacement) after building the child environment map; never mutate agent-context's own environment; resolved secrets go only into that constructed map. Windows path is spawn+wait (code it per spec; it will not be machine-verified here).
- Exit 127 exactly and only for target-not-executable (`run`); distinguish it from exit 4 (credential resolution failure) and exit 1 (usage).
- All CLI output English; no secret bytes in any output; the target's inherited channels are outside the no-secret invariant by design — do not add filtering there.

## Verification (record actual output tails in your report)

- `cargo test --features test-keychain --all-targets` — ALL suites green, including previously red `security_p3` (3 tests) and your new `tests/run_p3.rs`.
- `cargo clippy --all-targets -- -D warnings` AND `cargo clippy --all-targets --features test-keychain -- -D warnings` — both clean.
- `cargo fmt --check` — clean.
- `cargo build --release` — succeeds; `cargo build --release --features test-keychain` — fails on the `compile_error!` guard (record as "failed as required", never "pass").

## Worker Execution Boundary

You are a leaf executor: complete this task directly; never spawn subagents or delegate.
