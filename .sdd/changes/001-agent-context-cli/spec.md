# Implementation Specification: agent-context CLI

## Source Artifacts

- Change ID: 001-agent-context-cli
- PRD: prd.md
- Architecture: architecture.md
- Design contract: design-source.md (user-approved design document, Chinese; §ref numbers below cite it)
- Current specs: none (greenfield)

## Scope

### In Scope

- TOML config loading, profile selection, open-schema entries with mandatory descriptions (§3–§4).
- Query commands `list`, `show`, `get`, `find` with text and stable JSON output (§5).
- `validate` with the full §5.7/§8 rule set and exit codes.
- Credential references (`credential://name[?as=ENV]`), providers `env` / `keychain` / `command`, shallow status display (§4.3, §5.6, §6.1).
- `credential list` / `check` / `set` (§5.8).
- `run --with` injection of credentials and `inject`-table values with conflict detection and transparent process semantics (§4.4, §6.2).
- Security invariants: no secret values in any query output, error, or log (§10).

### Out of Scope

- GUI, cloud sync, config-file writing, guessing missing fields, printing plaintext credentials (§11).
- Defense against a launched target process deliberately reading its own environment (§10 threat model).
- Localization: all CLI output is English (Clarifications, 2026-08-21).

## Phase Map

| Phase | Name | Priority | Objective | Depends on | Independent test |
| --- | --- | --- | --- | --- | --- |
| Phase 1 | Config core & queries | P1 (MVP) | Load/validate config, select profile, run `list`/`show`/`get`/`find`/`validate` with text+JSON and exit codes; credential fields shown as references with shallow status | None | Against a fixture TOML file, every query command returns the documented text and JSON, and every documented error path returns its exit code — no credential machinery invoked |
| Phase 2 | Credential providers | P2 | `credential list`/`check`/`set` resolve and store secrets through env/keychain/command providers | Phase 1 | With a mock keychain store, a fake command script, and env vars: `check` reports success/failure correctly, `set` round-trips a value, no secret appears on stdout |
| Phase 3 | Injection runner | P3 | `run --with` builds a conflict-checked injection plan and launches the target transparently | Phase 1, 2 | `run --with llm -- env`-style probe (in tests only) sees exactly the injected variables; exit codes, stdio, and signals pass through |

## Requirements

### SPEC-001: Config file location

The CLI MUST read its configuration from, in priority order: the `AGENT_CONTEXT_FILE` environment variable if set; else `$XDG_CONFIG_HOME/agent-context/context.toml` if `XDG_CONFIG_HOME` is set (Unix); else `~/.config/agent-context/context.toml` (Unix) / `%APPDATA%\agent-context\context.toml` (Windows). (§3)

Source trace: PRD-FR-001. Architecture: `config` module.

Acceptance criteria:

- AC-001.1: GIVEN `AGENT_CONTEXT_FILE=/tmp/x.toml` pointing at a valid file, WHEN any query command runs, THEN that file is used even if a default-path file exists.
- AC-001.2: GIVEN no config file at the resolved path, WHEN any command runs, THEN the CLI exits 2 with a message containing the resolved path.

Verification: Automated: integration tests with temp dirs and scrubbed env.

### SPEC-002: Strict core validation on load

Every command MUST refuse to operate on a config that fails core validation, reporting the offending config path and exiting 2. Core rules (§8): `version` is a supported integer (only `1`); `default_profile`, when present, names an existing profile; every profile and every entry has a non-empty string `description`; every credential has `description`, `provider`, `inject_as`, and its provider-specific required fields (`env`: `name`; `keychain`: `service`, `account`; `command`: non-empty `argv` array of strings); every `credential://` reference resolves to a defined credential; `?as=` values and `inject_as` values are valid environment variable names (`[A-Za-z_][A-Za-z0-9_]*`); `inject` tables satisfy SPEC-013.

Source trace: PRD-FR-006. Design §8, §9.

Acceptance criteria:

- AC-002.1: GIVEN a config where `contexts`-style structure is valid but `profiles.work.llm` lacks `description`, WHEN `list` runs, THEN exit 2 and the message contains `profiles.work.llm`.
- AC-002.2: GIVEN `version = 2`, WHEN any command runs, THEN exit 2 with a message naming the supported version.
- AC-002.3: GIVEN an entry field `credential = "credential://missing"`, WHEN `list` runs, THEN exit 2 and the message contains the unresolved name `missing`.

Verification: Automated: table-driven validation tests.

### SPEC-003: Open schema

Users MUST be able to add arbitrary entries and fields (strings, integers, floats, booleans, datetimes, arrays, sub-tables) under a profile; new fields appear in `list`, `show`, `get`, and `find` with no CLI change. Reserved keys: `description` (profile and entry level), `inject` (entry level). (§4.2)

Source trace: PRD-FR-001.

Acceptance criteria:

- AC-003.1: GIVEN a fixture entry with one field of each TOML type plus a nested sub-table, WHEN `list <entry>` and `show <entry>` run, THEN every field appears with its correct type label and nested fields appear with dotted paths.
- AC-003.2: GIVEN the same fixture, WHEN `get <entry>.<nested>.<field>` runs, THEN the stored value is returned.

Verification: Automated: fixture round-trip tests.

### SPEC-004: Profile selection

The active profile MUST be resolved as: `--profile` flag, else `AGENT_CONTEXT_PROFILE` env var, else `default_profile` from the file. If the result names no existing profile, or nothing is configured, the CLI MUST exit 3 listing available profile names. (§4.1)

Source trace: PRD-FR-002.

Acceptance criteria:

- AC-004.1: GIVEN `default_profile = "work"` and `AGENT_CONTEXT_PROFILE=personal`, WHEN `list` runs, THEN the `personal` profile is listed.
- AC-004.2: GIVEN the same, WHEN `list --profile work` runs, THEN the `work` profile is listed (flag beats env var).
- AC-004.3: GIVEN `AGENT_CONTEXT_PROFILE=nope`, WHEN `list` runs, THEN exit 3 and the message lists the defined profile names.

Verification: Automated: env-permutation tests.

### SPEC-005: Path grammar

`get` (and path-typed arguments elsewhere) MUST accept dot-separated paths whose first segment is an entry name; a segment containing dots or spaces MUST be addressable by wrapping it in double quotes (e.g. `server."my.field"`). Paths never include profile names. An unknown path MUST exit 3 with a message naming the nearest valid container and suggesting `agent-context list <entry>`. (§5.1)

Source trace: PRD-FR-003.

Acceptance criteria:

- AC-005.1: GIVEN a field literally named `my.field` under entry `server`, WHEN `get server."my.field"` runs, THEN its value is returned.
- AC-005.2: GIVEN a valid entry `llm`, WHEN `get llm.region` runs and `region` does not exist, THEN exit 3 and the message contains `llm.region` and `list llm`.
- AC-005.3: GIVEN an unterminated quote in the path argument, WHEN `get` runs, THEN exit 1 with a grammar error message.

Verification: Automated: unit tests on the path parser + CLI integration.

### SPEC-006: `list`

`list` MUST show every entry of the active profile with its description, and each field with its dotted path (relative to the entry) and type label; `list <entry>` MUST limit output to one entry; `list --profiles` MUST show every profile with its description and mark the default. Credential-reference fields show shallow status per SPEC-012. (§5.2)

Source trace: PRD-FR-003.

Acceptance criteria:

- AC-006.1: GIVEN the design-doc example config (translated to `profiles.*`), WHEN `list` runs, THEN output contains entries `llm`, `ci`, `kubernetes`, their descriptions, field names, and type labels.
- AC-006.2: WHEN `list --profiles` runs, THEN output contains `work` marked as default and `personal`.
- AC-006.3: GIVEN `list nosuch`, THEN exit 3 listing available entry names.

Verification: Automated: golden-output integration tests (layout asserted loosely: required tokens present, one field per line).

### SPEC-007: `show`

`show <entry>` MUST print the entry description and every field with its value, except credential references, which show only the credential name and shallow status; `inject` tables are shown as env-name ← field-path pairs. (§5.3)

Source trace: PRD-FR-003, PRD-NFR-001.

Acceptance criteria:

- AC-007.1: GIVEN the example config, WHEN `show llm` runs, THEN output contains the description, `endpoint` with its URL, `model` with its value, and the credential name — and does NOT contain the string `credential://`'s resolved secret nor any provider secret.
- AC-007.2: GIVEN an entry with an `inject` table, WHEN `show` runs, THEN each mapping appears with target env name and source field path.

Verification: Automated: integration tests.

### SPEC-008: `get`

`get <path>` MUST print a scalar value verbatim (no quoting, single trailing newline). For array or table values it MUST print them only with `--json` (JSON encoding); without `--json` it exits 1 directing the caller to `--json`. A path resolving to a credential reference returns the reference string unchanged (including `?as=` if present) and never the secret. (§5.4)

Source trace: PRD-FR-003, PRD-FR-004.

Acceptance criteria:

- AC-008.1: WHEN `get llm.endpoint` runs, THEN stdout is exactly the URL plus newline; exit 0.
- AC-008.2: WHEN `get ci.tags --json` runs, THEN stdout is a JSON array equal to the stored values.
- AC-008.3: WHEN `get ci.tags` runs without `--json`, THEN exit 1 and the message mentions `--json`.
- AC-008.4: WHEN `get llm.credential` runs, THEN stdout is the stored reference string, e.g. `credential://company_llm`.

Verification: Automated: byte-exact stdout assertions.

### SPEC-009: `find`

`find <needle>` MUST case-insensitively substring-match entry names, field names, and descriptions within the active profile, printing each match's path plus its value (scalars), description (entries), or credential name + shallow status (references). `--all-profiles` widens the search to every profile and includes the profile name in each match. (§5.5)

Source trace: PRD-FR-003.

Acceptance criteria:

- AC-009.1: WHEN `find llm` runs on the example config, THEN matches include the `llm` entry and `llm.endpoint`, and no match reveals a secret value.
- AC-009.2: WHEN `find LLM --all-profiles` runs, THEN matches from both `work` and `personal` appear, each labeled with its profile.
- AC-009.3: WHEN `find zzz-no-match` runs, THEN exit 0 with a "no matches" message on stderr or empty result (JSON: empty `matches` array).

Verification: Automated: integration tests.

### SPEC-010: Stable JSON output

`list`, `show`, `get`, `find` (and `list --profiles`) MUST accept `--json`. JSON goes to stdout, is the only stdout content, and has these stable shapes: every top-level object carries `"version"` (config version); fields are objects `{"path", "type", "value"}` where `type` ∈ `string|integer|float|boolean|datetime|array|table|credential_ref`; `credential_ref` fields carry `{"path", "type", "credential": {"name", "provider", "status"}}` and NO `value`; `status` ∈ `available|not_set|configured|command_missing`. `list --json` groups fields under `{"name", "description", "fields": [...]}` entries; `find --json` returns `{"version", "matches": [...]}` with a `"profile"` key per match. (§5.9)

Source trace: PRD-NFR-003, PRD-NFR-001.

Acceptance criteria:

- AC-010.1: WHEN `list --json` runs on the example config, THEN output parses as JSON, carries `version: 1`, and the `llm.credential` field object has no `value` key.
- AC-010.2: WHEN `show llm --json` runs, THEN the credential object contains exactly `name`, `provider`, `status`.
- AC-010.3: Snapshot tests lock all five JSON shapes; any shape change fails the suite.

Verification: Automated: JSON schema/snapshot tests.

### SPEC-011: `validate`

`validate` MUST run all SPEC-002 core rules plus: sensitive-field-name rule (SPEC-020); Unix file-permission check (fail when the config file is readable or writable by group or others); and MUST report every failure (not just the first) with its config path, exiting 2 on any failure, 0 when clean. (§5.7)

Source trace: PRD-FR-006.

Acceptance criteria:

- AC-011.1: GIVEN a config with two independent violations, WHEN `validate` runs, THEN both are reported and exit is 2.
- AC-011.2: GIVEN a clean config with mode 0644 on Unix, WHEN `validate` runs, THEN the permission failure names the file and the expected mode `0600`.
- AC-011.3: GIVEN a clean 0600 config, THEN exit 0.

Verification: Automated: integration tests with `chmod` in temp dirs.

### SPEC-012: Credential references and shallow status

Any string value beginning with `credential://` anywhere under an entry (including nested sub-tables) MUST be treated as a credential reference of form `credential://<name>[?as=<ENV>]`. Query commands MUST display references with a shallow status that never resolves a secret: `env` → `available`/`not_set` via variable presence; `keychain` → `configured` (no store read); `command` → `configured`/`command_missing` via PATH lookup of `argv[0]`. Query commands MUST NOT execute provider commands, read the keychain, or perform network I/O. (§4.3, §5.6)

Source trace: PRD-FR-004, PRD-NFR-001.

Acceptance criteria:

- AC-012.1: GIVEN a reference nested two tables deep, WHEN `list` runs, THEN it is displayed as a credential reference.
- AC-012.2: GIVEN a `command` credential whose `argv[0]` is an absent binary, WHEN `show` runs, THEN status is `command_missing` and no process was spawned (asserted via a canary script that would create a file if executed).
- AC-012.3: GIVEN an `env` credential with the variable set, THEN status is `available`; unset, `not_set`.

Verification: Automated: canary-based no-execution test; status matrix test.

### SPEC-013: `inject` tables

An entry MAY contain a reserved `inject` sub-table mapping environment variable names to field paths within the same entry. Validation MUST reject: keys that are not valid env names; paths that do not resolve within the entry; paths resolving to non-scalar values; paths resolving to credential references (credentials inject via `inject_as`/`?as=` only). (§4.4)

Source trace: PRD-FR-005.

Acceptance criteria:

- AC-013.1: GIVEN `inject = { OPENAI_BASE_URL = "endpoint" }` where `endpoint` is a string, WHEN `validate` runs, THEN exit 0.
- AC-013.2: GIVEN an inject value pointing at an array field, WHEN `validate` runs, THEN exit 2 naming the inject key and the path.
- AC-013.3: GIVEN an inject value pointing at a credential-reference field, WHEN `validate` runs, THEN exit 2 with a message directing to `inject_as`.

Verification: Automated: validation tests.

### SPEC-014: Credential providers

`env` resolution reads the named variable, failing when unset. `keychain` resolution reads the platform credential store (macOS Keychain / Windows Credential Manager / Linux secret-service) by `service` + `account`, failing when the item is missing; on systems with no usable store the error names the provider and suggests `command`/`env`. `command` resolution executes `argv` directly with no shell, captures stdout as the secret (one trailing newline stripped), inherits stderr, and fails on non-zero exit or empty output. Resolution failure is always explicit (exit 4 at CLI level) and never substitutes another credential. (§6.1, §10)

Source trace: PRD-FR-004.

Acceptance criteria:

- AC-014.1: GIVEN `COMPANY_LLM_TOKEN` unset, WHEN `credential check company_llm` runs, THEN exit 4 and the message names the variable — and does not print any value.
- AC-014.2: GIVEN a fake command provider script printing `s3cret\n` , WHEN resolution runs (via `check`), THEN success is reported and the string `s3cret` does NOT appear in stdout/stderr.
- AC-014.3: GIVEN a command provider with `argv = ["sh", "-c", "..."]`-free config and a script that echoes its arguments, WHEN resolved, THEN arguments arrive without shell interpretation (a `$HOME`-containing argument stays literal).
- AC-014.4: GIVEN a mock keychain store containing the item, WHEN `check` runs, THEN success; with the item absent, exit 4 naming service/account.

Verification: Automated: keyring mock store; fixture scripts.

### SPEC-015: `credential list` / `check` / `set`

`credential list` MUST print every credential definition with provider type and shallow status. `credential check <name>` MUST perform a real resolution, report success or the concrete failure reason, and never print the secret. `credential set <name>` MUST work only for `keychain` credentials: it reads the value interactively without echo (or from stdin when not a TTY) and writes it to the platform store; for `env`/`command` credentials it exits 1 explaining those are externally managed. (§5.8)

Source trace: PRD-FR-007.

Acceptance criteria:

- AC-015.1: WHEN `credential list` runs on the example config, THEN both credentials appear with provider labels and statuses, and no store read occurs (shallow only).
- AC-015.2: GIVEN a piped stdin value, WHEN `credential set openai_personal` runs against the mock store, THEN a following `credential check openai_personal` succeeds and stdout never contained the value.
- AC-015.3: WHEN `credential set company_llm` (env provider) runs, THEN exit 1 and the message says env credentials are managed externally.

Verification: Automated: mock-store round-trip.

### SPEC-016: `run --with` injection plan

`run --with <entry>... -- <cmd> [args...]` MUST: resolve each named entry in the active profile (exit 3 if absent); recursively collect its credential references and its `inject` table; build the full target-env mapping BEFORE resolving any secret; and fail with exit 4, naming both sources, when two injections target the same env variable name — without launching the target or resolving any provider. Repeating the same entry is idempotent. `?as=` overrides the credential's `inject_as` for that reference. An entry with no references and no `inject` table injects nothing and still runs the target. (§6.2)

Source trace: PRD-FR-005.

Acceptance criteria:

- AC-016.1: GIVEN entry `llm` (credential → `OPENAI_API_KEY`, inject → `OPENAI_BASE_URL`, `OPENAI_MODEL`), WHEN `run --with llm -- <probe>` runs, THEN the probe process sees exactly those three variables added to its environment.
- AC-016.2: GIVEN two `--with` entries whose credentials both target `OPENAI_API_KEY`, WHEN `run` executes, THEN exit 4, the message names both credential names, and a canary proves no provider command ran.
- AC-016.3: GIVEN `credential = "credential://company_llm?as=LLM_API_KEY"`, WHEN `run --with llm -- <probe>` runs, THEN the probe sees `LLM_API_KEY` and not `OPENAI_API_KEY`.

Verification: Automated: probe binary/script writing its env to a temp file.

### SPEC-017: `run` process transparency

After a successful injection plan, `run` MUST hand stdio to the target unbuffered and uncaptured, and the exit status observed by the caller MUST be the target's own (including signal termination observed as 128+N by the shell). Pre-launch failures use SPEC-018 exit codes; a target binary that cannot be executed exits 127 with a clear message. `agent-context` itself MUST NOT write anything to stdout/stderr on the happy path. (§6.2)

Source trace: PRD-FR-005.

Acceptance criteria:

- AC-017.1: WHEN `run --with ci -- sh -c "echo out; echo err >&2; exit 7"` runs, THEN stdout is `out`, stderr is `err`, exit code is 7, with no additional wrapper output.
- AC-017.2: WHEN the target does not exist, THEN exit 127 and the message names the missing command.
- AC-017.3: (Unix) WHEN the target kills itself with SIGTERM, THEN the shell observes exit 143.

Verification: Automated: integration tests; signal case Unix-only.

### SPEC-018: Exit codes and error messages

The CLI MUST use: 0 success; 1 usage/argument errors (including `get` non-scalar without `--json`, `credential set` on non-keychain); 2 config-file errors (missing file, parse, validation); 3 profile or path not found; 4 credential unavailable or injection conflict; 127 target-not-executable (run only). Every error message MUST name the failing thing and a next action (a command to run or a config path to edit), in English. (§9)

Source trace: PRD-NFR-002.

Acceptance criteria:

- AC-018.1: A table-driven test exercises one representative failure per exit code and asserts both the code and a required message token.
- AC-018.2: No error path prints a secret value (covered by grep over all captured outputs in the suite).

Verification: Automated.

### SPEC-019: No-secret invariant (security)

No invocation of `list`, `show`, `get`, `find`, `validate`, `credential list`, or any error path of any command may emit a resolved secret value on stdout, stderr, or into any file. `credential check` and `run` may resolve secrets but never print them. This invariant is enforced structurally (secret type without Display/Serialize, per ARCH-005) and by tests. (§10)

Source trace: PRD-NFR-001.

Acceptance criteria:

- AC-019.1: A suite-wide assertion greps every captured stdout/stderr from every integration test for planted sentinel secret values (distinct high-entropy strings per provider) and fails on any hit.
- AC-019.2: Compile-time: the secret type implements neither `Display` nor `Serialize`, and its `Debug` prints a redaction marker (unit-asserted).

Verification: Automated: sentinel grep + unit test.

### SPEC-020: Sensitive field names

`validate` MUST fail (exit 2) when a field whose name exactly equals `token`, `password`, `secret`, `api_key`, or `private_key`, or ends with `_token`, `_password`, `_secret`, `_api_key`, or `_private_key`, holds a string that is not a `credential://` reference. Non-string values and names like `token_endpoint` are unaffected. (§8)

Source trace: PRD-NFR-001.

Acceptance criteria:

- AC-020.1: GIVEN `api_key = "sk-live-123"`, WHEN `validate` runs, THEN exit 2 naming the path and suggesting a `credential://` reference.
- AC-020.2: GIVEN `github_token = "credential://gh"` (defined credential), THEN valid.
- AC-020.3: GIVEN `token_endpoint = "https://x"` and `use_token = true`, THEN both valid.

Verification: Automated: validation tests.

## Edge Cases

| ID | Case | Expected behavior | Verification |
| --- | --- | --- | --- |
| EDGE-001 | Profile with only `description` (no entries) | `list` prints the profile header and an explicit "no entries" line, exit 0 | integration |
| EDGE-002 | `AGENT_CONTEXT_FILE` points at a directory | exit 2, message names the path and that it is not a file | integration |
| EDGE-003 | Config file is empty (0 bytes) | exit 2: `version` missing | integration |
| EDGE-004 | `run --with a --with a` | identical to a single `--with a` | integration |
| EDGE-005 | `run` with no `--` separator / no command | exit 1 usage error showing the expected form | integration |
| EDGE-006 | Command provider prints only whitespace | resolution fails: "produced no output", exit 4 | integration |
| EDGE-007 | `?as=` with invalid env name (`?as=1BAD`) | `validate`/load exit 2 naming the reference | unit |
| EDGE-008 | `find` needle matching a description only | the owning entry is a match | integration |
| EDGE-009 | Datetime field | type label `datetime`; `get` prints RFC 3339 text | integration |
| EDGE-010 | `get <entry>` (path with no field part) | exit 1 directing to `show <entry>` (entries are not scalar) | integration |

## Dependencies

| Requirement | Dependency | Reason |
| --- | --- | --- |
| SPEC-006..011 | SPEC-002..005 | queries need a validated config, profile, and path grammar |
| SPEC-014, SPEC-015 | SPEC-002, SPEC-012 | providers operate on validated credential definitions |
| SPEC-016, SPEC-017 | SPEC-012, SPEC-013, SPEC-014 | injection consumes references, inject tables, and providers |

## Acceptance Matrix

| Acceptance ID | Requirement | Phase | Verification method | Status |
| --- | --- | --- | --- | --- |
| AC-001.1–2 | SPEC-001 | 1 | cargo integration tests | Draft |
| AC-002.1–3 | SPEC-002 | 1 | validation tests | Draft |
| AC-003.1–2 | SPEC-003 | 1 | fixture tests | Draft |
| AC-004.1–3 | SPEC-004 | 1 | env-permutation tests | Draft |
| AC-005.1–3 | SPEC-005 | 1 | parser unit + CLI tests | Draft |
| AC-006.1–3 | SPEC-006 | 1 | integration | Draft |
| AC-007.1–2 | SPEC-007 | 1 | integration | Draft |
| AC-008.1–4 | SPEC-008 | 1 | byte-exact stdout tests | Draft |
| AC-009.1–3 | SPEC-009 | 1 | integration | Draft |
| AC-010.1–3 | SPEC-010 | 1 | JSON snapshot tests | Draft |
| AC-011.1–3 | SPEC-011 | 1 | integration + chmod | Draft |
| AC-012.1–3 | SPEC-012 | 1 | canary + status matrix | Draft |
| AC-013.1–3 | SPEC-013 | 1 | validation tests | Draft |
| AC-014.1–4 | SPEC-014 | 2 | mock store + fixture scripts | Draft |
| AC-015.1–3 | SPEC-015 | 2 | mock-store round-trip | Draft |
| AC-016.1–3 | SPEC-016 | 3 | probe env-dump tests | Draft |
| AC-017.1–3 | SPEC-017 | 3 | integration (signal: Unix) | Draft |
| AC-018.1–2 | SPEC-018 | 1 | table-driven exit-code tests | Draft |
| AC-019.1–2 | SPEC-019 | 1–3 | sentinel grep + unit | Draft |
| AC-020.1–3 | SPEC-020 | 1 | validation tests | Draft |

## Implementation Notes

- Binary and crate name: `agent-context`. Library + thin `main` per architecture.md.
- The design document's Chinese output examples are illustrative; layout may differ, content requirements above govern. All output English.
- Shallow-status strings in text output may be humanized (`available`, `not set`, `configured`, `command missing`) but JSON uses the exact enum tokens of SPEC-010.

## Assumptions

- SPEC-AS-001: `get` refuses non-scalar values without `--json` (exit 1) instead of inventing an unspecified text encoding; design §5.4 only defines JSON for complex data.
- SPEC-AS-002: `find` matching is case-insensitive substring; the design doc does not specify case rules and agent needles are unpredictable.
- SPEC-AS-003: Array element addressing (`tags.0`) is not supported in v1; arrays are retrieved whole. The design doc's path grammar addresses keys only.
- SPEC-AS-004: `command` provider has no timeout in v1 and inherits stderr/stdin so interactive password-manager auth (e.g. `op`) can complete; design §5.8 anticipates such interaction.
- SPEC-AS-005: Valid env names are `[A-Za-z_][A-Za-z0-9_]*` (POSIX portable set).
- SPEC-AS-006: The Unix permission gate accepts only files with no group/other read or write bits (0600 or stricter, e.g. 0400).
- SPEC-AS-007: `run` target-not-executable exits 127 (shell convention); the design doc's table does not cover this case.
- SPEC-AS-008: `credential set` reads from stdin when stdin is not a TTY, enabling scripted setup while remaining no-echo interactively.

## Clarifications

### Session 2026-08-21

- Q: Implementation language? -> A: Rust (applied to architecture.md ARCH-001).
- Q: CLI output language? -> A: English for all user-visible output; no i18n in v1 (applied to Scope, SPEC-018).

## Open Questions

| ID | Question | Blocking? | Resolution |
| --- | --- | --- | --- |
| — | none | — | — |
