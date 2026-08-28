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
- Trust-on-first-use lifecycle: `agentenv project allow`, `agentenv project revoke`, `agentenv project status`, content-hash invalidation, durable trust-store persistence.
- Trusted profile pin inserted into the selection precedence chain for every command that resolves a profile, including write commands.
- Requirement declarations (`[requires.<entry>]`) and a structural satisfied/unsatisfied report.
- Discovery bypass (`--no-project`, `AGENTENV_NO_PROJECT`).
- The no-secret invariant extended over the new untrusted input surface.
- Documentation and agent-skill updates, including the `.env`/docker-compose pairing guidance.

### Out of Scope

- Project-file content that defines values, credentials, `inject` tables, or credential references (PRD non-goal, D-01).
- Generating `.env` files or exporting values to a shell.
- Blocking `run` or read commands on unsatisfied requirements (deferred; ARCH-006).
- Merging multiple project files along the ancestor walk (nearest file wins).
- Credential resolution or shallow provider checks inside the requirement report.
- Cross-process locking of the trust store (concurrency is last-writer-wins per mutation; see SPEC-003).

## Phase Map

| Phase | Name | Priority | Objective | Depends on | Independent test |
| --- | --- | --- | --- | --- | --- |
| Phase 1 | Pin and trust | P1 (MVP) | Discover, validate, trust, and honor a profile pin; keep untrusted files inert; ship `project status` (text and frozen JSON envelope), `allow`, `revoke` | None | In a fresh tree, a pinned profile takes effect only after `allow` and stops after an edit or `revoke`; `project status --json` reports discovery/trust/pin; without a project file all commands behave as today |
| Phase 2 | Requirements report | P2 | Populate requirement checking inside the existing envelope | Phase 1 | With a trusted file declaring requirements, `project status` reports satisfied/unsatisfied correctly in text and JSON against configs that do and do not define them |
| Phase 3 | Docs and protocol | P3 | README, skill, and pairing documentation | Phase 1–2 | Documented commands and workflows execute as written against the built binary |

## Requirements

### SPEC-001: Project file discovery

The CLI MUST discover at most one project file per invocation: starting at the working directory and walking parent directories to the filesystem root, the nearest file named `.agentenv.toml` is the project file; no other file on the walk is consulted. Discovery runs for every invocation except `--help`, `--version`, and bypassed invocations.

Source trace:

- PRD: PRD-FR-001, PRD-FR-008
- Architecture: ARCH-001

Acceptance criteria:

- AC-001.1: GIVEN `.agentenv.toml` in a repository root, WHEN any command runs from a nested subdirectory of that repository, THEN that file is the discovered project file.
- AC-001.2: GIVEN an untrusted project file, WHEN the same read command runs once normally and once with `--no-project`, THEN stdout bytes and exit status are identical between the two runs and only the normal run carries the stderr notice.
- AC-001.3: GIVEN `.agentenv.toml` in both the working directory and an ancestor directory, WHEN any command runs, THEN only the working directory's file is discovered.
- AC-001.4: GIVEN `--no-project` on the command line or a non-empty `AGENTENV_NO_PROJECT` environment value, WHEN any command runs, THEN no discovery occurs: no project file is read, no notice is emitted, and no pin applies. An empty `AGENTENV_NO_PROJECT` counts as unset.

Verification:

- Automated: integration tests running the binary from temp directory trees (existing `assert_cmd` + `tempfile` style, with the harness isolation of SPEC-009).

### SPEC-002: Closed project-file schema

A project file MUST parse as TOML and contain only: `version = 1` (required), an optional non-empty string `profile`, and an optional `[requires]` table whose subtables `[requires.<entry>]` each carry a required non-empty string `reason` and an optional non-empty array `fields` of field-path strings. Each `fields` member MUST be a path in the accepted segment grammar (the grammar of `get` paths), interpreted relative to the declaring entry, exactly as `inject`-table field paths are interpreted; a duplicate member within one `fields` array is a violation. Any other key, table, value type, or version MUST be a validation violation that names the offending TOML path and never echoes the offending value. A string value shaped like a credential reference (`credential://` prefix) in any allowed string position (`profile`, `reason`, or a `fields` member) is a validation violation naming the path.

Source trace:

- PRD: PRD-FR-006
- Architecture: ARCH-001, `project::model`

Acceptance criteria:

- AC-002.1: GIVEN a project file containing any top-level key other than `version`, `profile`, or `requires` (for example `credentials`, `profiles`, or `inject`), WHEN it is validated (`project allow` or `project status`), THEN validation fails with a message naming the offending path, and the value is not echoed.
- AC-002.2: GIVEN `version` missing or not equal to `1`, WHEN validated, THEN validation fails naming `version`.
- AC-002.3: GIVEN `profile = ""` or a non-string `profile`, WHEN validated, THEN validation fails naming `profile`.
- AC-002.4: GIVEN `[requires.llm]` without `reason`, or with an empty `reason`, or with `fields = []`, or with a `fields` member that does not parse in the segment grammar, or with a duplicate `fields` member, WHEN validated, THEN validation fails naming the offending path.
- AC-002.5: GIVEN a syntactically valid file with `version = 1`, a `profile`, and well-formed `[requires.*]` tables, WHEN validated, THEN validation succeeds.
- AC-002.6: GIVEN a `credential://…` string as the value of `profile`, of a `reason`, or of a `fields` member, WHEN validated, THEN validation fails naming the path, and the string is not echoed.

Verification:

- Automated: integration tests with fixture project files per violation class.

### SPEC-003: Trust lifecycle and store durability

A project file MUST have no effect until the user approves its exact content. Approval state MUST be stored outside the repository in a user-owned store keyed by the file's canonical absolute path with a fingerprint of its exact bytes, so that any content change or path change returns the file to the untrusted state. `agentenv project allow` MUST validate then record approval; `agentenv project revoke` MUST remove it. Every store mutation MUST be atomic: the store is replaced via a temporary file (created with `0600` permissions on Unix before content is written) and rename, so an interrupted mutation leaves the previous store intact. Concurrent mutations serialize as last-writer-wins per whole-store mutation: a mutation MUST preserve every record present in the store snapshot it read, and a concurrent update committed after that snapshot was read may be overwritten (the loser re-runs its operation) — this trade-off is documented behavior.

Source trace:

- PRD: PRD-FR-004, PRD-FR-005
- Architecture: ARCH-002

Acceptance criteria:

- AC-003.1: GIVEN a newly created (never-approved) project file with a profile pin, WHEN any command resolves a profile, THEN the pin has no effect.
- AC-003.2: GIVEN an untrusted valid project file, WHEN `agentenv project allow` runs, THEN approval is recorded, the command reports the file path and what approval enables, and subsequent commands honor the pin.
- AC-003.3: GIVEN a trusted project file, WHEN its content changes in any way (including whitespace), THEN it is untrusted again until re-approved.
- AC-003.4: GIVEN a trusted project file, WHEN `agentenv project revoke` runs, THEN the approval is removed and the file is inert; a second `revoke` succeeds and reports that no approval existed.
- AC-003.5: GIVEN an invalid project file, WHEN `agentenv project allow` runs, THEN no approval is recorded, the violations are reported, and the exit status is 2.
- AC-003.6: GIVEN no discovered project file, WHEN `project allow` or `project revoke` runs, THEN the command fails with exit status 5 and a message stating no project file was found.
- AC-003.7: GIVEN a project file reached through a symlinked ancestor directory, WHEN it is approved and later read, THEN trust matches (identity is the canonical path).
- AC-003.8: GIVEN a trust store file that exists but cannot be parsed, WHEN any command consults it, THEN the command fails with exit status 2 and a message naming the store path — never silently treating the store as empty.
- AC-003.9: On Unix systems, WHEN the trust store file is created, THEN its permission bits are `0600`.
- AC-003.10: GIVEN two different project files approved in sequence, WHEN the second `allow` completes, THEN both approval records are present and the store parses.
- AC-003.11: GIVEN a store mutation whose final rename is made to fail (unit-level fault injection), WHEN the mutation aborts, THEN the previous store content is byte-intact and an explicit error names the store path.

Verification:

- Automated: integration tests covering the lifecycle in temp trees with an overridden state base (`XDG_STATE_HOME`/`HOME`/`LOCALAPPDATA`); unit tests for AC-003.11 and the atomic-replace path; Unix permission assertion gated to Unix.

### SPEC-004: Profile pin precedence

Profile selection MUST resolve, in order: `--profile` flag, `AGENTENV_PROFILE` (non-empty), the trusted project file's `profile` pin, then `default_profile`. The precedence applies uniformly to every command that resolves a profile — read commands, `run`, and write commands (`set`, `unset`, and `--create-profile` target resolution) alike. The pin travels with its origin (the project file path); for any command other than the `project` subcommands, a trusted pin naming an undefined profile MUST fail with exit status 3 and a message that names the project file and lists the defined profiles. An untrusted file's pin MUST NOT participate at any position.

Source trace:

- PRD: PRD-FR-002
- Architecture: ARCH-005

Acceptance criteria:

- AC-004.1: GIVEN a trusted pin `work` and no flag or environment selection, WHEN a read command runs, THEN profile `work` is active.
- AC-004.2: GIVEN a trusted pin `work` and `AGENTENV_PROFILE=personal`, WHEN a read command runs, THEN profile `personal` is active.
- AC-004.3: GIVEN a trusted pin `work` and `--profile personal`, WHEN a read command runs, THEN profile `personal` is active.
- AC-004.4: GIVEN a trusted pin naming an undefined profile, WHEN any command other than a `project` subcommand resolves a profile, THEN it fails with exit status 3 and the message names the project file path.
- AC-004.5: GIVEN a trusted pin and a config file with a different `default_profile`, WHEN a read command runs with no flag or environment selection, THEN the pin wins over `default_profile`.
- AC-004.6: GIVEN an untrusted file pinning `work` and a config `default_profile = "personal"`, WHEN a read command runs, THEN profile `personal` is active.
- AC-004.7: GIVEN a trusted pin, WHEN `agentenv run --with <entry> -- <target>` runs, THEN injection planning uses the pinned profile (probe target observes the pinned profile's values).
- AC-004.8: GIVEN a trusted pin `work`, a config `default_profile = "personal"`, and no flag or environment selection, WHEN `agentenv set llm.model x` runs, THEN the value is written under `profiles.work`.

Verification:

- Automated: integration tests including a `run` test via the existing `test-probe` binary and a write-path test.

### SPEC-005: Inertness, notice, and command scope

While a discovered project file is untrusted, every command outside the `project` subcommand group MUST behave exactly as if the file were absent — identical stdout bytes and exit status — except that one single-line notice MUST be written to stderr naming the file path and referring to `agentenv project status`.

Command scope and evaluation order:

- Discovery runs for every invocation except `--help`, `--version`, and bypassed invocations (AC-001.4).
- The `project` subcommands consume the file directly and never emit the notice; their output is the report itself.
- Every other command (including `init`, `validate`, the write commands, and `run`) emits the notice when an untrusted file is discovered — on success and on failure alike, and for `run` before the target process replaces or starts.
- Evaluation order per invocation: (1) discovery; (2) trust resolution, ordered as canonical-path resolution → path-only approval-record lookup → content read → fingerprint comparison → parse — a corrupt trust store fails with exit 2 (AC-003.8); for commands outside the `project` group an unresolvable state base (required environment variables unset) degrades the file to untrusted with the notice naming the unresolvable state location, while `project status` represents the same state in its report (`trust` = `unavailable`, AC-006.11) and `project allow`/`revoke` fail explicitly (exit 2, EDGE-004b); (3) trusted-file content read — a file with an approval record for its canonical path that cannot be read fails with exit status 2 naming the file; a file without an approval record that cannot be read or parsed is untrusted; (4) user-config load and the command proper, unchanged.
- The notice MUST never appear on stdout and MUST never alter any stdout payload, JSON or text.

Source trace:

- PRD: PRD-FR-004, PRD-FR-007
- Architecture: ARCH-004

Acceptance criteria:

- AC-005.1: GIVEN an untrusted project file, WHEN `agentenv list --json` runs, THEN stdout is byte-identical to the same invocation without the file, the exit status is unchanged, and stderr contains exactly one notice line naming the file.
- AC-005.2: GIVEN an untrusted and unparseable project file, WHEN any read command runs, THEN the command succeeds as if no file existed, with the single stderr notice.
- AC-005.3: GIVEN a trusted project file that is then made unreadable while its approval record remains, WHEN a command that resolves a profile runs, THEN it fails with exit status 2 and a message naming the file.
- AC-005.4: GIVEN `--no-project` or non-empty `AGENTENV_NO_PROJECT` with an untrusted file present, WHEN any command runs, THEN no notice is emitted.
- AC-005.5: GIVEN an untrusted project file and a command that fails for an unrelated reason (for example `agentenv get` on an unknown path), WHEN the command runs, THEN the notice still appears on stderr alongside the error and the exit status is the unrelated failure's status.
- AC-005.6: GIVEN an untrusted project file, WHEN `agentenv run --with <entry> -- <target>` launches successfully, THEN the notice appears on stderr before the target's own output.
- AC-005.7: GIVEN an untrusted project file and an environment with no resolvable state base (relevant variables unset), WHEN a read command runs, THEN the command succeeds as if the file were absent and the notice names the unresolvable state location.

Verification:

- Automated: integration tests asserting stdout snapshots, stderr content, and exit codes.

### SPEC-006: `agentenv project status`

`agentenv project status` MUST always produce its report except on the two infrastructure failures that exit 2 (corrupt trust store; unreadable file with an approval record) — it never fails because the user configuration is missing, invalid, or has no selectable profile, never fails because the pin names an undefined profile, and represents an unresolvable state base inside the report (`trust` = `unavailable`) rather than failing. It reports, in text and `--json`: whether a project file was discovered (and its path), its trust state (`trusted`, `untrusted-new`, `untrusted-changed`, `invalid` with violations, or `unavailable` with a reason when the trust store's state base cannot be resolved), its profile pin, and the requirement report. When requirements cannot be checked (no user config, unparseable user config, no selectable profile, or a pin naming an undefined profile), the requirement section MUST state that requirements were not checked and name the reason.

Exit status (exhaustive matrix; the first matching row applies):

| Condition | Status |
| --- | --- |
| Corrupt trust store (AC-003.8), or a discovered file with an approval record that cannot be read (SPEC-005 order step 3) | 2 |
| No project file discovered | 0 |
| Discovered file untrusted (new or changed), invalid, or trust `unavailable` (state base unresolvable) | 5 |
| Trusted; zero requirements declared (regardless of whether selection is degraded) | 0 |
| Trusted; ≥1 declared requirement unsatisfied, or requirements declared but uncheckable | 6 |
| Trusted; all declared requirements checked and satisfied | 0 |

The `--json` output is a single JSON document with this envelope, frozen from Phase 1 and extended only additively:

```json
{
  "version": 1,
  "project": {
    "discovered": true,
    "path": "/abs/path/.agentenv.toml",
    "trust": "trusted",
    "trust_reason": null,
    "violations": [ { "path": "requires.llm.reason", "message": "…" } ],
    "profile_pin": "work",
    "requirements": {
      "checked": true,
      "reason": null,
      "profile": "work",
      "entries": [
        { "entry": "llm", "reason": "…", "satisfied": true, "missing": [] }
      ]
    }
  }
}
```

Member semantics: `version` is the user-config schema version, `null` when the user config is unavailable; `path` and `trust` are `null` exactly when `discovered` is `false`; `profile_pin` is the file's pin whenever the file parses (`trusted`, `untrusted-new`, `untrusted-changed`) and `null` when it declares no pin, does not parse (`invalid`), or `trust` is `unavailable`; `trust` is one of `trusted`, `untrusted-new`, `untrusted-changed`, `invalid`, `unavailable`; `trust_reason` is a non-null string exactly when `trust` is `unavailable` (naming the unresolvable state location) and `null` otherwise; `violations` is an empty array except in the `invalid` state; `requirements.checked` is `false` with a non-null `reason` when checking did not run; `entries` lists every declared requirement with `missing` naming absent entries or field paths (`entries` is empty when `trust` is not `trusted` because an untrusted file's declarations are not consumed); members are never omitted.

Source trace:

- PRD: PRD-FR-003, PRD-FR-005, PRD-NFR-002, PRD-NFR-003
- Architecture: ARCH-003, ARCH-006

Acceptance criteria:

- AC-006.1: GIVEN no project file, WHEN `project status` runs, THEN it reports that no file was discovered and exits 0.
- AC-006.2: GIVEN an untrusted (new or changed) file, WHEN `project status` runs, THEN it reports the trust state and the approval command to run, and exits 5.
- AC-006.3: GIVEN an invalid file, WHEN `project status` runs, THEN it reports each violation by TOML path (values not echoed) and exits 5.
- AC-006.4: GIVEN a trusted file whose declared requirements are all satisfied, WHEN `project status` runs, THEN it reports every requirement as satisfied with its reason and exits 0.
- AC-006.5: GIVEN a trusted file with an unsatisfied requirement, WHEN `project status` runs, THEN that requirement is reported unsatisfied with what is missing, and the exit status is 6.
- AC-006.6a (Phase 1): GIVEN each of the states no-file, untrusted-new, untrusted-changed, invalid, and trusted, WHEN `project status --json` runs, THEN stdout is a single JSON document matching the frozen envelope (with `requirements.checked = false` and a stated reason while checking is not yet implemented), and no notice line is mixed into stdout.
- AC-006.6b (Phase 2): GIVEN a trusted file with declared requirements, WHEN `project status --json` runs, THEN the same envelope carries the populated requirement entries; no member is renamed, removed, or re-typed relative to Phase 1.
- AC-006.7: GIVEN a discovered trusted file and no user config file, WHEN `project status` runs, THEN it reports discovery, trust, and pin; the requirement section states requirements were not checked because the user config is unavailable; `version` is `null` in JSON; and the exit status is 6 when requirements are declared, else 0.
- AC-006.8: GIVEN a trusted pin naming an undefined profile, WHEN `project status` runs, THEN the report states the pin and that requirements were not checked because the pinned profile is not defined, and the exit status is 6 when requirements are declared, else 0.
- AC-006.9: GIVEN a discovered trusted file and an unparseable user config file, WHEN `project status` runs, THEN the report is produced, the requirement section names the user config as the reason checking did not run, `version` is `null` in JSON, and the exit status is 6 when requirements are declared, else 0.
- AC-006.10: GIVEN a trusted file declaring requirements but no pin, a user config with no `default_profile`, and no flag or environment selection, WHEN `project status` runs, THEN the requirement section states no profile was selectable and the exit status is 6.
- AC-006.11: GIVEN a discovered project file and no resolvable state base (relevant environment variables unset), WHEN `project status` runs, THEN the report carries `trust` = `unavailable` with `trust_reason` naming the unresolvable state location, no notice is emitted, and the exit status is 5.
- AC-006.12: GIVEN a corrupt trust store, WHEN `project status` runs, THEN it fails with exit status 2 naming the store path (AC-003.8), producing no report.

Verification:

- Automated: integration tests plus JSON snapshots under `tests/snapshots/` for every state in AC-006.6a.

### SPEC-007: Requirement declarations and structural checking

A trusted project file's `[requires.<entry>]` declarations MUST be checked structurally against the active profile: a requirement is satisfied when the named entry exists in the active profile and, when `fields` is declared, every listed entry-relative field path resolves inside that entry (the same resolution the `inject` table uses for its field paths). Checking MUST NOT resolve credentials, execute provider commands, or read secret stores. Requirement checking MUST NOT block or alter any command other than `project status`.

Source trace:

- PRD: PRD-FR-003
- Architecture: ARCH-006

Acceptance criteria:

- AC-007.1: GIVEN a requirement naming an entry present in the active profile with all declared fields resolvable, WHEN `project status` runs, THEN the requirement is satisfied.
- AC-007.2: GIVEN a requirement naming an entry absent from the active profile, WHEN `project status` runs, THEN the requirement is unsatisfied, naming the missing entry.
- AC-007.3: GIVEN a requirement whose `fields` lists a path that does not resolve in the entry, WHEN `project status` runs, THEN the requirement is unsatisfied, naming the missing field path.
- AC-007.4: GIVEN requirements against a profile whose entries reference credentials, WHEN `project status` runs, THEN no provider command is executed and no secret store is read while the report is produced.
- AC-007.5: GIVEN a trusted file with unsatisfied requirements, WHEN `agentenv get`, `list`, or `run` executes, THEN those commands behave exactly as without the requirements (no warning, no failure attributable to requirements).
- AC-007.6: GIVEN a requirement with `fields = ["auth.endpoint"]` where the entry contains a nested table `auth` with key `endpoint`, WHEN `project status` runs, THEN the requirement is satisfied; and GIVEN the nested key is absent, THEN it is unsatisfied naming `auth.endpoint`.

Verification:

- Automated: integration tests; AC-007.4 verified with `tests/fixtures/counting_provider.sh` (execution count unchanged).

### SPEC-008: Documentation and agent protocol

The README, the agent skill (`skills/agentenv/SKILL.md`), and the README's `AGENTS.md` block MUST document: the project file schema and discovery, the trust lifecycle, the extended exit-status table (statuses 5 and 6 added; statuses 0–4 and 127 unchanged in meaning, with status 2 explicitly noted as also covering project-file validation errors), the precedence chain including the pin and its uniform application to read, `run`, and write commands, the bypass flag/variable, and the pairing guidance — non-secret values may live in `.env`; credentials reach tools only through `agentenv run` (worked docker compose example using variable passthrough or `${VAR}` interpolation); `env_file:` with secrets is documented as the anti-pattern this replaces. The skill MUST add a project-discovery step (`agentenv project status --json`) to its reading protocol.

Source trace:

- PRD: PRD-FR-008 (bypass documentation), UX and Interaction Notes; PRD-SM-001
- Architecture: ARCH-003, ARCH-004

Acceptance criteria:

- AC-008.1: GIVEN the updated README, WHEN each documented `project` command and the compose pairing example are executed against the built binary in a scratch tree, THEN they behave as documented.
- AC-008.2: GIVEN the updated exit-status table, WHEN compared with implemented behavior, THEN statuses 0–6 and 127 match, and no pre-existing status changed meaning.
- AC-008.3: GIVEN the updated skill document, WHEN its reading protocol is followed in order in a trusted project, THEN the first steps discover the project state before profile-dependent reads.

Verification:

- Automated: none (prose); Manual: execute each documented command sequence once on macOS or Linux.

### SPEC-009: Backward compatibility and hermetic verification

In the absence of a discovered project file, every existing command MUST behave byte-identically to the pre-change release: same stdout, same stderr, same exit statuses. To make this verifiable on any machine, the shared integration-test harness MUST pin each invocation's working directory to a directory the test controls and MUST set `AGENTENV_NO_PROJECT` for all tests that do not exercise project behavior; project-behavior tests construct their own temp trees. Adapting the shared harness this way is a permitted mechanical change; test assertions themselves remain unmodified.

Source trace:

- PRD: PRD-NFR-004
- Architecture: Risks table (behavior drift)

Acceptance criteria:

- AC-009.1: GIVEN the full pre-existing test suite with only the mechanical harness isolation applied (working-directory pinning and `AGENTENV_NO_PROJECT`), WHEN `cargo test` runs on the completed change, THEN every pre-existing test assertion passes unmodified.
- AC-009.2: GIVEN the JSON snapshots under `tests/snapshots/`, WHEN snapshot tests run, THEN all pre-existing snapshots are byte-identical.

Verification:

- Automated: `cargo test` on Linux, macOS, and Windows via existing CI.

### SPEC-010: No-secret invariant under a project file

A discovered project file — in any trust state — MUST NOT cause a credential value to be printed, persisted, or rerouted: discovery, validation, trust operations, and `project status` MUST NOT resolve credentials, execute provider commands, or read secret stores; and the only way a project file changes which credentials `run` injects, or under which environment names, is by selecting a different profile through a trusted pin (SPEC-004).

Output discipline: every *diagnostic* introduced by this change — validation violations, error messages, and the untrusted-file notice — names TOML paths and file paths only and never echoes project-file string values, user-config values, or TOML source lines, consistent with the accepted no-echo rules. The `project status` *report* is the deliberate, bounded exception: it exposes exactly the schema-declared selection/declaration metadata — the file path, trust state and reason, violation paths, the profile pin, requirement entry names, field paths, and requirement reasons — and nothing else. No other command surface exposes project-file string values.

Source trace:

- PRD: PRD-NFR-001
- Architecture: ARCH-002, ARCH-004, ARCH-006

Acceptance criteria:

- AC-010.1: GIVEN an invalid project file whose forbidden or malformed positions carry a distinctive sentinel string, WHEN `project allow` and `project status` report its violations and WHEN any other command emits the notice, THEN every message names paths only and the sentinel never appears in any stdout or stderr output.
- AC-010.2: GIVEN an untrusted but schema-valid project file whose `profile` and `reason` values carry a sentinel string, WHEN any command outside the `project` group runs, THEN the sentinel never appears in that command's stdout or stderr (the notice names the file path only).
- AC-010.3: GIVEN a trusted valid project file, WHEN `project status` runs, THEN its report contains the pin and requirement reasons (the intended exposure) and contains no user-config values beyond profile and entry names.
- AC-010.4: GIVEN a profile whose entries reference a counting command provider, WHEN `project status`, `project allow`, and `project revoke` run, THEN the provider execution count is unchanged.
- AC-010.5: GIVEN a trusted pin selecting a profile, WHEN `run` injects credentials, THEN the injected names and sources are exactly those of the selected profile's entries as defined in the user config — the project file contributes no injection target and no credential selection beyond the profile choice.

Verification:

- Automated: integration tests using sentinel-laden project file fixtures and `tests/fixtures/counting_provider.sh`; AC-010.5 via the `test-probe` target.

## Edge Cases

| ID | Case | Expected behavior | Verification |
| --- | --- | --- | --- |
| EDGE-001 | `.agentenv.toml` in `$HOME`, command run in an unrelated tree under `$HOME` | Discovered (walk reaches it); trust gate applies as usual | Integration test |
| EDGE-002 | Empty project file (zero bytes) | Invalid: `version` missing (AC-002.2); inert + notice until fixed | Integration test |
| EDGE-003 | `AGENTENV_NO_PROJECT=""` (empty) | Counts as unset; discovery proceeds | Integration test |
| EDGE-004a | State base unset (no `XDG_STATE_HOME`/`HOME` on Unix, no `LOCALAPPDATA` on Windows) on a read path with a discovered file | File degrades to untrusted; command proceeds; notice names the unresolvable state location (AC-005.7) | Integration test, injected env |
| EDGE-004b | State base unset when `project allow` or `project revoke` must write | Explicit configuration error naming the variables, exit 2 | Unit + integration test |
| EDGE-005 | `project allow` run twice on the same content | Second run succeeds and reports approval already current | Integration test |
| EDGE-006 | Project file deleted after approval | No file discovered; commands behave as with no project file; stale record is harmless | Integration test |
| EDGE-007 | Pin equals the profile already selected by `default_profile` | Identical outcome; no special casing observable | Integration test |
| EDGE-008 | `--profile` with empty value while a trusted pin exists | Existing usage error (unchanged) | Existing test suite |
| EDGE-009 | Requirement entry name containing punctuation (quoted TOML key) | Validated as an entry name by the accepted segment grammar; checked against the profile | Integration test |
| EDGE-010 | stderr notice with `--json` commands piped to a JSON parser | stdout parses as JSON; notice only on stderr | Integration test (AC-005.1, AC-006.6a) |

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
| SPEC-010 | SPEC-002..006 | The invariant spans every new surface |

## Acceptance Matrix

| Acceptance ID | Requirement | Phase | Verification method | Status |
| --- | --- | --- | --- | --- |
| AC-001.1..4 | SPEC-001 | Phase 1 | `cargo test` integration | Draft |
| AC-002.1..6 | SPEC-002 | Phase 1 | `cargo test` integration | Draft |
| AC-003.1..11 | SPEC-003 | Phase 1 | `cargo test` integration/unit | Draft |
| AC-004.1..8 | SPEC-004 | Phase 1 | `cargo test` integration | Draft |
| AC-005.1..7 | SPEC-005 | Phase 1 | `cargo test` integration | Draft |
| AC-006.1..3, 6.6a, 6.7..12 | SPEC-006 | Phase 1 | `cargo test` integration + snapshots | Draft |
| AC-006.4, 6.5, 6.6b | SPEC-006 | Phase 2 | `cargo test` integration + snapshot | Draft |
| AC-007.1..6 | SPEC-007 | Phase 2 | `cargo test` integration | Draft |
| AC-008.1..3 | SPEC-008 | Phase 3 | Manual doc walkthrough | Draft |
| AC-009.1..2 | SPEC-009 | all | `cargo test` full suite | Draft |
| AC-010.1..5 | SPEC-010 | Phase 1 | `cargo test` integration | Draft |

## Implementation Notes

- The trust fingerprint and store format are internal; only the behaviors above are contractual. The store location is an architecture decision (ARCH-002), documented for support but not a stability contract.
- Tests control the trust store location by overriding the state base environment (`XDG_STATE_HOME`/`HOME` on Unix, `LOCALAPPDATA` on Windows); no dedicated override variable exists.
- Phase 1 ships the complete `project status` surface including the frozen JSON envelope; Phase 2 only populates requirement checking inside it.
- The pin is carried with its origin (project file path) so selection errors can name the file (ARCH-005); command outcomes carry stdout, stderr, and exit status explicitly so `project status` can emit a full report with a non-zero status (architecture: `cli::project`).

## Assumptions

- SPEC-AS-001: The project file name is exactly `.agentenv.toml` (hidden file, matching `.envrc`/`.mise.toml` convention) because the name should read as tool configuration, not project data.
- SPEC-AS-002: Discovery includes the working directory itself as the first candidate because that is the least surprising reading of "nearest".
- SPEC-AS-003: The stderr notice's exact wording is not contractual; its properties are: exactly one line, names the file path, names `agentenv project status`.
- SPEC-AS-004: Exit statuses 5 (project trust-state failure: untrusted/invalid at `status`, or `allow`/`revoke` with no discovered file) and 6 (requirements unsatisfied or uncheckable) are free because 0–4 and 127 are the only statuses documented today; status 2 keeps its documented meaning (configuration-file error) and now also covers project-file validation errors, which are configuration-file errors.
- SPEC-AS-005: A stale trust record for a deleted or moved file is retained harmlessly (no garbage collection in v1) because records are tiny and pruning adds state-mutation paths for no behavioral gain.
- SPEC-AS-006: `project allow`/`revoke`/`status` ignore `--profile` for trust decisions (trust is per-file, not per-profile); `status` computes the requirement report against normal selection and degrades per SPEC-006 when selection cannot complete.
- SPEC-AS-007: The startup-latency half of PRD-NFR-003 is not gated by an automated test (a portable, non-flaky CI latency bound does not exist for a sub-millisecond-order walk). It is discharged at validation by a recorded manual cold-start comparison (`agentenv list` timed in a depth-20 tree with and without discovery) documented in `validation.md`. The design constraint stands: discovery adds at most one file-existence probe per ancestor plus one small-file read and hash.

## Clarifications

### Session 2026-08-28

- Q: What may the project file contain? -> A: Selection-only — profile pin plus `[requires]` declarations (applied to Scope, SPEC-002).
- Q: Trust model? -> A: Trust-on-first-use with content-hash invalidation, user-owned state (applied to SPEC-003).
- Q: Pin position in precedence? -> A: Below `AGENTENV_PROFILE`, above `default_profile` (applied to SPEC-004).

## Open Questions

| ID | Question | Blocking? | Resolution |
| --- | --- | --- | --- |
| — | none | — | — |
