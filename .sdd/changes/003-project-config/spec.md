# Implementation Specification: Project-Scoped Configuration

## Source Artifacts

- Change ID: 003-project-config
- PRD: `.sdd/changes/003-project-config/prd.md`
- Architecture: `.sdd/changes/003-project-config/architecture.md`
- Current specs: `.sdd/specs/current/001-agent-context-cli/spec.md`, `.sdd/specs/current/002-002-config-write/spec.md`

## Scope

### In Scope

- Discovery of a checked-in project file (`.agentenv.toml`) from the working directory.
- Closed-schema validation restricting the file to selection and declaration content.
- Trust-on-first-use lifecycle: `agentenv project allow`, `agentenv project revoke`, `agentenv project status`, content-hash invalidation.
- Trusted profile pin inserted into the selection precedence chain.
- Requirement declarations (`[requires.<entry>]`) and a structural satisfied/unsatisfied report.
- Discovery bypass (`--no-project`, `AGENTENV_NO_PROJECT`).
- Documentation and agent-skill updates, including the `.env`/docker-compose pairing guidance.

### Out of Scope

- Project-file content that defines values, credentials, `inject` tables, or credential references (PRD non-goal, D-01).
- Generating `.env` files or exporting values to a shell.
- Blocking `run` or read commands on unsatisfied requirements (deferred; ARCH-006).
- Merging multiple project files along the ancestor walk (nearest file wins).
- Credential resolution or shallow provider checks inside the requirement report.

## Phase Map

| Phase | Name | Priority | Objective | Depends on | Independent test |
| --- | --- | --- | --- | --- | --- |
| Phase 1 | Pin and trust | P1 (MVP) | Discover, validate, trust, and honor a profile pin; keep untrusted files inert | None | In a fresh tree, a pinned profile takes effect only after `allow` and stops after an edit or `revoke`; without a project file all commands behave as today |
| Phase 2 | Requirements report | P2 | Declare required entries/fields and report satisfaction | Phase 1 | With a trusted file declaring requirements, `project status` reports satisfied/unsatisfied correctly in text and JSON against configs that do and do not define them |
| Phase 3 | Docs and protocol | P3 | README, skill, and pairing documentation | Phase 1–2 | Documented commands and workflows execute as written against the built binary |

## Requirements

### SPEC-001: Project file discovery

The CLI MUST discover at most one project file per invocation: starting at the working directory and walking parent directories to the filesystem root, the nearest file named `.agentenv.toml` is the project file; no other file on the walk is consulted.

Source trace:

- PRD: PRD-FR-001
- Architecture: ARCH-001

Acceptance criteria:

- AC-001.1: GIVEN `.agentenv.toml` in a repository root, WHEN any command runs from a nested subdirectory of that repository, THEN that file is the discovered project file.
- AC-001.2: GIVEN no `.agentenv.toml` in the working directory or any ancestor, WHEN any command runs, THEN behavior, output bytes, and exit status are identical to a build of the previous release semantics (no notice, no new output).
- AC-001.3: GIVEN `.agentenv.toml` in both the working directory and an ancestor directory, WHEN any command runs, THEN only the working directory's file is discovered.
- AC-001.4: GIVEN `--no-project` on the command line or a non-empty `AGENTENV_NO_PROJECT` environment value, WHEN any command runs, THEN no discovery occurs: no project file is read, no notice is emitted, and no pin applies. An empty `AGENTENV_NO_PROJECT` counts as unset.

Verification:

- Automated: integration tests running the binary from temp directory trees (existing `assert_cmd` + `tempfile` style).

### SPEC-002: Closed project-file schema

A project file MUST parse as TOML and contain only: `version = 1` (required), an optional non-empty string `profile`, and an optional `[requires]` table whose subtables `[requires.<entry>]` each carry a required non-empty string `reason` and an optional non-empty array `fields` of field-path strings. Any other key, table, value type, or version MUST be a validation violation that names the offending TOML path and never echoes the offending value.

Source trace:

- PRD: PRD-FR-006
- Architecture: ARCH-001, `project::model`

Acceptance criteria:

- AC-002.1: GIVEN a project file containing any top-level key other than `version`, `profile`, or `requires` (for example `credentials`, `profiles`, or `inject`), WHEN it is validated (`project allow` or `project status`), THEN validation fails with a message naming the offending path, and the value is not echoed.
- AC-002.2: GIVEN `version` missing or not equal to `1`, WHEN validated, THEN validation fails naming `version`.
- AC-002.3: GIVEN `profile = ""` or a non-string `profile`, WHEN validated, THEN validation fails naming `profile`.
- AC-002.4: GIVEN `[requires.llm]` without `reason`, or with an empty `reason`, or with `fields = []`, or with a `fields` member that is not a valid field path, WHEN validated, THEN validation fails naming the offending path.
- AC-002.5: GIVEN a syntactically valid file with `version = 1`, a `profile`, and well-formed `[requires.*]` tables, WHEN validated, THEN validation succeeds.

Verification:

- Automated: integration tests with fixture project files per violation class.

### SPEC-003: Trust lifecycle

A project file MUST have no effect until the user approves its exact content. Approval state MUST be stored outside the repository in a user-owned store keyed by the file's canonical absolute path with a fingerprint of its exact bytes, so that any content change or path change returns the file to the untrusted state. `agentenv project allow` MUST validate then record approval; `agentenv project revoke` MUST remove it.

Source trace:

- PRD: PRD-FR-004, PRD-FR-005
- Architecture: ARCH-002

Acceptance criteria:

- AC-003.1: GIVEN a newly created (never-approved) project file with a profile pin, WHEN any command resolves a profile, THEN the pin has no effect.
- AC-003.2: GIVEN an untrusted valid project file, WHEN `agentenv project allow` runs, THEN approval is recorded, the command reports the file path and what approval enables, and subsequent commands honor the pin.
- AC-003.3: GIVEN a trusted project file, WHEN its content changes in any way (including whitespace), THEN it is untrusted again until re-approved.
- AC-003.4: GIVEN a trusted project file, WHEN `agentenv project revoke` runs, THEN the approval is removed and the file is inert; a second `revoke` succeeds and reports that no approval existed.
- AC-003.5: GIVEN an invalid project file, WHEN `agentenv project allow` runs, THEN no approval is recorded, the violations are reported, and the exit status is 2.
- AC-003.6: GIVEN no discovered project file, WHEN `project allow` or `project revoke` runs, THEN the command fails with exit status 3 and a message stating no project file was found.
- AC-003.7: GIVEN a project file reached through a symlinked ancestor directory, WHEN it is approved and later read, THEN trust matches (identity is the canonical path).
- AC-003.8: GIVEN a trust store file that exists but cannot be parsed, WHEN any command consults it, THEN the command fails with exit status 2 and a message naming the store path — never silently treating the store as empty.
- AC-003.9: On Unix systems, WHEN the trust store file is created, THEN its permission bits are `0600`.

Verification:

- Automated: integration tests covering the full lifecycle in temp trees with an overridden state location; Unix permission assertion gated to Unix.

### SPEC-004: Profile pin precedence

Profile selection MUST resolve, in order: `--profile` flag, `AGENTENV_PROFILE` (non-empty), the trusted project file's `profile` pin, then `default_profile`. A trusted pin naming an undefined profile MUST fail with exit status 3 and a message that names the project file and lists the defined profiles. An untrusted file's pin MUST NOT participate at any position.

Source trace:

- PRD: PRD-FR-002
- Architecture: ARCH-005

Acceptance criteria:

- AC-004.1: GIVEN a trusted pin `work` and no flag or environment selection, WHEN a read command runs, THEN profile `work` is active.
- AC-004.2: GIVEN a trusted pin `work` and `AGENTENV_PROFILE=personal`, WHEN a read command runs, THEN profile `personal` is active.
- AC-004.3: GIVEN a trusted pin `work` and `--profile personal`, WHEN a read command runs, THEN profile `personal` is active.
- AC-004.4: GIVEN a trusted pin naming an undefined profile, WHEN a command that resolves a profile runs, THEN it fails with exit status 3 and the message names the project file path.
- AC-004.5: GIVEN a trusted pin and a config file with a different `default_profile`, WHEN a read command runs with no flag or environment selection, THEN the pin wins over `default_profile`.
- AC-004.6: GIVEN an untrusted file pinning `work` and a config `default_profile = "personal"`, WHEN a read command runs, THEN profile `personal` is active.
- AC-004.7: GIVEN a trusted pin, WHEN `agentenv run --with <entry> -- <target>` runs, THEN injection planning uses the pinned profile (probe target observes the pinned profile's values).

Verification:

- Automated: integration tests including a `run` test via the existing `test-probe` binary.

### SPEC-005: Inertness and the untrusted-file notice

While a discovered project file is untrusted (never approved, changed since approval, unreadable, or invalid), every command MUST behave exactly as if the file were absent — identical stdout bytes and exit status — except that one single-line notice MUST be written to stderr naming the file path and referring to `agentenv project status`. The notice MUST never appear on stdout and MUST never alter `--json` payloads. A *trusted* project file that cannot be read at use time MUST fail the command with exit status 2.

Source trace:

- PRD: PRD-FR-004, PRD-FR-007
- Architecture: ARCH-004

Acceptance criteria:

- AC-005.1: GIVEN an untrusted project file, WHEN `agentenv list --json` runs, THEN stdout is byte-identical to the same invocation without the file, the exit status is unchanged, and stderr contains exactly one notice line naming the file.
- AC-005.2: GIVEN an untrusted and unparseable project file, WHEN any read command runs, THEN the command succeeds as if no file existed, with the single stderr notice.
- AC-005.3: GIVEN a trusted project file that is then made unreadable (for example permissions removed), WHEN a command that resolves a profile runs, THEN it fails with exit status 2 and a message naming the file.
- AC-005.4: GIVEN `--no-project` or non-empty `AGENTENV_NO_PROJECT` with an untrusted file present, WHEN any command runs, THEN no notice is emitted.

Verification:

- Automated: integration tests asserting stdout snapshots, stderr content, and exit codes.

### SPEC-006: `agentenv project status`

`agentenv project status` MUST report, in text and `--json`: whether a project file was discovered (and its path), its trust state (`trusted`, `untrusted-new`, `untrusted-changed`, or `invalid` with violations), its profile pin, and (Phase 2) the requirement report. Exit status MUST be: 0 when no file is discovered or when a trusted file's requirements are all satisfied; 5 when the discovered file is untrusted or invalid; 3 when the file is trusted but at least one requirement is unsatisfied. The JSON shape MUST be stable and MUST include the config `version` context consistent with existing JSON surfaces.

Source trace:

- PRD: PRD-FR-003, PRD-FR-005, PRD-NFR-002, PRD-NFR-003
- Architecture: ARCH-003, ARCH-006

Acceptance criteria:

- AC-006.1: GIVEN no project file, WHEN `project status` runs, THEN it reports that no file was discovered and exits 0.
- AC-006.2: GIVEN an untrusted (new or changed) file, WHEN `project status` runs, THEN it reports the trust state and the approval command to run, and exits 5.
- AC-006.3: GIVEN an invalid file, WHEN `project status` runs, THEN it reports each violation by TOML path (values not echoed) and exits 5.
- AC-006.4: GIVEN a trusted file whose declared requirements are all satisfied, WHEN `project status` runs, THEN it reports every requirement as satisfied with its reason and exits 0.
- AC-006.5: GIVEN a trusted file with an unsatisfied requirement, WHEN `project status` runs, THEN that requirement is reported unsatisfied with what is missing, and the exit status is 3.
- AC-006.6: GIVEN any of the above states, WHEN `project status --json` runs, THEN stdout is a single JSON document carrying the same information, and no notice line is mixed into stdout.

Verification:

- Automated: integration tests plus a JSON snapshot under `tests/snapshots/`.

### SPEC-007: Requirement declarations and structural checking

A trusted project file's `[requires.<entry>]` declarations MUST be checked structurally against the active profile: a requirement is satisfied when the named entry exists in the active profile and, when `fields` is declared, every listed field path resolves inside that entry. Checking MUST NOT resolve credentials, execute provider commands, or read secret stores. Requirement checking MUST NOT block or alter any command other than `project status`.

Source trace:

- PRD: PRD-FR-003
- Architecture: ARCH-006

Acceptance criteria:

- AC-007.1: GIVEN a requirement naming an entry present in the active profile with all declared fields resolvable, WHEN `project status` runs, THEN the requirement is satisfied.
- AC-007.2: GIVEN a requirement naming an entry absent from the active profile, WHEN `project status` runs, THEN the requirement is unsatisfied, naming the missing entry.
- AC-007.3: GIVEN a requirement whose `fields` lists a path that does not resolve in the entry, WHEN `project status` runs, THEN the requirement is unsatisfied, naming the missing field path.
- AC-007.4: GIVEN requirements against a profile whose entries reference credentials with unavailable providers, WHEN `project status` runs, THEN the report is unaffected by credential availability and no provider command executes (observable via the counting-provider fixture).
- AC-007.5: GIVEN a trusted file with unsatisfied requirements, WHEN `agentenv get`, `list`, or `run` executes, THEN those commands behave exactly as without the requirements (no warning, no failure attributable to requirements).

Verification:

- Automated: integration tests including `tests/fixtures/counting_provider.sh` to prove no provider execution.

### SPEC-008: Documentation and agent protocol

The README, the agent skill (`skills/agentenv/SKILL.md`), and the README's `AGENTS.md` block MUST document: the project file schema and discovery, the trust lifecycle and its exit statuses (including the new status 5), the precedence chain including the pin, the bypass flag/variable, and the pairing guidance — non-secret values may live in `.env`; credentials reach tools only through `agentenv run` (worked docker compose example using variable passthrough or `${VAR}` interpolation); `env_file:` with secrets is documented as the anti-pattern this replaces. The skill MUST add a project-discovery step (`agentenv project status --json`) to its reading protocol.

Source trace:

- PRD: PRD-FR-008 (bypass documentation), UX and Interaction Notes; PRD-SM-001
- Architecture: ARCH-003, ARCH-004

Acceptance criteria:

- AC-008.1: GIVEN the updated README, WHEN each documented `project` command and the compose pairing example are executed against the built binary in a scratch tree, THEN they behave as documented.
- AC-008.2: GIVEN the updated exit-status table, WHEN compared with implemented behavior, THEN statuses 0–5 and 127 match, and no existing status changed meaning.
- AC-008.3: GIVEN the updated skill document, WHEN its reading protocol is followed in order in a trusted project, THEN the first steps discover the project state before profile-dependent reads.

Verification:

- Automated: none (prose); Manual: execute each documented command sequence once on macOS or Linux.

### SPEC-009: Backward compatibility

In the absence of a discovered project file, every existing command MUST behave byte-identically to the pre-change release: same stdout, same stderr, same exit statuses. The existing test suite MUST pass without modification, except where a test's working directory now sits inside a tree containing a fixture project file it created itself.

Source trace:

- PRD: PRD-NFR-004
- Architecture: Risks table (behavior drift)

Acceptance criteria:

- AC-009.1: GIVEN the full pre-existing test suite, WHEN `cargo test` runs on the completed change, THEN all pre-existing tests pass unmodified.
- AC-009.2: GIVEN the JSON snapshots under `tests/snapshots/`, WHEN snapshot tests run, THEN all snapshots are byte-identical (no project file present in those tests).

Verification:

- Automated: `cargo test` on Linux, macOS, and Windows via existing CI.

## Edge Cases

| ID | Case | Expected behavior | Verification |
| --- | --- | --- | --- |
| EDGE-001 | `.agentenv.toml` in `$HOME`, command run in an unrelated tree under `$HOME` | Discovered (walk reaches it); trust gate applies as usual | Integration test |
| EDGE-002 | Empty project file (zero bytes) | Invalid: `version` missing (AC-002.2); inert + notice until fixed | Integration test |
| EDGE-003 | `AGENTENV_NO_PROJECT=""` (empty) | Counts as unset; discovery proceeds | Integration test |
| EDGE-004 | Trust store base environment variable unset (no `XDG_STATE_HOME`/`HOME` on Unix, no `LOCALAPPDATA` on Windows) when trust must be read or written | Explicit configuration error naming the variables, exit 2 (mirrors config-locate behavior) | Unit test, injected env |
| EDGE-005 | `project allow` run twice on the same content | Second run succeeds and reports approval already current | Integration test |
| EDGE-006 | Project file deleted after approval | No file discovered; commands behave as with no project file; stale record is harmless | Integration test |
| EDGE-007 | Pin equals the profile already selected by `default_profile` | Identical outcome; no special casing observable | Integration test |
| EDGE-008 | `--profile` with empty value while a trusted pin exists | Existing usage error (unchanged) | Existing test suite |
| EDGE-009 | Requirement entry name containing punctuation (quoted TOML key) | Validated as an entry name by existing path rules; checked against the profile | Integration test |
| EDGE-010 | stderr notice with `--json` commands piped to a JSON parser | stdout parses as JSON; notice only on stderr | Integration test (AC-005.1, AC-006.6) |

## Dependencies

| Requirement | Dependency | Reason |
| --- | --- | --- |
| SPEC-003 | SPEC-002 | `allow` validates before recording |
| SPEC-004 | SPEC-003 | Only a trusted pin participates in precedence |
| SPEC-005 | SPEC-001, SPEC-003 | Inertness is defined relative to discovery and trust state |
| SPEC-006 | SPEC-003 | Status reports trust state |
| SPEC-007 | SPEC-006 | The report surfaces through `project status` |
| SPEC-008 | SPEC-001..007 | Documents shipped behavior |
| SPEC-009 | all | Compatibility is a property of the whole change |

## Acceptance Matrix

| Acceptance ID | Requirement | Phase | Verification method | Status |
| --- | --- | --- | --- | --- |
| AC-001.1..4 | SPEC-001 | Phase 1 | `cargo test` integration | Draft |
| AC-002.1..5 | SPEC-002 | Phase 1 | `cargo test` integration | Draft |
| AC-003.1..9 | SPEC-003 | Phase 1 | `cargo test` integration/unit | Draft |
| AC-004.1..7 | SPEC-004 | Phase 1 | `cargo test` integration | Draft |
| AC-005.1..4 | SPEC-005 | Phase 1 | `cargo test` integration | Draft |
| AC-006.1..3 | SPEC-006 | Phase 1 | `cargo test` integration | Draft |
| AC-006.4..6 | SPEC-006 | Phase 2 | `cargo test` integration + snapshot | Draft |
| AC-007.1..5 | SPEC-007 | Phase 2 | `cargo test` integration | Draft |
| AC-008.1..3 | SPEC-008 | Phase 3 | Manual doc walkthrough | Draft |
| AC-009.1..2 | SPEC-009 | all | `cargo test` full suite | Draft |

## Implementation Notes

- The trust fingerprint and store format are internal; only the behaviors above are contractual. The store lives at `$XDG_STATE_HOME/agentenv/trust.toml` (else `~/.local/state/agentenv/trust.toml`; Windows `%LOCALAPPDATA%\agentenv\trust.toml`) — internal location, documented for support but not a stability contract.
- Tests overriding the trust store location need an override mechanism analogous to `AGENTENV_FILE`; `AGENTENV_TRUST_FILE` is reserved for this and follows the same empty-counts-as-unset rule. It is a supported escape hatch, documented alongside `AGENTENV_FILE`.
- Phase 1 ships `project status` without the requirement report (discovery, trust state, pin only); Phase 2 extends the same command and JSON shape additively.

## Assumptions

- SPEC-AS-001: The project file name is exactly `.agentenv.toml` (hidden file, matching `.envrc`/`.mise.toml` convention) because the name should read as tool configuration, not project data.
- SPEC-AS-002: Discovery includes the working directory itself as the first candidate because that is the least surprising reading of "nearest".
- SPEC-AS-003: The stderr notice's exact wording is not contractual; its properties are: exactly one line, names the file path, names `agentenv project status`.
- SPEC-AS-004: Exit status 5 is free for "project file present but not trusted" because 0–4 and 127 are the only statuses documented today.
- SPEC-AS-005: A stale trust record for a deleted or moved file is retained harmlessly (no garbage collection in v1) because records are tiny and pruning adds state-mutation paths for no behavioral gain.
- SPEC-AS-006: `project allow`/`revoke`/`status` ignore `--profile` for trust decisions (trust is per-file, not per-profile); `status` uses normal selection to compute the requirement report and reports which profile it checked against.

## Clarifications

### Session 2026-08-28

- Q: What may the project file contain? -> A: Selection-only — profile pin plus `[requires]` declarations (applied to Scope, SPEC-002).
- Q: Trust model? -> A: Trust-on-first-use with content-hash invalidation, user-owned state (applied to SPEC-003).
- Q: Pin position in precedence? -> A: Below `AGENTENV_PROFILE`, above `default_profile` (applied to SPEC-004).

## Open Questions

| ID | Question | Blocking? | Resolution |
| --- | --- | --- | --- |
| — | none | — | — |
