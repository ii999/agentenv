# Task Brief: T005

## Change

- Change ID: 003-project-config
- Task group: ### Group 2: Phase 1 leaf modules
- Task ID: T005
- Report path: .sdd/changes/003-project-config/reports/task-T005-report.md

## Task Text

```markdown
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

Generated on 2026-08-28 against tracked working tree at commit 4ae601d (clean)

This map is a verified starting point and NOT a complete boundary. Earlier tasks may have shifted call sites. The worker MUST re-verify each seed with `git grep -n -F -e <seed> -- ':(exclude).sdd/'` before implementing (identical semantics to generation) and record differences in the report's Impact Delta section. Search domain is the tracked working tree; untracked files are not searched.

### Seed: `TrustStore` (16 call sites)

- src/project/trust.rs:169
- src/project/trust.rs:180
- src/project/trust.rs:188
- src/project/trust.rs:248
- src/project/trust.rs:358
- tests/project_trust.rs:13
- tests/project_trust.rs:95
- tests/project_trust.rs:125
- tests/project_trust.rs:142
- tests/project_trust.rs:159
- tests/project_trust.rs:183
- tests/project_trust.rs:208
- tests/project_trust.rs:213
- tests/project_trust.rs:237
- tests/project_trust.rs:259
- tests/project_trust.rs:265

### Seed: `store_path` (19 call sites)

- src/credential/test_store.rs:114
- src/project/trust.rs:122
- src/project/trust.rs:248
- src/project/trust.rs:384
- src/project/trust.rs:392
- src/project/trust.rs:403
- src/project/trust.rs:414
- src/project/trust.rs:426
- src/project/trust.rs:440
- src/project/trust.rs:454
- src/project/trust.rs:466
- tests/credential_p2.rs:46
- tests/credential_p2.rs:139
- tests/credential_p2.rs:187
- tests/credential_p2.rs:188
- tests/credential_p2.rs:195
- tests/credential_p2.rs:271
- tests/credential_p2.rs:356
- tests/credential_p2.rs:437

### Seed: `fingerprint` (33 call sites)

- src/project/trust.rs:4
- src/project/trust.rs:165
- src/project/trust.rs:166
- src/project/trust.rs:192
- src/project/trust.rs:208
- src/project/trust.rs:232
- src/project/trust.rs:235
- src/project/trust.rs:237
- tests/project_trust.rs:13
- tests/project_trust.rs:58
- tests/project_trust.rs:61
- tests/project_trust.rs:62
- tests/project_trust.rs:63
- tests/project_trust.rs:66
- tests/project_trust.rs:67
- tests/project_trust.rs:68
- tests/project_trust.rs:71
- tests/project_trust.rs:72
- tests/project_trust.rs:73
- tests/project_trust.rs:76
- tests/project_trust.rs:77
- tests/project_trust.rs:78
- tests/project_trust.rs:81
- tests/project_trust.rs:85
- tests/project_trust.rs:101
- tests/project_trust.rs:102
- tests/project_trust.rs:131
- tests/project_trust.rs:216
- tests/project_trust.rs:221
- tests/project_trust.rs:245
- tests/project_trust.rs:272
- tests/project_trust.rs:273
- tests/project_trust.rs:277

### Seed: `StoreFs` (10 call sites)

- src/project/trust.rs:15
- src/project/trust.rs:34
- src/project/trust.rs:49
- src/project/trust.rs:60
- src/project/trust.rs:188
- src/project/trust.rs:227
- src/project/trust.rs:248
- src/project/trust.rs:277
- src/project/trust.rs:310
- tests/project_trust.rs:6

## Do Not Explore

Planning confirmed these regions are unaffected. Exploration budget must not be spent there. Touching them requires reporting BLOCKED or NEEDS_CONTEXT.

- `tests/project_trust.rs`
- `src/cli/`
- `src/config/`
- `src/main.rs`

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
.sdd/changes/003-project-config/reports/task-T005-report.md
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

The T004 contract is already in the tree at your base commit: `src/project/trust.rs` contains the pinned interfaces with `todo!()` bodies, and `tests/project_trust.rs` plus the in-file `#[cfg(test)]` unit tests (including the `FaultyFs` fault-injection test) are the read-only specification. Current state: `cargo test --test project_trust` fails 0/8 by design; one lib unit test (`project::trust::tests::a_failing_commit_leaves_the_previous_store_intact`) also fails by design; everything else is green.

Specifics pinned by T004 that you must honor:

- `RealFs` and `store_path` are ALREADY implemented — do not rewrite them. Implement only: `TrustStore::{load, lookup, allow, revoke, save}` and `fingerprint`, plus private helpers.
- `TrustStore.records` is `BTreeMap<String, String>` (canonical path string → hex SHA-256); the representation is yours to keep or change as long as every pinned public signature is untouched.
- Three diagnostics are contractually asserted to name the full store path AND the next action `agentenv project allow`: corrupt store on load, failed commit in save, and the unset-state-base error (the last is already implemented in `store_path`).
- Remove the `#[allow(dead_code)]` on `records` and the `#[allow(unused_variables)]` on the `impl TrustStore` block and on `fingerprint` once bodies land.
- `sha2 = "0.10"` is already in `Cargo.toml`.
- Do NOT modify `tests/project_trust.rs`, any pinned signature, `src/project/{mod,model,locate}.rs`, or `src/lib.rs`. Test files are a read-only contract; an implementer modifying them is a quality failure.

Done means: `cargo test --test project_trust` all pass; `cargo test` fully green (the lib fault-injection test included); `cargo fmt --check` clean; no `todo!()` remains in `src/project/trust.rs` (`grep -c 'todo!' src/project/trust.rs` = 0).
