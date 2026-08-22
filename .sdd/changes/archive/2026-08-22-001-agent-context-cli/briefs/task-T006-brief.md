# Task Brief: T006

## Change

- Change ID: 001-agent-context-cli
- Task group: ### Group 4: Phase 3 (P3) — Injection runner and docs
- Task ID: T006
- Report path: .sdd/changes/001-agent-context-cli/reports/task-T006-report.md

## Task Text

```markdown
- [ ] T006 [Phase 3] Author the conflict-before-resolution test suite (red)
  - Files: `tests/security_p3.rs`, `tests/fixtures/` (add: canary provider script that creates a file when executed; probe helper usage docs in comments), `tests/fixtures/bin/probe.rs` + `Cargo.toml` `[[bin]] name = "test-probe"` (a tiny binary that writes its env and argv to the file named by `TEST_PROBE_OUT`, prints `out`/`err` markers, exits per `TEST_PROBE_EXIT`)
  - Depends on: T005
  - Spec refs: SPEC-016 (AC-016.2 canary half, AC-016.9 matrix), SPEC-019
  - Acceptance refs: AC-016.2, AC-016.9 (Unix-runnable rows), AC-018.1
  - Interfaces: Consumes: `run_ac` (T002), fixture script conventions (T005). Produces: `tests/security_p3.rs` — table-driven injection-plan matrix asserting exit 4 + both source names in stderr + canary file absent for every conflict row, and dedup rows asserting success + single provider invocation via the counting provider; `test-probe` binary contract as described
  - Impact seeds: run_ac
  - No-go: src/ (tests only)
  - TDD: n/a (this IS the paired test task for T007)
  - Dispatch: agent (orchestrator-equivalent capability — high-capability route)
  - Verification: `cargo test --features test-keychain --test security_p3 2>&1 | tail -5` — expected: every test fails by assertion (the `run` subcommand does not exist yet → usage exit 1 where 4/0 expected); zero compile/collection errors
  - Report: `.sdd/changes/001-agent-context-cli/reports/task-T006-report.md`
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

Generated on 2026-08-22 against tracked working tree at commit f0445b0 (clean)

This map is a verified starting point and NOT a complete boundary. Earlier tasks may have shifted call sites. The worker MUST re-verify each seed with `git grep -n -F -e <seed> -- ':(exclude).sdd/'` before implementing (identical semantics to generation) and record differences in the report's Impact Delta section. Search domain is the tracked working tree; untracked files are not searched.

### Seed: `run_ac` (58 call sites)

- tests/credential_p2.rs:15
- tests/credential_p2.rs:26
- tests/credential_p2.rs:34
- tests/credential_p2.rs:65
- tests/credential_p2.rs:112
- tests/credential_p2.rs:154
- tests/credential_p2.rs:162
- tests/credential_p2.rs:203
- tests/credential_p2.rs:218
- tests/credential_p2.rs:332
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
- tests/security_p1.rs:6
- tests/security_p1.rs:14

Truncated: showing 50 of 58 matches — the map for this seed is partial; run the full search yourself.

## Do Not Explore

Planning confirmed these regions are unaffected. Exploration budget must not be spent there. Touching them requires reporting BLOCKED or NEEDS_CONTEXT.

- `src/ (tests only)`

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
.sdd/changes/001-agent-context-cli/reports/task-T006-report.md
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
