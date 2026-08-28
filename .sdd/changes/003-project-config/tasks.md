# Tasks: Project-Scoped Configuration

## Source Artifacts

- Change ID: 003-project-config
- Plan: `.sdd/changes/003-project-config/plan.md`
- Spec: `.sdd/changes/003-project-config/spec.md`

## Execution Rules

- Use local files under `.sdd/changes/003-project-config/` for all workflow state.
- Mark a task complete only after its verification and task review pass.
- `[P]` means parallel-safe because the task touches independent files or subsystems.
- TDD is exceptional: only T005 is `TDD: yes` (security-sensitive boundary), paired with test task T004. Implementers never self-decide TDD; if implementation reveals a necessity trigger mid-task, escalate to the controller.
- For T005, T004's failing suite is checkpointed first and is a read-only contract for the implementer.
- Acceptance criteria are the primary gate. The plan's Global Constraints section applies verbatim to every task.

## Authoring Rules

Per `docs/authoring-discipline.md`: exact paths, exact signatures, verification with expected output, `Interfaces:`/`Impact seeds:`/`No-go:` per task; no placeholder phrases.

## Dispatch Preference

Full tier: `Dispatch: agent` is the default. Reviews run at Checkpoint granularity by default; tasks touching the security-sensitive selection/trust surface carry `Review: per-task`.

## Task Groups

### Group 1: Foundation

- [x] T001 [Foundation] Make every test invocation hermetic to project discovery
  - Files: `tests/helpers/mod.rs`, `tests/run_p3.rs`, `tests/credential_p2.rs`
  - Depends on: none
  - Spec refs: SPEC-009
  - Acceptance refs: AC-009.1, AC-009.2
  - Task: In the shared helper (`tests/helpers/mod.rs`), make every constructed command (a) set `current_dir` to a temp directory the test controls (default: the per-test temp dir that already holds the fixture config; add one where absent) and (b) pass environment `AGENTENV_NO_PROJECT=1`. Add a helper constructor variant that omits `AGENTENV_NO_PROJECT` for future project-behavior tests (name it `command_with_project_discovery`, returning the same command type). Apply the same two properties to every direct binary invocation that bypasses the helper: the PTY/signal invocations in `tests/run_p3.rs` and the stdin invocations in `tests/credential_p2.rs` (search for `Command::new` / `CommandBuilder` in those files). Change no test assertion.
  - Interfaces: Produces: `command_with_project_discovery()` in `tests/helpers/mod.rs` (same return type as the existing command constructor).
  - Impact seeds: `run_ac`, `Command::new`, `CommandBuilder`
  - No-go: `src/`
  - TDD: no
  - Dispatch: agent (mechanical but multi-file; impl-light)
  - Verification: `cargo test` — expected: all existing tests pass with zero `src/` changes (`git status --porcelain -- src/` empty).
  - Report: `.sdd/changes/003-project-config/reports/task-T001-report.md`

Checkpoint: `cargo test` green with the suite fully hermetic; a `.agentenv.toml` placed in the repo root or `$HOME` no longer influences any test (observable: create one temporarily, suite still green).

### Group 2: Phase 1 leaf modules

- [ ] T002 [P] [Phase 1] Implement the closed project-file schema (`project::model`)
  - Files: `src/project/mod.rs` (create, module declarations only for now), `src/project/model.rs` (create), `src/lib.rs` (add `pub mod project;`), `tests/project_schema.rs` (create), `tests/fixtures/project/` (create fixture `.toml` files per violation class)
  - Depends on: none
  - Spec refs: SPEC-002; SPEC-010 (no-echo diagnostics)
  - Acceptance refs: AC-002.1, AC-002.2, AC-002.3, AC-002.4, AC-002.5, AC-002.6, AC-002.7, AC-010.1 (violation-message half)
  - Task: Implement parsing and validation of `.agentenv.toml` content per SPEC-002's requirement paragraph (read it in full; it is the contract). Public surface in `src/project/model.rs`:
    - `pub const MAX_PROJECT_FILE_BYTES: usize = 65536;`
    - `pub struct ProjectPin { pub name: String, pub file: std::path::PathBuf }`
    - `pub struct Requirement { pub entry: String, pub reason: String, pub fields: Vec<String> }`
    - `pub struct ProjectFileMeta { pub pin: Option<ProjectPin>, pub requires: Vec<Requirement> }` (`requires` in file declaration order)
    - `pub fn parse(bytes: &[u8], file: &std::path::Path) -> Result<ProjectFileMeta, Vec<crate::error::Violation>>` — size check first (over-limit ⇒ one violation naming the file and the 64 KiB limit), then TOML parse (failure ⇒ one violation with parser message + position, no source echoed — match the style of config parse errors in `src/config/mod.rs`), then closed-schema validation. Field-path members are validated with `crate::path::Segments::parse`; requires-entry keys must be single segments. Violations name TOML paths only; never echo values (sentinel discipline per SPEC-010).
  - Interfaces: Produces: `project::model::{parse, ProjectFileMeta, ProjectPin, Requirement, MAX_PROJECT_FILE_BYTES}` as above. Consumes: `crate::path::Segments::parse` (existing), `crate::error::Violation` (existing).
  - Impact seeds: `Segments::parse`, `Violation`
  - No-go: `src/cli/`, `src/config/`, `src/main.rs`, `src/runner.rs`
  - TDD: no
  - Dispatch: agent (impl-standard)
  - Verification: `cargo test --test project_schema` — expected: all pass, covering every SPEC-002 acceptance criterion plus the valid-file case; `cargo test` — expected: no regressions.
  - Report: `.sdd/changes/003-project-config/reports/task-T002-report.md`

- [ ] T003 [P] [Phase 1] Implement project-file discovery (`project::locate`)
  - Files: `src/project/locate.rs` (create), `src/project/mod.rs` (add `pub mod locate;`)
  - Depends on: none (coordinate `src/project/mod.rs` with T002: each task adds only its own `pub mod` line)
  - Spec refs: SPEC-001
  - Acceptance refs: AC-001.1, AC-001.2, AC-001.3 (walk behavior; bypass ACs land with T007)
  - Task: `pub fn discover(cwd: &std::path::Path) -> Option<std::path::PathBuf>` — walk from `cwd` (inclusive) through parents to the filesystem root; return the first path `dir/.agentenv.toml` whose metadata says regular file (`std::fs::metadata(..).map(|m| m.is_file()).unwrap_or(false)` — a directory, dangling symlink, or probe error is not a project file and the walk continues). Never return an error; discovery cannot fail a command. Unit tests in the same file using `tempfile` trees: nested discovery, nearest-wins, dir-named-`.agentenv.toml` skipped, dangling symlink skipped (Unix-gated), no file ⇒ `None`.
  - Interfaces: Produces: `project::locate::discover(cwd: &Path) -> Option<PathBuf>`.
  - Impact seeds: none
  - No-go: `src/cli/`, `src/config/`, `src/main.rs`
  - TDD: no
  - Dispatch: agent (small and isolated; impl-light)
  - Verification: `cargo test project::locate` — expected: new unit tests pass; `cargo test` — expected: no regressions.
  - Report: `.sdd/changes/003-project-config/reports/task-T003-report.md`

- [ ] T004 [Phase 1] Author the failing trust-gate test suite (TDD contract for T005)
  - Files: `src/project/trust.rs` (create: interface skeletons with `todo!()` bodies), `src/project/mod.rs` (add `pub mod trust;`), `tests/project_trust.rs` (create), `Cargo.toml` (add `sha2 = "0.10"` to `[dependencies]`)
  - Depends on: none
  - Spec refs: SPEC-003 (store semantics); EDGE-004b
  - Acceptance refs: AC-003.3, AC-003.7, AC-003.8, AC-003.9, AC-003.10, AC-003.11, AC-003.12, AC-003.13 (store-level halves), AC-003.9
  - Task: Pin the trust-store interface and write its tests so they compile and FAIL (panics from `todo!()` are the expected failure mode at this stage; assertion-level failures take over once T005 stubs turn real). Interface skeletons in `src/project/trust.rs` (bodies `todo!()`):
    - `pub trait StoreFs { fn read(&self, path: &Path) -> std::io::Result<Vec<u8>>; fn write_temp(&self, dir: &Path, bytes: &[u8]) -> std::io::Result<PathBuf>; fn rename(&self, from: &Path, to: &Path) -> std::io::Result<()>; }` plus `pub struct RealFs;` implementing it over `std::fs` (temp files created `0600` on Unix before content is written).
    - `pub fn store_path(env: &impl Fn(&str) -> Option<String>) -> Result<PathBuf, crate::error::AppError>` — `$XDG_STATE_HOME/agentenv/trust.toml`, else `$HOME/.local/state/agentenv/trust.toml`; Windows `%LOCALAPPDATA%\agentenv\trust.toml`; empty env counts as unset; no base ⇒ `AppError::Config` naming the variables and a next action (mirror `src/config/locate.rs` style).
    - `pub struct TrustStore { /* records: canonical path -> hex SHA-256 */ }` with `pub fn load(path: &Path, fs: &dyn StoreFs) -> Result<TrustStore, AppError>` (missing file ⇒ empty store; unparseable ⇒ `AppError::Config` naming the store path and remedy — never empty), `pub fn lookup(&self, canonical: &Path) -> Option<&str>`, `pub fn allow(&mut self, canonical: &Path, content: &[u8])`, `pub fn revoke(&mut self, canonical: &Path) -> bool`, `pub fn save(&self, path: &Path, fs: &dyn StoreFs) -> Result<(), AppError>` (write-temp + rename; atomic).
    - `pub fn fingerprint(content: &[u8]) -> String` (hex SHA-256 via `sha2`).
  - Tests in `tests/project_trust.rs` (plus unit tests in-file where the `StoreFs` seam is needed): fingerprint changes on any byte change (AC-003.3 store half); lookup by canonical path; load-missing ⇒ empty; load-corrupt ⇒ error naming path (AC-003.8); save creates `0600` on Unix (AC-003.9); two allows then revoke-one preserves the other (AC-003.10, AC-003.13 store half); failing `rename` leaves previous bytes intact and errors naming the path (AC-003.11, via a fault-injecting `StoreFs` test adapter); allow-binds-snapshot: allow(content A), replace file content, fingerprint mismatch on re-check (AC-003.12 store half); `store_path` per-platform resolution incl. unset-base error (EDGE-004b half).
  - Interfaces: Produces (as the read-only contract for T005): every signature above, exactly.
  - Impact seeds: none
  - No-go: `src/cli/`, `src/config/`, `src/main.rs`
  - TDD: no (this IS the paired test-authoring task for T005)
  - Dispatch: agent (orchestrator-equivalent capability required for TDD authoring — host high-capability native route)
  - Verification: `cargo test --test project_trust` — expected: compiles; every test FAILS via `todo!()` panic (no compilation errors); `cargo build` — expected: exit 0.
  - Report: `.sdd/changes/003-project-config/reports/task-T004-report.md`

- [ ] T005 [Phase 1] Implement the trust store against the T004 contract
  - Files: `src/project/trust.rs` (fill in the `todo!()` bodies; add private helpers as needed)
  - Depends on: T004 (its tests and signatures are a read-only contract — do not modify `tests/project_trust.rs` or any T004-pinned signature)
  - Spec refs: SPEC-003, SPEC-AS-008; EDGE-004b
  - Acceptance refs: AC-003.3, AC-003.7, AC-003.8, AC-003.9, AC-003.10, AC-003.11, AC-003.12, AC-003.13 (store-level), AC-003.9
  - Task: Implement every T004 skeleton so the T004 suite passes: TOML-serialized record table, atomic save through the `StoreFs` seam (temp file `0600`-before-content on Unix, then rename), snapshot-preserving mutations (load-mutate-save; never drop records present in the loaded snapshot), corrupt-store as explicit `AppError::Config` with store path and remedy, no permission check on load (SPEC-AS-008).
  - Interfaces: Consumes: the exact T004 signatures. Produces: the same, now functional, for T006.
  - Impact seeds: `TrustStore`, `store_path`, `fingerprint`, `StoreFs`
  - No-go: `tests/project_trust.rs`, `src/cli/`, `src/config/`, `src/main.rs`
  - TDD: yes (security-sensitive boundary; tests: T004)
  - Dispatch: agent (impl-standard)
  - Review: per-task
  - Verification: `cargo test --test project_trust` — expected: all pass; `cargo test` — expected: no regressions; `cargo fmt --check` — expected: exit 0.
  - Report: `.sdd/changes/003-project-config/reports/task-T005-report.md`

- [ ] T006 [Phase 1] Implement the project facade (`ProjectContext` resolve / allow / revoke)
  - Files: `src/project/mod.rs` (facade lives here), `tests/project_facade.rs` (create)
  - Depends on: T002, T003, T005
  - Spec refs: SPEC-003 (allow/revoke command halves), SPEC-005 evaluation-order step 2 (the single-snapshot classification — read it in full), SPEC-001 (bypass handled by callers, not here)
  - Acceptance refs: AC-003.1, AC-003.2, AC-003.3, AC-003.4, AC-003.5, AC-003.6, AC-003.7, AC-003.8, AC-003.12, AC-003.13, AC-005.3 (facade half), AC-006.13 (classification), EDGE-004a/b, EDGE-011
  - Task: Public surface in `src/project/mod.rs`:
    - `pub enum UntrustedReason { New, Changed, Invalid(Vec<Violation>), StateUnavailable(String) }`
    - `pub enum ProjectContext { None, Untrusted { path: PathBuf, reason: UntrustedReason, meta: Option<ProjectFileMeta> }, Trusted { path: PathBuf, meta: ProjectFileMeta } }`
    - `pub fn resolve(cwd: &Path, env: &impl Fn(&str) -> Option<String>) -> Result<ProjectContext, AppError>`
    - `pub struct AllowOutcome { pub path: PathBuf, pub already_current: bool }`; `pub fn allow(cwd: &Path, env: &impl Fn(&str) -> Option<String>) -> Result<AllowOutcome, AppError>`
    - `pub struct RevokeOutcome { pub path: PathBuf, pub record_existed: bool }`; `pub fn revoke(cwd: &Path, env: &impl Fn(&str) -> Option<String>) -> Result<RevokeOutcome, AppError>`
  - `resolve` composition (single immutable snapshot; SPEC-005 step 2 verbatim): discover → canonicalize → `store_path`/`TrustStore::load` (corrupt store ⇒ `Err`; unresolvable base ⇒ `Untrusted(StateUnavailable(msg))`) → path-only `lookup` → single read of the file bytes → classify: read/canonicalize failure with approval record ⇒ `Err` (`AppError::Config`, exit 2, message names file + next action: restore the file or `agentenv project revoke`); without record ⇒ `Untrusted(Invalid(vec![read-failure violation]))` → `fingerprint(snapshot)` vs record → `model::parse(snapshot)` — validation failure ⇒ `Invalid(violations)` regardless of fingerprint result (invalid outranks changed); parse OK + fingerprint match ⇒ `Trusted{meta}`; parse OK + no/mismatched record ⇒ `Untrusted{New|Changed, meta: Some(meta)}`. `allow`: discovery ⇒ none found is `AppError::ProjectTrust` (exit 5, message per AC-003.6); unresolvable base ⇒ exit-2 error naming variables (EDGE-004b); single read; validate (violations ⇒ exit-2 error listing them + remedy per AC-003.5); fingerprint and record the same snapshot; report `already_current` when the record matched. `revoke`: discovery + canonicalize + load + path-only remove + save; no content read.
  - Interfaces: Consumes: `model::parse`, `model::ProjectFileMeta`, `locate::discover`, `trust::{TrustStore, store_path, fingerprint, RealFs}` (exact T002/T003/T004 signatures). Produces: the facade surface above, for T007/T008.
  - Impact seeds: `ProjectContext`, `UntrustedReason`, `resolve`, `allow`, `revoke`
  - No-go: `src/cli/`, `src/config/`, `src/main.rs`, `tests/project_trust.rs`
  - TDD: no
  - Dispatch: agent (impl-standard)
  - Review: per-task
  - Verification: `cargo test --test project_facade` — expected: all pass, covering trust lifecycle end-to-end in temp trees with overridden `XDG_STATE_HOME`, classification precedence (EDGE-011), symlinked ancestor (AC-003.7), state-base cases (EDGE-004a/b); `cargo test` — expected: no regressions.
  - Report: `.sdd/changes/003-project-config/reports/task-T006-report.md`

Checkpoint: `src/project/` is complete and library-level tested: schema, discovery, trust store, and facade all green via `cargo test --test project_schema --test project_trust --test project_facade`, with the CLI still untouched (`git diff --stat src/cli src/main.rs src/config` empty).

### Group 3: Phase 1 CLI integration

- [ ] T007 [Phase 1] Thread the pin through selection and add the pre-dispatch prelude
  - Files: `src/config/model.rs` (`select_profile`), `src/config/write.rs` (two standard-chain call sites), `src/cli/mod.rs`, `src/main.rs`, `src/error.rs`, `tests/project_precedence.rs` (create), `tests/project_notice.rs` (create)
  - Depends on: T006
  - Spec refs: SPEC-004 (read in full — `--create-profile` exemption included), SPEC-005 (notice scope + evaluation order), SPEC-001 (bypass)
  - Acceptance refs: AC-001.4, AC-001.5 (discovery halves), AC-004.1, AC-004.2, AC-004.3, AC-004.4, AC-004.5, AC-004.6, AC-004.7, AC-004.8, AC-005.1, AC-005.2, AC-005.3, AC-005.4, AC-005.5, AC-005.6, AC-005.7, AC-005.8, AC-005.9, AC-010.2, EDGE-003, EDGE-007, EDGE-008, EDGE-010
  - Task:
    1. `src/config/model.rs`: change `pub fn select_profile(&self, flag: Option<&str>, env_val: Option<&str>) -> Result<&Profile, AppError>` to `select_profile(&self, flag: Option<&str>, env_val: Option<&str>, project_pin: Option<&crate::project::model::ProjectPin>) -> Result<&Profile, AppError>` — pin slots between env and `default_profile`; a pin naming an undefined profile produces the existing unknown-profile error text extended to name `pin.file` (exit 3 unchanged). Update the two standard-chain call sites in `src/config/write.rs` (the `set`/`unset` selection paths) and the CLI call site. The `--create-profile` branch (`resolve_write_profile`) is NOT changed.
    2. `src/error.rs`: add variant `ProjectTrust(String)` mapping to exit status 5.
    3. `src/main.rs`/`src/cli/mod.rs`: introduce the pre-dispatch prelude — after clap parsing succeeds and before command dispatch: honor bypass (`--no-project` new global flag, `AGENTENV_NO_PROJECT` non-empty) for non-`project` commands; otherwise call `project::resolve`; on `Untrusted`, write and flush exactly one stderr notice line (file path + `agentenv project status` + next action) before dispatch — which guarantees notice-before-`run`-exec and notice-on-command-failure; hand the `ProjectContext` to the command. `--help`, `--version`, no-subcommand help, and parse failures never reach the prelude. Command outcomes carry an explicit exit status alongside stdout/stderr so T008 can emit a report with exit 5/6 (extend the existing dispatch result type accordingly; existing commands keep their current statuses).
  - Interfaces: Consumes: `project::{resolve, ProjectContext, UntrustedReason}`, `project::model::ProjectPin` (T002/T006 signatures). Produces: `select_profile(flag, env_val, project_pin)` (new signature), the prelude handing `ProjectContext` into dispatch, `AppError::ProjectTrust` → exit 5, and the status-carrying outcome type — all consumed by T008.
  - Impact seeds: `select_profile`, `resolve_write_profile`, `run_ac`, `AppError`
  - No-go: `src/project/` (read-only), `tests/project_trust.rs`, `src/runner.rs` (launch semantics unchanged; only the prelude order guarantees the notice)
  - TDD: no
  - Dispatch: agent (impl-standard)
  - Review: per-task
  - Verification: `cargo test --test project_precedence --test project_notice` — expected: all pass, covering AC-004.1, AC-004.2, AC-004.3, AC-004.4, AC-004.5, AC-004.6, AC-004.7, AC-004.8 (incl. probe-based AC-004.7 via `test-probe` and the `set` write path), AC-005.1, AC-005.2, AC-005.3, AC-005.4, AC-005.5, AC-005.6, AC-005.7, AC-005.8, AC-005.9 (stdout byte-identity, single notice, run-notice-order, parse-failure no-notice, trusted-no-notice), AC-010.2 sentinel; `cargo test` — expected: full suite green (pre-existing assertions unmodified).
  - Report: `.sdd/changes/003-project-config/reports/task-T007-report.md`

- [ ] T008 [Phase 1] Implement `agentenv project status|allow|revoke` with the frozen JSON envelope and requirement checking
  - Files: `src/cli/project.rs` (create), `src/cli/mod.rs` (subcommand wiring), `src/query/render.rs` (only if shared JSON helpers are needed), `tests/project_status.rs` (create), `tests/snapshots/project-status-*.json` (create, one per member-state-table row)
  - Depends on: T007
  - Spec refs: SPEC-006 (read in full — exit matrix, deviation note, member state table), SPEC-007, SPEC-AS-006
  - Acceptance refs: AC-003.2, AC-003.4, AC-003.5, AC-003.6 (command halves), AC-006.1, AC-006.2, AC-006.3, AC-006.4, AC-006.5, AC-006.6, AC-006.7, AC-006.8, AC-006.9, AC-006.10, AC-006.11, AC-006.12, AC-006.13, AC-007.1, AC-007.2, AC-007.3, AC-007.4, AC-007.5, AC-007.6, AC-007.7, AC-010.3, AC-010.4, EDGE-005, EDGE-009, EDGE-013
  - Task: Add the `project` subcommand group. `status [--json]`: render the SPEC-006 member state table exactly — text form covers the same members; JSON is the frozen envelope, emitted on stdout for exits 0/5/6 (exit 2 emits nothing on stdout); exit per the first-match matrix. Requirement checking (SPEC-007): against the profile selected by the standard chain (SPEC-AS-006); satisfied = entry exists in the active profile and every `fields` member resolves via `resolve_in_entry` (`src/config/validate.rs`) to ANY value — tables, arrays, and credential references satisfy; entries in file declaration order; degraded selection ⇒ `checked: false` with reason + next action, never an error. No credential resolution, provider execution, or secret-store read anywhere in the group. `allow`/`revoke` call the T006 facade and render its outcomes (messages per AC-003.2/.4/.5/.6, all naming next actions).
  - Interfaces: Consumes: `project::{resolve, allow, revoke, ProjectContext, UntrustedReason, AllowOutcome, RevokeOutcome}`, `resolve_in_entry`, `select_profile(flag, env, pin)`, the T007 outcome type — exact signatures from T006/T007. Produces: the `project` subcommand surface (external CLI contract).
  - Impact seeds: `Command` (clap enum in `src/cli/mod.rs`), `resolve_in_entry`, `entry_table`
  - No-go: `src/project/` (read-only), `src/runner.rs`, `src/config/write.rs`
  - TDD: no
  - Dispatch: agent (impl-standard)
  - Review: per-task
  - Verification: `cargo test --test project_status` — expected: all pass, covering every SPEC-006 acceptance criterion (exits 0/2/5/6), AC-007.1, AC-007.2, AC-007.3, AC-007.4, AC-007.5, AC-007.6, AC-007.7 (incl. counting-provider AC-007.4 and table/credential-ref AC-007.7), snapshot per state-table row, AC-010.3 sentinel + full-envelope assertion; `cargo test` — expected: full suite green.
  - Report: `.sdd/changes/003-project-config/reports/task-T008-report.md`

- [ ] T009 [Phase 1] Cross-cutting acceptance tests: sentinels, canary, injection probe
  - Files: `tests/project_security.rs` (create), `tests/fixtures/project/` (sentinel fixtures as needed)
  - Depends on: T008
  - Spec refs: SPEC-010, SPEC-009
  - Acceptance refs: AC-010.1, AC-010.2, AC-010.3, AC-010.4, AC-010.5, AC-009.1, AC-009.2
  - Task: Add the acceptance suite that spans modules: AC-010.1 (invalid file with sentinels in forbidden positions — no sentinel in any output of `allow`/`status`/notice paths), AC-010.2 already covered in T007 — extend here only if a gap remains, AC-010.4 (counting provider untouched across `status`/`allow`/`revoke`), AC-010.5 (trusted pin + `run` via `test-probe`: injected names/sources are exactly the pinned profile's), and a final assertion that all pre-existing snapshots under `tests/snapshots/` are byte-identical (AC-009.2 is otherwise implicit in the suite). Model test structure on `tests/security_p1.rs`/`tests/security_p3.rs`.
  - Interfaces: Consumes: the complete CLI surface from T007/T008; `tests/fixtures/counting_provider.sh`; `test-probe`. Produces: none.
  - Impact seeds: none
  - No-go: `src/`
  - TDD: no
  - Dispatch: agent (impl-standard)
  - Verification: `cargo test` — expected: entire suite green including the new security tests; `cargo fmt --check` — expected: exit 0.
  - Report: `.sdd/changes/003-project-config/reports/task-T009-report.md`

Checkpoint: all Phase 1 acceptance criteria (SPEC-001 through SPEC-007, SPEC-009, SPEC-010) pass via `cargo test`; the MVP is observable end-to-end: in a scratch tree, `agentenv project status --json` walks the documented lifecycle (untrusted → `allow` → pinned reads → edit → untrusted) with the documented exit statuses.

### Group 4: Phase 2 — Docs and protocol

- [ ] T010 [Phase 2] Update README, agent skill, and pairing documentation
  - Files: `README.md`, `skills/agentenv/SKILL.md`
  - Depends on: T009
  - Spec refs: SPEC-008 (read in full — it enumerates every required topic)
  - Acceptance refs: AC-008.1, AC-008.2, AC-008.3
  - Task: Document per SPEC-008: project file schema (incl. 64 KiB limit) and discovery; trust lifecycle (`project status/allow/revoke`); extended exit-status table (add 5 and 6; note status 2 also covers project-file validation errors); the `project status --json` stdout-with-nonzero-exit deviation; precedence chain incl. pin and the `--create-profile` exemption; bypass (`--no-project`/`AGENTENV_NO_PROJECT`) and its non-application to `project` subcommands; the `.env`/docker-compose pairing section with a worked example (`agentenv run --with llm -- docker compose up` + a compose snippet using `environment: - OPENAI_API_KEY` passthrough or `${OPENAI_API_KEY}` interpolation; name `env_file:`-with-secrets as the anti-pattern); update the README `AGENTS.md` block and the skill's reading protocol to begin with `agentenv project status --json`. Follow `~/.agents/agent-standards/user-facing-copy.md`; match the README's existing register and structure.
  - Interfaces: Consumes: the shipped CLI behavior (T007/T008). Produces: none.
  - Impact seeds: none
  - No-go: `src/`, `tests/`
  - TDD: no
  - Dispatch: agent (impl-standard; prose quality gated at review against user-facing-copy standards)
  - Verification: `cargo build` — expected: exit 0 (no code touched: `git status --porcelain -- src/ tests/` empty); manual: execute each documented `project` command sequence and the compose example in a scratch tree against the built binary — expected: behavior matches the text (record transcript in the task report).
  - Report: `.sdd/changes/003-project-config/reports/task-T010-report.md`

Checkpoint: Documentation matches shipped behavior; AC-008.1, AC-008.2, AC-008.3 walked and recorded.

### Group 5: Validation

- [ ] T900 [Validation] Run acceptance validation and update validation report
  - Files: `.sdd/changes/003-project-config/validation.md`
  - Depends on: all implementation tasks
  - Spec refs: all
  - Acceptance refs: all
  - Impact seeds: none
  - No-go: none
  - TDD: no
  - Dispatch: agent (final validation on the host high-capability native route)
  - Verification: `python <package-root>/scripts/sdd.py verify 003-project-config --compare-baseline --update-validation` — expected: no new failures vs baseline; every AC row in `validation.md` marked with evidence; the SPEC-AS-007 manual latency measurement recorded.
  - Report: `.sdd/changes/003-project-config/reports/task-T900-report.md`

Checkpoint: All acceptance criteria pass or carry recorded deferrals; `validation.md` is complete.

## Dependency Notes

- T004 → T005 is the TDD pair: T004's failing suite is checkpointed before T005 dispatches; T005 must not touch `tests/project_trust.rs`.
- T002 and T003 both add one `pub mod` line to `src/project/mod.rs`; dispatch them to the same provider as a stack or serialize the tiny merge at integration.
- T007 and T008 share `src/cli/mod.rs` and must be serialized (same provider, stacked).
- `Cargo.toml` is touched only by T004 (`sha2`).

## Parallel Dispatch Notes

- Tasks safe to dispatch together: T002 ∥ T003 (after the `mod.rs` coordination note); T001 independent.
- Tasks that must be serialized: T004 → T005 → T006 → T007 → T008 → T009 → T010 → T900.
- Shared files requiring controller integration: `src/project/mod.rs` (T002/T003/T006), `src/cli/mod.rs` (T007/T008).

## Dispatch Grouping

- Routed to agents: T001..T010, T900.
- Expected native by host affinity: T004 (high-capability TDD authoring), T900 (final validation) — Claude host, native subagent.
- Expected external worker: T001 (impl-light → codex gpt-5.6-luna xhigh), T003 (impl-light → codex gpt-5.6-luna xhigh), T002/T005/T006/T007/T008/T009/T010 (impl-standard → codex gpt-5.6-terra high, per the ordered ladder with grok switched off).
- Keep inline: none.
- Frontend tasks actively dispatched: none (no UI surface).

## Coverage

| Spec / Acceptance ID | Task IDs | Notes |
| --- | --- | --- |
| SPEC-001 / AC-001.1, AC-001.2, AC-001.3 | T003 (walk), T007 (integration) | AC-001.2 = non-regular-file skip |
| SPEC-001 / AC-001.4, 1.5 | T007, T008 | Bypass + project-group exemption |
| SPEC-002 / AC-002.1, AC-002.2, AC-002.3, AC-002.4, AC-002.5, AC-002.6, AC-002.7 | T002 | Fixtures per violation class |
| SPEC-003 / AC-003.1, 3.12 | T006, T007 | Pin inert until trusted; snapshot binding |
| SPEC-003 / AC-003.2, 3.4..6 | T006, T008 | Command surfaces |
| SPEC-003 / AC-003.3, 3.7..11, 3.13 | T004, T005, T006 | Store + facade |
| SPEC-004 / AC-004.1, AC-004.2, AC-004.3, AC-004.4, AC-004.5, AC-004.6, AC-004.7, AC-004.8 | T007 | Incl. write path and create-profile exemption |
| SPEC-005 / AC-005.1, AC-005.2, AC-005.3, AC-005.4, AC-005.5, AC-005.6, AC-005.7, AC-005.8, AC-005.9 | T007 | Notice + inertness suite |
| SPEC-006 / AC-006.1, AC-006.2, AC-006.3, AC-006.4, AC-006.5, AC-006.6, AC-006.7, AC-006.8, AC-006.9, AC-006.10, AC-006.11, AC-006.12, AC-006.13 | T008 | Exit matrix + envelope snapshots |
| SPEC-007 / AC-007.1, AC-007.2, AC-007.3, AC-007.4, AC-007.5, AC-007.6, AC-007.7 | T008 | Structural checking |
| SPEC-008 / AC-008.1, AC-008.2, AC-008.3 | T010 | Manual walkthrough |
| SPEC-009 / AC-009.1, AC-009.2 | T001, T009 | Hermetic harness + snapshot byte-identity |
| SPEC-010 / AC-010.1, AC-010.2, AC-010.3, AC-010.4, AC-010.5 | T002 (violation messages), T007 (10.2), T008 (10.3, 10.4), T009 (10.1, 10.5) | Sentinel/canary/probe |
| EDGE-001..013 | T003, T006, T007, T008 | Mapped inside each task's test list |
| T900 | all | Final validation |
