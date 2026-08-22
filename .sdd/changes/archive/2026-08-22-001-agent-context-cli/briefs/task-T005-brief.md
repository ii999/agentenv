# Task Brief: T005

## Change

- Change ID: 001-agent-context-cli
- Task group: ### Group 3: Phase 2 (P2) — Credential providers
- Task ID: T005
- Report path: .sdd/changes/001-agent-context-cli/reports/task-T005-report.md

## Task Text

```markdown
- [ ] T005 [Phase 2] Credential module, providers, and `credential check`/`set`
  - Files: `src/credential/mod.rs`, `src/credential/secret.rs`, `src/credential/env.rs`, `src/credential/keychain.rs`, `src/credential/command.rs`, `src/credential/test_store.rs` (cfg-gated), `src/cli/credential.rs`, `src/main.rs` (wire `credential check|set`), `Cargo.toml` (`keyring`, `rpassword`, `[features] test-keychain = []`), `src/shallow.rs` (absorb into `credential::shallow_status`, updating T004 call sites), `tests/credential_p2.rs`, `tests/fixtures/bin/` (fixture provider scripts: sentinel-then-exit-1, newline-only, invalid-UTF-8-sentinel, argv-recorder, counting provider)
  - Depends on: T004
  - Spec refs: SPEC-014, SPEC-015 (check/set), SPEC-018 (code 4), SPEC-019 (AC-019.2, captured-bytes clause), SPEC-AS-016/-019
  - Acceptance refs: AC-014.1, AC-014.2, AC-014.3, AC-014.4, AC-014.5, AC-015.2, AC-015.3, AC-015.4, AC-015.5, AC-018.1, AC-019.2; edge cases EDGE-006, EDGE-011, EDGE-012, EDGE-013
  - Interfaces: Consumes: `CredentialDef`, `CredentialRef`, `Status`, `AppError` (T003/T004 signatures). Produces: `credential::CapturedSecret(Vec<u8>)` and `credential::Secret(String)` — neither implements `Display`/`Serialize`, both redact in `Debug`; `CapturedSecret::into_secret(self) -> Result<Secret, SecretDomainError>` (empty/whitespace-only/NUL/invalid-UTF-8 rejected, one trailing newline stripped, error carries no bytes); `trait Provider { fn shallow_status(&self) -> Status; fn resolve(&self) -> Result<Secret, AppError>; fn store(&self, value: Secret) -> Result<(), AppError>; }`; `pub fn provider_for(def: &CredentialDef) -> Box<dyn Provider>`; test store: `#[cfg(all(feature = "test-keychain", debug_assertions))]` file-backed adapter selected by `AGENT_CONTEXT_TEST_KEYCHAIN=<path>`, plus `#[cfg(all(feature = "test-keychain", not(debug_assertions)))] compile_error!(...)`
  - Impact seeds: shallow_status, run_ac, AppError::Credential
  - No-go: tests/security_p1.rs, tests/helpers.rs, .sdd/
  - TDD: no (behavior pinned by AC-driven tests written with the code; the no-leak property is already enforced per-invocation by the T002 helper)
  - Dispatch: agent (impl-standard: security-core module)
  - Review: per-task
  - Verification: `cargo test --features test-keychain --all-targets && cargo build --release 2>&1 | tail -2` — expected: all green including AC-015.2 byte-exact round-trip and AC-014.5 sentinel-free failures; release build (no feature) succeeds, and `cargo build --release --features test-keychain` FAILS with the compile_error message
  - Report: `.sdd/changes/001-agent-context-cli/reports/task-T005-report.md`

Checkpoint: Phase 2 acceptance passes — `credential check` reports per-provider success/failure without ever printing a value, `set` round-trips exact bytes through the test store, exit 4 paths live. Observable: `cargo test --features test-keychain` green; release-with-feature build refuses to compile.
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

Generated on 2026-08-22 against tracked working tree at commit 0e58a8a (clean)

This map is a verified starting point and NOT a complete boundary. Earlier tasks may have shifted call sites. The worker MUST re-verify each seed with `git grep -n -F -e <seed> -- ':(exclude).sdd/'` before implementing (identical semantics to generation) and record differences in the report's Impact Delta section. Search domain is the tracked working tree; untracked files are not searched.

### Seed: `shallow_status` (19 call sites)

- src/query.rs:14
- src/query.rs:378
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

### Seed: `run_ac` (48 call sites)

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
- tests/security_p1.rs:19
- tests/security_p1.rs:41
- tests/security_p1.rs:54
- tests/security_p1.rs:69
- tests/security_p1.rs:83
- tests/security_p1.rs:96
- tests/security_p1.rs:104
- tests/security_p1.rs:130

### Seed: `AppError::Credential` (0 call sites)

> **WARNING:** Seed `AppError::Credential` matched 0 call sites in the tracked working tree (excluding `.sdd/`). This may indicate name drift. Resolve the correct symbol before implementing; do not guess a similar name.

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
.sdd/changes/001-agent-context-cli/reports/task-T005-report.md
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
