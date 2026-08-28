# Implementation Plan: Project-Scoped Configuration

## Source Artifacts

- Change ID: 003-project-config
- PRD: `.sdd/changes/003-project-config/prd.md`
- Architecture: `.sdd/changes/003-project-config/architecture.md`
- Spec: `.sdd/changes/003-project-config/spec.md`
- Spec review: `.sdd/changes/003-project-config/spec-review.md` (Decision: Approved, round 6)

## Strategy

Build bottom-up along the architecture's module seams, keeping the existing CLI untouched until the new `src/project/` module is complete and tested. First make the existing test suite hermetic (working-directory pinning plus `AGENTENV_NO_PROJECT` in the shared helper and the direct invocations in `tests/run_p3.rs` and `tests/credential_p2.rs`) so the compatibility gate (SPEC-009) is meaningful on any machine before any behavior changes.

Then implement the three leaf modules — `project::model` (closed-schema parsing), `project::locate` (ancestor walk), `project::trust` (store with atomic mutations) — with the trust module developed test-first: the trust gate is a security-sensitive boundary (the architecture's declared TDD seam), so a high-capability lane authors its failing tests and interface skeletons before the implementation task runs. The `project` facade composes the three over one immutable byte snapshot and is the only seam the rest of the CLI sees.

Integration lands in two steps: the selection/prelude step threads `Option<&ProjectPin>` through `Config::select_profile` and its standard-chain call sites, introduces the pre-dispatch prelude (discovery, notice emission, explicit exit-status outcome) in the CLI entry path; the subcommand step adds `agentenv project status|allow|revoke` with the frozen JSON envelope, the exhaustive exit matrix, and structural requirement checking. A cross-cutting acceptance-test task then covers the sentinel, canary, probe, and snapshot criteria that span modules. Phase 2 is documentation only (README, skill, AGENTS.md block, pairing guidance).

## Global Constraints

Copied verbatim from the approved artifacts:

- Profile precedence: `--profile` flag, then `AGENTENV_PROFILE` (non-empty), then the trusted project file's `profile` pin, then `default_profile` (SPEC-004). `--create-profile` requires an explicit `--profile` and never consults the pin.
- Exit statuses: `0` success; `1` usage; `2` configuration-file error (now also project-file validation errors, corrupt trust store, trusted-unreadable file, state-base-unset on `allow`/`revoke`); `3` unknown profile/entry/field/credential (a dangling trusted pin names the project file); `4` credential/injection failure; `5` project trust-state failure (`status` on untrusted/invalid/unavailable; `allow`/`revoke` with no discovered file); `6` requirements unsatisfied or uncheckable (`status` only); `127` run target not executable. No pre-existing status changes meaning (SPEC-008/AC-008.2).
- Project file: `.agentenv.toml`, nearest regular file on the CWD→root walk; closed schema `version = 1`, optional non-empty `profile`, optional `[requires.<entry>]` with mandatory non-empty `reason` and optional non-empty `fields`; single-segment entry keys; entry-relative `fields` in the accepted segment grammar; duplicates are violations; files over 64 KiB are invalid; `credential://`-prefixed strings in any allowed position are violations (SPEC-002).
- Trust: approval keyed by canonical absolute path + SHA-256 of exact bytes; `allow` binds approval to its single-read snapshot; `revoke` is path-only; store mutations are atomic (0600-first temp + rename on Unix); a corrupt store is exit 2, never treated as empty; store permission bits checked at creation only (SPEC-003, SPEC-AS-008).
- Store location: `$XDG_STATE_HOME/agentenv/trust.toml`, else `~/.local/state/agentenv/trust.toml`; Windows `%LOCALAPPDATA%\agentenv\trust.toml` (ARCH-002). Tests override via `XDG_STATE_HOME`/`HOME`/`LOCALAPPDATA` only.
- Inertness: untrusted files change nothing except one single-line stderr notice (path + `agentenv project status` + next action); notice only from the pre-dispatch prelude after successful CLI parse; never on stdout; never from `project` subcommands, `--help`, `--version`, parse failures, or bypassed invocations; classification precedence `invalid` outranks `untrusted-changed` (SPEC-005).
- No-secret invariant: no credential resolution, provider execution, or secret-store read from discovery/validation/trust/status; diagnostics name paths only and a next action, never values or TOML source lines; the `status` report exposes exactly the frozen envelope members and nothing else (SPEC-010).
- JSON: the frozen SPEC-006 envelope with the member state table; `project status --json` emits its report on stdout with exits 0/5/6 (documented deviation); exit 2 leaves stdout empty; members never omitted.
- Compatibility: functional command invocations byte-identical without a project file; help/usage/version surfaces exempt (SPEC-009); every test invocation hermetic.
- Language/tooling: Rust 2021, `cargo build` / `cargo test` / `cargo fmt --check` gates; English for all code, diagnostics, and docs; TOML edits preserve formatting (existing `toml_edit` conventions).

## Principles Check

- [x] No principle in `.sdd/memory/principles.md` is violated: artifacts stay local, acceptance criteria gate implementation, and the design keeps one deep facade seam with clear interfaces.
- [x] The structure is the simplest that satisfies the spec: one new module tree (`src/project/`), one signature extension, one prelude; no speculative layers (strict-mode `run`, GC, locking all recorded as deferred).

## Research Decisions

| Unknown | Decision | Rationale | Alternatives considered |
| --- | --- | --- | --- |
| Hash dependency | `sha2` crate (RustCrypto), default features | The registry-standard pure-Rust SHA-256; project-scoped dependency addition is permitted by the manifest rule | Hand-rolled SHA-256 (unacceptable risk); `ring` (heavier, C code, overkill for one digest) |
| Fault-injection seam for AC-003.11/.12 | `project::trust` takes a small `StoreFs` trait (read, write-temp, rename, set-permissions) with a production `std::fs` adapter and a test adapter injecting failures | Only seam that makes interrupted-rename and post-read-replacement observable in unit tests without platform tricks; matches the architecture's declared adapter pair | `#[cfg(test)]` hooks inside functions (hidden control flow); no seam (ACs untestable) |
| Windows canonical paths (`\\?\` prefix) | Canonical path is trust identity only and never rendered in reports; reports use the discovered spelling (spec Implementation Notes) | Avoids user-visible `\\?\C:\…` noise while keeping identity exact | Stripping the prefix for storage (lossy, alias risk) |
| TOML parse-position reporting for invalid project files | Reuse the existing config parse-diagnostic style: parser message + position, no source lines (`src/config/mod.rs` convention) | Consistent no-echo behavior already accepted in change 001 | Custom miette-style rendering (new dependency, echoes source) |
| Where the prelude lives | `run_cli` path in `src/main.rs`/`src/cli/mod.rs`: after clap parse succeeds, before command dispatch; returns `(ProjectContext, notice-flushed)` and commands receive the context | Matches ARCH-004; guarantees notice-before-`exec` for `run` and notice-on-failure for commands | Notice inside each command (duplication, missed paths); stderr writes in `Drop` guards (ordering fragile) |
| Requirement-check resolution function | Reuse `resolve_in_entry` (`src/config/validate.rs`) for entry-relative field paths, accepting any resolved value | Exactly the accepted grammar; SPEC-007 explicitly decouples from inject's scalar-only restriction | New resolver (duplication); `Segments::resolve` full-profile variant (wrong relativity) |

## Complexity Tracking

| Deviation | Why needed | Simpler alternative rejected because |
| --- | --- | --- |
| `StoreFs` trait seam in `project::trust` | AC-003.11/AC-003.12 require observing interrupted-rename and post-read-replacement outcomes | Direct `std::fs` calls make both ACs untestable — the fault window cannot be reached deterministically from an integration test |
| Explicit command outcome carrying exit status (replacing `Result<Output, AppError>` for the new paths) | SPEC-006 requires a full report on stdout together with exit 5/6; the current type cannot express output-plus-nonzero | Mapping through `AppError` would discard the report or print it to stderr, violating the frozen JSON contract |

## Workstreams

| Workstream | Purpose | Files / areas | Depends on | Parallel safe? |
| --- | --- | --- | --- | --- |
| WS-001 Hermetic harness | Make every test invocation project-hermetic | `tests/helpers/mod.rs`, `tests/run_p3.rs`, `tests/credential_p2.rs` | none | Yes — test-only files |
| WS-002 Project leaf modules | Schema, discovery, trust store | `src/project/{model,locate,trust}.rs`, `Cargo.toml` (`sha2`), fixtures | none (trust impl after its test task) | model ∥ locate; trust serialized behind its tests |
| WS-003 Facade | `ProjectContext` resolve/allow/revoke over one snapshot | `src/project/mod.rs` | WS-002 | No — single file |
| WS-004 CLI integration | `select_profile` pin, prelude + notice, outcome type, `project` subcommands, requirement checking, JSON | `src/config/model.rs`, `src/config/write.rs`, `src/cli/`, `src/main.rs`, `src/error.rs`, `src/query/render.rs` | WS-003 | No — shared dispatch files, serialized |
| WS-005 Cross-cutting acceptance tests | Sentinel/canary/probe/snapshot suites | `tests/project_*.rs`, `tests/snapshots/` | WS-004 | Yes vs docs |
| WS-006 Docs | README, skill, AGENTS.md block, pairing | `README.md`, `skills/agentenv/SKILL.md` | WS-004 behavior final | Yes vs WS-005 |

## Dependency Graph

```text
T001 (hermetic harness)
T002 (project::model) ─┐
T003 (project::locate) ─┼→ T006 (facade) → T007 (selection + prelude + outcome) → T008 (project subcommands + JSON + requirements)
T004 (trust tests, TDD) → T005 (project::trust) ─┘                                      ↓
                                                                 T009 (cross-cutting acceptance tests) → T010 (docs) → T900 (validation)
```

## Phase Plan

### Phase 1: Pin, trust, and status (MVP)

- Objective: complete behavioral surface — discovery, schema, trust lifecycle, pin precedence, inertness + notice, `project` subcommands with frozen JSON and requirement checking, hermetic compatibility.
- Spec references: SPEC-001..007, SPEC-009, SPEC-010
- Acceptance gate: AC-001.1..5, AC-002.1..7, AC-003.1..13, AC-004.1..8, AC-005.1..9, AC-006.1..13, AC-007.1..7, AC-009.1..2, AC-010.1..5
- Implementation notes: tasks T001–T009; trust module is the TDD seam (T004 authors tests first).

### Phase 2: Docs and protocol

- Objective: README, skill, and pairing documentation reflecting shipped behavior.
- Spec references: SPEC-008
- Acceptance gate: AC-008.1..3
- Implementation notes: task T010; manual walkthrough is the gate.

## Parallelization Plan

- Parallel-safe tasks: T002 ∥ T003 (different new files); T001 independent of both; T009 ∥ T010 only if T010 waits for behavior freeze — keep serialized to be safe.
- Serialized tasks: T004 → T005 → T006 → T007 → T008 → T009 → T010 → T900.
- Shared-file conflict risks: `src/cli/mod.rs`, `src/main.rs`, `src/error.rs` (T007/T008 only — same provider, stacked); `Cargo.toml` (T005 adds `sha2` — no other task touches it).
- Integration owner: orchestrator (this session).

## Verification Plan

| Gate | Command or check | Expected result | Owner |
| --- | --- | --- | --- |
| Build | `cargo build` | exit 0 | each implementer |
| Unit / integration | `cargo test` | all pass; new tests included | each implementer |
| Format | `cargo fmt --check` | exit 0 | each implementer |
| Baseline compatibility | `cargo test` after T001 with zero `src/` changes | identical pass set to pre-change baseline | orchestrator |
| Acceptance | per-task `Verification:` commands from `tasks.md` | as stated per task | orchestrator (review gate) |
| Manual | doc walkthrough (AC-008.1..3); latency measurement (SPEC-AS-007) | recorded in `validation.md` | orchestrator |

## TDD Policy

`TDD: yes` tasks — exactly one, citing its necessity trigger:

- T005 — trigger: security-sensitive boundary (the trust gate decides which profile, and therefore which credentials, a repo file may select) — tests authored in T004.

T004 routes at orchestrator-equivalent capability (host high-capability native lane), its reviewed failing suite is checkpointed before T005 dispatches, and T005's brief lists the test files as a read-only contract.

## Rollback Plan

- All work is on branch `sdd/003-project-config`; abandon by not merging. Post-merge revert is a single `git revert` of the merge commit: the feature adds a module and extends signatures without data migration. User-side rollback: delete `.agentenv.toml` files and the state-dir `trust.toml`; no other state exists.

## Plan Review Checklist

- [x] Every spec requirement has at least one implementation task (Coverage table in `tasks.md`).
- [x] Every task has a verification method with expected output.
- [x] Parallel tasks do not edit the same files (only T002 ∥ T003).
- [x] The single `TDD: yes` cites a necessity trigger and a paired test-authoring task.
- [x] Acceptance gates are executable locally (`cargo test`; manual steps enumerated).
- [x] No unresolved unknowns: every Research Decisions row has a decision.
- [x] Principles Check passes; both deviations have Complexity Tracking rows.
- [x] Global Constraints carry exact values copied from the approved artifacts.
