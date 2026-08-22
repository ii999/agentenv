# Implementation Plan: agent-context CLI

## Source Artifacts

- Change ID: 001-agent-context-cli
- PRD: prd.md
- Architecture: architecture.md
- Spec: spec.md (approved round 5, 2026-08-22)
- Spec review: spec-review.md

## Strategy

Build a single Rust crate (`agent-context`: library + thin `main`) in three spec phases, each independently verifiable. Phase 1 delivers the entire read-only surface — config load with aggregate validation, profile selection, path grammar, `list`/`show`/`get`/`find`/`validate`/`credential list`, text + frozen JSON, exit codes 1/2/3 — against fixture configs, with shallow credential status computed by a free function (no provider seam yet). Phase 2 introduces the `credential` module (two-stage secret types, `Provider` trait with env/keychain/command adapters, the `test-keychain` file-backed store), `credential check`/`set`, and exit 4. Phase 3 adds the injection planner and `run` (conflict-before-resolution, exec on Unix / spawn+wait on Windows, exit 127) plus the README.

Testing rides on one shared integration-test helper that invokes the built binary with a scrubbed environment, captures output, and asserts per invocation that no planted sentinel appears (AC-019.1). The two security-sensitive behaviors carry TDD with separate test authorship: the Phase-1 no-secret/load-refusal suite (T002) and the Phase-3 conflict-before-resolution suite (T007) are authored and checkpointed red before their implementation tasks dispatch, and their test files are read-only contracts for implementers.

Workers are dispatched per task with self-contained briefs; each task lands as a reviewed `PATCH.diff` on the change branch with verification run on the branch before checkpoint. JSON snapshot tests freeze the SPEC-010 shapes from the first Phase-1 review onward.

## Global Constraints

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

## Principles Check

- [x] `.sdd/memory/principles.md`: artifacts local (all state under `.sdd/`); acceptance criteria are the gates (tasks reference AC IDs; verification commands assert them); simple design with clear interfaces (module table in architecture.md; no speculative layers).
- [x] Structure is the simplest satisfying the spec: no plugin system, no async, no config writing; the only trait seam (`Provider`) has three production adapters.

## Research Decisions

| Unknown | Decision | Rationale | Alternatives considered |
| --- | --- | --- | --- |
| TOML parsing with order + open schema | `toml` 1.1.x, `preserve_order`, typed core over `toml::Table` | verified feature exists; order contract SPEC-021 | `toml_edit` (heavier, write-oriented; rejected) |
| Keychain access | `keyring` 4.1.x default `v1` feature (apple-native/windows-native/zbus-secret-service) | verified via `cargo info` | direct Security.framework bindings (per-OS work; rejected) |
| Out-of-process keychain testing | `test-keychain` file store, `all(feature, debug_assertions)` + `compile_error!` guard | satisfies no-mock-in-production; assert_cmd-compatible | keyring-core `mock` (in-process only; kept for unit tests) |
| Unix `run` mechanics | `std::os::unix::process::CommandExt::exec` | transparency structural (ARCH-003) | spawn+forward (more code, signal races; Windows only) |
| No-echo input | `rpassword` 7 | maintained, cross-platform | dialoguer (heavier) |
| First-rung implementation providers | availability probed at dispatch time; ladder order from route-policy.json | providers are machine-local CLIs; probing at plan time goes stale | pinning providers in the plan (rejected — routing is dispatch-time policy) |

## Complexity Tracking

| Deviation | Why needed | Simpler alternative rejected because |
| --- | --- | --- |
| Two secret types (`CapturedSecret` → `Secret`) | provider output must be held pre-UTF-8-validation without leak risk | single `Secret(String)` cannot contain invalid UTF-8 bytes (R5-IMP-05) |
| `test-keychain` compile-gated store | out-of-process AC-014.4/AC-015.2 need a real store the child process can reach | keyring mock is process-local; real keychain prompts and pollutes user store |

## Workstreams

| Workstream | Purpose | Files / areas | Depends on | Parallel safe? |
| --- | --- | --- | --- | --- |
| WS-1 scaffold | crate layout, error/exit-code spine | Cargo.toml, src/lib.rs, src/main.rs, src/error.rs | — | No (root) |
| WS-2 security test suite (P1) | TDD suite for no-secret + load-refusal | tests/helpers.rs, tests/security_p1.rs, tests/fixtures/ | WS-1 | No (defines helper all later tests use) |
| WS-3 config core | path grammar, load, validate, profile | src/path.rs, src/config/ | WS-1 | Yes vs WS-4 skeleton, No vs WS-5 |
| WS-4 query/render | list/show/get/find + JSON contract | src/query.rs, src/render.rs, src/cli/ (query commands) | WS-3 | No (consumes WS-3) |
| WS-5 providers | credential module, check/set | src/credential/, src/cli/credential.rs | WS-3 | After Phase 1 review |
| WS-6 runner | injection plan + run + conflict TDD suite | src/runner.rs, src/cli/run.rs, tests/security_p3.rs | WS-5 | After Phase 2 review |
| WS-7 docs/validation | README, validation.md | README.md, .sdd validation | WS-6 | README parallel-safe with WS-6 tail |

## Dependency Graph

```text
T001 (scaffold) -> T002 (P1 security tests, red) -> T003 (config+path) -> T004 (query/render/CLI, turns T002 green)
  -> Checkpoint P1 -> T005 (providers+credential CLI) -> Checkpoint P2
  -> T006 (P3 conflict tests, red) -> T007 (runner+run, turns T006 green) -> T008 (README) -> T900 (validation)
```

## Phase Plan

### Phase 1: Config core & queries (spec Phase 1)

- Objective: full read-only surface on fixtures.
- Spec references: SPEC-001..013 (validation parts), SPEC-015 (`credential list`), SPEC-018 (codes 1/2/3), SPEC-019 (suite-wide), SPEC-020, SPEC-021.
- Acceptance gate: AC-001.* AC-002.* AC-003.* AC-004.* AC-005.* AC-006.* AC-007.* AC-008.* AC-009.* AC-010.1–4 AC-011.* AC-012.1–5 AC-013.* AC-015.1 AC-018.1(P1) AC-019.1/.3 AC-020.* AC-021.1.
- Implementation notes: tasks T001–T004.

### Phase 2: Credential providers (spec Phase 2)

- Objective: real resolution/storage; exit 4.
- Spec references: SPEC-014, SPEC-015 (check/set), SPEC-018 (code 4), SPEC-019 (AC-019.2).
- Acceptance gate: AC-014.* AC-015.2–5 AC-018.1(P2) AC-019.2.
- Implementation notes: task T005.

### Phase 3: Injection runner (spec Phase 3)

- Objective: `run --with` end to end; docs.
- Spec references: SPEC-016, SPEC-017, SPEC-018 (127, conflict/resolution 4), SPEC-022.
- Acceptance gate: AC-016.* AC-017.* AC-018.1(P3) AC-022.1.
- Implementation notes: tasks T006–T008.

## Parallelization Plan

- Parallel-safe tasks: none dispatched concurrently — every task feeds the next through shared files (single crate, one test helper). T008 (README) may overlap T900 preparation.
- Serialized tasks: T001 → T002 → T003 → T004 → T005 → T006 → T007 → T008 → T900.
- Shared-file conflict risks: src/cli/ and tests/helpers.rs touched by several tasks — serialization removes the risk.
- Integration owner: orchestrator (Claude Code session).

## Verification Plan

| Gate | Command or check | Expected result | Owner |
| --- | --- | --- | --- |
| Build + lint | `cargo build --all-targets && cargo clippy --all-targets -- -D warnings` | clean | implementer |
| Unit / integration | `cargo test --features test-keychain` | all green (phase-scoped) | implementer |
| Fmt | `cargo fmt --check` | clean | implementer |
| Release guard | `cargo build --release` (no test feature) + negative `AGENT_CONTEXT_TEST_KEYCHAIN` check | test store absent | orchestrator (T900) |
| Acceptance | phase AC suites via `sdd.py verify` | all green or triaged | orchestrator |
| Manual | macOS Keychain round-trip; cold-start `list` < 100 ms; README checklist | recorded in validation.md | orchestrator + user |

## TDD Policy

`TDD: yes` tasks (necessity trigger: security-sensitive boundary):

- T004 — trigger: security-sensitive boundary (query surface must be incapable of emitting secrets; SPEC-019/SPEC-020 load refusal) — tests authored in T002.
- T007 — trigger: security-sensitive boundary (conflict detection strictly before provider resolution; SPEC-016) — tests authored in T006.

Both test-authoring tasks route at orchestrator-equivalent capability (high-capability route via provider-host affinity), their reviewed failing suites are checkpointed before the paired implementation dispatch, and the test files are read-only contracts for the implementers.

## Rollback Plan

- All work on branch `sdd/001-agent-context-cli`; revert = drop the branch. No migrations, no external state (test keychain files live in temp dirs).

## Plan Review Checklist

- [x] Every spec requirement has at least one implementation task (Coverage table in tasks.md).
- [x] Every task has a verification method with expected output.
- [x] Parallel tasks do not edit the same files (plan serializes all implementation tasks).
- [x] Every `TDD: yes` cites a necessity trigger and a paired test-authoring task (2, both security-boundary).
- [x] Acceptance gates are executable locally.
- [x] No unresolved unknowns.
- [x] Principles Check passes.
- [x] Global Constraints carry exact values copied verbatim.
