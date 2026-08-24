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
- Preserve the user's hand-written TOML formatting: comments (including a trailing comment on a mutated line), blank lines, key order, and untouched sections survive every mutation.
- Extend the existing no-secret-leak invariant to every new code path, with the same scope the validator's diagnostics already observe: no open-schema profile value and no user-supplied value is ever echoed outside the v1 SPEC-019 metadata boundary (SPEC-006).
- Close the credential bootstrap gap: define a credential from the CLI, then store its value with the existing `credential set`.

### In Scope

- `agentenv init` — create the config file at the resolved path.
- `agentenv set <path> <value>` — write one value at a profile-scoped path (`--type`, `--description`, `--create-profile`, `--profile`).
- `agentenv unset <path>` — remove a field or table at a profile-scoped path (`--profile`).
- `agentenv credential add <name> ...` — add a credential definition.
- README updates: CLI reference, agent usage protocol (including the `credential add` → `set` ordering), safety section (write-surface delta).

### Out of Scope

- `agentenv edit` ($EDITOR round-trip) — deferred.
- Modifying or removing existing credential definitions (`credential remove` / `credential update`) — deferred; hand-edit plus `validate` remains the path.
- Mutating top-level fields (`default_profile`, `version`) — deferred; per-invocation `--profile` covers agent use.
- Deleting a whole profile — not addressable by the profile-scoped path grammar; deferred.
- File locking or concurrent-write coordination — single-user local tool, last writer wins.
- Secret-content scanning beyond the existing sensitive-field-name guardrail.
- Writing TOML datetime values — `--type` offers no datetime variant.

## Phase Map

| Phase | Name | Priority | Objective | Depends on | Independent test |
| --- | --- | --- | --- | --- | --- |
| Phase 1 | Write pipeline + set/unset | P1 (MVP) | Format-preserving, validated, atomic mutations of profile data | None | Round-trip `set`/`unset` against a commented fixture file; refusal cases leave the file byte-identical |
| Phase 2 | init | P2 | Bootstrap a valid config file | Phase 1 (atomic write) | `init` on an empty directory yields a file that passes `agentenv validate` |
| Phase 3 | credential add | P2 | Define credentials from the CLI | Phase 1 (write pipeline) | `credential add` then `credential list` shows the definition; invalid flag sets are refused |

## Requirements

### SPEC-001: Format-preserving validated atomic write pipeline

Every config mutation MUST pass through one pipeline: read the current file, apply the mutation to a format-preserving document, validate the complete resulting document with the same rule set as `config::validate::validate` — which excludes the Unix permission gate that only the `validate` command adds (see EDGE-010) — and atomically replace the file only when validation passes.

Write commands have no JSON output; they MUST reject the global `--json` output flag as a usage error, consistent with `run` and the credential actions.

Acceptance criteria:

- AC-001.1: GIVEN a config file containing comments, blank lines, and a deliberate key order, WHEN `set` changes one field, THEN every line outside the mutated assignment is byte-identical to before, and no table header is added or removed for tables that existed only implicitly (dotted headers).
- AC-001.2: GIVEN a mutation whose result violates validation (for example a new entry without a description), WHEN the command runs, THEN it exits with status 2, stderr lists each violation with its config path, and the file is byte-identical to before.
- AC-001.3: GIVEN an existing config file with permission bits 0600 on a Unix-like system, WHEN a mutation succeeds, THEN the replacement file's permission bits equal the pre-existing file's bits.
- AC-001.4: GIVEN any failure after the command starts (parse error, validation refusal, I/O error while writing or renaming), WHEN the command exits, THEN the status is 2 with a diagnostic naming the config path, and the original file content is intact — the pipeline writes a temporary file in the resolved target's directory and renames it over the target only as the final step.
- AC-001.5: GIVEN a config file that is not valid TOML or fails `config::validate::validate` before the mutation, WHEN any write command runs, THEN it exits with status 2 reporting the pre-existing problem and writes nothing. (Permission bits broader than 0600 do not block writes: EDGE-010.)
- AC-001.6: GIVEN a field assignment carrying a trailing comment (`endpoint = "…"  # prod`), WHEN `set` replaces that field's value, THEN the trailing comment and the surrounding whitespace on that line survive the replacement.
- AC-001.7: GIVEN any write command invoked with the global `--json` flag, WHEN it runs, THEN it exits with status 1 stating the command has no JSON output.

Verification:

- Automated: integration tests over fixture files with comments; byte-comparison before/after refusals.

### SPEC-002: `set` writes one profile-scoped value

`agentenv set <path> <value>` MUST write exactly one value at a path interpreted with the same segment grammar as `get`, resolved against the active profile's table (one level above `get`'s entry root, so `description` addresses the profile's own description — see Design Notes for the read/write asymmetry this creates). Profile selection uses the v1 precedence (`--profile` / `AGENTENV_PROFILE` / `default_profile`); an unknown profile from any of those sources is an error unless `--create-profile` is given. Missing intermediate tables below an existing entry are created.

Value typing (`--type string|int|float|bool|json`, default `string`):

- `string` (default): the value is written as a TOML string.
- `int|float|bool`: the value is parsed to that TOML type; a value that does not parse is a usage error (exit 1) that does not echo the value. `float` accepts what Rust float parsing accepts, including `inf` and `nan`, mirroring how v1 renders them on read.
- `json`: the value argument is parsed as JSON and converted to the equivalent TOML value; arrays and objects are allowed, JSON `null` anywhere is a usage error (exit 1), and malformed JSON is a usage error (exit 1) that does not echo the input. JSON objects are written as inline tables; see Design Notes.

Credential references are ordinary string values with extra validation: writing a `credential://` value whose name is not defined under `[credentials]`, or whose reference grammar is invalid, is refused by the pipeline (exit 2) — with the same scope as the validator's reference scan: entry string fields at any depth, excluding `description` keys, the reserved `inject` table, and array elements. The working order is `credential add` first, then `set` the reference (reflected in the README agent protocol).

Acceptance criteria:

- AC-002.1: GIVEN an existing scalar field, WHEN `set` targets it, THEN the stored value is replaced and the success message names the rendered path and never the value.
- AC-002.2: GIVEN a path whose entry does not exist in an existing profile, WHEN `set` runs with `--description <text>`, THEN the entry table is created with that description and the target value in one atomic write, emitted as a standard `[profiles.<p>.<entry>]` table.
- AC-002.3: GIVEN a path whose entry does not exist, WHEN `set` runs without `--description`, THEN the command exits with status 2 and the violation list names the missing description path.
- AC-002.4: GIVEN an unknown profile selected via `--profile`, `AGENTENV_PROFILE`, or `default_profile`, WHEN `set` runs without `--create-profile`, THEN it exits with status 3 listing the defined profiles, exactly as `get` does.
- AC-002.5: GIVEN `--create-profile <text>` together with an explicit `--profile <name>` naming an absent profile, WHEN `set` runs, THEN the profile table is created with `<text>` as its description plus the target mutation in one atomic write; when the target path itself addresses the new profile's `description`, the path's value wins over the `--create-profile` text. `--create-profile` without an explicit `--profile`, or with a profile that already exists, is a usage error (exit 1).
- AC-002.6: GIVEN a path that traverses an existing non-table value (scalar or array), WHEN `set` runs, THEN it exits with status 3 naming the conflicting path and writes nothing.
- AC-002.7: GIVEN a string field whose name matches the validator's sensitive-name rule — the same predicate and scope as `config::validate`: exact names `token`, `password`, `secret`, `api_key`, `private_key` or names ending in `_token`, `_password`, `_secret`, `_api_key`, `_private_key`, case-insensitive, string values only, excluding the reserved entry-level `inject` table — WHEN `set` runs with a value that is not a `credential://` reference, THEN it exits with status 2, the message names the field, states the guardrail, points to `credential add` plus `credential set`, and does not echo the value.
- AC-002.8: GIVEN the guardrail's exclusions, WHEN `set` writes (a) an `inject`-table mapping whose target name looks sensitive (for example `GITHUB_TOKEN = "field"`), (b) a non-string value into a sensitive-named field (for example `--type int` into `token`), or (c) a string into a field named `mytoken` (no `_` separator), THEN none of the three is refused by the guardrail (writes remain subject to normal validation).
- AC-002.9: GIVEN `--type json` with an array or object value, WHEN `set` runs, THEN `get <path> --json` returns the equivalent JSON value.
- AC-002.10: GIVEN a deeper path under an existing entry whose intermediate tables do not exist (`set llm.limits.rpm 60 --type int`), WHEN `set` runs without `--description`, THEN the write succeeds: only profiles and entries require descriptions, nested tables do not.
- AC-002.11: GIVEN a value `credential://absent` where no credential `absent` is defined, or a reference with an invalid `?as=` value, WHEN `set` runs, THEN it exits with status 2 naming the problem (undefined credential names include the `credential add` remedy) and the file is unchanged.
- AC-002.12: GIVEN `--description <text>` targeting an entry that already has a description, WHEN `set` runs, THEN the entry's description is overwritten with `<text>`; when the target path itself is `<entry>.description`, the path's value wins over `--description` (same precedence as AC-002.5).
- AC-002.13: GIVEN a `credential://`-shaped string written where the validator's reference scan does not look (a `description` field, or an element of a `--type json` array), WHEN `set` runs, THEN reference validation does not refuse the write (normal validation still applies).

Verification:

- Automated: integration tests including a sentinel-value leak check on every error path.

### SPEC-003: `unset` removes one profile-scoped value

`agentenv unset <path>` MUST remove the field or table at a profile-scoped path, subject to the SPEC-001 pipeline. Path-resolution failure modes (unknown profile, missing path, traversal through a non-table) are inherited from SPEC-002 verbatim, all exit 3.

Acceptance criteria:

- AC-003.1: GIVEN an existing field, WHEN `unset` targets it, THEN the field is removed and the success message names the rendered path.
- AC-003.2: GIVEN an existing entry table, WHEN `unset` targets the entry, THEN the whole table (including its `inject` table) is removed.
- AC-003.3: GIVEN a path that does not exist in the active profile, or an unknown profile from any selection source, WHEN `unset` runs, THEN it exits with status 3 and the file is unchanged.
- AC-003.4: GIVEN a removal whose result violates validation (for example removing an entry's `description`), WHEN `unset` runs, THEN it exits with status 2 listing the violation and the file is unchanged.

Verification:

- Automated: integration tests over fixture files.

### SPEC-004: `init` bootstraps the config file

`agentenv init` MUST create the config file at the resolved path (default path rules and `AGENTENV_FILE` exactly as in v1) when it does not exist: parent directories are created as needed, the file is created with permission bits 0600 (applied explicitly after creation, so the umask can neither widen nor narrow them) on Unix-like systems, and its content is a minimal valid configuration (`version = 1` plus a brief comment header pointing at the README).

Acceptance criteria:

- AC-004.1: GIVEN no config file at the resolved path, WHEN `init` runs, THEN the file exists with permission bits exactly 0600, `agentenv validate` passes, and stdout names the created path plus the next step (`agentenv set <entry>.<field> <value> --profile <name> --create-profile "<text>" --description "<text>"`; phrasing follows the existing remedy-command style).
- AC-004.2: GIVEN a config file already present at the resolved path, WHEN `init` runs, THEN it exits with status 2 naming the path and the file is untouched.
- AC-004.3: GIVEN `AGENTENV_FILE` pointing into a nonexistent directory, WHEN `init` runs, THEN the directory chain is created and the file lands there.
- AC-004.4: GIVEN a parent directory that cannot be created, WHEN `init` runs, THEN it exits with status 2 with a diagnostic naming the path.

Verification:

- Automated: integration tests with temp HOME / `AGENTENV_FILE`.

### SPEC-005: `credential add` defines a credential

`agentenv credential add <name>` MUST append a `[credentials.<name>]` table through the SPEC-001 pipeline. Flags mirror the credential schema: `--description <text>`, `--provider env|keychain|command`, and `--inject-as <ENV>` are required for every provider (missing any of them is exit 1 naming the flag); provider-specific fields are `--env-var <NAME>` (env), `--service <s>` and `--account <a>` (keychain), repeated ordered `--argv <arg>` (command). A missing or extraneous provider-specific flag for the chosen provider is a usage error (exit 1) naming the flag. A name outside the schema's `[A-Za-z0-9_-]+` pattern is a usage error (exit 1) naming the required pattern, refused before the document is touched.

Acceptance criteria:

- AC-005.1: GIVEN valid flags for each provider kind, WHEN `credential add` runs, THEN `credential list --json` shows the new definition with its `inject_as`, and the config file contains the provider fields (the v1 `credential list` shape deliberately omits provider fields, so the file is their verification surface).
- AC-005.2: GIVEN a name that already exists under `[credentials]`, WHEN `credential add` runs, THEN it exits with status 1 stating the name is taken and the file is unchanged.
- AC-005.3: GIVEN a successful `credential add` with `--provider keychain`, WHEN the command completes, THEN stdout includes a hint to run `agentenv credential set <name>` to store the value.
- AC-005.4: GIVEN provider-specific flags that do not match `--provider` (for example `--argv` with `--provider env`), WHEN `credential add` runs, THEN it exits with status 1 naming the mismatched flag.
- AC-005.5: GIVEN a name containing characters outside `[A-Za-z0-9_-]`, WHEN `credential add` runs, THEN it exits with status 1 naming the required pattern and the file is unchanged.

Verification:

- Automated: integration tests per provider kind.

### SPEC-006: No-value-echo invariant across write commands

No write command (`init`, `set`, `unset`, `credential add`) may print a user-supplied value or an open-schema profile-data value to stdout or stderr in any code path, success or failure. Success messages name paths only. The v1 SPEC-019 diagnostic boundary applies unchanged and is incorporated by reference (`.sdd/specs/current/001-agent-context-cli/spec.md`): credential names and closed credential-schema metadata (the `name`, `inject_as`, `service`, `account` values and `argv[0]`), `default_profile`, `inject`-table source strings, and `credential://` reference strings may appear in diagnostics, exactly as in v1. In particular, AC-002.11's refusal echoes the offending reference string by design; AC-006.1's sentinel inputs are accordingly chosen not to be `credential://`-shaped.

Acceptance criteria:

- AC-006.1: GIVEN a sentinel value passed to `set` (as the value under each `--type`, including a `--type int` parse failure and a malformed `--type json` input), WHEN each command runs, THEN the sentinel appears in no stdout or stderr output.
- AC-006.2: GIVEN a config file containing a sentinel string as an open-schema profile field value, WHEN any write command fails (validation refusal, conflict, missing path), THEN the sentinel appears in no output.

Verification:

- Automated: sentinel leak checks folded into the SPEC-002/003/005 error-path tests.

## Edge Cases

| ID | Case | Expected behavior | Verification |
| --- | --- | --- | --- |
| EDGE-001 | `set`/`unset`/`credential add` with no config file present | Exit 2; message includes a hint to run `agentenv init` | Integration test |
| EDGE-002 | `set` with an empty or grammar-invalid path | Exit 1 per the existing segment grammar errors | Integration test |
| EDGE-003 | `set` writing an empty-string `description` | Exit 2: validator requires non-empty descriptions | Integration test |
| EDGE-004 | `--type json` with `null` anywhere in the value | Exit 1: TOML has no null | Unit test |
| EDGE-005 | Config path is a symlink (possibly a chain) | Resolution follows the full chain; the temporary file is created in the resolved target's directory and the resolved target is atomically replaced; the symlink itself is preserved | Integration test (Unix) |
| EDGE-006 | Two concurrent writers | Last writer wins; no locking; documented | README statement |
| EDGE-007 | `unset` the last field of a profile (`description`) | Exit 2: a profile table must keep a valid description; profile deletion is out of scope | Integration test |
| EDGE-008 | `set` into the reserved `inject` table with a non-string value | Exit 2 via validation | Integration test |
| EDGE-009 | `--type int` with a non-integer value | Exit 1; the diagnostic does not echo the value | Integration test |
| EDGE-010 | Mutation on a file whose permissions are broader than 0600 | The write preserves the existing bits; `validate` remains the permission gate, consistent with read commands | Integration test |
| EDGE-011 | Config directory not writable (temp file cannot be created) although the file itself is writable | Exit 2 naming the config path | Integration test |
| EDGE-012 | Read-only or full filesystem at write/rename time | Exit 2 naming the config path; original file intact | Code review (hard to simulate portably) |
| EDGE-013 | Config path is a dangling symlink | Write commands and `init` exit 2 naming the resolved, nonexistent target | Integration test (Unix) |

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
| AC-001.1..7 | SPEC-001 | Phase 1 | cargo test (integration) | Draft |
| AC-002.1..13 | SPEC-002 | Phase 1 | cargo test (integration) | Draft |
| AC-003.1..4 | SPEC-003 | Phase 1 | cargo test (integration) | Draft |
| AC-004.1..4 | SPEC-004 | Phase 2 | cargo test (integration) | Draft |
| AC-005.1..5 | SPEC-005 | Phase 3 | cargo test (integration) | Draft |
| AC-006.1..2 | SPEC-006 | Phases 1–3 | cargo test (sentinel checks) | Draft |

## Verification

- Automated: `cargo test` — pipeline unit tests (`config::write`) plus the four integration suites (`write_set`, `write_unset`, `write_init`, `write_credential_add`), including sentinel leak checks on every invocation via the shared harness.
- Static: `cargo clippy --all-targets`, `cargo fmt --check`.
- Code review: EDGE-012 (read-only/full filesystem) and the Windows rename-replace posture are verified by review, consistent with v1.

## Implementation Notes

- Exit statuses reuse the v1 mapping: 1 usage, 2 configuration/validation/write-I/O, 3 unknown profile/entry/path/name. No new statuses.
- User-facing messages follow the existing style: lowercase diagnostics naming the config path, with a concrete remedy command where one exists.
- The guardrail (AC-002.7) MUST be implemented by calling the same sensitive-name predicate `config::validate` uses, not by re-deriving the rule in the CLI layer.

## Design Notes (light tier)

- Format preservation: add `toml_edit = "0.25"` as a direct dependency for the mutation document (`DocumentMut`) — the 0.25 line is built on the same `toml_parser 1.x` core as `toml 1.x`, so one parser implementation is compiled and the mutation and validation halves cannot diverge. The existing `toml` crate remains the validation parser: the mutated document is serialized to a string and fed through the existing parse-and-validate pipeline (`config::validate`) before any disk write. Rejected: serializing via `toml::Table` (destroys comments and formatting of a hand-maintained file); migrating the whole read pipeline to `toml_edit` (larger blast radius, no behavioral gain).
- Module seam: one new module `src/config/write.rs` owns the pipeline — locate/read the document, apply a mutation operation, validate the result, atomically persist. CLI subcommands in `src/cli/commands.rs` are thin clients. Rejected: spreading write logic across per-command modules (duplicated atomic-write and validation plumbing).
- Path addressing reuses `path::Segments` verbatim. Write paths resolve against the profile's raw table, one level above `get`'s entry-rooted view; the deliberate asymmetry is that `description` fields (profile and entry) are writable by path while v1's `get` strips them from the entry view — they remain observable through `list --profiles --json`, `list`/`show` output, which is the verification surface for description ACs.
- Mutation mechanics: ancestor tables that were already implicit keep their implicit status after traversal (`set_implicit(true)` on touched ancestors), so no header materializes for a previously implicit table (AC-001.1) — a newly created intermediate table that holds direct key/value pairs naturally emits its own header; replacing a value carries over the previous item's decor so trailing comments survive (AC-001.6); new entries created via `--description` are emitted as standard `[profiles.<p>.<entry>]` tables; `--type json` objects are written as inline tables (their sub-keys are value-level content, matching how `get --json` reads them back).
- Atomic persist: create the temporary file in the (fully symlink-resolved) target's directory with mode 0600 first, then write content, then apply the target's pre-existing permission bits (or leave 0600 for a new file), fsync the file, rename over the resolved target, fsync the directory on Unix. The temp file never exists with bits wider than the final file. Windows rename-replace behavior is covered by the Windows integration suite and machine verification.
- Descriptions are ordinary fields: `--create-profile <text>` is the explicit profile-bootstrap flag (unknown profiles are never auto-created from a typo or an inherited env var); `--description` is sugar that writes `description` on the entry named by the first path segment, overwriting an existing one.
- Threat-model delta (reflected in the README safety section): v1 had no CLI path that mutates the TOML file; this change adds one. A caller able to invoke agentenv can now rewrite injection topology — for example pointing an entry's `credential` reference at a different defined credential, adding a `?as=` override, or editing `inject` tables — without touching a secret value. Mitigations: the file is validated on every write, secret values still never enter the TOML (guardrail + `credential set` remains the only value-storage path), all writes are visible in the file for review, and `run`'s injection-conflict check still refuses colliding targets. This is a deliberate, documented widening: the CLI's no-secret-leak invariant is unchanged, while the config file itself is now CLI-writable exactly like it was always hand-editable.

## Assumptions

- SPEC-AS-001: Pre-write validation delegates entirely to `config::validate::validate`; its rules are not restated here, and any result it accepts is writable.
- SPEC-AS-002: If the credential schema's required-ness drifts in the future, `credential add` flags follow the validator, with SPEC-005's flag list updated in the same change (today `inject_as` is unconditionally required and SPEC-005 states it directly).
- SPEC-AS-003: Only profiles, entries (first path segment), and credentials require descriptions; intermediate tables created along a deeper path require nothing (AC-002.10). Matches the three `require_description` call sites in `config/validate.rs`.
- SPEC-AS-004: Writing through a symlinked config path replaces the fully resolved target, preserving the symlink (EDGE-005), because `AGENTENV_FILE` users commonly symlink into dotfile repos.
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

Lanes: Claude provider lane only (native subagent, claude=opus, spec-review effort xhigh). The required Codex provider lane was skipped without substitution: the `codex` provider is switched off in the `route-policy.json` providers switchboard (explicit user override). Round recorded as single-provider with reduced coverage.

Findings (Claude lane; all Critical/Important revised in place):

- Critical: `set --json <value>` collided with the global `--json` output flag (clap duplicate-arg panic). → Folded value typing into `--type string|int|float|bool|json`; write commands reject the global `--json` (AC-001.7).
- Critical: AC-002.6 guardrail contradicted `config/validate.rs` (inject-table exclusion, string-only scope, `_`-suffix matching). → Restated as "same predicate, same scope" with negative ACs (AC-002.7/AC-002.8) and an implementation note requiring predicate reuse.
- Critical: unknown `--profile` silently created a profile, contradicting `get`'s exit-3 semantics. → Profile creation is now explicit opt-in `--create-profile <text>` requiring an explicit `--profile`; unknown profiles exit 3 from every selection source (AC-002.4/AC-002.5); `unset` analog added (AC-003.3).
- Important: SPEC-AS-003 asserted a nested-description rule the validator does not have. → Rewritten; AC-002.10 added.
- Important: SPEC-006 absolute no-echo contradicted the validator's SPEC-019 closed-schema-metadata carve-out. → Invariant scoped to open-schema and user-supplied values; carve-out re-affirmed; AC-006.2 restated.
- Important: `set description` writes a path `get` cannot read. → Asymmetry stated in SPEC-002 and Design Notes with `list`/`show` as the verification surface.
- Important: Goal vs AC-001.1 disagreed on trailing comments of the mutated line. → AC-001.6 added; decor carry-over required.
- Important: credential-reference ordering (`credential add` before `set`) and malformed-reference refusal unstated. → SPEC-002 prose + AC-002.11.
- Important: "threat model unchanged" was inaccurate. → Replaced with an explicit threat-model delta in Design Notes and the README scope.
- Important: permission-gate ambiguity in "same rules as `agentenv validate`". → SPEC-001 now names `config::validate::validate` and excludes the permission gate; AC-001.5 cross-references EDGE-010.
- Important: write-side I/O failures had no pinned exit status. → AC-001.4 pinned to exit 2; EDGE-011/012 added.
- Minor (all applied): `--inject-as` required-ness stated directly (SPEC-005); invalid credential name usage error (AC-005.5); implicit-table and emission-style design notes (AC-001.1); temp-file 0600-first ordering and umask-proof `init` bits (SPEC-004, Design Notes); `--description` overwrite pinned (AC-002.12); malformed JSON / `inf`/`nan` / datetime positions stated (SPEC-002, Out of Scope); `unset` inherits SPEC-002 failure modes; ownership clause dropped from AC-001.3; symlink chain/dangling cases specified (EDGE-005/013).
- Suggestion (both applied): `toml_edit` pinned to the major `toml` already pulls in; `init` success output names the next step (AC-004.1).

### Targeted re-check 1 — 2026-08-22

Lane: Claude provider (host lane; cross-provider re-check unavailable, `codex` off in the switchboard — single-provider, reduced coverage).

All 11 round-1 Critical/Important revisions verified as resolved against the cited code. Four new findings in revised sections, all applied:

- Important: the credential-reference paragraph over-claimed the validator's scan scope. → Qualified with the actual scope (excludes `description` keys, `inject`, array elements); AC-002.13 added as the negative case.
- Important: the re-enumerated no-echo carve-out was narrower than v1 SPEC-019 and contradicted AC-002.11. → Carve-out now incorporates SPEC-019 by reference with the complete list (including `credential://` reference strings, `service`/`account`, `argv[0]`); AC-002.11's echo-by-design stated; AC-006.1 sentinels constrained to non-reference-shaped values.
- Minor: `init`'s next-step hint wrote `description` twice with no precedence. → Hint changed to a non-overlapping example; path-wins precedence stated in AC-002.5.
- Minor: `set_implicit` design note over-claimed header suppression. → Narrowed to preserving already-implicit ancestors.

### Targeted re-check 2 — 2026-08-22

Lane: Claude provider (host lane; single-provider, reduced coverage — `codex` off in the switchboard).

Verdict: approve-with-minor. All four cycle-1 fixes resolved and code-verified. Four Minor findings, all applied in place: Goals bullet aligned with the SPEC-019 boundary; `toml_edit` version rationale corrected (shared `toml_parser 1.x` core, no transitive-pull claim); path-wins precedence extended to the `<entry>.description` + `--description` collision (AC-002.12); carve-out enumeration lists credential names explicitly.

Decision: Approved (single-provider round; Codex lane skipped per providers switchboard — reduced coverage recorded)

### Post-approval amendment — 2026-08-22

- AC-005.1's verification surface corrected during implementation review: the v1 `credential list --json` shape deliberately omits provider fields, so the config file is their verification surface. No behavioral requirement changed.
