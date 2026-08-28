# Task Brief: T008

## Change

- Change ID: 003-project-config
- Task group: ### Group 3: Phase 1 CLI integration
- Task ID: T008
- Report path: .sdd/changes/003-project-config/reports/task-T008-report.md

## Task Text

```markdown
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

Generated on 2026-08-28 against tracked working tree at commit b41b336 (clean)

This map is a verified starting point and NOT a complete boundary. Earlier tasks may have shifted call sites. The worker MUST re-verify each seed with `git grep -n -F -e <seed> -- ':(exclude).sdd/'` before implementing (identical semantics to generation) and record differences in the report's Impact Delta section. Search domain is the tracked working tree; untracked files are not searched.

### Seed: ``Command` (clap enum in `src/cli/mod.rs`)` (0 call sites)

> **WARNING:** Seed ``Command` (clap enum in `src/cli/mod.rs`)` matched 0 call sites in the tracked working tree (excluding `.sdd/`). This may indicate name drift. Resolve the correct symbol before implementing; do not guess a similar name.

### Seed: `resolve_in_entry` (4 call sites)

- src/config/validate.rs:250
- src/config/validate.rs:291
- src/runner.rs:13
- src/runner.rs:189

### Seed: `entry_table` (16 call sites)

- src/config/validate.rs:189
- src/config/validate.rs:193
- src/config/validate.rs:250
- src/config/validate.rs:292
- src/config/validate.rs:296
- src/config/validate.rs:305
- src/config/validate.rs:309
- src/config/validate.rs:339
- src/config/validate.rs:342
- src/query/mod.rs:104
- src/query/mod.rs:108
- src/query/mod.rs:190
- src/query/mod.rs:193
- src/query/mod.rs:210
- src/runner.rs:18
- src/runner.rs:81

## Do Not Explore

Planning confirmed these regions are unaffected. Exploration budget must not be spent there. Touching them requires reporting BLOCKED or NEEDS_CONTEXT.

- ``src/project/` (read-only)`
- `src/runner.rs`
- `src/config/write.rs`

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
.sdd/changes/003-project-config/reports/task-T008-report.md
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

State at your base commit (read these files before coding):

- The facade (`src/project/mod.rs`) provides `resolve/allow/revoke`, `ProjectContext`, `UntrustedReason::{New, Changed, Invalid(Vec<Violation>), StateUnavailable(String)}`, `AllowOutcome { path, already_current }`, `RevokeOutcome { path, record_existed }`; `meta` carries `pin: Option<ProjectPin>` and `requires: Vec<Requirement>` in declaration order.
- The prelude in `src/main.rs` (`resolve_project_context` + `write_untrusted_project_notice`) currently applies the bypass and emits the untrusted notice for EVERY command, and passes `ProjectContext` into `Invocation.project`. You MUST adjust it for the `project` subcommand group per SPEC-001/SPEC-005: when the parsed command is in the `project` group, the prelude neither applies the bypass nor emits the notice, and the group performs its own discovery via the facade (`resolve`/`allow`/`revoke`) so AC-001.5 and AC-006.11 hold. Keep prelude behavior for every other command byte-identical.
- `Output` now carries `status: i32` (default 0) and `main.rs` exits with it; `AppError::ProjectTrust(String)` maps to exit 5. Use `Output.status` for the report-with-5/6 cases; use errors only where the spec says no report is produced (corrupt store, trusted-unreadable ⇒ exit 2; `allow`/`revoke` failures).
- The requirement check consumes `resolve_in_entry` and `entry_table`: see `src/config/validate.rs` (`resolve_in_entry(entry, &Segments)`) and `src/query/mod.rs` (`entry_table(profile, name)`). A `fields` member resolving to ANY value satisfies (scalar, table, array, credential reference) — do not reuse inject's scalar-only restriction.
- Selection for the requirement report: `config.select_profile(flag, env_profile, pin)` exactly as other commands (see `src/cli/mod.rs` for how the flag/env/pin arrive); degraded selection (missing/unparseable config, no selectable profile, dangling pin) must NOT error — render `requirements.checked = false` with the reason and next action per SPEC-006's member state table, and compute the exit from the first-match matrix.
- JSON: follow the frozen envelope and member state table in SPEC-006 VERBATIM (member names, nullability, `entries` empty unless trusted+checked, `trust_reason` non-null only for `unavailable`). Emit the JSON document on stdout even when exiting 5/6; exit-2 paths produce no stdout. Follow the JSON style of existing surfaces in `src/query/render.rs` (serde_json with preserve_order).
- Snapshots: place one JSON snapshot per member-state-table row under `tests/snapshots/` named `project-status-<state>.json`, and normalize machine-dependent members (absolute paths) the way existing snapshot tests handle temp paths — if existing tests don't normalize, compare structurally in the test rather than byte-comparing paths (byte-stable members stay snapshot-asserted).
- Use `command_with_project_discovery(config)` from `tests/helpers/mod.rs` for project-behavior tests; set `XDG_STATE_HOME` to a temp dir for trust state; `tests/fixtures/counting_provider.sh` proves AC-007.4/AC-010.4-style no-provider-execution.
