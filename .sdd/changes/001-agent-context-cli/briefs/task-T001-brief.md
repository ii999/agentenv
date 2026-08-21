# Task Brief: T001

## Change

- Change ID: 001-agent-context-cli
- Task group: ### Group 1: Foundation
- Task ID: T001
- Report path: .sdd/changes/001-agent-context-cli/reports/task-T001-report.md

## Task Text

```markdown
- [ ] T001 [Foundation] Establish the crate: manifest, error spine, CLI shell
  - Files: `Cargo.toml`, `src/main.rs`, `src/lib.rs`, `src/error.rs`, `rustfmt.toml` (defaults), `.gitignore` (append `/target` if absent)
  - Depends on: none
  - Spec refs: SPEC-018 (exit-code spine); plan Global Constraints (dependencies)
  - Acceptance refs: groundwork for AC-018.1
  - Interfaces: Produces: `agent_context::error::AppError` (thiserror enum with variants `Usage(String)`, `Config(Vec<Violation>)`, `NotFound(String)`, `Credential(String)`, `TargetNotExecutable(String)`; `pub struct Violation { pub path: String, pub message: String }`; `impl AppError { pub fn exit_code(&self) -> i32 }` returning 1/2/3/4/127); `fn main()` dispatching a clap 4 derive `Cli` with global `--profile <NAME>` and `--json` flags, subcommand enum stub containing only `Version` handling via clap's built-in `--version`/`--help`, argument errors remapped to exit 1, unknown/no subcommand printing help and exiting 1
  - Impact seeds: none
  - No-go: .sdd/, .claude/
  - TDD: no
  - Dispatch: agent (bounded scaffold with exact deliverables)
  - Verification: `cargo build --all-targets && cargo clippy --all-targets -- -D warnings && cargo fmt --check && ./target/debug/agent-context --version` — expected: clean build/lint/fmt; version line `agent-context 0.1.0`; `./target/debug/agent-context; echo $?` prints help then `1`
  - Report: `.sdd/changes/001-agent-context-cli/reports/task-T001-report.md`

Checkpoint: `cargo build` succeeds from a clean clone; the binary runs, reports its version, and exits 1 with help on no subcommand.
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

Generated on 2026-08-22 against tracked working tree at commit 2b3def7 (clean)

This map is a verified starting point and NOT a complete boundary. Earlier tasks may have shifted call sites. The worker MUST re-verify each seed with `git grep -n -F -e <seed> -- ':(exclude).sdd/'` before implementing (identical semantics to generation) and record differences in the report's Impact Delta section. Search domain is the tracked working tree; untracked files are not searched.

No existing call sites are expected. Any discovered coupling must be reported in Impact Delta.

## Do Not Explore

Planning confirmed these regions are unaffected. Exploration budget must not be spent there. Touching them requires reporting BLOCKED or NEEDS_CONTEXT.

- `.sdd/`
- `.claude/`

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
.sdd/changes/001-agent-context-cli/reports/task-T001-report.md
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
