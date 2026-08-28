# Task Brief: T002

## Change

- Change ID: 003-project-config
- Task group: ### Group 2: Phase 1 leaf modules
- Task ID: T002
- Report path: .sdd/changes/003-project-config/reports/task-T002-report.md

## Task Text

```markdown
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
```

## Global Constraints

These bind this task in addition to its own requirements:

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

## Impact Map

Generated on 2026-08-28 against tracked working tree at commit f68e71d (clean)

This map is a verified starting point and NOT a complete boundary. Earlier tasks may have shifted call sites. The worker MUST re-verify each seed with `git grep -n -F -e <seed> -- ':(exclude).sdd/'` before implementing (identical semantics to generation) and record differences in the report's Impact Delta section. Search domain is the tracked working tree; untracked files are not searched.

### Seed: `Segments::parse` (9 call sites)

- src/cli/mod.rs:286
- src/config/validate.rs:226
- src/config/write.rs:85
- src/config/write.rs:159
- src/path.rs:83
- src/path.rs:219
- src/path.rs:262
- src/path.rs:285
- src/runner.rs:188

### Seed: `Violation` (68 call sites)

- src/cli/validate.rs:34
- src/cli/validate.rs:41
- src/config/locate.rs:12
- src/config/locate.rs:47
- src/config/locate.rs:61
- src/config/mod.rs:25
- src/config/mod.rs:68
- src/config/mod.rs:103
- src/config/validate.rs:15
- src/config/validate.rs:30
- src/config/validate.rs:41
- src/config/validate.rs:45
- src/config/validate.rs:52
- src/config/validate.rs:57
- src/config/validate.rs:65
- src/config/validate.rs:71
- src/config/validate.rs:82
- src/config/validate.rs:92
- src/config/validate.rs:96
- src/config/validate.rs:107
- src/config/validate.rs:113
- src/config/validate.rs:125
- src/config/validate.rs:144
- src/config/validate.rs:165
- src/config/validate.rs:169
- src/config/validate.rs:173
- src/config/validate.rs:177
- src/config/validate.rs:190
- src/config/validate.rs:197
- src/config/validate.rs:208
- src/config/validate.rs:217
- src/config/validate.rs:229
- src/config/validate.rs:241
- src/config/validate.rs:251
- src/config/validate.rs:260
- src/config/validate.rs:268
- src/config/validate.rs:279
- src/config/validate.rs:307
- src/config/validate.rs:312
- src/config/validate.rs:319
- src/config/validate.rs:386
- src/config/validate.rs:430
- src/config/validate.rs:440
- src/config/validate.rs:461
- src/config/validate.rs:482
- src/config/validate.rs:486
- src/config/validate.rs:496
- src/config/validate.rs:511
- src/config/validate.rs:514
- src/config/validate.rs:532

Truncated: showing 50 of 68 matches — the map for this seed is partial; run the full search yourself.

## Do Not Explore

Planning confirmed these regions are unaffected. Exploration budget must not be spent there. Touching them requires reporting BLOCKED or NEEDS_CONTEXT.

- `src/cli/`
- `src/config/`
- `src/main.rs`
- `src/runner.rs`

## Relevant Source Artifacts

- PRD: /Users/zhaiqifeng/Dev/agentenv/.sdd/changes/003-project-config/prd.md
- Architecture: /Users/zhaiqifeng/Dev/agentenv/.sdd/changes/003-project-config/architecture.md
- Spec: /Users/zhaiqifeng/Dev/agentenv/.sdd/changes/003-project-config/spec.md
- Plan: /Users/zhaiqifeng/Dev/agentenv/.sdd/changes/003-project-config/plan.md
- Tasks: /Users/zhaiqifeng/Dev/agentenv/.sdd/changes/003-project-config/tasks.md

## Binding Instructions

Read only the artifacts needed to complete this task. Preserve exact acceptance criteria and constraints from the spec. Implement only this task's scope.

## Report Contract

Write the implementation report to this path, relative to the root of the
repository you are working in:

```text
.sdd/changes/003-project-config/reports/task-T002-report.md
```

Resolve it inside your own workspace. Running in a git worktree, that is the
worktree root, so the report travels with the change it describes; never write
it into any other checkout.

Follow `templates/implementation-report.md`. The report must open with a
`# ` title, carry `Status: <value>`, `Provider: <provider>`, and `Model: <model>`
as plain header lines before the first `##` section, and include the
`## Implemented`, `## Verification`, `## Files Changed`, and `## Concerns`
sections. `<value>` is DONE, DONE_WITH_CONCERNS, NEEDS_CONTEXT, or BLOCKED on
one plain `Status:` line, not a `## Status` heading with the value underneath.

Return only:

- The `Status: <value>` line
- Changed files or commit summary
- Verification summary
- Concerns, if any

## Orchestrator addendum (binding)

The module scaffold already exists at the base commit: `src/project/mod.rs` declares `pub mod locate; pub mod model; pub mod trust;`, the three submodule files exist (near-empty), and `src/lib.rs` already declares `pub mod project;`. Do NOT modify `src/project/mod.rs` or `src/lib.rs`; write only your own submodule file(s) and test/fixture files (plus `Cargo.toml` if and only if your task lists it). This supersedes any instruction in the task text about adding `pub mod` lines or creating `mod.rs`.
