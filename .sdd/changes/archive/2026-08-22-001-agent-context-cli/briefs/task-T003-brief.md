# Task Brief: T003

## Change

- Change ID: 001-agent-context-cli
- Task group: ### Group 2: Phase 1 (P1, MVP) — Config core & queries
- Task ID: T003
- Report path: .sdd/changes/001-agent-context-cli/reports/task-T003-report.md

## Task Text

```markdown
- [ ] T003 [Phase 1] Config core: locate, parse, validate, profile, path grammar, shallow status
  - Files: `src/config/mod.rs`, `src/config/locate.rs`, `src/config/model.rs`, `src/config/validate.rs`, `src/path.rs`, `src/shallow.rs`, `src/lib.rs` (module wiring), unit tests inline per module
  - Depends on: T001
  - Spec refs: SPEC-001, SPEC-002 (all 11 rules + diagnostics rule), SPEC-004, SPEC-005, SPEC-012 (grammar + shallow status), SPEC-013 (validation), SPEC-020
  - Acceptance refs: AC-001.1, AC-001.2, AC-001.3, AC-001.4, AC-002.1, AC-002.2, AC-002.3, AC-002.4, AC-002.6, AC-002.7, AC-002.8, AC-004.1, AC-004.2, AC-004.3, AC-005.1, AC-005.2, AC-005.3, AC-012.3, AC-012.4, AC-013.1, AC-013.2, AC-013.3, AC-013.4, AC-013.5, AC-020.1, AC-020.2, AC-020.3, AC-020.4, AC-020.5, AC-020.6 (logic level; CLI-level assertions turn green in T004)
  - Interfaces: Consumes: `AppError`, `Violation` (T001). Produces: `config::Config { pub version: i64, pub profiles: toml::Table-backed ordered model, pub credentials: indexed ordered model }`; `Config::load(explicit_file: Option<&Path>, env: &impl Fn(&str) -> Option<String>) -> Result<Config, AppError>` (aggregates ALL violations into `AppError::Config(Vec<Violation>)`; empty env values treated as unset); `Config::select_profile(&self, flag: Option<&str>, env_val: Option<&str>) -> Result<&Profile, AppError>`; `path::Segments` with `Segments::parse(&str) -> Result<Segments, AppError>` and `Segments::render(&self) -> String` (round-trip property); `path::resolve<'a>(profile: &'a Profile, segs: &Segments) -> Result<&'a toml::Value, AppError>`; `shallow::Status` enum (`Available`, `NotSet`, `Configured`, `CommandMissing`) with `Status::json_token(&self) -> &'static str` and `pub fn shallow_status(cred: &CredentialDef, env: &impl Fn(&str) -> Option<String>) -> Status` (env presence / `configured` / executable discovery per SPEC-012 — no store read, no process launch); `CredentialRef { pub name: String, pub target_override: Option<String> }` with `CredentialRef::parse(&str) -> Result<CredentialRef, String>` (strict grammar)
  - Impact seeds: AppError, Violation
  - No-go: tests/security_p1.rs, tests/helpers.rs (read-only contract), .sdd/
  - TDD: no (module-level unit tests written with the code)
  - Dispatch: agent (impl-standard: multi-module core)
  - Verification: `cargo test --lib && cargo clippy --all-targets -- -D warnings` — expected: all unit tests green (grammar table, validation table incl. AC-002.6/.7/.8 cases, reference grammar AC-012.4 cases, sensitive-name matrix incl. array-nested and uppercase, inject rules AC-013.1–5), clippy clean
  - Report: `.sdd/changes/001-agent-context-cli/reports/task-T003-report.md`
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

Generated on 2026-08-22 against tracked working tree at commit 6545e84 (clean)

This map is a verified starting point and NOT a complete boundary. Earlier tasks may have shifted call sites. The worker MUST re-verify each seed with `git grep -n -F -e <seed> -- ':(exclude).sdd/'` before implementing (identical semantics to generation) and record differences in the report's Impact Delta section. Search domain is the tracked working tree; untracked files are not searched.

### Seed: `AppError` (6 call sites)

- src/error.rs:18
- src/error.rs:35
- src/main.rs:4
- src/main.rs:77
- src/main.rs:82
- src/main.rs:85

### Seed: `Violation` (3 call sites)

- src/error.rs:6
- src/error.rs:11
- src/error.rs:23

## Do Not Explore

Planning confirmed these regions are unaffected. Exploration budget must not be spent there. Touching them requires reporting BLOCKED or NEEDS_CONTEXT.

- `tests/security_p1.rs`
- `tests/helpers.rs (read-only contract)`
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
.sdd/changes/001-agent-context-cli/reports/task-T003-report.md
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
