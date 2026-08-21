# Tasks: agent-context CLI

## Source Artifacts

- Change ID: 001-agent-context-cli
- Plan: plan.md
- Spec: spec.md

## Execution Rules

- Use local files under `.sdd/changes/001-agent-context-cli/` for all workflow state.
- Mark a task complete only after its verification and task review pass.
- `[P]` means parallel-safe because the task touches independent files or subsystems.
- TDD is exceptional: only tasks marked `TDD: yes` use it, each citing a necessity trigger and a paired test-authoring task. Implementers never self-decide TDD; if implementation reveals a necessity trigger mid-task, escalate to the controller.
- For a `TDD: yes` task, the paired test task's failing suite is checkpointed first and is a read-only contract for the implementer.
- Acceptance criteria are the primary gate. Global Constraints in plan.md bind every task.

## Dispatch Preference

Full tier: `Dispatch: agent` is the default. Reviews run per-task (every task here touches a security-sensitive surface or is a review/validation task; SPEC-019's suite-wide invariant spans all of them).

## Task Groups

### Group 1: Foundation

- [x] T001 [Foundation] Establish the crate: manifest, error spine, CLI shell
  - Files: `Cargo.toml`, `src/main.rs`, `src/lib.rs`, `src/error.rs`, `rustfmt.toml` (defaults), `.gitignore` (append `/target` if absent)
  - Depends on: none
  - Spec refs: SPEC-018 (exit-code spine); plan Global Constraints (dependencies)
  - Acceptance refs: groundwork for AC-018.1
  - Interfaces: Produces: `agent_context::error::AppError` (thiserror enum with variants `Usage(String)`, `Config(Vec<Violation>)`, `NotFound(String)`, `Credential(String)`, `TargetNotExecutable(String)`; `pub struct Violation { pub path: String, pub message: String }`; `impl AppError { pub fn exit_code(&self) -> i32 }` returning 1/2/3/4/127); `fn main()` dispatching a clap 4 derive `Cli` with global `--profile <NAME>` and `--json` flags, subcommand enum stub containing only `Version` handling via clap's built-in `--version`/`--help`, argument errors remapped to exit 1, unknown/no subcommand printing help and exiting 1
  - Impact seeds: none
  - No-go: .sdd/, .claude/
  - TDD: no
  - Dispatch: agent (bounded scaffold with exact deliverables)
  - Verification: `cargo build --all-targets && cargo clippy --all-targets -- -D warnings && cargo fmt --check && ./target/debug/agent-context --version` — expected: clean build/lint/fmt; version line `agent-context 0.1.0`; `./target/debug/agent-context; echo $?` prints help then `1`
  - Report: `.sdd/changes/001-agent-context-cli/reports/task-T001-report.md`

Checkpoint: `cargo build` succeeds from a clean clone; the binary runs, reports its version, and exits 1 with help on no subcommand.

### Group 2: Phase 1 (P1, MVP) — Config core & queries

- [x] T002 [Phase 1] Author the Phase-1 security test suite (red)
  - Files: `tests/helpers.rs`, `tests/security_p1.rs`, `tests/fixtures/` (fixture TOML files incl. the design-example config translated to `profiles.*`, a sentinel-bearing malformed-TOML file, sensitive-name fixtures incl. `records = [{ api_key = "sk-sentinel-a1" }]` and uppercase `TOKEN`), `Cargo.toml` (dev-dependencies only: `assert_cmd`, `predicates`, `tempfile`)
  - Depends on: T001
  - Spec refs: SPEC-019 (AC-019.1, AC-019.3), SPEC-020 (AC-020.1–6), SPEC-002 (AC-002.5)
  - Acceptance refs: AC-019.1, AC-019.3, AC-020.1, AC-020.2, AC-020.3, AC-020.4, AC-020.5, AC-020.6, AC-002.5
  - Interfaces: Consumes: the built `agent-context` binary via `assert_cmd`; Produces: `tests/helpers.rs` — `pub const SENTINELS: &[&str]`; `pub struct Run { pub stdout: String, pub stderr: String, pub code: Option<i32> }`; `pub fn run_ac(config: &std::path::Path, envs: &[(&str, &str)], args: &[&str]) -> Run` (scrubs `AGENT_CONTEXT_*`/`XDG_*`/`HOME` first, sets `AGENT_CONTEXT_FILE` to `config`, executes the binary, then asserts no member of `SENTINELS` appears in stdout or stderr before returning)
  - Impact seeds: none
  - No-go: src/ (tests only; the binary is exercised, never edited)
  - TDD: n/a (this IS the paired test task for T004)
  - Dispatch: agent (orchestrator-equivalent capability — high-capability route; test authorship separated from implementation per policy)
  - Verification: `cargo test --test security_p1 2>&1 | tail -5` — expected: every test fails with assertion failures on exit codes/output (e.g. expected exit 2, binary exits 1 from the unimplemented-subcommand shell), zero compilation or collection errors
  - Report: `.sdd/changes/001-agent-context-cli/reports/task-T002-report.md`

- [ ] T003 [Phase 1] Config core: locate, parse, validate, profile, path grammar, shallow status
  - Files: `src/config/mod.rs`, `src/config/locate.rs`, `src/config/model.rs`, `src/config/validate.rs`, `src/path.rs`, `src/shallow.rs`, `src/lib.rs` (module wiring), unit tests inline per module
  - Depends on: T001
  - Spec refs: SPEC-001, SPEC-002 (all 11 rules + diagnostics rule), SPEC-004, SPEC-005, SPEC-012 (grammar + shallow status), SPEC-013 (validation), SPEC-020
  - Acceptance refs: AC-001.1, AC-001.2, AC-001.3, AC-001.4, AC-002.1, AC-002.2, AC-002.3, AC-002.4, AC-002.6, AC-002.7, AC-002.8, AC-004.1, AC-004.2, AC-004.3, AC-005.1, AC-005.2, AC-005.3, AC-012.3, AC-012.4, AC-013.1, AC-013.2, AC-013.3, AC-013.4, AC-013.5, AC-020.1, AC-020.2, AC-020.3, AC-020.4, AC-020.5, AC-020.6 (logic level; CLI-level assertions turn green in T004)
  - Interfaces: Consumes: `AppError`, `Violation` (T001). Produces: `config::Config { pub version: i64, pub profiles: toml::Table-backed ordered model, pub credentials: indexed ordered model }`; `Config::load(explicit_file: Option<&Path>, env: &impl Fn(&str) -> Option<String>) -> Result<Config, AppError>` (aggregates ALL violations into `AppError::Config(Vec<Violation>)`; empty env values treated as unset); `Config::select_profile(&self, flag: Option<&str>, env_val: Option<&str>) -> Result<&Profile, AppError>`; `path::Segments` with `Segments::parse(&str) -> Result<Segments, AppError>` and `Segments::render(&self) -> String` (round-trip property); `path::resolve<'a>(profile: &'a Profile, segs: &Segments) -> Result<&'a toml::Value, AppError>`; `shallow::Status` enum (`Available`, `NotSet`, `Configured`, `CommandMissing`) with `Status::json_token(&self) -> &'static str` and `pub fn shallow_status(cred: &CredentialDef, env: &impl Fn(&str) -> Option<String>) -> Status` (env presence / `configured` / executable discovery per SPEC-012 — no store read, no process launch); `CredentialRef { pub name: String, pub target_override: Option<String> }` with `CredentialRef::parse(&str) -> Result<CredentialRef, String>` (strict grammar)
  - Impact seeds: AppError, Violation
  - No-go: tests/security_p1.rs, tests/helpers.rs (read-only contract), .sdd/
  - TDD: no (module-level unit tests written with the code)
  - Dispatch: agent (impl-standard: multi-module core)
  - Verification: `cargo test --lib && cargo clippy --all-targets -- -D warnings` — expected: all unit tests green (grammar table, validation table incl. AC-002.6/.7/.8 cases, reference grammar AC-012.4 cases, sensitive-name matrix incl. array-nested and uppercase, inject rules AC-013.1–5), clippy clean
  - Report: `.sdd/changes/001-agent-context-cli/reports/task-T003-report.md`

- [ ] T004 [Phase 1] Query engine, renderer, and CLI for the read-only surface
  - Files: `src/query.rs`, `src/render.rs`, `src/cli/mod.rs`, `src/cli/query_cmds.rs`, `src/main.rs` (wire subcommands `list`, `show`, `get`, `find`, `validate`, `credential list`), `tests/query_p1.rs` (AC-driven integration tests for the non-security Phase-1 ACs), `tests/snapshots/` (JSON shape snapshots)
  - Depends on: T003, T002
  - Spec refs: SPEC-003, SPEC-006, SPEC-007, SPEC-008, SPEC-009, SPEC-010, SPEC-011, SPEC-015 (`credential list` half), SPEC-018 (codes 1/2/3), SPEC-021
  - Acceptance refs: AC-003.1, AC-003.2, AC-006.1, AC-006.2, AC-006.3, AC-006.4, AC-007.1, AC-007.2, AC-007.3, AC-008.1, AC-008.2, AC-008.3, AC-008.4, AC-009.1, AC-009.2, AC-009.3, AC-009.4, AC-009.5, AC-010.1, AC-010.2, AC-010.3, AC-010.4, AC-011.1, AC-011.2, AC-011.3, AC-012.1, AC-012.2, AC-012.5, AC-015.1, AC-018.1, AC-018.2, AC-021.1; turns the T002 suite green (AC-002.5, AC-019.1, AC-019.3, AC-020.1, AC-020.2, AC-020.3, AC-020.4, AC-020.5, AC-020.6); edge cases EDGE-001, EDGE-002, EDGE-003, EDGE-008, EDGE-009, EDGE-010, EDGE-014, EDGE-015, EDGE-016, EDGE-017, EDGE-019, EDGE-020
  - Interfaces: Consumes: every T003 signature above; `run_ac` helper and fixtures from T002 (read-only contract — do not modify `tests/helpers.rs`, `tests/security_p1.rs`, or `tests/fixtures/` except to ADD fixture files). Produces: `query::EntryView`/`Listing`/`Matches` (no variant can hold a secret); `render::text(&View) -> String`, `render::json(&View) -> serde_json::Value` (SPEC-010 shapes exactly: envelopes with `version`; `Field` recursion with nested `fields`; `reference` on credential_ref; `addressable`/`key`/`path: null` markers; raw `get --json`)
  - Impact seeds: Config::load, Config::select_profile, Segments::parse, path::resolve, shallow_status, run_ac
  - No-go: tests/security_p1.rs, tests/helpers.rs, .sdd/
  - TDD: yes (security-sensitive boundary — query surface must never emit secrets and must refuse sensitive-name configs at load; tests: T002)
  - Dispatch: agent (impl-standard: largest single surface)
  - Verification: `cargo test --all-targets && cargo clippy --all-targets -- -D warnings && cargo fmt --check` — expected: ALL tests green including the previously red `security_p1` suite; snapshot files created for the six SPEC-010 shapes
  - Report: `.sdd/changes/001-agent-context-cli/reports/task-T004-report.md`

Checkpoint: Phase 1 acceptance passes — on the design-example fixture every query command produces the documented text and JSON with exit codes 1/2/3, a plaintext-secret config is unusable by every command, and the sentinel grep helper guards every invocation. Observable: `cargo test` green; `AGENT_CONTEXT_FILE=tests/fixtures/example.toml ./target/debug/agent-context list`.

### Group 3: Phase 2 (P2) — Credential providers

- [ ] T005 [Phase 2] Credential module, providers, and `credential check`/`set`
  - Files: `src/credential/mod.rs`, `src/credential/secret.rs`, `src/credential/env.rs`, `src/credential/keychain.rs`, `src/credential/command.rs`, `src/credential/test_store.rs` (cfg-gated), `src/cli/credential.rs`, `src/main.rs` (wire `credential check|set`), `Cargo.toml` (`keyring`, `rpassword`, `[features] test-keychain = []`), `src/shallow.rs` (absorb into `credential::shallow_status`, updating T004 call sites), `tests/credential_p2.rs`, `tests/fixtures/bin/` (fixture provider scripts: sentinel-then-exit-1, newline-only, invalid-UTF-8-sentinel, argv-recorder, counting provider)
  - Depends on: T004
  - Spec refs: SPEC-014, SPEC-015 (check/set), SPEC-018 (code 4), SPEC-019 (AC-019.2, captured-bytes clause), SPEC-AS-016/-019
  - Acceptance refs: AC-014.1, AC-014.2, AC-014.3, AC-014.4, AC-014.5, AC-015.2, AC-015.3, AC-015.4, AC-015.5, AC-018.1, AC-019.2; edge cases EDGE-006, EDGE-011, EDGE-012, EDGE-013
  - Interfaces: Consumes: `CredentialDef`, `CredentialRef`, `Status`, `AppError` (T003/T004 signatures). Produces: `credential::CapturedSecret(Vec<u8>)` and `credential::Secret(String)` — neither implements `Display`/`Serialize`, both redact in `Debug`; `CapturedSecret::into_secret(self) -> Result<Secret, SecretDomainError>` (empty/whitespace-only/NUL/invalid-UTF-8 rejected, one trailing newline stripped, error carries no bytes); `trait Provider { fn shallow_status(&self) -> Status; fn resolve(&self) -> Result<Secret, AppError>; fn store(&self, value: Secret) -> Result<(), AppError>; }`; `pub fn provider_for(def: &CredentialDef) -> Box<dyn Provider>`; test store: `#[cfg(all(feature = "test-keychain", debug_assertions))]` file-backed adapter selected by `AGENT_CONTEXT_TEST_KEYCHAIN=<path>`, plus `#[cfg(all(feature = "test-keychain", not(debug_assertions)))] compile_error!(...)`
  - Impact seeds: shallow_status, run_ac, AppError::Credential
  - No-go: tests/security_p1.rs, tests/helpers.rs, .sdd/
  - TDD: no (behavior pinned by AC-driven tests written with the code; the no-leak property is already enforced per-invocation by the T002 helper)
  - Dispatch: agent (impl-standard: security-core module)
  - Review: per-task
  - Verification: `cargo test --features test-keychain --all-targets && cargo build --release 2>&1 | tail -2` — expected: all green including AC-015.2 byte-exact round-trip and AC-014.5 sentinel-free failures; release build (no feature) succeeds, and `cargo build --release --features test-keychain` FAILS with the compile_error message
  - Report: `.sdd/changes/001-agent-context-cli/reports/task-T005-report.md`

Checkpoint: Phase 2 acceptance passes — `credential check` reports per-provider success/failure without ever printing a value, `set` round-trips exact bytes through the test store, exit 4 paths live. Observable: `cargo test --features test-keychain` green; release-with-feature build refuses to compile.

### Group 4: Phase 3 (P3) — Injection runner and docs

- [ ] T006 [Phase 3] Author the conflict-before-resolution test suite (red)
  - Files: `tests/security_p3.rs`, `tests/fixtures/` (add: canary provider script that creates a file when executed; probe helper usage docs in comments), `tests/fixtures/bin/probe.rs` + `Cargo.toml` `[[bin]] name = "test-probe"` (a tiny binary that writes its env and argv to the file named by `TEST_PROBE_OUT`, prints `out`/`err` markers, exits per `TEST_PROBE_EXIT`)
  - Depends on: T005
  - Spec refs: SPEC-016 (AC-016.2 canary half, AC-016.9 matrix), SPEC-019
  - Acceptance refs: AC-016.2, AC-016.9 (Unix-runnable rows), AC-018.1
  - Interfaces: Consumes: `run_ac` (T002), fixture script conventions (T005). Produces: `tests/security_p3.rs` — table-driven injection-plan matrix asserting exit 4 + both source names in stderr + canary file absent for every conflict row, and dedup rows asserting success + single provider invocation via the counting provider; `test-probe` binary contract as described
  - Impact seeds: run_ac
  - No-go: src/ (tests only)
  - TDD: n/a (this IS the paired test task for T007)
  - Dispatch: agent (orchestrator-equivalent capability — high-capability route)
  - Verification: `cargo test --features test-keychain --test security_p3 2>&1 | tail -5` — expected: every test fails by assertion (the `run` subcommand does not exist yet → usage exit 1 where 4/0 expected); zero compile/collection errors
  - Report: `.sdd/changes/001-agent-context-cli/reports/task-T006-report.md`

- [ ] T007 [Phase 3] Injection planner and `run`
  - Files: `src/runner.rs`, `src/cli/run.rs`, `src/main.rs` (wire `run`), `tests/run_p3.rs` (non-conflict ACs)
  - Depends on: T006
  - Spec refs: SPEC-016, SPEC-017, SPEC-018 (127), SPEC-AS-011/-012/-013/-018
  - Acceptance refs: AC-016.1, AC-016.2, AC-016.3, AC-016.4, AC-016.5, AC-016.6, AC-016.7, AC-016.8, AC-016.9, AC-017.1, AC-017.2, AC-017.3, AC-018.1; edge cases EDGE-004, EDGE-005, EDGE-018; turns the T006 suite green
  - Interfaces: Consumes: T003–T005 signatures; `test-probe` and canary/counting fixtures (T006; `tests/security_p3.rs` is a read-only contract). Produces: `runner::InjectionPlan` with `InjectionPlan::build(cfg: &Config, profile: &Profile, entries: &[String]) -> Result<InjectionPlan, AppError>` (collect → dedup by effective pair → conflict-check under platform name identity → NO provider touched yet) and `InjectionPlan::resolve_and_launch(self, cmd: Vec<String>) -> Result<Infallible, AppError>` (resolve each credential once, build the child env map — never mutate agent-context's own env — then Unix `exec` / Windows spawn+wait)
  - Impact seeds: InjectionPlan, provider_for, CredentialRef, run_ac
  - No-go: tests/security_p3.rs, tests/helpers.rs, tests/security_p1.rs, .sdd/
  - TDD: yes (security-sensitive boundary — conflict detection strictly before provider resolution; tests: T006)
  - Dispatch: agent (impl-standard)
  - Review: per-task
  - Verification: `cargo test --features test-keychain --all-targets && cargo clippy --all-targets -- -D warnings && cargo fmt --check` — expected: ALL suites green including previously red `security_p3`; AC-017.1 byte assertions and AC-017.3 signal(15) pass on macOS
  - Report: `.sdd/changes/001-agent-context-cli/reports/task-T007-report.md`

- [ ] T008 [Phase 3] README per SPEC-022
  - Files: `README.md`
  - Depends on: T007 (documents the final surface)
  - Spec refs: SPEC-022
  - Acceptance refs: AC-022.1
  - Interfaces: Consumes: final CLI surface (run `--help` output for accuracy). Produces: `README.md` with sections: overview; config schema by example (the design-example TOML); agent usage protocol (the six-bullet AGENTS.md snippet verbatim from design-source.md §7, plus discover → inspect → get → `run --with` flow and the no-guessing rule); threat model (SPEC-019 boundary incl. `run`-target/provider-stderr carve-outs); provider guidance (prefer `keychain`/`command` locally; `env` readable by any inheriting process); sensitive-check guardrail caveat; target-name discovery (`inject_as` via `credential list`, `?as=` via JSON `reference`/`get`); Windows support statement (specified, not machine-verified in v1)
  - Impact seeds: none
  - No-go: src/, tests/
  - TDD: no
  - Dispatch: agent (impl-bounded documentation with a fixed checklist)
  - Verification: `grep -c '^' README.md && grep -n 'agent-context list --json' README.md` — expected: non-trivial line count; the snippet's first command present; orchestrator reviews against the AC-022.1 checklist
  - Report: `.sdd/changes/001-agent-context-cli/reports/task-T008-report.md`

Checkpoint: Phase 3 acceptance passes — `run --with llm -- <probe>` injects exactly the documented variables with conflict/dedup/precedence semantics and full process transparency; README ships the complete agent protocol. Observable: full `cargo test --features test-keychain` green.

### Group 5: Validation

- [ ] T900 [Validation] Run acceptance validation and update validation report
  - Files: `.sdd/changes/001-agent-context-cli/validation.md`
  - Depends on: T001–T008
  - Spec refs: all
  - Acceptance refs: all (incl. manual rows: macOS Keychain round-trip, cold-start < 100 ms, README checklist, release-guard negative check, SPEC-AS-025 risk surfacing)
  - Impact seeds: none
  - No-go: none
  - TDD: no
  - Dispatch: agent (final validation on the host high-capability route, resolved natively)
  - Verification: `python3 <package-root>/scripts/sdd.py verify 001-agent-context-cli --compare-baseline --update-validation` — expected: all verification commands green or triaged; validation.md complete
  - Report: `.sdd/changes/001-agent-context-cli/reports/task-T900-report.md`

Checkpoint: All acceptance criteria pass or carry recorded deferrals; `validation.md` is complete.

## Dependency Notes

- Strict chain T001→T002→T003→T004→T005→T006→T007→T008→T900; single crate, shared test helper — no concurrent implementation dispatch.
- T002 and T006 are the paired test-authoring tasks; their suites are checkpointed red before T004/T007 dispatch and are read-only contracts thereafter.

## Parallel Dispatch Notes

- Tasks safe to dispatch together: none (serialized by design).
- Tasks that must be serialized: all.
- Shared files requiring controller integration: `src/main.rs`, `Cargo.toml`, `tests/helpers.rs` (each touched by multiple sequential tasks).

## Dispatch Grouping

- Routed to agents: T001 (impl-bounded), T003/T004/T005/T007 (impl-standard), T008 (impl-bounded) — provider ladder per route-policy.json, external workers under provider-host affinity.
- Expected native by host affinity: T002, T006 (test authoring at orchestrator-equivalent capability → claude high-cap native), T900 (host high-cap native).
- Expected external worker: T001, T003, T004, T005, T007, T008 (ladder heads are non-claude providers).
- Keep inline: none.
- Frontend tasks actively dispatched: none (no UI surface).

## Coverage

| Spec / Acceptance ID | Task IDs | Notes |
| --- | --- | --- |
| SPEC-001 (AC-001.*) | T003, T004 | locate logic / CLI-level tests |
| SPEC-002 (AC-002.*) | T003, T004, T002 | AC-002.5 authored in T002 |
| SPEC-003 (AC-003.*) | T004 | |
| SPEC-004 (AC-004.*) | T003, T004 | |
| SPEC-005 (AC-005.*) | T003, T004 | |
| SPEC-006..009, 021 (AC-006–9, 021) | T004 | |
| SPEC-010 (AC-010.1–4) | T004 | snapshots frozen here |
| SPEC-011 (AC-011.*) | T003, T004 | |
| SPEC-012 (AC-012.*) | T003, T004, T007 | AC-012.5 run-half = AC-016.7 |
| SPEC-013 (AC-013.*) | T003 | |
| SPEC-014 (AC-014.*) | T005 | |
| SPEC-015 (AC-015.1 / .2–5) | T004 / T005 | |
| SPEC-016 (AC-016.*) | T006, T007 | conflict suite authored in T006 |
| SPEC-017 (AC-017.*) | T007 | |
| SPEC-018 (AC-018.1–2) | T001, T004, T005, T007 | per-phase rows |
| SPEC-019 (AC-019.1–3) | T002, T004, T005 | helper in T002; AC-019.2 in T005 |
| SPEC-020 (AC-020.*) | T002, T003, T004 | |
| SPEC-022 (AC-022.1) | T008 | |
| Validation / manual rows | T900 | |
