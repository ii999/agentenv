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
- Trusted profile pin inserted into the selection precedence chain for every command that resolves a profile through the standard chain, including `set` and `unset`.
- Requirement declarations (`[requires.<entry>]`) and a structural satisfied/unsatisfied report.
- Discovery bypass (`--no-project`, `AGENTENV_NO_PROJECT`) for commands outside the `project` group.
- The no-secret invariant extended over the new untrusted input surface.
- Documentation and agent-skill updates, including the `.env`/docker-compose pairing guidance.

### Out of Scope

- Project-file content that defines values, credentials, `inject` tables, or credential references (PRD non-goal, D-01).
- Generating `.env` files or exporting values to a shell.
- Blocking `run` or read commands on unsatisfied requirements (deferred; ARCH-006).
- Merging multiple project files along the ancestor walk (nearest file wins).
- Credential resolution or shallow provider checks inside the requirement report.
- Cross-process locking of the trust store (concurrency is last-writer-wins per mutation; see SPEC-003).
- Changing `--create-profile` semantics: it continues to require an explicit `--profile` and never consults the pin (accepted change-002 contract).

## Phase Map

| Phase | Name | Priority | Objective | Depends on | Independent test |
| --- | --- | --- | --- | --- | --- |
| Phase 1 | Pin, trust, and status | P1 (MVP) | Discover, validate, trust, and honor a profile pin; keep untrusted files inert; ship `project status` (text and frozen JSON envelope, requirement checking included), `allow`, `revoke` | None | In a fresh tree, a pinned profile takes effect only after `allow` and stops after an edit or `revoke`; `project status --json` reports discovery/trust/pin/requirements with exit 0 on a satisfied setup; without a project file all commands behave as today |
| Phase 2 | Docs and protocol | P2 | README, skill, and pairing documentation | Phase 1 | Documented commands and workflows execute as written against the built binary |

## Requirements

### SPEC-001: Project file discovery

The CLI MUST discover at most one project file per invocation: starting at the working directory and walking parent directories to the filesystem root, the nearest regular file named `.agentenv.toml` is the project file; no other file on the walk is consulted. A path entry that is not a regular file (a directory, a dangling symlink) is not a project file and the walk continues past it. Discovery itself never fails a command. Discovery runs for every invocation that passes command-line parsing, except `--help`, `--version`, and bypassed invocations; the bypass (`--no-project`, non-empty `AGENTENV_NO_PROJECT`) applies to every command *outside* the `project` subcommand group — the `project` subcommands always perform discovery, because inspecting and managing the file is their purpose.

Source trace:

- PRD: PRD-FR-001, PRD-FR-008
- Architecture: ARCH-001

Acceptance criteria:

- AC-001.1: GIVEN `.agentenv.toml` in a repository root, WHEN any command runs from a nested subdirectory of that repository, THEN that file is the discovered project file.
- AC-001.2: GIVEN a directory or dangling symlink named `.agentenv.toml` in the working directory and a regular `.agentenv.toml` in an ancestor, WHEN any command runs, THEN the ancestor's file is the discovered project file.
- AC-001.3: GIVEN `.agentenv.toml` in both the working directory and an ancestor directory, WHEN any command runs, THEN only the working directory's file is discovered.
- AC-001.4: GIVEN `--no-project` on the command line or a non-empty `AGENTENV_NO_PROJECT` environment value, WHEN any command outside the `project` group runs, THEN no discovery occurs: no project file is read, no notice is emitted, and no pin applies. An empty `AGENTENV_NO_PROJECT` counts as unset.
- AC-001.5: GIVEN `--no-project` or non-empty `AGENTENV_NO_PROJECT` in a tree with a project file, WHEN `project status`, `project allow`, or `project revoke` runs, THEN discovery still occurs and the command operates on the discovered file.

Verification:

- Automated: integration tests running the binary from temp directory trees (existing `assert_cmd` + `tempfile` style, with the harness isolation of SPEC-009).

### SPEC-002: Closed project-file schema

A project file MUST parse as TOML and contain only: `version = 1` (required), an optional non-empty string `profile`, and an optional `[requires]` table whose subtables `[requires.<entry>]` each carry a required non-empty string `reason` and an optional non-empty array `fields` of field-path strings. Each `[requires.<entry>]` key MUST be a single segment in the accepted segment grammar (an empty or unaddressable key is a violation naming `requires.<key>`). Each `fields` member MUST parse in the accepted segment grammar (the grammar of `get` paths) and is interpreted relative to the declaring entry; a duplicate member within one `fields` array is a violation. A project file larger than 64 KiB is invalid, with one violation naming the file and the size limit. Any other key, table, value type, or version MUST be a validation violation that names the offending TOML path and never echoes the offending value. A string value shaped like a credential reference (`credential://` prefix) in any allowed string position (`profile`, `reason`, or a `fields` member) is a validation violation naming the path.

Source trace:

- PRD: PRD-FR-006
- Architecture: ARCH-001, `project::model`

Acceptance criteria:

- AC-002.1: GIVEN a project file containing any top-level key other than `version`, `profile`, or `requires` (for example `credentials`, `profiles`, or `inject`), WHEN it is validated (`project allow` or `project status`), THEN validation fails with a message naming the offending path, and the value is not echoed.
- AC-002.2: GIVEN `version` missing or not equal to `1`, WHEN validated, THEN validation fails naming `version`.
- AC-002.3: GIVEN `profile = ""` or a non-string `profile`, WHEN validated, THEN validation fails naming `profile`.
- AC-002.4: GIVEN `[requires.llm]` without `reason`, or with an empty `reason`, or with `fields = []`, or with a `fields` member that does not parse in the segment grammar, or with a duplicate `fields` member, or a `[requires.""]` (empty or unaddressable entry key), WHEN validated, THEN validation fails naming the offending path.
- AC-002.5: GIVEN a syntactically valid file with `version = 1`, a `profile`, and well-formed `[requires.*]` tables, WHEN validated, THEN validation succeeds.
- AC-002.6: GIVEN a `credential://…` string as the value of `profile`, of a `reason`, or of a `fields` member, WHEN validated, THEN validation fails naming the path, and the string is not echoed.
- AC-002.7: GIVEN a project file larger than 64 KiB, WHEN validated, THEN it is invalid with a violation naming the file and the limit.

Verification:

- Automated: integration tests with fixture project files per violation class.

### SPEC-003: Trust lifecycle and store durability

A project file MUST have no effect until the user approves its exact content. Approval state MUST be stored outside the repository in a user-owned store keyed by the file's canonical absolute path with a fingerprint of its exact bytes, so that any content change or path change returns the file to the untrusted state. `agentenv project allow` MUST validate then record approval, and MUST bind the approval to the exact bytes it validated: `allow` performs one content read, and the bytes it validates are the bytes it approves — a file modified concurrently with `allow` is approved as the validated snapshot, so the on-disk content resolves as untrusted afterward. `agentenv project revoke` MUST remove the approval by canonical path alone — it performs no content read or validation, so a changed, invalid, or unreadable file can always be revoked. Every store mutation MUST be atomic: the store is replaced via a temporary file (created with `0600` permissions on Unix before content is written) and rename, so an interrupted mutation leaves the previous store intact. Concurrent mutations serialize as last-writer-wins per whole-store mutation: a mutation MUST preserve every record present in the store snapshot it read, and a concurrent update committed after that snapshot was read may be overwritten (the loser re-runs its operation) — this trade-off is documented behavior. The trust store's permission bits are constrained at creation only; they are not checked on read (SPEC-AS-008).

Source trace:

- PRD: PRD-FR-004, PRD-FR-005
- Architecture: ARCH-002

Acceptance criteria:

- AC-003.1: GIVEN a newly created (never-approved) project file with a profile pin, WHEN any command resolves a profile, THEN the pin has no effect.
- AC-003.2: GIVEN an untrusted valid project file, WHEN `agentenv project allow` runs, THEN approval is recorded, the command reports the file path and what approval enables, and subsequent commands honor the pin.
- AC-003.3: GIVEN a trusted project file, WHEN its content changes in any way (including whitespace), THEN it is untrusted again until re-approved.
- AC-003.4: GIVEN a trusted project file, WHEN `agentenv project revoke` runs, THEN the approval is removed and the file is inert; a second `revoke` succeeds and reports that no approval existed.
- AC-003.5: GIVEN an invalid project file, WHEN `agentenv project allow` runs, THEN no approval is recorded, the violations are reported with the remedy (fix the named paths, then re-run `agentenv project allow`), and the exit status is 2.
- AC-003.6: GIVEN no discovered project file, WHEN `project allow` or `project revoke` runs, THEN the command fails with exit status 5 and a message stating no project file was found and naming the expected file name and search scope (working directory and ancestors).
- AC-003.7: GIVEN a project file reached through a symlinked ancestor directory, WHEN it is approved and later read, THEN trust matches (identity is the canonical path).
- AC-003.8: GIVEN a trust store file that exists but cannot be parsed, WHEN any command consults it, THEN the command fails with exit status 2 and a message naming the store path and the remedy (repair or delete the store, then re-approve projects) — never silently treating the store as empty.
- AC-003.9: On Unix systems, WHEN the trust store file is created, THEN its permission bits are `0600`.
- AC-003.10: GIVEN two different project files approved in sequence, WHEN the second `allow` completes, THEN both approval records are present and the store parses; and WHEN one is revoked, THEN the other record is preserved.
- AC-003.11: WHEN a store mutation fails to commit, THEN the previous store content is byte-intact and an explicit error names the store path.
- AC-003.12: GIVEN the project file is replaced with different content after `allow`'s single content read, WHEN `allow` completes and a subsequent command resolves the file, THEN the on-disk file resolves as untrusted.
- AC-003.13: GIVEN a trusted project file that is edited into an invalid file, WHEN `agentenv project revoke` runs, THEN the stale approval is removed successfully.

Verification:

- Automated: integration tests covering the lifecycle in temp trees with an overridden state base (`XDG_STATE_HOME`/`HOME`/`LOCALAPPDATA`); AC-003.11 and AC-003.12 via unit-level fault injection through the trust module's filesystem seam (architecture, `project::trust`); Unix permission assertion gated to Unix.

### SPEC-004: Profile pin precedence

Profile selection MUST resolve, in order: `--profile` flag, `AGENTENV_PROFILE` (non-empty), the trusted project file's `profile` pin, then `default_profile`. The precedence applies uniformly to every command that resolves a profile through the standard chain — read commands, `run`, `set`, and `unset` alike. `--create-profile` keeps its accepted contract: it requires an explicit `--profile` and never consults the pin. The pin travels with its origin (the project file path); for any command other than the `project` subcommands, a trusted pin naming an undefined profile MUST fail with exit status 3 and a message that names the project file and lists the defined profiles. An untrusted file's pin MUST NOT participate at any position.

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
- AC-004.8: GIVEN a trusted pin `work`, a config `default_profile = "personal"`, and no flag or environment selection, WHEN `agentenv set llm.model x` runs, THEN the value is written under `profiles.work`; and WHEN `agentenv set --create-profile "d" x.y z` runs without `--profile`, THEN the existing usage error is unchanged (the pin is not consulted).

Verification:

- Automated: integration tests including a `run` test via the existing `test-probe` binary and write-path tests.

### SPEC-005: Inertness, notice, and command scope

While a discovered project file is untrusted, every command outside the `project` subcommand group MUST behave exactly as if the file were absent — identical stdout bytes and exit status — except that one single-line notice MUST be written to stderr naming the file path and referring to `agentenv project status`. Every diagnostic introduced by this change follows the accepted next-action contract (change-001 SPEC-018): it names the failing thing and a next action.

Command scope and evaluation order:

- Discovery and the notice happen only after command-line parsing succeeds: invocations that fail CLI parsing (usage errors), `--help`, `--version`, and the no-subcommand help emit no notice and perform no discovery.
- The `project` subcommands consume the file directly and never emit the notice; their output is the report itself.
- Every other successfully parsed command (including `init`, `validate`, the write commands, and `run`) emits the notice when an untrusted file is discovered — on success and on failure alike, and for `run` before the target process replaces or starts (the notice is written and flushed in the pre-dispatch prelude; architecture, ARCH-004).
- Evaluation order per invocation: (1) discovery; (2) trust resolution over one immutable byte snapshot — canonical-path resolution → path-only approval-record lookup → a single content read → classification: a corrupt trust store fails with exit 2 (AC-003.8); an unresolvable state base (required environment variables unset) degrades the file to untrusted with the notice naming the unresolvable state location and the variables to set, for commands outside the `project` group; is represented in `project status`'s report (`trust` = `unavailable`, AC-006.11); and fails `project allow`/`revoke` explicitly (exit 2, EDGE-004b); a canonicalization failure or a file that disappears between discovery and read is classified as if the read failed; a read failure with an approval record for the canonical path fails with exit status 2 naming the file and the remedy (restore the file or run `agentenv project revoke`); a read or canonicalization failure without an approval record is untrusted (`invalid`, with a violation naming the file and the failure class); otherwise the snapshot bytes are fingerprinted, compared against the approval record, and parsed — the same bytes serve fingerprinting and parsing, so no content change between check and use is possible. Classification precedence: a snapshot that fails validation is `invalid` (with violations) regardless of the fingerprint result — the diagnosis outranks the bare change signal; (3) user-config load and the command proper, unchanged.
- The notice MUST never appear on stdout and MUST never alter any stdout payload, JSON or text.

Source trace:

- PRD: PRD-FR-004, PRD-FR-007, PRD-NFR-002
- Architecture: ARCH-004

Acceptance criteria:

- AC-005.1: GIVEN an untrusted project file, WHEN `agentenv list --json` runs, THEN stdout is byte-identical to the same invocation without the file, the exit status is unchanged, and stderr contains exactly one notice line naming the file.
- AC-005.2: GIVEN an untrusted and unparseable project file, WHEN any read command runs, THEN the command succeeds as if no file existed, with the single stderr notice.
- AC-005.3: GIVEN a trusted project file that is then made unreadable while its approval record remains, WHEN a command that resolves a profile runs, THEN it fails with exit status 2 and a message naming the file and a next action.
- AC-005.4: GIVEN `--no-project` or non-empty `AGENTENV_NO_PROJECT` with an untrusted file present, WHEN any command outside the `project` group runs, THEN no notice is emitted.
- AC-005.5: GIVEN an untrusted project file and a command that fails for an unrelated reason (for example `agentenv get` on an unknown path), WHEN the command runs, THEN the notice still appears on stderr alongside the error and the exit status is the unrelated failure's status.
- AC-005.6: GIVEN an untrusted project file, WHEN `agentenv run --with <entry> -- <target>` launches successfully, THEN the notice appears on stderr before the target's own output.
- AC-005.7: GIVEN an untrusted project file and an environment with no resolvable state base (relevant variables unset), WHEN a read command runs, THEN the command succeeds as if the file were absent and the notice names the unresolvable state location and the variables to set.
- AC-005.8: GIVEN a trusted project file, WHEN any command outside the `project` group runs, THEN stderr is byte-identical to the same invocation with no project file present (no notice for trusted files).
- AC-005.9: GIVEN an untrusted project file and an invocation that fails CLI parsing (for example an unknown flag), WHEN it runs, THEN no notice is emitted.

Verification:

- Automated: integration tests asserting stdout snapshots, stderr content, and exit codes.

### SPEC-006: `agentenv project status`

`agentenv project status` MUST always produce its report except on the two infrastructure failures that exit 2 (corrupt trust store; unreadable file with an approval record) — it never fails because the user configuration is missing, invalid, or has no selectable profile, never fails because the pin names an undefined profile, and represents an unresolvable state base inside the report (`trust` = `unavailable`) rather than failing. It reports, in text and `--json`: whether a project file was discovered (and its path, in the discovered spelling — canonicalization is trust identity only), its trust state, its profile pin, and the requirement report. When requirements cannot be checked (no user config, unparseable user config, no selectable profile, or a pin naming an undefined profile), the requirement section MUST state that requirements were not checked, name the reason, and name the next action.

Exit status (exhaustive matrix; the first matching row applies):

| Condition | Status |
| --- | --- |
| Corrupt trust store (AC-003.8), or a discovered file with an approval record that cannot be read (SPEC-005 order step 2 classification) | 2 |
| No project file discovered | 0 |
| Discovered file untrusted (new or changed), invalid, or trust `unavailable` (state base unresolvable) | 5 |
| Trusted; zero requirements declared (regardless of whether selection is degraded) | 0 |
| Trusted; ≥1 declared requirement unsatisfied, or requirements declared but uncheckable | 6 |
| Trusted; all declared requirements checked and satisfied | 0 |

JSON contract deviation (deliberate, documented): the accepted change-001 rule "a failing `--json` invocation leaves stdout empty" does not apply to `project status --json` exit statuses 5 and 6 — the report is the payload and is emitted on stdout together with the non-zero status. Exit 2 produces no report (stdout empty, error on stderr), matching the accepted rule. The nested `project` envelope is a new surface and changes no existing envelope; `version: null` is a documented extension of the `version` member for report states where the user config is unavailable. SPEC-008 requires this deviation to be documented in the README and skill.

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

Member state table (members are never omitted):

| State | `version` | `path` | `trust` | `trust_reason` | `violations` | `profile_pin` | `requirements` |
| --- | --- | --- | --- | --- | --- | --- | --- |
| No file discovered | config version, or `null` if config unavailable | `null` | `null` | `null` | `[]` | `null` | `checked: false`, `reason: "no project file discovered"`, `profile: null`, `entries: []` |
| Untrusted (new/changed), parses | as above | discovered path | `untrusted-new` / `untrusted-changed` | `null` | `[]` | file's pin or `null` | `checked: false`, reason states the file is untrusted, `profile: null`, `entries: []` |
| Invalid (schema violation, unparseable TOML, oversized, unapproved-unreadable, canonicalization failure) — outranks changed | as above | discovered path | `invalid` | `null` | one or more violations (paths and failure class only) | `null` | `checked: false`, reason states the file is invalid, `profile: null`, `entries: []` |
| State base unresolvable | as above | discovered path | `unavailable` | names the unresolvable state location and variables | `[]` | `null` | `checked: false`, reason states trust could not be determined, `profile: null`, `entries: []` |
| Trusted, checking degraded (no/unparseable config, no selectable profile, dangling pin) | `null` when config unavailable, else config version | discovered path | `trusted` | `null` | `[]` | file's pin or `null` | `checked: false`, reason names the degradation and next action, `profile: null`, `entries: []` |
| Trusted, checked | config version | discovered path | `trusted` | `null` | `[]` | file's pin or `null` | `checked: true`, `reason: null`, `profile` = active profile, `entries` = every declared requirement in file declaration order |

Source trace:

- PRD: PRD-FR-003, PRD-FR-005, PRD-NFR-002, PRD-NFR-003
- Architecture: ARCH-003, ARCH-006

Acceptance criteria:

- AC-006.1: GIVEN no project file, WHEN `project status` runs, THEN it reports that no file was discovered and exits 0.
- AC-006.2: GIVEN an untrusted (new or changed) file, WHEN `project status` runs, THEN it reports the trust state and the approval command to run, and exits 5.
- AC-006.3: GIVEN an invalid file, WHEN `project status` runs, THEN it reports each violation by TOML path (values not echoed) with the remedy, and exits 5.
- AC-006.4: GIVEN a trusted file whose declared requirements are all satisfied, WHEN `project status` runs, THEN it reports every requirement as satisfied with its reason and exits 0.
- AC-006.5: GIVEN a trusted file with an unsatisfied requirement, WHEN `project status` runs, THEN that requirement is reported unsatisfied with what is missing and how to add it, and the exit status is 6.
- AC-006.6: GIVEN each row of the member state table, WHEN `project status --json` runs, THEN stdout is a single JSON document matching the frozen envelope with exactly the members that row specifies, and no notice line is mixed into stdout.
- AC-006.7: GIVEN a discovered trusted file and no user config file, WHEN `project status` runs, THEN it reports discovery, trust, and pin; the requirement section states requirements were not checked because the user config is unavailable and names the next action; `version` is `null` in JSON; and the exit status is 6 when requirements are declared, else 0.
- AC-006.8: GIVEN a trusted pin naming an undefined profile, WHEN `project status` runs, THEN the report states the pin and that requirements were not checked because the pinned profile is not defined, and the exit status is 6 when requirements are declared, else 0.
- AC-006.9: GIVEN a discovered trusted file and an unparseable user config file, WHEN `project status` runs, THEN the report is produced, the requirement section names the user config as the reason checking did not run, `version` is `null` in JSON, and the exit status is 6 when requirements are declared, else 0.
- AC-006.10: GIVEN a trusted file declaring requirements but no pin, a user config with no `default_profile`, and no flag or environment selection, WHEN `project status` runs, THEN the requirement section states no profile was selectable and the exit status is 6.
- AC-006.11: GIVEN a discovered project file and no resolvable state base (relevant environment variables unset), WHEN `project status` runs, THEN the report carries `trust` = `unavailable` with `trust_reason` naming the unresolvable state location and variables, no notice is emitted, and the exit status is 5.
- AC-006.12: GIVEN a corrupt trust store, WHEN `project status` runs, THEN it fails with exit status 2 naming the store path and remedy (AC-003.8), producing no report (stdout empty in `--json` mode).
- AC-006.13: GIVEN a trusted project file that is then edited into an unparseable file, WHEN `project status` runs, THEN the report classifies it `invalid` with violations (not merely changed) and exits 5.

Verification:

- Automated: integration tests plus JSON snapshots under `tests/snapshots/` for every row of the member state table.

### SPEC-007: Requirement declarations and structural checking

A trusted project file's `[requires.<entry>]` declarations MUST be checked structurally against the active profile: a requirement is satisfied when the named entry exists in the active profile and, when `fields` is declared, every listed entry-relative field path resolves to any value inside that entry — scalar, table, array, or credential reference all satisfy; the `inject`-table source restrictions (injectable-scalar-only) do NOT apply to requirement checking. Checking MUST NOT resolve credentials, execute provider commands, or read secret stores. Requirement checking MUST NOT block or alter any command other than `project status`.

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
- AC-007.7: GIVEN a requirement whose `fields` member resolves to a table, and another whose member resolves to a field holding a credential reference, WHEN `project status` runs, THEN both are satisfied.

Verification:

- Automated: integration tests; AC-007.4 verified with `tests/fixtures/counting_provider.sh` (execution count unchanged).

### SPEC-008: Documentation and agent protocol

The README, the agent skill (`skills/agentenv/SKILL.md`), and the README's `AGENTS.md` block MUST document: the project file schema (including the 64 KiB limit) and discovery, the trust lifecycle, the extended exit-status table (statuses 5 and 6 added; statuses 0–4 and 127 unchanged in meaning, with status 2 explicitly noted as also covering project-file validation errors), the `project status --json` deviation from the empty-stdout-on-failure JSON rule (SPEC-006), the precedence chain including the pin and its application to read, `run`, `set`, and `unset` (and `--create-profile`'s unchanged explicit-flag rule), the bypass flag/variable and its non-application to `project` subcommands, and the pairing guidance — non-secret values may live in `.env`; credentials reach tools only through `agentenv run` (worked docker compose example using variable passthrough or `${VAR}` interpolation); `env_file:` with secrets is documented as the anti-pattern this replaces. The skill MUST add a project-discovery step (`agentenv project status --json`) to its reading protocol.

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

In the absence of a discovered project file, every existing *functional* command invocation MUST behave byte-identically to the pre-change release: same stdout payload, same error text, same exit statuses. Help, usage, and version renderings are exempt — they necessarily change to list the new global flag, the new subcommand, and the new release version; this additive surface change is recorded for the release notes. To make the guarantee verifiable on any machine, every test invocation of the binary MUST be hermetic with respect to project discovery: the shared integration harness pins each invocation's working directory to a directory the test controls and sets `AGENTENV_NO_PROJECT` for tests that do not exercise project behavior, and the direct invocations that bypass the shared helper (the PTY/signal tests in `tests/run_p3.rs` and the stdin tests in `tests/credential_p2.rs`) are isolated the same way individually. Adapting the harness and these call sites this way is a permitted mechanical change; test assertions themselves remain unmodified.

Source trace:

- PRD: PRD-NFR-004
- Architecture: Risks table (behavior drift)

Acceptance criteria:

- AC-009.1: GIVEN the full pre-existing test suite with only the mechanical hermeticity changes applied (working-directory pinning and `AGENTENV_NO_PROJECT`, including the direct invocations in `tests/run_p3.rs` and `tests/credential_p2.rs`), WHEN `cargo test` runs on the completed change, THEN every pre-existing test assertion passes unmodified.
- AC-009.2: GIVEN the JSON snapshots under `tests/snapshots/`, WHEN snapshot tests run, THEN all pre-existing snapshots are byte-identical.

Verification:

- Automated: `cargo test` on Linux, macOS, and Windows via existing CI.

### SPEC-010: No-secret invariant under a project file

A discovered project file — in any trust state — MUST NOT cause a credential value to be printed, persisted, or rerouted: discovery, validation, trust operations, and `project status` MUST NOT resolve credentials, execute provider commands, or read secret stores; and the only way a project file changes which credentials `run` injects, or under which environment names, is by selecting a different profile through a trusted pin (SPEC-004).

Output discipline: every *diagnostic* introduced by this change — validation violations, error messages, and the untrusted-file notice — names TOML paths and file paths only, names a next action (accepted change-001 SPEC-018 contract), and never echoes project-file string values, user-config values, or TOML source lines, consistent with the accepted no-echo rules. The `project status` *report* is the deliberate, bounded exception: it exposes exactly the members of the frozen SPEC-006 envelope — the file path, trust state and reason, violation paths, the profile pin, requirement entry names, field paths, requirement reasons and satisfaction results, plus the structural context members `version` (user-config schema version) and `requirements.profile` (the active profile name) — and nothing else. In particular it never exposes open-schema user-config values (entry field values, descriptions, credential definitions). No other command surface exposes project-file string values.

Source trace:

- PRD: PRD-NFR-001, PRD-NFR-002
- Architecture: ARCH-002, ARCH-004, ARCH-006

Acceptance criteria:

- AC-010.1: GIVEN an invalid project file whose forbidden or malformed positions carry a distinctive sentinel string, WHEN `project allow` and `project status` report its violations and WHEN any other command emits the notice, THEN every message names paths only and the sentinel never appears in any stdout or stderr output.
- AC-010.2: GIVEN an untrusted but schema-valid project file whose `profile` and `reason` values carry a sentinel string, WHEN any command outside the `project` group runs, THEN the sentinel never appears in that command's stdout or stderr (the notice names the file path only).
- AC-010.3: GIVEN a trusted valid project file and a user config whose open-schema field values, descriptions, and credential definitions carry a distinctive sentinel string, WHEN `project status` runs, THEN the report contains every envelope member (including `version`, the active profile name, the pin, and requirement reasons) and the sentinel never appears in its output.
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
| EDGE-004a | State base unset (no `XDG_STATE_HOME`/`HOME` on Unix, no `LOCALAPPDATA` on Windows) on a read path with a discovered file | File degrades to untrusted; command proceeds; notice names the unresolvable state location and variables (AC-005.7) | Integration test, injected env |
| EDGE-004b | State base unset when `project allow` or `project revoke` must write | Explicit configuration error naming the variables, exit 2 | Unit + integration test |
| EDGE-005 | `project allow` run twice on the same content | Second run succeeds and reports approval already current | Integration test |
| EDGE-006 | Project file deleted after approval | No file discovered; commands behave as with no project file; stale record is harmless | Integration test |
| EDGE-007 | Pin equals the profile already selected by `default_profile` | Identical outcome; no special casing observable | Integration test |
| EDGE-008 | `--profile` with empty value while a trusted pin exists | Existing usage error (unchanged) | Existing test suite |
| EDGE-009 | Requirement entry name containing punctuation (quoted TOML key) | Validated as an entry name by the accepted segment grammar; checked against the profile | Integration test |
| EDGE-010 | stderr notice with `--json` commands piped to a JSON parser | stdout parses as JSON; notice only on stderr | Integration test (AC-005.1, AC-006.6) |
| EDGE-011 | Approved file edited into a schema-invalid or unparseable file | Classified `invalid` (outranks `untrusted-changed`); other commands inert + notice; `status` reports violations, exit 5; `revoke` still works (AC-003.13, AC-006.13) | Integration test |
| EDGE-012 | `.agentenv.toml` is a directory or dangling symlink | Not a project file; walk continues (AC-001.2) | Integration test |
| EDGE-013 | Project file over 64 KiB | Invalid with a size violation (AC-002.7); inert + notice | Integration test |

## Dependencies

| Requirement | Dependency | Reason |
| --- | --- | --- |
| SPEC-003 | SPEC-002 | `allow` validates before recording |
| SPEC-004 | SPEC-003 | Only a trusted pin participates in precedence |
| SPEC-005 | SPEC-001, SPEC-003 | Inertness is defined relative to discovery and trust state |
| SPEC-006 | SPEC-003, SPEC-007 | Status reports trust state and the requirement report |
| SPEC-007 | SPEC-002 | Checking consumes validated declarations |
| SPEC-008 | SPEC-001..007 | Documents shipped behavior |
| SPEC-009 | all | Compatibility is a property of the whole change |
| SPEC-010 | SPEC-002..006 | The invariant spans every new surface |

## Acceptance Matrix

| Acceptance ID | Requirement | Phase | Verification method | Status |
| --- | --- | --- | --- | --- |
| AC-001.1..5 | SPEC-001 | Phase 1 | `cargo test` integration | Draft |
| AC-002.1..7 | SPEC-002 | Phase 1 | `cargo test` integration | Draft |
| AC-003.1..13 | SPEC-003 | Phase 1 | `cargo test` integration/unit | Draft |
| AC-004.1..8 | SPEC-004 | Phase 1 | `cargo test` integration | Draft |
| AC-005.1..9 | SPEC-005 | Phase 1 | `cargo test` integration | Draft |
| AC-006.1..13 | SPEC-006 | Phase 1 | `cargo test` integration + snapshots | Draft |
| AC-007.1..7 | SPEC-007 | Phase 1 | `cargo test` integration | Draft |
| AC-008.1..3 | SPEC-008 | Phase 2 | Manual doc walkthrough | Draft |
| AC-009.1..2 | SPEC-009 | all | `cargo test` full suite | Draft |
| AC-010.1..5 | SPEC-010 | Phase 1 | `cargo test` integration | Draft |

## Implementation Notes

- The trust fingerprint and store format are internal; only the behaviors above are contractual. The store location is an architecture decision (ARCH-002), documented for support but not a stability contract.
- Tests control the trust store location by overriding the state base environment (`XDG_STATE_HOME`/`HOME` on Unix, `LOCALAPPDATA` on Windows); no dedicated override variable exists.
- Phase 1 ships the complete `project status` surface including the frozen JSON envelope and requirement checking; Phase 2 is documentation only.
- The pin is carried with its origin (project file path) so selection errors can name the file (ARCH-005); command outcomes carry stdout, stderr, and exit status explicitly so `project status` can emit a full report with a non-zero status; the notice is written by the pre-dispatch prelude in the CLI entry path (architecture, ARCH-004).
- Reported project paths use the discovered spelling; the canonical path is trust identity only and appears in no report member.

## Assumptions

- SPEC-AS-001: The project file name is exactly `.agentenv.toml` (hidden file, matching `.envrc`/`.mise.toml` convention) because the name should read as tool configuration, not project data.
- SPEC-AS-002: Discovery includes the working directory itself as the first candidate because that is the least surprising reading of "nearest".
- SPEC-AS-003: The stderr notice's exact wording is not contractual; its properties are: exactly one line, names the file path, names `agentenv project status`.
- SPEC-AS-004: Exit statuses 5 (project trust-state failure: untrusted/invalid/unavailable at `status`, or `allow`/`revoke` with no discovered file) and 6 (requirements unsatisfied or uncheckable) are free because 0–4 and 127 are the only statuses documented today; status 2 keeps its documented meaning (configuration-file error) and now also covers project-file validation errors, which are configuration-file errors.
- SPEC-AS-005: A stale trust record for a deleted or moved file is retained harmlessly (no garbage collection in v1) because records are tiny and pruning adds state-mutation paths for no behavioral gain.
- SPEC-AS-006: `project allow`/`revoke`/`status` ignore `--profile` for trust decisions (trust is per-file, not per-profile); `status` computes the requirement report against normal selection and degrades per SPEC-006 when selection cannot complete.
- SPEC-AS-007: The startup-latency half of PRD-NFR-003 is not gated by an automated test (a portable, non-flaky CI latency bound does not exist for a sub-millisecond-order walk). It is discharged at validation by a recorded manual cold-start comparison (`agentenv list` timed in a depth-20 tree with and without discovery) documented in `validation.md`. The design constraint stands: discovery adds at most one file-existence probe per ancestor plus one bounded (64 KiB) file read and hash.
- SPEC-AS-008: The trust store's permission bits are not checked on read; only creation is constrained (`0600` on Unix). The state directory is user-owned and the recorded threat model excludes malicious local processes; mirroring the user-config permission gate would add a failure mode without a defended threat.
- SPEC-AS-009: PRD-SM-002's "zero behavior change" is read as: zero change to stdout, exit status, and command semantics; the single stderr notice is the deliberate exception required by PRD-FR-007.

## Clarifications

### Session 2026-08-28

- Q: What may the project file contain? -> A: Selection-only — profile pin plus `[requires]` declarations (applied to Scope, SPEC-002).
- Q: Trust model? -> A: Trust-on-first-use with content-hash invalidation, user-owned state (applied to SPEC-003).
- Q: Pin position in precedence? -> A: Below `AGENTENV_PROFILE`, above `default_profile` (applied to SPEC-004).

## Open Questions

| ID | Question | Blocking? | Resolution |
| --- | --- | --- | --- |
| — | none | — | — |
