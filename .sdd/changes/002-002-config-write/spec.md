# Implementation Specification: 002-config-write

Authoring rules: behavior altitude, think-like-a-tester, clarification marker cap (max 3, everything else resolved as a recorded assumption) — see `docs/authoring-discipline.md`.

## Source Artifacts

- Change ID: 002-002-config-write
- PRD: merged into `## Scope` (light tier)
- Architecture: merged into `## Design Notes` (light tier)
- Current specs: `.sdd/specs/current/` (agentenv v1 CLI behavior), `README.md`

## Scope

### Problem

agentenv v1 exposes configuration read-only. The only write path is `credential set`, which targets the platform credential store and never touches the TOML file. Users hand-author `config.toml` against a strict validator (mandatory descriptions, sensitive-field guardrail, reserved `inject` table) and iterate through `agentenv validate` failures. Coding agents following the agent usage protocol can read configuration but cannot register new entries or credentials on the user's behalf.

### Goals

- Add mutation commands whose writes are always whole-file-validated before anything touches disk: an invalid result is refused and the file is untouched.
- Preserve the user's hand-written TOML formatting: comments, blank lines, key order, and untouched sections survive every mutation.
- Extend the existing no-secret-leak invariant to every new code path: no command prints a user-provided value, an existing field value, or a TOML source line in any diagnostic.
- Close the credential bootstrap gap: define a credential from the CLI, then store its value with the existing `credential set`.

### In Scope

- `agentenv init` — create the config file at the resolved path.
- `agentenv set <path> <value>` — write one value at a profile-scoped path (`--type`, `--json`, `--description`, `--profile`).
- `agentenv unset <path>` — remove a field or table at a profile-scoped path (`--profile`).
- `agentenv credential add <name> ...` — add a credential definition.
- README updates: CLI reference, agent usage protocol, safety section.

### Out of Scope

- `agentenv edit` ($EDITOR round-trip) — deferred.
- Modifying or removing existing credential definitions (`credential remove` / `credential update`) — deferred; hand-edit plus `validate` remains the path.
- Mutating top-level fields (`default_profile`, `version`) — deferred; per-invocation `--profile` covers agent use.
- Deleting a whole profile — not addressable by the profile-scoped path grammar; deferred.
- File locking or concurrent-write coordination — single-user local tool, last writer wins.
- Secret-content scanning beyond the existing sensitive-field-name guardrail.
- Machine-verified Windows behavior — specified and code-reviewed, consistent with v1.

## Phase Map

| Phase | Name | Priority | Objective | Depends on | Independent test |
| --- | --- | --- | --- | --- | --- |
| Phase 1 | Write pipeline + set/unset | P1 (MVP) | Format-preserving, validated, atomic mutations of profile data | None | Round-trip `set`/`unset` against a commented fixture file; refusal cases leave the file byte-identical |
| Phase 2 | init | P2 | Bootstrap a valid config file | Phase 1 (atomic write) | `init` on an empty directory yields a file that passes `agentenv validate` |
| Phase 3 | credential add | P2 | Define credentials from the CLI | Phase 1 (write pipeline) | `credential add` then `credential list` shows the definition; invalid flag sets are refused |

## Requirements

### SPEC-001: Format-preserving validated atomic write pipeline

Every config mutation MUST pass through one pipeline: read the current file, apply the mutation to a format-preserving document, validate the complete resulting document with the same schema rules as `agentenv validate`, and atomically replace the file only when validation passes.

Acceptance criteria:

- AC-001.1: GIVEN a config file containing comments, blank lines, and a deliberate key order, WHEN `set` changes one field, THEN every line outside the mutated assignment is byte-identical to before.
- AC-001.2: GIVEN a mutation whose result violates validation (for example a new entry without a description), WHEN the command runs, THEN it exits with status 2, stderr lists each violation with its config path, and the file is byte-identical to before.
- AC-001.3: GIVEN an existing config file with permission bits 0600 on a Unix-like system, WHEN a mutation succeeds, THEN the replacement file has the same permission bits and is owned by the invoking user.
- AC-001.4: GIVEN any failure after the command starts (parse error, validation refusal, I/O error while writing), WHEN the command exits non-zero, THEN the original file content is intact — the pipeline writes a temporary file in the config file's directory and renames it over the target only as the final step.
- AC-001.5: GIVEN a config file that is not valid TOML or fails validation before the mutation, WHEN any write command runs, THEN it exits with status 2 reporting the pre-existing problem and writes nothing.

Verification:

- Automated: integration tests over fixture files with comments; byte-comparison before/after refusals.

### SPEC-002: `set` writes one profile-scoped value

`agentenv set <path> <value>` MUST write exactly one value at a path interpreted with the same segment grammar and profile selection (`--profile` / `AGENTENV_PROFILE` / `default_profile`) as `get`. Missing intermediate tables along the path are created.

Value typing:

- Default: the value is written as a TOML string.
- `--type int|float|bool`: the value is parsed to that TOML type; a value that does not parse is a usage error (exit 1) that does not echo the value.
- `--json <json>`: the value argument is parsed as JSON and converted to the equivalent TOML value (arrays and tables allowed; JSON `null` anywhere is a usage error, exit 1). `--type` and `--json` are mutually exclusive (exit 1).

Acceptance criteria:

- AC-002.1: GIVEN an existing scalar field, WHEN `set` targets it, THEN the stored value is replaced and the success message names the rendered path and never the value.
- AC-002.2: GIVEN a path whose entry does not exist, WHEN `set` runs with `--description <text>`, THEN the entry table is created with that description and the target value in one atomic write.
- AC-002.3: GIVEN a path whose entry does not exist, WHEN `set` runs without `--description`, THEN the command exits with status 2 and the violation list names the missing description path.
- AC-002.4: GIVEN a nonexistent profile selected via `--profile`, WHEN `set description <text>` runs, THEN the profile table is created with that description (this is the documented profile-bootstrap path).
- AC-002.5: GIVEN a path that traverses an existing non-table value (scalar or array), WHEN `set` runs, THEN it exits with status 3 naming the conflicting path and writes nothing.
- AC-002.6: GIVEN a field whose final segment name is exactly or ends with `token`, `password`, `secret`, `api_key`, or `private_key`, WHEN `set` runs with a value that is not a `credential://` reference, THEN it exits with status 2, the message names the field, states the guardrail, points to `credential add` plus `credential set`, and does not echo the value.
- AC-002.7: GIVEN `--json` with an array or inline-table value, WHEN `set` runs, THEN `get <path> --json` returns the equivalent JSON value.

Verification:

- Automated: integration tests including a sentinel-value leak check on every error path.

### SPEC-003: `unset` removes one profile-scoped value

`agentenv unset <path>` MUST remove the field or table at a profile-scoped path, subject to the SPEC-001 pipeline.

Acceptance criteria:

- AC-003.1: GIVEN an existing field, WHEN `unset` targets it, THEN the field is removed and the success message names the rendered path.
- AC-003.2: GIVEN an existing entry table, WHEN `unset` targets the entry, THEN the whole table (including its `inject` table) is removed.
- AC-003.3: GIVEN a path that does not exist in the active profile, WHEN `unset` runs, THEN it exits with status 3 and the file is unchanged.
- AC-003.4: GIVEN a removal whose result violates validation (for example removing an entry's `description`), WHEN `unset` runs, THEN it exits with status 2 listing the violation and the file is unchanged.

Verification:

- Automated: integration tests over fixture files.

### SPEC-004: `init` bootstraps the config file

`agentenv init` MUST create the config file at the resolved path (default path rules and `AGENTENV_FILE` exactly as in v1) when it does not exist: parent directories are created as needed, the file is created with permission bits 0600 on Unix-like systems, and its content is a minimal valid configuration (`version = 1` plus a brief comment header pointing at the README).

Acceptance criteria:

- AC-004.1: GIVEN no config file at the resolved path, WHEN `init` runs, THEN the file exists, `agentenv validate` passes, and stdout names the created path.
- AC-004.2: GIVEN a config file already present at the resolved path, WHEN `init` runs, THEN it exits with status 2 naming the path and the file is untouched.
- AC-004.3: GIVEN `AGENTENV_FILE` pointing into a nonexistent directory, WHEN `init` runs, THEN the directory chain is created and the file lands there.
- AC-004.4: GIVEN a parent directory that cannot be created, WHEN `init` runs, THEN it exits with status 2 with a diagnostic naming the path.

Verification:

- Automated: integration tests with temp HOME / `AGENTENV_FILE`.

### SPEC-005: `credential add` defines a credential

`agentenv credential add <name>` MUST append a `[credentials.<name>]` table through the SPEC-001 pipeline. Flags mirror the credential schema: `--description <text>` and `--provider env|keychain|command` are required; provider-specific fields are `--env-var <NAME>` (env), `--service <s>` and `--account <a>` (keychain), repeated ordered `--argv <arg>` (command); `--inject-as <ENV>` follows the schema's required-ness for `inject_as`. A missing or extraneous flag for the chosen provider is a usage error (exit 1) naming the flag.

Acceptance criteria:

- AC-005.1: GIVEN valid flags for each provider kind, WHEN `credential add` runs, THEN `credential list --json` shows the new definition with its provider fields and `inject_as`.
- AC-005.2: GIVEN a name that already exists under `[credentials]`, WHEN `credential add` runs, THEN it exits with status 1 stating the name is taken and the file is unchanged.
- AC-005.3: GIVEN a successful `credential add` with `--provider keychain`, WHEN the command completes, THEN stdout includes a hint to run `agentenv credential set <name>` to store the value.
- AC-005.4: GIVEN provider-specific flags that do not match `--provider` (for example `--argv` with `--provider env`), WHEN `credential add` runs, THEN it exits with status 1 naming the mismatched flag.

Verification:

- Automated: integration tests per provider kind.

### SPEC-006: No-secret-echo invariant across write commands

No write command (`init`, `set`, `unset`, `credential add`) may print a user-provided value, an existing config field value, or a TOML source line to stdout or stderr in any code path, success or failure. Success messages name paths only.

Acceptance criteria:

- AC-006.1: GIVEN a sentinel value passed to `set` (as the value, as `--json`, and as a `--type` parse failure), WHEN each command runs, THEN the sentinel appears in no stdout or stderr output.
- AC-006.2: GIVEN a config file containing a sentinel string value, WHEN any write command fails (validation refusal, conflict, missing path), THEN the sentinel appears in no output.

Verification:

- Automated: sentinel leak checks folded into the SPEC-002/003/005 error-path tests.

## Edge Cases

| ID | Case | Expected behavior | Verification |
| --- | --- | --- | --- |
| EDGE-001 | `set`/`unset`/`credential add` with no config file present | Exit 2; message includes a hint to run `agentenv init` | Integration test |
| EDGE-002 | `set` with an empty or grammar-invalid path | Exit 1 per the existing segment grammar errors | Integration test |
| EDGE-003 | `set` writing an empty-string `description` | Exit 2: validator requires non-empty descriptions | Integration test |
| EDGE-004 | `--json 'null'` or JSON containing null | Exit 1: TOML has no null | Unit test |
| EDGE-005 | Config path is a symlink | The pipeline resolves the path and atomically replaces the resolved target file; the symlink itself is preserved | Integration test (Unix) |
| EDGE-006 | Two concurrent writers | Last writer wins; no locking; documented | README statement |
| EDGE-007 | `unset` the last field of a profile (`description`) | Exit 2: a profile table must keep a valid description; profile deletion is out of scope | Integration test |
| EDGE-008 | `set` into the reserved `inject` table with a non-string value | Exit 2 via validation | Integration test |
| EDGE-009 | `--type int` with a non-integer value | Exit 1; the diagnostic does not echo the value | Integration test |
| EDGE-010 | Mutation on a file whose permissions are broader than 0600 | The write preserves the existing bits; `validate` remains the permission gate, consistent with read commands | Integration test |

## Dependencies

| Requirement | Dependency | Reason |
| --- | --- | --- |
| SPEC-002 | SPEC-001 | `set` is a pipeline client |
| SPEC-003 | SPEC-001 | `unset` is a pipeline client |
| SPEC-004 | SPEC-001 | `init` reuses the atomic create path |
| SPEC-005 | SPEC-001 | `credential add` is a pipeline client |
| SPEC-006 | SPEC-002..005 | Invariant spans all write commands |

## Acceptance Matrix

| Acceptance ID | Requirement | Phase | Verification method | Status |
| --- | --- | --- | --- | --- |
| AC-001.1..5 | SPEC-001 | Phase 1 | cargo test (integration) | Draft |
| AC-002.1..7 | SPEC-002 | Phase 1 | cargo test (integration) | Draft |
| AC-003.1..4 | SPEC-003 | Phase 1 | cargo test (integration) | Draft |
| AC-004.1..4 | SPEC-004 | Phase 2 | cargo test (integration) | Draft |
| AC-005.1..4 | SPEC-005 | Phase 3 | cargo test (integration) | Draft |
| AC-006.1..2 | SPEC-006 | Phases 1–3 | cargo test (sentinel checks) | Draft |

## Implementation Notes

- Exit statuses reuse the v1 mapping: 1 usage, 2 configuration/validation, 3 unknown profile/entry/path/name. No new statuses.
- User-facing messages follow the existing style: lowercase diagnostics naming the config path, with a concrete remedy command where one exists.

## Design Notes (light tier)

- Format preservation: add `toml_edit` as a dependency for the mutation document (`DocumentMut`). The existing `toml` crate remains the validation parser: the mutated document is serialized to a string and fed through the existing parse-and-validate pipeline (`config::validate`) before any disk write. Rejected: serializing via `toml::Table` (destroys comments and formatting of a hand-maintained file); migrating the whole read pipeline to `toml_edit` (larger blast radius, no behavioral gain).
- Module seam: one new module `src/config/write.rs` owns the pipeline — locate/read the document, apply a mutation operation, validate the result, atomically persist. CLI subcommands in `src/cli/commands.rs` are thin clients. Rejected: spreading write logic across per-command modules (duplicated atomic-write and validation plumbing).
- Path addressing reuses `path::Segments` verbatim, so `get` and `set`/`unset` share one grammar and rendering.
- Atomic persist: temporary file in the same directory as the (symlink-resolved) target, write, apply permission bits (copied from the existing file, 0600 for a new file), fsync the file, rename over the target, fsync the directory on Unix. Windows semantics are specified as rename-replace and code-reviewed only, matching the v1 Windows posture.
- Descriptions are ordinary fields: `set description <text>` bootstraps a profile; `--description` is sugar that additionally writes `description` on the entry named by the first path segment. No dedicated `profile add`/`entry add` commands.
- Threat model unchanged: credential values still reach only the platform store via `credential set`; the TOML gains definitions, never secrets. The SPEC-002 guardrail refusal is a targeted UX layer in front of the same rule the validator enforces.

## Assumptions

- SPEC-AS-001: Pre-write validation delegates entirely to the existing validator; its rules are not restated here, and any result the validator accepts is writable.
- SPEC-AS-002: `credential add` flag required-ness (including `--inject-as`) mirrors the current schema in `config/validate.rs`; the implementation derives it from the validator's rules rather than this spec.
- SPEC-AS-003: `--description` targets the entry named by the first path segment; deeper nested tables that also require descriptions surface through the AC-002.3 violation path rather than extra flags.
- SPEC-AS-004: Writing through a symlinked config path replaces the resolved target, preserving the symlink (EDGE-005), because `AGENTENV_FILE` users commonly symlink into dotfile repos.
- SPEC-AS-005: No file locking (EDGE-006); a single-user local CLI accepts last-writer-wins.

## Clarifications

### Session 2026-08-22

- Q: Create a dedicated branch for this change? -> A: Yes, `sdd/002-config-write` (applied to manifest git block).
- Q: Which command surface? -> A: Plan B confirmed by the user: `set`/`unset`/`init` + `credential add`; `edit` deferred (applied to Scope).

## Open Questions

| ID | Question | Blocking? | Resolution |
| --- | --- | --- | --- |
| — | None | — | — |

## Review Log (light tier)

### Round 1 — 2026-08-22

- Pending: two-lane review (Codex provider + Claude provider) in flight.

Decision: Pending
