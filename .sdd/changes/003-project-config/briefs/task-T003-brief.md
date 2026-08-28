# Task Brief: T003

## Change

- Change ID: 003-project-config
- Task group: ### Group 2: Phase 1 leaf modules
- Task ID: T003
- Report path: .sdd/changes/003-project-config/reports/task-T003-report.md

## Task Text

```markdown
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

Generated on 2026-08-28 against tracked working tree at commit f68e71d (dirty)

This map is a verified starting point and NOT a complete boundary. Earlier tasks may have shifted call sites. The worker MUST re-verify each seed with `git grep -n -F -e <seed> -- ':(exclude).sdd/'` before implementing (identical semantics to generation) and record differences in the report's Impact Delta section. Search domain is the tracked working tree; untracked files are not searched.

No existing call sites are expected. Any discovered coupling must be reported in Impact Delta.

## Do Not Explore

Planning confirmed these regions are unaffected. Exploration budget must not be spent there. Touching them requires reporting BLOCKED or NEEDS_CONTEXT.

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
.sdd/changes/003-project-config/reports/task-T003-report.md
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
