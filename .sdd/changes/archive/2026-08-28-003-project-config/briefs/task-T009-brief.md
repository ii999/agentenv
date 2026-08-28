# Task Brief: T009

## Change

- Change ID: 003-project-config
- Task group: ### Group 3: Phase 1 CLI integration
- Task ID: T009
- Report path: .sdd/changes/003-project-config/reports/task-T009-report.md

## Task Text

```markdown
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

Generated on 2026-08-28 against tracked working tree at commit dca6e59 (clean)

This map is a verified starting point and NOT a complete boundary. Earlier tasks may have shifted call sites. The worker MUST re-verify each seed with `git grep -n -F -e <seed> -- ':(exclude).sdd/'` before implementing (identical semantics to generation) and record differences in the report's Impact Delta section. Search domain is the tracked working tree; untracked files are not searched.

No existing call sites are expected. Any discovered coupling must be reported in Impact Delta.

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
.sdd/changes/003-project-config/reports/task-T009-report.md
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

The complete behavioral surface exists at your base commit. Facts you need:

- Test helpers: `run_ac(config, envs, args)` (bypasses discovery) and `command_with_project_discovery(config)` (does not) in `tests/helpers/mod.rs`; `SENTINELS` there holds the planted-secret sentinels and `run_ac` auto-checks them.
- Project-behavior tests must set `current_dir` inside a temp tree containing the `.agentenv.toml` and set `XDG_STATE_HOME` to a temp dir; approve files by running `["project", "allow"]` through the binary.
- `test-probe` reports its environment (see `tests/run_p3.rs` for the invocation pattern) — use it for AC-010.5.
- Existing security suites `tests/security_p1.rs` / `tests/security_p3.rs` are the structural model; `tests/fixtures/counting_provider.sh` counts executions via a file path argument (see `tests/credential_p2.rs` usage).
- AC-010.1/AC-010.2 sentinels: place distinctive strings (reuse the `SENTINELS` values or new ones) in the project file's forbidden/malformed positions and in `profile`/`reason` values; assert absence from all stdout/stderr of the exercised commands (the notice must name only the file path).
- Existing snapshot byte-identity (AC-009.2): the pre-existing snapshots live under `tests/snapshots/` (`list.json`, `entry.json`, `credentials.json`, `find.json`, `profiles.json`, `raw-get.json`) and are already asserted by `tests/query_p1.rs` — your task only needs to confirm the suite passes; do not duplicate those assertions.
- Do not touch `src/` or existing test files; new tests go in `tests/project_security.rs` (+ fixtures under `tests/fixtures/project/`).
