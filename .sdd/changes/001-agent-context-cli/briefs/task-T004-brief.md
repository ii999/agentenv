# Task Brief: T004

## Change

- Change ID: 001-agent-context-cli
- Task group: ### Group 2: Phase 1 (P1, MVP) — Config core & queries
- Task ID: T004
- Report path: .sdd/changes/001-agent-context-cli/reports/task-T004-report.md

## Task Text

```markdown
- [ ] T004 [Phase 1] Query engine, renderer, and CLI for the read-only surface
  - Files: `src/query.rs`, `src/render.rs`, `src/cli/mod.rs`, `src/cli/query_cmds.rs`, `src/main.rs` (wire subcommands `list`, `show`, `get`, `find`, `validate`, `credential list`), `tests/query_p1.rs` (AC-driven integration tests for the non-security Phase-1 ACs), `tests/snapshots/` (JSON shape snapshots)
  - Depends on: T003, T002
  - Spec refs: SPEC-003, SPEC-006, SPEC-007, SPEC-008, SPEC-009, SPEC-010, SPEC-011, SPEC-015 (`credential list` half), SPEC-018 (codes 1/2/3), SPEC-021
  - Acceptance refs: AC-003.1, AC-003.2, AC-006.1, AC-006.2, AC-006.3, AC-006.4, AC-007.1, AC-007.2, AC-007.3, AC-008.1, AC-008.2, AC-008.3, AC-008.4, AC-009.1, AC-009.2, AC-009.3, AC-009.4, AC-009.5, AC-010.1, AC-010.2, AC-010.3, AC-010.4, AC-011.1, AC-011.2, AC-011.3, AC-012.1, AC-012.2, AC-012.5, AC-015.1, AC-018.1, AC-018.2, AC-021.1; turns the T002 suite green (AC-002.5, AC-019.1, AC-019.3, AC-020.1, AC-020.2, AC-020.3, AC-020.4, AC-020.5, AC-020.6); edge cases EDGE-001, EDGE-002, EDGE-003, EDGE-008, EDGE-009, EDGE-010, EDGE-014, EDGE-015, EDGE-016, EDGE-017, EDGE-019, EDGE-020
  - Interfaces: Consumes: every T003 signature above; `run_ac` helper and fixtures from T002 (read-only contract — do not modify `tests/helpers.rs`, `tests/security_p1.rs`, or `tests/fixtures/` except to ADD fixture files). Produces: `query::EntryView`/`Listing`/`Matches` (no variant can hold a secret); `render::text(&View) -> String`, `render::json(&View) -> serde_json::Value` (SPEC-010 shapes exactly: envelopes with `version`; `Field` recursion with nested `fields`; `reference` on credential_ref; `addressable`/`key`/`path: null` markers; raw `get --json`)
  - Impact seeds: Config::load, Config::select_profile, Segments::parse, path::resolve, shallow_status, run_ac
  - No-go: tests/security_p1.rs, tests/helpers.rs, .sdd/
  - TDD: yes (security-sensitive boundary — query surface must never emit secrets and must refuse sensitive-name configs at load; tests: T002)
  - Dispatch: agent (impl-standard: largest single surface)
  - Verification: `cargo test --all-targets && cargo clippy --all-targets -- -D warnings && cargo fmt --check` — expected: ALL tests green including the previously red `security_p1` suite; snapshot files created for the six SPEC-010 shapes
  - Report: `.sdd/changes/001-agent-context-cli/reports/task-T004-report.md`

Checkpoint: Phase 1 acceptance passes — on the design-example fixture every query command produces the documented text and JSON with exit codes 1/2/3, a plaintext-secret config is unusable by every command, and the sentinel grep helper guards every invocation. Observable: `cargo test` green; `AGENT_CONTEXT_FILE=tests/fixtures/example.toml ./target/debug/agent-context list`.
```

## Global Constraints

These bind this task in addition to its own requirements:

Copied verbatim from spec.md/architecture.md — every task inherits these:

- Exit codes: `0` success; `1` usage/argument errors; `2` config-file errors (missing file/base dir, parse, any SPEC-002 violation, permission check); `3` name-resolution failures (unknown profile, entry, path, or credential name); `4` credential resolution failure, store write failure, or injection conflict; `127` target-not-executable (`run` only). clap's default argument-error exit code (2) must be remapped to 1.
- Valid env names: `[A-Za-z_][A-Za-z0-9_]*`. Credential names (definition keys and reference names): `[A-Za-z0-9_-]+`.
- Credential reference grammar: exactly `credential://<name>` or `credential://<name>?as=<ENV>`; anything else with the prefix is a load-time violation.
- Sensitive field names (ASCII case-insensitive): exact `token`, `password`, `secret`, `api_key`, `private_key`, or suffixes `_token`, `_password`, `_secret`, `_api_key`, `_private_key`; traversal per SPEC-020 (all profile-tree table fields at any depth, including tables inside arrays; excluding the reserved `inject` table).
- JSON `type` tokens: `string|integer|float|boolean|datetime|array|table|credential_ref`. `status` tokens: `available|not_set|configured|command_missing`. Envelope and `Field`/`Match` shapes exactly as SPEC-010; they are frozen.
- All CLI output English. No log files. Diagnostics never echo config source lines or open-schema field values; `argv` cited as `argv[0]` only. Provider-captured candidate bytes never appear in any output. `toml::de::Error` `Display` is never forwarded.
- Unix config-file permission predicate: permission bits ⊆ 0600.
- Dependencies: `clap` 4 (derive), `toml` 1.x (`preserve_order`), `serde`/`serde_json`, `keyring` 4.x (default features), `rpassword` 7, `thiserror` 2; dev: `assert_cmd`, `predicates`, `tempfile`. Core maps must be order-preserving (`toml::Table`/IndexMap, never HashMap/BTreeMap).
- The `test-keychain` store compiles only under `all(feature = "test-keychain", debug_assertions)`; the feature in a release-profile build is a `compile_error!`.
- Secrets: `CapturedSecret(Vec<u8>)` at capture boundary → checked conversion → `Secret(String)`; both without `Display`/`Serialize`; resolved secrets go only into the target's constructed environment map.

## Impact Map

Generated on 2026-08-22 against tracked working tree at commit f1d343a (clean)

This map is a verified starting point and NOT a complete boundary. Earlier tasks may have shifted call sites. The worker MUST re-verify each seed with `git grep -n -F -e <seed> -- ':(exclude).sdd/'` before implementing (identical semantics to generation) and record differences in the report's Impact Delta section. Search domain is the tracked working tree; untracked files are not searched.

### Seed: `Config::load` (15 call sites)

- src/config/mod.rs:3
- src/config/mod.rs:172
- src/config/mod.rs:199
- src/config/mod.rs:211
- src/config/mod.rs:224
- src/config/mod.rs:242
- src/config/mod.rs:264
- src/config/mod.rs:304
- src/config/mod.rs:326
- src/config/mod.rs:345
- src/config/mod.rs:361
- src/config/mod.rs:386
- src/config/mod.rs:393
- src/config/model.rs:3
- src/config/validate.rs:5

### Seed: `Config::select_profile` (0 call sites)

> **WARNING:** Seed `Config::select_profile` matched 0 call sites in the tracked working tree (excluding `.sdd/`). This may indicate name drift. Resolve the correct symbol before implementing; do not guess a similar name.

### Seed: `Segments::parse` (4 call sites)

- src/config/validate.rs:223
- src/path.rs:204
- src/path.rs:247
- src/path.rs:270

### Seed: `path::resolve` (0 call sites)

> **WARNING:** Seed `path::resolve` matched 0 call sites in the tracked working tree (excluding `.sdd/`). This may indicate name drift. Resolve the correct symbol before implementing; do not guess a similar name.

### Seed: `shallow_status` (17 call sites)

- src/shallow.rs:59
- src/shallow.rs:157
- src/shallow.rs:208
- src/shallow.rs:211
- src/shallow.rs:214
- src/shallow.rs:224
- src/shallow.rs:243
- src/shallow.rs:255
- src/shallow.rs:269
- src/shallow.rs:281
- src/shallow.rs:293
- src/shallow.rs:306
- src/shallow.rs:316
- src/shallow.rs:320
- src/shallow.rs:332
- src/shallow.rs:345
- src/shallow.rs:356

### Seed: `run_ac` (12 call sites)

- tests/helpers/mod.rs:24
- tests/helpers/mod.rs:64
- tests/security_p1.rs:6
- tests/security_p1.rs:14
- tests/security_p1.rs:19
- tests/security_p1.rs:41
- tests/security_p1.rs:54
- tests/security_p1.rs:69
- tests/security_p1.rs:83
- tests/security_p1.rs:96
- tests/security_p1.rs:104
- tests/security_p1.rs:130

## Do Not Explore

Planning confirmed these regions are unaffected. Exploration budget must not be spent there. Touching them requires reporting BLOCKED or NEEDS_CONTEXT.

- `tests/security_p1.rs`
- `tests/helpers.rs`
- `.sdd/`

## Relevant Source Artifacts

- PRD: /Users/zhaiqifeng/Dev/agent-context/.sdd/changes/001-agent-context-cli/prd.md
- Architecture: /Users/zhaiqifeng/Dev/agent-context/.sdd/changes/001-agent-context-cli/architecture.md
- Spec: /Users/zhaiqifeng/Dev/agent-context/.sdd/changes/001-agent-context-cli/spec.md
- Plan: /Users/zhaiqifeng/Dev/agent-context/.sdd/changes/001-agent-context-cli/plan.md
- Tasks: /Users/zhaiqifeng/Dev/agent-context/.sdd/changes/001-agent-context-cli/tasks.md

## Binding Instructions

Read only the artifacts needed to complete this task. Preserve exact acceptance criteria and constraints from the spec. Implement only this task's scope.

## Report Contract

Write the implementation report to this path, relative to the root of the
repository you are working in:

```text
.sdd/changes/001-agent-context-cli/reports/task-T004-report.md
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
