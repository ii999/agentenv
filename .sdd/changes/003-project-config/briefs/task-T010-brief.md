# Task Brief: T010

## Change

- Change ID: 003-project-config
- Task group: ### Group 4: Phase 2 — Docs and protocol
- Task ID: T010
- Report path: .sdd/changes/003-project-config/reports/task-T010-report.md

## Task Text

```markdown
- [ ] T010 [Phase 2] Update README, agent skill, and pairing documentation
  - Files: `README.md`, `skills/agentenv/SKILL.md`
  - Depends on: T009
  - Spec refs: SPEC-008 (read in full — it enumerates every required topic)
  - Acceptance refs: AC-008.1, AC-008.2, AC-008.3
  - Task: Document per SPEC-008: project file schema (incl. 64 KiB limit) and discovery; trust lifecycle (`project status/allow/revoke`); extended exit-status table (add 5 and 6; note status 2 also covers project-file validation errors); the `project status --json` stdout-with-nonzero-exit deviation; precedence chain incl. pin and the `--create-profile` exemption; bypass (`--no-project`/`AGENTENV_NO_PROJECT`) and its non-application to `project` subcommands; the `.env`/docker-compose pairing section with a worked example (`agentenv run --with llm -- docker compose up` + a compose snippet using `environment: - OPENAI_API_KEY` passthrough or `${OPENAI_API_KEY}` interpolation; name `env_file:`-with-secrets as the anti-pattern); update the README `AGENTS.md` block and the skill's reading protocol to begin with `agentenv project status --json`. Follow `~/.agents/agent-standards/user-facing-copy.md`; match the README's existing register and structure.
  - Interfaces: Consumes: the shipped CLI behavior (T007/T008). Produces: none.
  - Impact seeds: none
  - No-go: `src/`, `tests/`
  - TDD: no
  - Dispatch: agent (impl-standard; prose quality gated at review against user-facing-copy standards)
  - Verification: `cargo build` — expected: exit 0 (no code touched: `git status --porcelain -- src/ tests/` empty); manual: execute each documented `project` command sequence and the compose example in a scratch tree against the built binary — expected: behavior matches the text (record transcript in the task report).
  - Report: `.sdd/changes/003-project-config/reports/task-T010-report.md`

Checkpoint: Documentation matches shipped behavior; AC-008.1, AC-008.2, AC-008.3 walked and recorded.
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

Generated on 2026-08-28 against tracked working tree at commit 705fcd7 (clean)

This map is a verified starting point and NOT a complete boundary. Earlier tasks may have shifted call sites. The worker MUST re-verify each seed with `git grep -n -F -e <seed> -- ':(exclude).sdd/'` before implementing (identical semantics to generation) and record differences in the report's Impact Delta section. Search domain is the tracked working tree; untracked files are not searched.

No existing call sites are expected. Any discovered coupling must be reported in Impact Delta.

## Do Not Explore

Planning confirmed these regions are unaffected. Exploration budget must not be spent there. Touching them requires reporting BLOCKED or NEEDS_CONTEXT.

- `src/`
- `tests/`

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
.sdd/changes/003-project-config/reports/task-T010-report.md
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

The complete behavior exists at your base commit. Ground every sentence in observed behavior — build the binary (`cargo build`) and run each command sequence you document in a scratch temp tree before writing it down; record the transcript in your report per the task's manual verification.

Facts to carry into the docs (verify each against the binary, not this list):

- New commands: `agentenv project status [--json]`, `agentenv project allow`, `agentenv project revoke`. New global flag `--no-project`; env `AGENTENV_NO_PROJECT` (non-empty; does not apply to the `project` subcommands, which always discover).
- Exit statuses: existing 0–4/127 unchanged; new `5` = project trust-state failure (`status` on an untrusted/invalid/unavailable file; `allow`/`revoke` with no discovered file); new `6` = requirements unsatisfied or uncheckable (`status` only); status `2` also covers project-file validation errors, corrupt trust store, and an unreadable approved file.
- `project status --json` emits its report on stdout together with exit 5/6 — document this explicitly as the one exception to the "failing --json invocations leave stdout empty" rule; exit-2 failures produce no stdout.
- Precedence: `--profile` > `AGENTENV_PROFILE` > trusted project pin > `default_profile`; applies to reads, `run`, `set`, `unset`; `--create-profile` still requires an explicit `--profile` and never consults the pin.
- Project file: `.agentenv.toml`, nearest regular file walking CWD→root; closed schema (`version = 1`, optional `profile`, optional `[requires.<entry>]` with `reason` and optional entry-relative `fields`); 64 KiB limit; no values, credentials, `inject` tables, or `credential://` strings.
- Trust: approval binds the exact bytes (any edit re-inerts); state lives in the user state directory, never the repo; untrusted files are inert except one stderr notice.
- Register/style: match the README's existing voice and structure (see the current Configuration/Safety sections); follow `~/.agents/agent-standards/user-facing-copy.md`. Update the skill's reading protocol so step 1 is `agentenv project status --json`, and update the README's `AGENTS.md` block equivalently.
- Docker compose pairing: worked example must use `agentenv run --with <entry> -- docker compose up` with `environment: - VAR` passthrough or `${VAR}` interpolation; name `env_file:`-with-secrets as the anti-pattern; `.env` files hold non-secret values only.
- Do not touch `src/` or `tests/`; only `README.md` and `skills/agentenv/SKILL.md`.
