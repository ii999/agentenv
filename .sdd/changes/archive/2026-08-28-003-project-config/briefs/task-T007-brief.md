# Task Brief: T007

## Change

- Change ID: 003-project-config
- Task group: ### Group 3: Phase 1 CLI integration
- Task ID: T007
- Report path: .sdd/changes/003-project-config/reports/task-T007-report.md

## Task Text

```markdown
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

Generated on 2026-08-28 against tracked working tree at commit a1d8c11 (clean)

This map is a verified starting point and NOT a complete boundary. Earlier tasks may have shifted call sites. The worker MUST re-verify each seed with `git grep -n -F -e <seed> -- ':(exclude).sdd/'` before implementing (identical semantics to generation) and record differences in the report's Impact Delta section. Search domain is the tracked working tree; untracked files are not searched.

### Seed: `select_profile` (26 call sites)

- src/cli/mod.rs:217
- src/cli/mod.rs:242
- src/cli/mod.rs:271
- src/cli/mod.rs:285
- src/cli/mod.rs:320
- src/cli/mod.rs:362
- src/cli/mod.rs:368
- src/config/model.rs:152
- src/config/model.rs:430
- src/config/model.rs:434
- src/config/model.rs:440
- src/config/model.rs:444
- src/config/model.rs:450
- src/config/model.rs:453
- src/config/model.rs:459
- src/config/model.rs:463
- src/config/model.rs:469
- src/config/model.rs:472
- src/config/model.rs:481
- src/config/model.rs:484
- src/config/model.rs:496
- src/config/model.rs:498
- src/config/model.rs:508
- src/config/model.rs:511
- src/config/write.rs:165
- src/config/write.rs:446

### Seed: `resolve_write_profile` (2 call sites)

- src/config/write.rs:87
- src/config/write.rs:416

### Seed: `run_ac` (158 call sites)

- tests/credential_p2.rs:20
- tests/credential_p2.rs:34
- tests/credential_p2.rs:42
- tests/credential_p2.rs:77
- tests/credential_p2.rs:125
- tests/credential_p2.rs:167
- tests/credential_p2.rs:175
- tests/credential_p2.rs:216
- tests/credential_p2.rs:231
- tests/credential_p2.rs:352
- tests/helpers/mod.rs:24
- tests/helpers/mod.rs:62
- tests/helpers/mod.rs:85
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

Truncated: showing 50 of 158 matches — the map for this seed is partial; run the full search yourself.

### Seed: `AppError` (238 call sites)

- plans/001-project-scoped-config-design.md:59
- src/cli/credential.rs:8
- src/cli/credential.rs:12
- src/cli/credential.rs:22
- src/cli/credential.rs:26
- src/cli/credential.rs:41
- src/cli/credential.rs:43
- src/cli/credential.rs:50
- src/cli/credential.rs:62
- src/cli/credential.rs:68
- src/cli/credential.rs:72
- src/cli/credential.rs:80
- src/cli/credential.rs:90
- src/cli/mod.rs:16
- src/cli/mod.rs:175
- src/cli/mod.rs:179
- src/cli/mod.rs:212
- src/cli/mod.rs:226
- src/cli/mod.rs:299
- src/cli/mod.rs:366
- src/cli/validate.rs:8
- src/cli/validate.rs:10
- src/cli/validate.rs:15
- src/cli/validate.rs:17
- src/cli/validate.rs:27
- src/cli/validate.rs:34
- src/cli/validate.rs:41
- src/cli/write.rs:6
- src/cli/write.rs:19
- src/cli/write.rs:28
- src/cli/write.rs:64
- src/cli/write.rs:76
- src/cli/write.rs:81
- src/config/locate.rs:12
- src/config/locate.rs:23
- src/config/locate.rs:34
- src/config/locate.rs:47
- src/config/locate.rs:57
- src/config/locate.rs:61
- src/config/locate.rs:75
- src/config/locate.rs:162
- src/config/locate.rs:178
- src/config/locate.rs:215
- src/config/mod.rs:4
- src/config/mod.rs:25
- src/config/mod.rs:44
- src/config/mod.rs:50
- src/config/mod.rs:58
- src/config/mod.rs:66
- src/config/mod.rs:68

Truncated: showing 50 of 238 matches — the map for this seed is partial; run the full search yourself.

## Do Not Explore

Planning confirmed these regions are unaffected. Exploration budget must not be spent there. Touching them requires reporting BLOCKED or NEEDS_CONTEXT.

- ``src/project/` (read-only)`
- `tests/project_trust.rs`
- ``src/runner.rs` (launch semantics unchanged; only the prelude order guarantees the notice)`

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
.sdd/changes/003-project-config/reports/task-T007-report.md
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

- The facade is complete: `src/project/mod.rs` exposes `resolve(cwd, env) -> Result<ProjectContext, AppError>`, `ProjectContext::{None, Untrusted{path, reason, meta}, Trusted{path, meta}}`, `UntrustedReason::{New, Changed, Invalid(Vec<Violation>), StateUnavailable(String)}`, and `model::ProjectPin { name, file }` (the pin arrives inside `Trusted.meta.pin`, already carrying the canonical file path).
- Task-text item 2 is ALREADY DONE: `AppError::ProjectTrust(String)` exists in `src/error.rs` mapping to exit status 5 (landed with T006). Do not re-add it.
- Your notice logic consumes the facade only — never re-read or re-parse the project file. The untrusted notice is one stderr line: the file path, `agentenv project status`, and a next action; for `UntrustedReason::StateUnavailable`, the line also names the unresolvable state location/variables (the reason string carries them).
- Scope note: wiring for the `project` subcommand group itself is T008. In THIS task, add the clap plumbing only so far as the prelude needs it: the global `--no-project` flag, and a rule that the prelude skips notice emission (but the later `project` group will always discover). If adding a placeholder `project` subcommand variant would help T008, do NOT: leave the enum untouched; T008 owns it.
- The existing dispatch returns `Result<Output, AppError>` (see `src/cli/mod.rs` and `src/main.rs`). Extend the outcome so an exit status can accompany output (needed by T008 for report-with-exit-5/6); keep every existing command's observable behavior identical. Prefer the smallest coherent refactor (for example `Output { stdout, stderr, status }` defaulting to 0) over a parallel type.
- `run` on Unix replaces the process via `exec` (`src/runner.rs:231`): the prelude MUST write and flush the notice to the real stderr before command dispatch, not into the buffered `Output`, or it is lost for `run`. Existing commands' stderr buffering may remain as is.
- Precedence tests: build on `tests/helpers/mod.rs` — `command_with_project_discovery(config)` returns a prepared `assert_cmd` command WITHOUT the `AGENTENV_NO_PROJECT` bypass (use it for all project-behavior tests; set `current_dir` further into your temp tree as needed and set `XDG_STATE_HOME` to a temp dir for trust state). `run_ac` keeps the bypass for ordinary tests.
- AC-004.8's second half (create-profile exemption) asserts the EXISTING usage error when `--create-profile` is passed without `--profile` — that behavior already exists; your test just proves the pin does not change it.
