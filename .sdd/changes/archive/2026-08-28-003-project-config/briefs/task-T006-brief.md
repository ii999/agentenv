# Task Brief: T006

## Change

- Change ID: 003-project-config
- Task group: ### Group 2: Phase 1 leaf modules
- Task ID: T006
- Report path: .sdd/changes/003-project-config/reports/task-T006-report.md

## Task Text

```markdown
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

Generated on 2026-08-28 against tracked working tree at commit b2e0d94 (clean)

This map is a verified starting point and NOT a complete boundary. Earlier tasks may have shifted call sites. The worker MUST re-verify each seed with `git grep -n -F -e <seed> -- ':(exclude).sdd/'` before implementing (identical semantics to generation) and record differences in the report's Impact Delta section. Search domain is the tracked working tree; untracked files are not searched.

### Seed: `ProjectContext` (1 call sites)

- src/project/mod.rs:4

### Seed: `UntrustedReason` (0 call sites)

> **WARNING:** Seed `UntrustedReason` matched 0 call sites in the tracked working tree (excluding `.sdd/`). This may indicate name drift. Resolve the correct symbol before implementing; do not guess a similar name.

### Seed: `resolve` (134 call sites)

- README.md:198
- README.md:259
- README.md:292
- README.md:342
- README.md:344
- README.md:356
- plans/001-project-scoped-config-design.md:52
- plans/001-project-scoped-config-design.md:56
- plans/001-project-scoped-config-design.md:244
- skills/agentenv/SKILL.md:28
- skills/agentenv/SKILL.md:84
- skills/agentenv/SKILL.md:116
- skills/agentenv/SKILL.md:117
- src/cli/credential.rs:15
- src/cli/mod.rs:41
- src/cli/mod.rs:219
- src/cli/validate.rs:32
- src/config/locate.rs:19
- src/config/locate.rs:20
- src/config/locate.rs:74
- src/config/locate.rs:90
- src/config/locate.rs:98
- src/config/locate.rs:107
- src/config/locate.rs:118
- src/config/locate.rs:127
- src/config/locate.rs:138
- src/config/locate.rs:149
- src/config/locate.rs:161
- src/config/locate.rs:177
- src/config/locate.rs:189
- src/config/locate.rs:203
- src/config/locate.rs:214
- src/config/mod.rs:51
- src/config/mod.rs:241
- src/config/model.rs:164
- src/config/model.rs:167
- src/config/model.rs:170
- src/config/model.rs:175
- src/config/validate.rs:250
- src/config/validate.rs:254
- src/config/validate.rs:291
- src/config/validate.rs:979
- src/config/validate.rs:1033
- src/config/validate.rs:1145
- src/config/validate.rs:1146
- src/config/validate.rs:1148
- src/config/validate.rs:1149
- src/config/write.rs:87
- src/config/write.rs:179
- src/config/write.rs:182

Truncated: showing 50 of 134 matches — the map for this seed is partial; run the full search yourself.

### Seed: `allow` (71 call sites)

- README.md:291
- plans/001-project-scoped-config-design.md:161
- plans/001-project-scoped-config-design.md:209
- plans/001-project-scoped-config-design.md:210
- skills/agentenv/SKILL.md:116
- src/cli/mod.rs:78
- src/config/model.rs:105
- src/config/validate.rs:9
- src/config/validate.rs:99
- src/config/validate.rs:664
- src/config/validate.rs:667
- src/config/validate.rs:673
- src/config/validate.rs:678
- src/config/validate.rs:679
- src/config/write.rs:315
- src/credential/command.rs:3
- src/credential/command.rs:26
- src/credential/env.rs:4
- src/credential/env.rs:23
- src/credential/keychain.rs:1
- src/credential/keychain.rs:39
- src/credential/mod.rs:7
- src/credential/mod.rs:15
- src/credential/mod.rs:22
- src/credential/shallow.rs:1
- src/credential/shallow.rs:16
- src/credential/shallow.rs:55
- src/credential/shallow.rs:58
- src/credential/shallow.rs:161
- src/credential/shallow.rs:212
- src/credential/shallow.rs:215
- src/credential/shallow.rs:218
- src/credential/shallow.rs:228
- src/credential/shallow.rs:263
- src/credential/shallow.rs:275
- src/credential/shallow.rs:289
- src/credential/shallow.rs:301
- src/credential/shallow.rs:313
- src/credential/shallow.rs:326
- src/credential/shallow.rs:336
- src/credential/shallow.rs:340
- src/credential/shallow.rs:352
- src/credential/shallow.rs:370
- src/credential/shallow.rs:381
- src/project/model.rs:49
- src/project/model.rs:58
- src/project/model.rs:169
- src/project/model.rs:333
- src/project/trust.rs:154
- src/project/trust.rs:168

Truncated: showing 50 of 71 matches — the map for this seed is partial; run the full search yourself.

### Seed: `revoke` (9 call sites)

- src/project/trust.rs:245
- src/project/trust.rs:247
- src/project/trust.rs:502
- tests/project_trust.rs:200
- tests/project_trust.rs:226
- tests/project_trust.rs:230
- tests/project_trust.rs:231
- tests/project_trust.rs:237
- tests/project_trust.rs:241

## Do Not Explore

Planning confirmed these regions are unaffected. Exploration budget must not be spent there. Touching them requires reporting BLOCKED or NEEDS_CONTEXT.

- `src/cli/`
- `src/config/`
- `src/main.rs`
- `tests/project_trust.rs`

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
.sdd/changes/003-project-config/reports/task-T006-report.md
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

Upstream interfaces are already implemented and tested at your base commit — consume them exactly as they exist (read the files before writing code):

- `src/project/model.rs`: `pub fn parse(bytes: &[u8], file: &Path) -> Result<ProjectFileMeta, Vec<Violation>>`; `pub struct ProjectFileMeta { pub pin: Option<ProjectPin>, pub requires: Vec<Requirement> }`; `pub struct ProjectPin { pub name: String, pub file: PathBuf }`; `pub const MAX_PROJECT_FILE_BYTES`.
- `src/project/locate.rs`: `pub fn discover(cwd: &Path) -> Option<PathBuf>`.
- `src/project/trust.rs`: `pub fn store_path(env: &impl Fn(&str) -> Option<String>) -> Result<PathBuf, AppError>`; `pub struct TrustStore` with `load(path, fs)`, `lookup(canonical) -> Option<&str>`, `allow(canonical, content)`, `revoke(canonical) -> bool`, `save(path, fs)`; `pub struct RealFs`; `pub fn fingerprint(content: &[u8]) -> String`. Note `store_path` errors when the state base is unset — your facade must catch that specific case on the READ path and degrade to `Untrusted(StateUnavailable(...))` instead of propagating it, while `allow`/`revoke` propagate it (exit 2, EDGE-004b). Distinguish it from a corrupt-store load error, which always propagates.
- Scope adjustment (logged): ADD the variant `ProjectTrust(String)` to `AppError` in `src/error.rs`, mapped to exit status 5, message printed like the other variants and naming a next action. The task text assigns this to T007, but your `allow`/`revoke` need it for the no-project-file failure (AC-003.6). Add the variant and its exit mapping only; change nothing else in `src/error.rs`. T007 will reuse it.
- The facade lives in `src/project/mod.rs` (currently only module docs + `pub mod` lines — extend it; keep the existing declarations).
- Write your integration tests in `tests/project_facade.rs` using the library API directly (no CLI surface exists yet): temp trees + overridden `XDG_STATE_HOME`/`HOME` env-closures, per the style of `src/config/locate.rs` tests and `tests/project_trust.rs`.
- Classification precedence reminder (SPEC-005): `invalid` outranks `changed`; a read/canonicalize failure WITH an approval record is an error (exit 2, message names the file and the next action: restore the file or run `agentenv project revoke`); WITHOUT a record it is `Untrusted(Invalid(...))` with one violation naming the file and the failure class.
