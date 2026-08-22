# Tasks: 002-config-write

## Source Artifacts

- Change ID: 002-002-config-write
- Plan: merged into `## Strategy` below (light tier)
- Spec: `.sdd/changes/002-002-config-write/spec.md`

## Strategy

Approach: build the shared write pipeline first (`src/config/write.rs`: format-preserving document, whole-file validation, atomic persist), then wire each command (`set`, `unset`, `init`, `credential add`) as a thin client, each landing with its integration tests in the same task. README updates close the change. All tasks run inline (light-tier default): the work is one subsystem, every command shares the pipeline's invariants, and a single orchestrator session keeps the no-echo discipline coherent across error paths.

Global constraints (binding for every task):

- Follow `~/.agents/agent-standards/rust.md`; match the existing codebase's error-handling (`AppError` + `Violation`), doc-comment, and test style.
- The no-value-echo invariant (SPEC-006) applies to every new diagnostic; the SPEC-019 metadata boundary is the only exception. Sentinel leak tests are part of each command's test set, not a separate pass.
- The sensitive-name guardrail must call the same predicate `config::validate` uses (Implementation Note in spec); export it from `validate.rs` rather than duplicating the suffix list.
- Exit statuses reuse the v1 mapping (1 usage, 2 config/validation/write-I/O, 3 unknown name/path); no new statuses.
- `toml_edit = "0.25"`; the mutated document is re-validated through the existing `toml` parse + `config::validate::validate` before any disk write.
- Never log or print credential values; no new dependencies beyond `toml_edit` (tempfile stays dev-only — the atomic write uses a manually named temp file in the target directory).

Verification plan: `cargo build` + `cargo test` green per task; `cargo fmt --check` at checkpoints; baseline recorded via `sdd.py verify --baseline` before T001. Reviews at Checkpoint granularity via a high-capability native code-review lane.

## Execution Rules

- Use local files under `.sdd/changes/002-002-config-write/` for all workflow state.
- Mark a task complete only after its verification and the covering checkpoint review pass.
- TDD is exceptional: no task here uses it — acceptance-criteria integration tests are authored inside each task (the ACs already pin observable CLI behavior; there is no contract-first design gap).
- Acceptance criteria are the primary gate.

## Dispatch Preference

All tasks: `Dispatch: inline` (light-tier default; one subsystem, shared invariants, orchestrator-owned no-echo discipline). Checkpoint reviews run on the host high-capability native route.

## Task Groups

### Group 1: Phase 1 (P1, MVP) — write pipeline + set/unset

- [ ] T001 [Phase 1] Write pipeline module and `toml_edit` dependency
  - Files: `Cargo.toml`, `src/config/write.rs` (new), `src/config/mod.rs` (module wiring)
  - Depends on: none
  - Spec refs: SPEC-001
  - Acceptance refs: AC-001.1, AC-001.3, AC-001.4, AC-001.5, AC-001.6
  - Interfaces: Produces: `write::WriteDocument::load(explicit_file: Option<&Path>, env: &impl Fn(&str) -> Option<String>) -> Result<WriteDocument, AppError>` (resolves path incl. full symlink chain, reads text, parses `toml_edit::DocumentMut`, pre-validates); `WriteDocument::doc_mut(&mut self) -> &mut toml_edit::DocumentMut`; `WriteDocument::validate_and_persist(self) -> Result<(), AppError>` (serialize → `toml` parse → `config::validate::validate` → atomic replace); `write::create_new(path: &Path, content: &str) -> Result<(), AppError>` (0600 create for `init`)
  - Impact seeds: `Config::load`, `config::validate::validate`, `config::locate::resolve_path`
  - No-go: `src/credential/`, `src/runner.rs`, `src/query.rs`
  - TDD: no
  - Dispatch: inline (light-tier default)
  - Verification: `cargo test config::write` — expected: unit tests for atomic replace, permission preservation, decor/implicit preservation, refusal-leaves-file-intact all pass
  - Report: `.sdd/changes/002-002-config-write/reports/task-T001-report.md`

- [ ] T002 [Phase 1] `set` command
  - Files: `src/cli/commands.rs`, `src/config/write.rs` (set mutation), `src/config/validate.rs` (export sensitive-name predicate), `tests/write_set.rs` (new)
  - Depends on: T001
  - Spec refs: SPEC-002, SPEC-006, SPEC-001 (AC-001.2, AC-001.7)
  - Acceptance refs: AC-001.2, AC-001.7, AC-002.1, AC-002.2, AC-002.3, AC-002.4, AC-002.5, AC-002.6, AC-002.7, AC-002.8, AC-002.9, AC-002.10, AC-002.11, AC-002.12, AC-002.13, AC-006.1, AC-006.2, EDGE-001, EDGE-002, EDGE-003, EDGE-004, EDGE-005, EDGE-008, EDGE-009, EDGE-010, EDGE-011, EDGE-012, EDGE-013
  - Interfaces: Consumes: `WriteDocument` API from T001, `path::Segments`; Produces: `Command::Set(SetArgs)` clap variant (`path`, `value`, `--type string|int|float|bool|json`, `--description`, `--create-profile`)
  - Impact seeds: `Command` enum in `src/cli/commands.rs`, `execute`, `Segments::parse`
  - No-go: `src/credential/`, `src/runner.rs`
  - TDD: no
  - Dispatch: inline (light-tier default)
  - Verification: `cargo test --test write_set` — expected: all SPEC-002 ACs incl. sentinel leak checks pass
  - Report: `.sdd/changes/002-002-config-write/reports/task-T002-report.md`

- [ ] T003 [Phase 1] `unset` command
  - Files: `src/cli/commands.rs`, `src/config/write.rs` (remove mutation), `tests/write_unset.rs` (new)
  - Depends on: T002
  - Spec refs: SPEC-003
  - Acceptance refs: AC-003.1, AC-003.2, AC-003.3, AC-003.4, EDGE-007
  - Interfaces: Consumes: `WriteDocument` API, `Segments`; Produces: `Command::Unset { path }` clap variant
  - Impact seeds: `Command` enum in `src/cli/commands.rs`, `execute`
  - No-go: `src/credential/`, `src/runner.rs`
  - TDD: no
  - Dispatch: inline (light-tier default)
  - Verification: `cargo test --test write_unset` — expected: all SPEC-003 ACs pass
  - Report: `.sdd/changes/002-002-config-write/reports/task-T003-report.md`

Checkpoint: `set`/`unset` round-trip against a commented fixture preserving formatting; refusals leave the file byte-identical; `cargo test` and `cargo fmt --check` green. Review: native high-capability code-review lane over the group diff.

### Group 2: Phase 2 + Phase 3 — init, credential add, docs

- [ ] T004 [Phase 2] `init` command
  - Files: `src/cli/commands.rs`, `src/config/write.rs` (bootstrap content), `tests/write_init.rs` (new)
  - Depends on: T001
  - Spec refs: SPEC-004
  - Acceptance refs: AC-004.1, AC-004.2, AC-004.3, AC-004.4, EDGE-013
  - Interfaces: Consumes: `write::create_new`, `config::locate::resolve_path`; Produces: `Command::Init` clap variant
  - Impact seeds: `Command` enum in `src/cli/commands.rs`, `locate::resolve_path`
  - No-go: `src/credential/`, `src/runner.rs`
  - TDD: no
  - Dispatch: inline (light-tier default)
  - Verification: `cargo test --test write_init` — expected: all SPEC-004 ACs pass
  - Report: `.sdd/changes/002-002-config-write/reports/task-T004-report.md`

- [ ] T005 [Phase 3] `credential add` command
  - Files: `src/cli/commands.rs`, `src/config/write.rs` (credential mutation), `tests/write_credential_add.rs` (new)
  - Depends on: T001
  - Spec refs: SPEC-005
  - Acceptance refs: AC-005.1, AC-005.2, AC-005.3, AC-005.4, AC-005.5
  - Interfaces: Consumes: `WriteDocument` API; Produces: `CredentialCommand::Add(CredentialAddArgs)` clap variant (`--description`, `--provider`, `--inject-as`, `--env-var`, `--service`, `--account`, `--argv`)
  - Impact seeds: `CredentialCommand` enum in `src/cli/commands.rs`, `execute`
  - No-go: `src/credential/keychain.rs`, `src/credential/command.rs`, `src/credential/env.rs`, `src/runner.rs`
  - TDD: no
  - Dispatch: inline (light-tier default)
  - Verification: `cargo test --test write_credential_add` — expected: all SPEC-005 ACs pass
  - Report: `.sdd/changes/002-002-config-write/reports/task-T005-report.md`

- [ ] T006 [Phase 2] README and agent-protocol updates
  - Files: `README.md`
  - Depends on: T002, T003, T004, T005
  - Spec refs: Scope (README updates), SPEC-002 (ordering note), Design Notes (threat-model delta)
  - Acceptance refs: EDGE-006 (concurrency statement)
  - Interfaces: Consumes: final CLI surface from T002–T005
  - Impact seeds: none
  - No-go: none
  - TDD: no
  - Dispatch: inline (light-tier default)
  - Verification: `cargo build` — expected: builds clean; README sections (CLI reference, agent protocol with `credential add` → `set` ordering, safety delta, concurrency note) present by manual read
  - Report: `.sdd/changes/002-002-config-write/reports/task-T006-report.md`

Checkpoint: all four commands work end to end; README documents them; full `cargo test` green. Review: native high-capability code-review lane over the group diff.

### Group 3: Validation

- [ ] T900 [Validation] Run acceptance validation and update validation report
  - Files: `.sdd/changes/002-002-config-write/validation.md`
  - Depends on: all implementation tasks
  - Spec refs: all
  - Acceptance refs: all
  - Impact seeds: none
  - No-go: none
  - TDD: no
  - Dispatch: inline (orchestrator-owned final validation, host high-capability)
  - Verification: `python <package-root>/scripts/sdd.py verify 002-002-config-write --compare-baseline --update-validation` — expected: no new failures vs baseline
  - Report: `.sdd/changes/002-002-config-write/reports/task-T900-report.md`

Checkpoint: all acceptance criteria pass or carry recorded deferrals; `validation.md` complete.

## Dependency Notes

- T001 is the foundation; T002–T005 are pipeline clients. T003 depends on T002 only because both edit the same `Command` enum region and `set`'s path plumbing is reused; T004/T005 depend only on T001 but are serialized inline anyway.
- T006 must reflect the final flag surface, so it runs last before validation.

## Parallel Dispatch Notes

- All tasks run inline and serialized; no parallel lanes (single shared file `src/cli/commands.rs` across T002–T005).

## Dispatch Grouping

- Routed to agents: none (light-tier inline default; no worker dispatch)
- Keep inline: T001–T006, T900 (one subsystem; shared invariants; dispatch overhead exceeds every task's scope)

## Coverage

| Spec / Acceptance ID | Task IDs | Notes |
| --- | --- | --- |
| SPEC-001 / AC-001.1, .3, .4, .5, .6 | T001 | Pipeline unit tests |
| SPEC-001 / AC-001.2, AC-001.7 | T002 | Observable through `set`; `unset`/`credential add` reuse the same rejection path |
| SPEC-002 / AC-002.1..13 | T002 | Includes guardrail negatives and reference-scope negatives |
| SPEC-003 / AC-003.1..4 | T003 | |
| SPEC-004 / AC-004.1..4 | T004 | |
| SPEC-005 / AC-005.1..5 | T005 | |
| SPEC-006 / AC-006.1..2 | T002 (T003/T005 reuse sentinel helpers) | Leak checks on every error path |
| EDGE-001..005, 008..013 | T002 | EDGE-013 also covered by T004 for `init` |
| EDGE-006 | T006 | README concurrency statement |
| EDGE-007 | T003 | |
| Scope: README updates | T006 | CLI reference, agent protocol, safety delta |
