# Task Brief: T001

## Change

- Change ID: 003-project-config
- Task group: ### Group 1: Foundation
- Task ID: T001
- Report path: .sdd/changes/003-project-config/reports/task-T001-report.md

## Task Text

```markdown
- [ ] T001 [Foundation] Make every test invocation hermetic to project discovery
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

Generated on 2026-08-28 against tracked working tree at commit 645d10e (clean)

This map is a verified starting point and NOT a complete boundary. Earlier tasks may have shifted call sites. The worker MUST re-verify each seed with `git grep -n -F -e <seed> -- ':(exclude).sdd/'` before implementing (identical semantics to generation) and record differences in the report's Impact Delta section. Search domain is the tracked working tree; untracked files are not searched.

### Seed: `run_ac` (157 call sites)

- tests/credential_p2.rs:20
- tests/credential_p2.rs:32
- tests/credential_p2.rs:40
- tests/credential_p2.rs:75
- tests/credential_p2.rs:123
- tests/credential_p2.rs:165
- tests/credential_p2.rs:173
- tests/credential_p2.rs:214
- tests/credential_p2.rs:229
- tests/credential_p2.rs:343
- tests/helpers/mod.rs:24
- tests/helpers/mod.rs:64
- tests/query_p1.rs:8
- tests/query_p1.rs:14
- tests/query_p1.rs:23
- tests/query_p1.rs:31
- tests/query_p1.rs:44
- tests/query_p1.rs:48
- tests/query_p1.rs:52
- tests/query_p1.rs:61
- tests/query_p1.rs:81
- tests/query_p1.rs:86
- tests/query_p1.rs:87
- tests/query_p1.rs:97
- tests/query_p1.rs:101
- tests/query_p1.rs:104
- tests/query_p1.rs:112
- tests/query_p1.rs:153
- tests/query_p1.rs:165
- tests/query_p1.rs:169
- tests/query_p1.rs:190
- tests/query_p1.rs:199
- tests/query_p1.rs:209
- tests/query_p1.rs:225
- tests/query_p1.rs:234
- tests/query_p1.rs:247
- tests/query_p1.rs:253
- tests/query_p1.rs:259
- tests/query_p1.rs:265
- tests/query_p1.rs:279
- tests/query_p1.rs:283
- tests/query_p1.rs:300
- tests/query_p1.rs:313
- tests/query_p1.rs:330
- tests/query_p1.rs:343
- tests/query_p1.rs:347
- tests/query_p1.rs:351
- tests/query_p1.rs:366
- tests/run_p3.rs:10
- tests/run_p3.rs:20

Truncated: showing 50 of 157 matches — the map for this seed is partial; run the full search yourself.

### Seed: `Command::new` (2 call sites)

- src/credential/command.rs:39
- src/runner.rs:224

### Seed: `CommandBuilder` (2 call sites)

- tests/credential_p2.rs:22
- tests/credential_p2.rs:257

## Do Not Explore

Planning confirmed these regions are unaffected. Exploration budget must not be spent there. Touching them requires reporting BLOCKED or NEEDS_CONTEXT.

- `src/`

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
.sdd/changes/003-project-config/reports/task-T001-report.md
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
