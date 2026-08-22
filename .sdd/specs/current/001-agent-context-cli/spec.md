# Implementation Specification: agent-context CLI

## Source Artifacts

- Change ID: 001-agent-context-cli
- PRD: prd.md
- Architecture: architecture.md
- Design contract: design-source.md (user-approved design document, Chinese; §ref numbers below cite it)
- Review state: revised per spec-review.md rounds 1–2
- Current specs: none (greenfield)

## Scope

### In Scope

- TOML config loading, profile selection, open-schema entries with mandatory descriptions (§3–§4).
- Query commands `list`, `show`, `get`, `find` with text and stable JSON output (§5).
- `validate` with the full §5.7/§8 rule set and exit codes.
- Credential references (`credential://name[?as=ENV]`), providers `env` / `keychain` / `command`, shallow status display (§4.3, §5.6, §6.1).
- `credential list` / `check` / `set` (§5.8).
- `run --with` injection of credentials and `inject`-table values with conflict detection and transparent process semantics (§4.4, §6.2).
- Security invariant: no secret value in any output agent-context itself writes (§10; boundary defined in SPEC-019). v1 writes no log files (SPEC-AS-020), which discharges §10's log clauses.
- README documenting the agent usage protocol (§7).

### Out of Scope

- GUI, cloud sync, config-file writing, guessing missing fields, printing plaintext credentials (§11).
- Defense against a launched target process deliberately reading its own environment (§10 threat model).
- Output written by external processes agent-context launches (the `run` target; a `command` provider's inherited stderr): those channels belong to the external tool. Accidental-leak protection covers agent-context's own output only (user decision 2026-08-21).
- Localization: all CLI output is English (Clarifications, 2026-08-21).
- JSON error envelopes and `validate --json` (deferred; SPEC-AS-014).

## Definitions

- **Entry**: a first-level sub-table of a profile. Besides `description`, every direct key of a profile MUST be a table; a scalar or array directly under a profile is a core validation violation (SPEC-002 rule 9).
- **Query commands** (the no-resolution set): `list`, `list --profiles`, `show`, `get`, `find`, `credential list`, `validate`. None of these may resolve a secret, execute a provider command, read a credential store, or perform network I/O.
- **Resolving commands**: `credential check`, `credential set`, `run`.
- **Scalar**: TOML string, integer, float, boolean, or datetime (any of TOML's four date-time forms). **Injectable scalar**: string, integer, float, or boolean.
- **Scalar-to-string conversion** (used by `get` text output and env injection): strings verbatim; integers in decimal; finite floats via their JSON number encoding (whole-valued floats keep the decimal point: `1.0` — text and JSON agree byte-for-byte); non-finite floats as `inf` / `-inf` / `nan` (NaN sign is not preserved); booleans as `true`/`false`; datetimes via their TOML lexical serialization (all four forms; text output only — datetimes are not injectable).
- **JSON value encoding**: integers/finite floats/booleans/strings as native JSON; datetimes and non-finite floats as JSON strings of their scalar-to-string form (their `type` tokens remain `datetime` / `float`). The encoding applies recursively inside arrays and tables.
- **Credential summary object**: `{"name", "provider", "status"}` — the single JSON representation of a credential wherever one appears in an envelope (`Field.credential`, `Match.credential`, `credential list` rows).
- **Description-as-metadata**: reserved `description` keys (profile and entry level) are metadata, not fields. They are excluded from field listings, `fields` arrays, and `find` field matches (descriptions participate in `find` through the description match dimension instead), and are rendered as headers/`description` envelope keys. They remain addressable via `get <entry>.description`, and raw `get <entry> --json` includes them as stored data.
- **Credential value domain**: a credential value is a non-empty, NUL-free, valid-UTF-8 string, for every provider. A value outside this domain is a resolution failure (exit 4) or, for `credential set` input, a usage error (exit 1).
- **Reference scanning scope** (where `credential://` strings are recognized): string values of table fields at any depth under an entry, excluding `description` keys, the entire reserved `inject` table, and array elements. A string inside an array is always ordinary data, never a reference (SPEC-AS-015). This scope governs reference recognition and injection only — the sensitive-field-name rule (SPEC-020) uses its own, broader traversal.

## Command–Profile Matrix

| Command | Requires active profile | Notes |
| --- | --- | --- |
| `list`, `list <entry>`, `show`, `get`, `find` | Yes | resolution order per SPEC-004 |
| `find --all-profiles` | No | searches every profile; `--profile` accepted, unused |
| `list --profiles` | No | this is the recovery command when profile resolution fails |
| `validate` | No | validates the whole file |
| `credential list` / `check` / `set` | No | credentials are global (top-level `credentials` table) |
| `run` | Yes | entries are looked up in the active profile |

`--profile` is a global flag: commands in the "No" rows accept it and do not use it (documented in `--help` per command).

## Phase Map

| Phase | Name | Priority | Objective | Depends on | Independent test |
| --- | --- | --- | --- | --- | --- |
| Phase 1 | Config core & queries | P1 (MVP) | Load/validate config, select profile, run every query command (including `credential list`) with text+JSON and exit codes 0/1/2/3. Shallow credential status is a Phase-1 free function over credential metadata (`getenv` + executable-discovery only); the `Provider` trait (resolve/store) does not exist until Phase 2, which absorbs the function — no unimplemented trait methods at any point | None | Against fixture TOML files: every query command returns the documented output and every Phase-1 error path (exit 1/2/3) returns its code; a canary proves no external process ran |
| Phase 2 | Credential providers | P2 | `Provider` seam with env/keychain/command adapters; `credential check` / `set`; exit 4 paths; `Secret` no-leak type; test-gated keychain store seam | Phase 1 | With the test-gated store, a fake command script, and env vars: `check` reports success/failure per provider, `set` round-trips exact bytes, sentinel secrets never appear in agent-context's output |
| Phase 3 | Injection runner | P3 | `run --with` builds a conflict-checked injection plan and launches the target transparently; exit 127 path; README (SPEC-022) | Phase 1, 2 | A probe target dumps its env to a file: injected names present with exact values, precedence and conflict rules observed, stdio/exit codes pass through |

## Requirements

### SPEC-001: Config file location

The CLI MUST read its configuration from, in priority order — on Unix: the `AGENT_CONTEXT_FILE` environment variable if set; else `$XDG_CONFIG_HOME/agent-context/context.toml` if `XDG_CONFIG_HOME` is set to an absolute path (an empty or relative value is treated as unset, per the XDG spec); else `~/.config/agent-context/context.toml`. On Windows: `AGENT_CONTEXT_FILE` if set, else `%APPDATA%\agent-context\context.toml` (`XDG_CONFIG_HOME` is not consulted on Windows, per design §3). An environment variable set to the empty string (`AGENT_CONTEXT_FILE=""`, `AGENT_CONTEXT_PROFILE=""`) is treated as unset; `--profile ""` is a usage error (exit 1). A config file that exists but cannot be read (e.g. EACCES) exits 2 naming the path and error. If the home/config base directory cannot be determined (e.g. `HOME` unset and no override), exit 2 naming the missing variable. (§3)

Source trace: PRD-FR-001.

Acceptance criteria:

- AC-001.1: GIVEN `AGENT_CONTEXT_FILE=/tmp/x.toml` pointing at a valid file, WHEN any query command runs, THEN that file is used even if a default-path file exists.
- AC-001.2: GIVEN no config file at the resolved path, WHEN any command runs, THEN exit 2 with a message containing the resolved path.
- AC-001.3: GIVEN `HOME` and `AGENT_CONTEXT_FILE` both unset (Unix test harness), WHEN `list` runs, THEN exit 2 naming `HOME` or `AGENT_CONTEXT_FILE` as remedies.
- AC-001.4: GIVEN `XDG_CONFIG_HOME=relative/dir`, WHEN `list` runs, THEN the variable is ignored and `~/.config/...` is used.

Verification: Automated: integration tests with temp dirs and scrubbed env.

### SPEC-002: Strict core validation on load

Every command MUST refuse to operate on a config that fails core validation, reporting **all** violations (not only the first), each with its config path, and exiting 2. Core rules (§8):

1. `version` is required and is a supported integer (only `1`); its absence (including an empty file) is a violation.
2. `default_profile`, when present, names an existing profile.
3. Every profile and every entry has a non-empty string `description`.
4. Every credential definition: its table key matches `[A-Za-z0-9_-]+`; it has non-empty string `description` and `inject_as`; `provider` ∈ {`env`, `keychain`, `command`} (any other value is a violation); provider-specific required fields are present, correctly typed, and non-empty (`env`: `name`, itself a valid environment variable name; `keychain`: `service`, `account`; `command`: `argv`, a non-empty array of strings whose first element is non-empty); no other fields exist in a credential definition (closed schema, SPEC-AS-021).
5. Every recognized `credential://` reference (see Reference scanning scope) satisfies the strict grammar of SPEC-012 and resolves to a defined credential.
6. `inject_as`, `?as=` values, and `inject`-table keys are valid environment variable names (`[A-Za-z_][A-Za-z0-9_]*`).
7. Reserved keys: at entry level, `inject` (if present) MUST be a table satisfying SPEC-013; an entry-level `inject` of any other TOML type is a violation. An empty `inject` table is valid and injects nothing. Tables nested deeper than entry level may freely use the names `inject` and `description` as ordinary fields.
8. Sensitive-field-name rule: SPEC-020, applied over SPEC-020's **own** traversal scope (deliberately broader than reference scanning; SPEC-020 is the sole authority for that scope) — so a config with a suspected plaintext secret is unusable by every command.
9. Besides `description`, every direct key of a profile is a table (its entries); profile-level scalars and arrays are violations naming the key.
10. The root schema is closed: the only top-level keys are `version`, `default_profile`, `profiles`, and `credentials`; any other top-level key (e.g. the typo `defualt_profile` or `[credential]`) is a violation naming the key.
11. Core container types: `default_profile` is a string; `profiles` and `credentials`, when present, are tables; every member of each is a table. A mistyped container (e.g. `profiles = []`, `credentials = "x"`) is a core validation violation, not a parse error: the file is parsed generically first, then all core rules are evaluated and aggregated, so mistyped containers are collected and reported alongside every other violation.

An absent `profiles` table (zero profiles) and an absent `credentials` table are both valid. Diagnostics rule: parse and validation diagnostics MUST name config paths and (for parse errors) line/column positions, and MUST NOT echo config source lines or the values of open-schema profile-tree fields (which may hold a plaintext secret). Values of the closed credential schema (credential names, `name`/`service`/`account` values, `inject_as`, and `argv[0]` — never the full `argv`, which can carry vault item paths) and `credential://` reference strings are non-secret config metadata and MAY appear in messages — design §9's own error examples require this (SPEC-019).

Source trace: PRD-FR-006, PRD-NFR-001.

Acceptance criteria:

- AC-002.1: GIVEN a config whose only defect is `profiles.work.llm` lacking `description`, WHEN `list` runs, THEN exit 2 and the message contains `profiles.work.llm`.
- AC-002.2: GIVEN `version = 2`, WHEN any command runs, THEN exit 2 with a message naming the supported version.
- AC-002.3: GIVEN an entry field `credential = "credential://missing"`, WHEN `list` runs, THEN exit 2 and the message contains `missing`.
- AC-002.4: GIVEN a config with three independent violations (missing description, unknown provider `vault`, entry-level `inject = "x"`), WHEN `get llm.endpoint` runs, THEN exit 2 and all three config paths appear in the error output.
- AC-002.5: GIVEN `api_key = "sk-live-123"` in an entry, WHEN `get <that entry>.api_key` runs, THEN exit 2 (the value is never printed) with the SPEC-020 message.
- AC-002.6: GIVEN `[credentials."my cred"]` (key with a space), a credential with `extra = 1`, and an `env` credential with `name = "1BAD"`, WHEN `validate` runs, THEN each is reported and exit is 2.
- AC-002.7: GIVEN `region = "eu"` directly under `[profiles.work]`, WHEN `list` runs, THEN exit 2 naming `profiles.work.region`.
- AC-002.8: GIVEN a syntactically valid file with `profiles = []` AND a credential missing `inject_as`, WHEN `validate` runs, THEN both violations are reported together and exit is 2 (mistyped containers aggregate with other violations).

Verification: Automated: table-driven validation tests.

### SPEC-003: Open schema

Users MUST be able to add arbitrary entries and fields (strings, integers, floats, booleans, datetimes, arrays, sub-tables) under a profile; new fields appear in `list`, `show`, `get`, and `find` with no CLI change. (§4.2)

Source trace: PRD-FR-001.

Acceptance criteria:

- AC-003.1: GIVEN a fixture entry with one field of each TOML type plus a nested sub-table, WHEN `list <entry>` and `show <entry>` run, THEN every field (including nested ones, with dotted paths) appears with its correct type label.
- AC-003.2: GIVEN the same fixture, WHEN `get <entry>.<nested>.<field>` runs, THEN the stored value is returned.

Verification: Automated: fixture round-trip tests.

### SPEC-004: Profile selection

The active profile MUST be resolved as: `--profile` flag, else `AGENT_CONTEXT_PROFILE` env var, else `default_profile` from the file. If the result names no existing profile, or nothing is configured, commands requiring a profile (see Command–Profile Matrix) MUST exit 3 listing available profile names and suggesting `agent-context list --profiles`. Note: a dangling `default_profile` never reaches this path — SPEC-002 rule 2 makes it a load-time exit 2 for every command, and that exit-2 message carries the config path to edit as its remedy; the exit-3 path therefore applies to flag/env-var selections and to the nothing-configured case. (§4.1)

Source trace: PRD-FR-002.

Acceptance criteria:

- AC-004.1: GIVEN `default_profile = "work"` and `AGENT_CONTEXT_PROFILE=personal`, WHEN `list` runs, THEN the `personal` profile is listed.
- AC-004.2: GIVEN the same, WHEN `list --profile work` runs, THEN the `work` profile is listed (flag beats env var).
- AC-004.3: GIVEN `AGENT_CONTEXT_PROFILE=nope`, WHEN `list` runs, THEN exit 3 and the message lists the defined profile names; WHEN `list --profiles` runs, THEN exit 0 (recovery path works without an active profile).

Verification: Automated: env-permutation tests.

### SPEC-005: Path grammar

One segment grammar serves every path-shaped input: `get` paths, the entry-name argument of `list <entry>` / `show <entry>` / `run --with` (a single segment), and `inject`-table values (paths relative to the entry, one or more segments deep). Paths are dot-separated segments; for `get`, the first segment is an entry name. An unquoted segment is one or more characters other than `.`, `"`, and whitespace. A segment MAY instead be fully quoted to contain dots or spaces: a quoted segment is `"` + one or more characters except `"` + `"`, occupying the whole segment. Keys containing a double quote, and keys that are the empty string, are not addressable in v1 (SPEC-AS-010); empty segments, empty quoted segments, and partial quoting are grammar errors. A multi-segment path supplied where a single entry name is expected (`list llm.endpoint`, `run --with llm.endpoint`) is a usage error (exit 1) — it can never be a name. No escape sequences exist. Documentation examples MUST show shell-safe invocation (single-quoted argument: `agent-context get 'server."my.field"'`). An unknown path MUST exit 3 with a message naming the failing path and suggesting `agent-context list <entry>`. (§5.1; design §5.1's mention of `show`/`find` paths is narrowed to `show <entry>` and `find <needle>` — recorded as SPEC-AS-023.)

Source trace: PRD-FR-003.

Acceptance criteria:

- AC-005.1: GIVEN a field literally named `my.field` under entry `server`, WHEN the CLI receives the argument `server."my.field"` (argv-level, no shell), THEN its value is returned.
- AC-005.2: GIVEN entry `llm` without field `region`, WHEN `get llm.region` runs, THEN exit 3 and the message contains `llm.region` and `list llm`.
- AC-005.3: GIVEN an unterminated quote, an empty segment (`llm..x`), or a partially quoted segment (`a"b".c`), WHEN `get` runs, THEN exit 1 with a grammar error message.

Verification: Automated: unit tests on the path parser + CLI integration via argv.

### SPEC-006: `list`

`list` MUST open with a header naming the active profile and its description, then show every entry with its description and each of the entry's **top-level** fields with its name and type label — nested sub-tables (including `inject`) appear as a single field of type `table` without recursion. `list <entry>` MUST show one entry and recurse into nested tables using dotted paths, including `inject` members as dotted fields (a structure view; `show`'s `←` rendering is the value view — the divergence is deliberate). Reserved `description` keys are metadata, never field rows (Definitions). `list --profiles` MUST show every profile with its description and mark the default; with zero profiles defined it prints an explicit "no profiles" line, exit 0. Credential-reference fields show the type label `credential reference`, the credential name, and shallow status per SPEC-012. All listings follow config-file order (SPEC-021). (§5.2)

Source trace: PRD-FR-003.

Acceptance criteria:

- AC-006.1: GIVEN the design document's example config, WHEN `list` runs, THEN output contains entries `llm`, `ci`, `kubernetes` with their descriptions, top-level field names, and type labels, and the `llm.inject` table appears as one `table` line without its members.
- AC-006.2: WHEN `list --profiles` runs, THEN output contains `work` marked as default and `personal`.
- AC-006.3: GIVEN `list nosuch`, THEN exit 3 listing available entry names.
- AC-006.4: WHEN `list llm` runs, THEN `inject.OPENAI_BASE_URL` and `inject.OPENAI_MODEL` appear as dotted-path fields.

Verification: Automated: golden-output integration tests (required tokens per line).

### SPEC-007: `show`

`show <entry>` MUST print the entry description and every field recursively with dotted paths and values, with two exceptions: credential references show only the credential name and shallow status; the `inject` table is rendered once, as `ENVNAME ← field.path` pairs (not additionally as dotted fields). Arrays render as their JSON value encoding on one line; non-`inject` nested tables render as their fields with dotted paths. `show nosuch` exits 3 listing available entry names. (§5.3)

Source trace: PRD-FR-003, PRD-NFR-001.

Acceptance criteria:

- AC-007.1: GIVEN the example config, WHEN `show llm` runs, THEN output contains the description, `endpoint` with its URL, `model` with its value, and the credential shown as its name plus status — and the output does not contain any resolved secret value.
- AC-007.2: GIVEN an entry with an `inject` table, WHEN `show` runs, THEN each mapping appears exactly once, as `ENVNAME ← field.path`.
- AC-007.3: GIVEN an entry with an array field, WHEN `show` runs, THEN the array renders as a one-line JSON array.

Verification: Automated: integration tests.

### SPEC-008: `get`

`get <path>` on a scalar MUST print the scalar-to-string conversion of the value with a single trailing newline. On an array or table it MUST exit 1 directing the caller to `--json` (entries themselves are tables, so `get <entry>` follows this rule and the message also suggests `show <entry>`). A path resolving to a credential reference prints the stored reference string unchanged (including `?as=` if present), never the secret. (§5.4)

Source trace: PRD-FR-003, PRD-FR-004.

Acceptance criteria:

- AC-008.1: WHEN `get llm.endpoint` runs, THEN stdout is exactly the URL plus newline; exit 0.
- AC-008.2: WHEN `get ci.tags` runs without `--json`, THEN exit 1 and the message mentions `--json`.
- AC-008.3: WHEN `get llm.credential` runs, THEN stdout is exactly the stored reference string (e.g. `credential://company_llm`) plus newline.
- AC-008.4: GIVEN boolean, integer, finite-float, non-finite-float, offset-datetime, and local-date fixture fields, WHEN `get` runs on each, THEN output follows the scalar-to-string conversion rules (`true`, `42`, `1.5`, `inf`, TOML lexical datetime forms).

Verification: Automated: byte-exact stdout assertions.

### SPEC-009: `find`

`find <needle>` MUST case-insensitively substring-match, within the active profile: entry names, field names (the bare segment name, at any depth), descriptions, and string-scalar field values (which includes credential reference strings; reference strings are config data, not secrets — SPEC-019 is unaffected). Array elements, non-string scalars, and the reserved `inject` table's members (its keys are env names, its values are paths — machinery, not data) are not matched; the `inject` table itself can match by its own field name, as any table field can. An empty needle is a usage error (exit 1).

Match output: entry match → path + description; string/scalar field match → path + scalar-to-string value; credential-reference field → path + the full stored reference string + shallow status (so the needle stays visible, matching design §5.5's `credential://company_llm` example); a name-match on an array or table field → path + type label only. On zero matches in text mode: stdout empty, stderr `No matches for '<needle>'`, exit 0; with `--json` the stderr line is omitted and stdout is the envelope with an empty `matches` array. Profile names and profile descriptions are not part of the match domain (profiles are labels; the Definitions' description dimension means entry descriptions here). A path that matches on several dimensions (e.g. name and value) yields exactly one match; matches follow config traversal order. `--all-profiles` widens the search to every profile, labels each match with its profile, and requires no active profile. (§5.5; value matching is what reproduces §5.5's example output, which includes `llm.endpoint` by its URL value and excludes `llm.model`.)

Source trace: PRD-FR-003.

Acceptance criteria:

- AC-009.1: WHEN `find llm` runs on the example config, THEN matches are exactly: the `llm` entry, `llm.endpoint` (value contains "llm"), and `llm.credential` (reference string contains "llm") — and `llm.model` is not a match.
- AC-009.2: WHEN `find LLM --all-profiles` runs with no `default_profile` configured, THEN matches from both `work` and `personal` appear, each labeled with its profile, exit 0.
- AC-009.3: WHEN `find zzz-no-match` runs, THEN stdout is empty, stderr contains `No matches`, exit 0; with `--json`, stdout is an envelope with an empty `matches` array.
- AC-009.4: WHEN `find inject` runs on the example config, THEN the `llm.inject` table field matches by name and prints path + `table` with no member values.
- AC-009.5: WHEN `find ""` runs, THEN exit 1 usage error.

Verification: Automated: integration tests.

### SPEC-010: JSON output contract

`list`, `list <entry>`, `list --profiles`, `show`, `get`, `find`, and `credential list` accept `--json`; JSON is the only stdout content. `validate` does not take `--json` in v1 (SPEC-AS-014). When a `--json` invocation fails, stdout is empty and the error is text on stderr with the normal exit code (SPEC-AS-014). Two output classes exist:

**Raw value** — `get <path> --json` prints exactly the JSON value encoding of the addressed value: scalar → JSON scalar per the Definitions encoding, array → JSON array, table → JSON object rendering the stored data verbatim and recursively (nested credential references stay raw `credential://…` strings; reserved `description`/`inject` keys are included as stored), credential reference → JSON string of the stored reference. No envelope, no `version` — a recorded deviation from the envelope rule and from PRD-NFR-003's version clause, adopting design §5.4's raw-retrieval reading over §5.9's envelope wording (SPEC-AS-022).

**Envelope** — every other JSON-producing command prints one object carrying `"version"` (config version) plus:

- `list --json`: `{"version", "profile", "profile_description", "entries": [{"name", "description", "fields": [Field...]}]}` (the description an agent needs for discovery travels in the same call).
- `list <entry> --json` and `show <entry> --json` are aliases emitting one identical entry envelope (deliberately on the record): `{"version", "profile", "name", "description", "fields": [Field...]}`.
- `list --profiles --json`: `{"version", "profiles": [{"name", "description", "default": bool}]}`.
- `find --json`: `{"version", "matches": [Match...]}` where `Match = {"profile", "path", "kind": "entry"|"field"}` plus, by match type: entry → `"description"`; scalar field → `"type"`, `"value"`; credential-reference field → `"type": "credential_ref"`, `"reference"` (the stored reference string), `"credential"` (a Credential summary object, carrying the shallow status the text form shows); array/table name-match → `"type"` only.
- `credential list --json`: `{"version", "credentials": [Credential summary object + "description" + "inject_as"]}` (the default target env name is programmatically discoverable here; per-reference `?as=` overrides are visible through `Field.reference`).

`Field` is recursive: scalar → `{"path", "type", "value"}` (value per the JSON value encoding); array → `{"path", "type": "array", "value": <JSON array>}`; credential reference → `{"path", "type": "credential_ref", "reference": <the stored reference string>, "credential": <Credential summary object>}` (no `value`; `?as=` is visible through `reference`, unresolved); table → `{"path", "type": "table", "fields": [Field...]}` with members **nested inside** the table's `fields` array, not as siblings (the nested `fields` array is the recorded reading of design §5.9's "value" for tables). Every `Field.path` and `Match.path` is the full dotted path starting at the entry name, rendered in the SPEC-005 grammar so `get` accepts it verbatim. Unaddressable keys (empty or quote-bearing, SPEC-AS-010/-024): a field whose own key — or any ancestor key — the grammar cannot render carries `"path": null`, `"key": <the raw key text as a JSON string>`, and `"addressable": false`; addressable fields carry none of `key`/`addressable`. `Match` uses the same three members. An entry whose *name* is unaddressable appears in profile-level `list`/`list --json` (with the same markers) and is not reachable via `list <entry>`/`show`/`get`/`run --with`. Reserved `description` keys never appear in `fields` arrays (Definitions). `type` ∈ `string|integer|float|boolean|datetime|array|table|credential_ref`; `status` ∈ `available|not_set|configured|command_missing`. In envelopes, `fields` arrays are fully recursive regardless of the text command's display depth (deliberate divergence from text `list`'s top-level-only view). These shapes are a frozen compatibility contract; changes must be additive. (§5.9)

Source trace: PRD-NFR-003, PRD-NFR-001.

Acceptance criteria:

- AC-010.1: WHEN `list --json` runs on the example config, THEN output parses, carries `version: 1` and `profile: "work"`, the `llm.credential` field object has `credential.{name,provider,status}` and no `value`, and the `llm.inject` field is a `table` Field whose members nest inside its `fields` array.
- AC-010.2: WHEN `get ci.tags --json` runs, THEN stdout is exactly the bare JSON array; WHEN `get llm.credential --json` runs, THEN stdout is the JSON string of the reference.
- AC-010.3: Snapshot tests lock all six shapes (raw value + five envelopes); any shape change fails the suite.
- AC-010.4: WHEN `list llm --json` and `show llm --json` run on the same config, THEN their stdout is byte-identical (alias equality).

Verification: Automated: JSON snapshot tests.

### SPEC-011: `validate`

`validate` MUST run all SPEC-002 core rules plus the Unix file-permission check: the file mode's permission bits must be a subset of `0600` (owner read/write only — group/other bits and **all execute bits** fail; `0600` and `0400` pass; `0601`, `0610`, `0644`, `0700` fail). It MUST report every failure with its config path, exiting 2 on any failure, 0 when clean. `validate` requires no active profile and is a query command (no resolution). (§5.7, §3)

Source trace: PRD-FR-006.

Acceptance criteria:

- AC-011.1: GIVEN a config with two independent violations, WHEN `validate` runs, THEN both are reported and exit is 2.
- AC-011.2: (Unix) GIVEN a structurally valid config with mode 0644, WHEN `validate` runs, THEN exit 2 and the failure names the file and `0600`; same for mode 0700.
- AC-011.3: (Unix) GIVEN a structurally valid 0600 config, THEN exit 0. (The permission check is Unix-only; AC-011.1 is platform-neutral.)

Verification: Automated: integration tests with `chmod` in temp dirs.

### SPEC-012: Credential references and shallow status

Within the Reference scanning scope (Definitions), any string value beginning with `credential://` MUST parse against the exact grammar `credential://<name>` or `credential://<name>?as=<ENV>` where `<name>` matches `[A-Za-z0-9_-]+` and `<ENV>` is a valid env name. Anything else beginning with `credential://` — empty name, unknown or duplicated query parameters, wrong-case `?As=`, trailing garbage — is a load-time violation (exit 2, SPEC-002 rule 5): a malformed reference is never silently treated as an ordinary string and never silently falls back to the credential's `inject_as`.

Query commands MUST display references with a shallow status computed without resolving: `env` → `available`/`not_set` via variable presence; `keychain` → `configured` (no store read); `command` → `configured`/`command_missing` via platform-appropriate executable discovery without launching anything: when `argv[0]` contains a path separator, the path (relative paths against the current working directory) must be a regular file that is executable; otherwise it is resolved through the platform's `PATH` search. Executability means direct-launch semantics matching SPEC-014's no-shell execution: Unix — any execute bit; Windows — only extensions the direct process API launches without an interpreter (`.exe`, `.com`); interpreter-dependent extensions (`.bat`, `.cmd`, `.ps1`) are NOT discoverable and NOT resolvable — a script provider must name its interpreter explicitly as `argv[0]` (e.g. `["pwsh", "-File", "get-token.ps1"]`). A directory or non-executable file is `command_missing`. (§4.3, §5.6)

Source trace: PRD-FR-004, PRD-NFR-001.

Acceptance criteria:

- AC-012.1: GIVEN a reference nested two tables deep, WHEN `list <entry>` runs, THEN it is displayed as a credential reference.
- AC-012.2: GIVEN a `command` credential whose `argv[0]` is a canary script that creates a file when executed, WHEN every query command runs, THEN status logic reports `configured` and the canary file does not exist afterward.
- AC-012.3: GIVEN an `env` credential with the variable set, THEN status is `available`; unset, `not_set`.
- AC-012.4: GIVEN `credential://company_llm?As=X`, `credential://`, and `credential://name?x=1` in three fixtures, WHEN any command runs, THEN each exits 2 citing the malformed reference.
- AC-012.5: GIVEN `tags = ["credential://company_llm"]` (array element), WHEN `validate` and `show` run, THEN the element is ordinary string data: not validated as a reference and rendered verbatim in the array. (The injection half of this scenario is AC-016.7, Phase 3.)

Verification: Automated: canary no-execution test; grammar table test.

### SPEC-013: `inject` tables

An entry MAY contain a reserved `inject` sub-table mapping environment variable names to field paths within the same entry. `inject` values are field paths (one or more segments, entry-relative, using the SPEC-005 segment grammar), never scanned as credential references. Beyond SPEC-002 rule 7 (must be a table), validation MUST reject: keys that are not valid env names; values that are not strings or do not parse under the segment grammar; paths that do not resolve within the entry; paths whose first segment is `inject` (the reserved table cannot be a source of itself); paths resolving to anything other than an injectable scalar (string/integer/float/boolean); string sources containing NUL (not representable in a process environment); paths resolving to credential references (credentials inject via `inject_as`/`?as=` only, message directs there). (§4.4)

Source trace: PRD-FR-005.

Acceptance criteria:

- AC-013.1: GIVEN `inject = { OPENAI_BASE_URL = "endpoint" }` where `endpoint` is a string, WHEN `validate` runs, THEN exit 0.
- AC-013.2: GIVEN inject values pointing at an array field and at a datetime field, WHEN `validate` runs, THEN exit 2 naming each inject key and path.
- AC-013.3: GIVEN an inject value pointing at a credential-reference field, WHEN `validate` runs, THEN exit 2 with a message directing to `inject_as`.
- AC-013.4: GIVEN `inject = { A = "inject.B", B = "endpoint" }` (self-referential path) and a string source field containing NUL, WHEN `validate` runs, THEN exit 2 naming each offending inject key and path, and a canary proves no provider ran.
- AC-013.5: GIVEN `inject = { A = 'nested."my.key"' }` where `nested."my.key"` is a string field, WHEN `validate` runs, THEN exit 0 (multi-segment quoted inject paths work).

Verification: Automated: validation tests.

### SPEC-014: Credential providers

Every provider yields values in the credential value domain (Definitions); a value outside it (empty, NUL-bearing, or not valid UTF-8 — e.g. a non-UTF-8 environment variable or store item) is a resolution failure. `env` resolution reads the named variable, failing when unset. `keychain` resolution reads the platform credential store (macOS Keychain / Windows Credential Manager / Linux secret-service) by `service` + `account`, failing when the item is missing; on systems with no usable store the error names the provider and suggests `command`/`env`. `command` resolution executes `argv` directly via the OS process API — agent-context itself never constructs a shell invocation or passes the command through a shell; the content of a user-authored `argv` is the config author's choice and is not policed. The provider command's stdout is captured as the secret (exactly one trailing `\n` or `\r\n` stripped); stdin and stderr are inherited so interactive password-manager auth can complete; resolution fails on non-zero exit or a captured value outside the credential value domain (whitespace-only counts as empty). Resolution failure is always explicit (exit 4 at CLI level) and never substitutes another credential. (§6.1, §10)

Source trace: PRD-FR-004.

Acceptance criteria:

- AC-014.1: GIVEN `COMPANY_LLM_TOKEN` unset, WHEN `credential check company_llm` runs, THEN exit 4 and the message names the variable without printing any value.
- AC-014.2: GIVEN a fake command-provider script printing a sentinel secret, WHEN `credential check` runs, THEN success is reported and the sentinel does not appear in agent-context's stdout or stderr.
- AC-014.3: GIVEN a command provider whose `argv` includes the literal argument `$HOME`, and a script that records its argv, WHEN resolved, THEN the script receives the literal string `$HOME` (no shell expansion occurred).
- AC-014.4: GIVEN the test-gated keychain store containing the item, WHEN `check` runs, THEN success; with the item absent, exit 4 naming service and account.
- AC-014.5: GIVEN command provider scripts that (a) print a distinct sentinel then exit 1, (b) print only `\n`, (c) print a sentinel embedded in invalid UTF-8, WHEN each is checked, THEN exit 4 with distinct messages (non-zero exit / empty output / invalid value) and no sentinel appears in agent-context's output (feeds AC-019.1).

Verification: Automated: test-gated store; fixture scripts.

### SPEC-015: `credential list` / `check` / `set`

`credential list` (Phase 1) MUST print every credential definition with its description, provider type, default target (`inject_as`), and shallow status (query command — no resolution), and supports `--json` (shape in SPEC-010). `credential check <name>` MUST perform a real resolution and report success or the concrete failure reason, never printing the secret; an undefined name exits 3 listing defined credential names (`credential set <name>` treats an undefined name identically). `credential set <name>` MUST work only for `keychain` credentials: with a TTY it prompts and reads without echo; without a TTY it reads stdin to EOF, requiring valid UTF-8 without NUL (else exit 1) and stripping exactly one trailing `\n`/`\r\n`; an empty value (after stripping) exits 1. The exact remaining bytes are stored. Store write failures exit 4 naming the platform error. For `env`/`command` credentials it exits 1 explaining those are externally managed. (§5.8)

Source trace: PRD-FR-007.

Acceptance criteria:

- AC-015.1: WHEN `credential list` runs on the example config, THEN both credentials appear with provider labels and statuses, and a command-provider canary proves no resolution ran.
- AC-015.2: GIVEN the piped value `hunter2\n`, WHEN `credential set openai_personal` runs against the test-gated store, THEN the stored bytes are exactly `hunter2`, a following `check` succeeds, and no output channel of agent-context ever contained the value.
- AC-015.3: WHEN `credential set company_llm` (env provider) runs, THEN exit 1 and the message says env credentials are managed externally.
- AC-015.4: WHEN `credential check nosuch` runs, THEN exit 3 listing the defined credential names.
- AC-015.5: (Unix, Phase 2) GIVEN a pseudo-terminal driving `credential set` interactively, WHEN the value is typed, THEN the typed characters are not echoed to the terminal stream and the value round-trips via `check`.

Verification: Automated: test-gated store round-trip with byte assertion.

### SPEC-016: `run --with` injection plan

`run --with <entry>... -- <cmd> [args...]` MUST: require at least one `--with` and a command after `--` (else exit 1 usage error showing the expected form); resolve each named entry in the active profile (exit 3 if absent); collect the entry's credential references (per the Reference scanning scope) and evaluate its `inject` table (scalar-to-string conversion); and build the full target-env mapping BEFORE resolving any secret.

Injection identity and conflicts: environment-variable name identity is ASCII case-insensitive on Windows (the platform's environment semantics) and case-sensitive elsewhere; original spelling is preserved for injection and diagnostics. A credential injection's identity is its **effective pair** (credential name, target env name after applying `?as=` or `inject_as`); an `inject`-table injection's identity is (entry name, inject key). Identical effective pairs — from repeated `--with` entries, from multiple references across entries, or from multiple references within a single entry — deduplicate to one injection (recorded refinement of design §6.2's conflict rule: SPEC-AS-012/-018). Any two injections with **different** identities targeting the same env name (under the platform name identity) fail with exit 4, naming both sources (credential names and/or `entry.inject` keys), without launching the target or resolving any provider. One credential referenced under several distinct effective pairs (e.g. `credential://c` → `X` and `credential://c?as=Y`) is resolved **once** and injected under each target name. Precedence: injected variables override same-named variables inherited from agent-context's own environment; inherited variables are never conflicts. Resolved secrets are placed **only** in the target's constructed environment map, never exported into agent-context's own process environment — so one credential's value can never leak into another provider's inherited environment. An entry with no references and no `inject` table injects nothing and still runs the target. (§6.2)

Source trace: PRD-FR-005.

Acceptance criteria:

- AC-016.1: GIVEN entry `llm` (credential → `OPENAI_API_KEY`, inject → `OPENAI_BASE_URL`, `OPENAI_MODEL`), WHEN `run --with llm -- <probe>` runs, THEN the probe's env contains those three names with the expected values (assertion restricted to the injected names).
- AC-016.2: GIVEN two `--with` entries whose distinct credentials both target `OPENAI_API_KEY`, WHEN `run` executes, THEN exit 4, the message names both credentials, and a canary proves no provider command ran.
- AC-016.3: GIVEN `credential://company_llm?as=LLM_API_KEY`, WHEN `run --with llm -- <probe>` runs, THEN the probe sees `LLM_API_KEY` and no injected `OPENAI_API_KEY`.
- AC-016.4: GIVEN `OPENAI_API_KEY=inherited` in agent-context's own env and entry `llm` injecting it, WHEN `run` executes, THEN the probe sees the injected value, not `inherited`.
- AC-016.5: GIVEN one entry referencing `credential://c` (whose `inject_as` is `X`) and another referencing `credential://c?as=X`, WHEN `run --with a --with b -- <probe>` executes, THEN the effective pairs are identical: no conflict, the value injected once.
- AC-016.6: WHEN `run -- true` (zero `--with`) runs, THEN exit 1 usage error.
- AC-016.7: GIVEN `tags = ["credential://company_llm"]` (array element) in entry `ci`, WHEN `run --with ci -- <probe>` executes, THEN nothing is injected from the array element and no provider ran (completes AC-012.5's scenario).
- AC-016.8: GIVEN one entry with `credential://c` (whose `inject_as` is `X`) and another with `credential://c?as=Y`, WHEN `run` executes with a counting command-provider fixture, THEN the provider ran exactly once and the probe sees both `X` and `Y` with the same value.
- AC-016.9: A table-driven injection-plan matrix covers every source pairing: credential-vs-credential (conflict), credential-vs-inject (conflict), inject-vs-inject across entries targeting one name (conflict), identical effective pairs across and within entries (dedup), and — on Windows only — a case-variant name pair (`OPENAI_API_KEY` vs `openai_api_key`) detected as a conflict.

Verification: Automated: probe writing its env to a temp file.

### SPEC-017: `run` process transparency

After a successful injection plan, the target's stdio MUST be the caller's stdio, uncaptured and unbuffered, and the exit status observed by the caller MUST be the target's own. Platform semantics: on Unix the CLI replaces itself with the target (`exec`), so signal delivery and exit/signal status (observed as 128+N by the shell) are structural; on Windows the CLI spawns the target in the same console (sharing console control events, so Ctrl-C reaches the target), waits, and exits with the target's exit code — design §6.2's SIGINT/SIGTERM forwarding is POSIX-scoped, and the Windows console-group behavior is the recorded platform equivalent (SPEC-AS-013). Pre-launch failures use SPEC-018 exit codes; a target that cannot be executed exits 127 naming the command. agent-context writes nothing to stdout/stderr on the happy path. (§6.2)

Source trace: PRD-FR-005.

Acceptance criteria:

- AC-017.1: WHEN `run --with ci -- <helper emitting "out" on stdout, "err" on stderr, exiting 7>` runs, THEN stdout is `out`, stderr is `err`, exit code 7, no wrapper output (helper is a cross-platform test binary).
- AC-017.2: WHEN the target does not exist, THEN exit 127 and the message names the missing command.
- AC-017.3: (Unix) WHEN the target kills itself with SIGTERM, THEN the termination is observed as signal 15 (via `ExitStatusExt::signal()` in the test, equivalently `$?` = 143 in a shell — signal-terminated processes report no plain exit code).

Verification: Automated: integration tests with a compiled helper; signal case Unix-only.

### SPEC-018: Exit codes and error messages

The CLI MUST use: 0 success; 1 usage/argument errors (bad flags, path grammar errors, empty `find` needle, `get` non-scalar without `--json`, `credential set` on non-keychain or invalid/empty value, `run` without `--with`/command); 2 config-file errors (missing file/base dir, parse, any SPEC-002 violation, permission check); 3 name-resolution failures (unknown profile, entry, path, or credential name); 4 credential resolution failure, store write failure, or injection conflict; 127 target-not-executable (`run` only). Every error message MUST name the failing thing and a next action (a command to run or a config path to edit), in English. (§9)

Source trace: PRD-NFR-002.

Acceptance criteria:

- AC-018.1: A table-driven test exercises at least one failure per exit code available in the phase under test (Phase 1: 1, 2, 3; Phase 2 adds 4; Phase 3 adds 127, injection-conflict 4, and provider-resolution failure reached through `run`) and asserts code plus a required message token.
- AC-018.2: No error path prints a secret value (enforced suite-wide by AC-019.1).

Verification: Automated.

### SPEC-019: No-secret invariant (security)

No output that agent-context itself writes — stdout, stderr, or any file, on any command, in success or failure — may contain a resolved secret value; v1 writes no log files at all (SPEC-AS-020). One authorized exception: `credential set` writes the secret to the platform credential store (and, in test-feature builds only, to the test-gated backing file) — that persistence is the command's purpose; the prohibition binds every other file and channel. Diagnostics follow the SPEC-002 diagnostics rule: never config source lines, never open-schema field values (a parse error on a line containing a plaintext token must not reproduce that token); closed credential-schema metadata and reference strings may appear (for `argv`, messages cite `argv[0]` only). **Candidate credential bytes captured from any provider MUST NOT appear in diagnostics whether or not resolution succeeded** — a `command` provider's captured stdout is covered even when the provider then exits non-zero or the value fails domain validation. `credential check` and `run` resolve secrets but never print them.

Enforcement boundary: channels owned by external processes that agent-context launches (the `run` target's stdio, which on Unix is agent-context's own stdio after `exec`; a `command` provider's inherited stderr and stdin) are outside this invariant — they belong to the external tool and are covered by the documented threat model (Scope). Test fixtures (probe targets, provider scripts) MUST therefore never print sentinel values to inherited channels, so the suite-wide grep of AC-019.1 measures exactly agent-context's own output. The invariant is enforced structurally (secret type without `Display`/`Serialize`, per ARCH-005) and by tests. (§10)

Source trace: PRD-NFR-001.

Acceptance criteria:

- AC-019.1: The shared test invocation helper asserts, on **every** agent-context invocation it captures, that no planted sentinel secret value (distinct high-entropy strings per provider fixture) appears in stdout/stderr — a per-invocation check inside the helper rather than a separate aggregator test, so `cargo test` parallelism cannot skip it; fixture scripts and probes are authored to keep sentinels off inherited channels.
- AC-019.2: (Phase 2) The secret type implements neither `Display` nor `Serialize`, and its `Debug` prints a redaction marker (unit-asserted).
- AC-019.3: GIVEN a config file with a TOML syntax error on a line containing a sentinel value, WHEN any command runs, THEN exit 2 and the sentinel does not appear in any output; the message carries the line/column position only.

Verification: Automated: sentinel grep + unit test + malformed-TOML sentinel test.

### SPEC-020: Sensitive field names

Sensitive-name traversal covers **every table field at any depth under every profile, including tables nested inside arrays** (diagnostic paths use display-only index notation, e.g. `profiles.work.db.records[0].api_key`), with exactly one exclusion: the reserved entry-level `inject` table, whose keys are target env names — machinery, not values — so `inject = { GITHUB_TOKEN = "gh_field" }` is legal (SPEC-013 governs it; a sensitive-named inject key with a path value has no other legal spelling, since SPEC-013 forbids credential references as inject sources). The closed credential schema of SPEC-002 rule 4 covers the `credentials` tables. This traversal is deliberately broader than the Reference scanning scope — arrays stop reference recognition, never the secret guardrail. A field whose name (matched ASCII case-insensitively, so `TOKEN` and `Api_Key` count) equals `token`, `password`, `secret`, `api_key`, or `private_key`, or ends with `_token`, `_password`, `_secret`, `_api_key`, or `_private_key`, and holds a string that does not begin with `credential://` (the prefix reading: inside the Reference scanning scope such strings are additionally validated as references by SPEC-002 rule 5; outside it — e.g. in a table nested in an array — the prefix alone satisfies this guardrail and the string stays inert data), is a core validation violation (SPEC-002 rule 8; every command exits 2). Non-string values and names like `token_endpoint` are unaffected. (§8)

Source trace: PRD-NFR-001.

Acceptance criteria:

- AC-020.1: GIVEN `api_key = "sk-live-123"`, WHEN `validate` runs, THEN exit 2 naming the path and suggesting a `credential://` reference (and per AC-002.5, `get` refuses identically).
- AC-020.2: GIVEN `github_token = "credential://gh"` (defined credential), THEN valid.
- AC-020.3: GIVEN `token_endpoint = "https://x"` and `use_token = true`, THEN both valid.
- AC-020.4: GIVEN `[profiles.work.llm.extra]` containing `api_key = "sk-live-123"` (nested one level down), WHEN any command runs, THEN exit 2 naming `profiles.work.llm.extra.api_key`.
- AC-020.5: GIVEN `records = [{ api_key = "sk-live-123" }]` inside an entry, WHEN any command runs, THEN exit 2 naming `records[0].api_key` and the value does not appear in any output.
- AC-020.6: GIVEN `TOKEN = "sk-live-123"` (uppercase), WHEN `validate` runs, THEN exit 2 (case-insensitive match).

Verification: Automated: validation tests.

### SPEC-021: Presentation order

`list`, `show`, and every JSON array in SPEC-010 envelopes MUST present profiles, entries, and fields in config-file order. (ARCH-002; design §5.2 examples)

Source trace: PRD-FR-003.

Acceptance criteria:

- AC-021.1: GIVEN a fixture whose entries and fields are deliberately non-alphabetical, WHEN `list` and `list --json` run, THEN order matches the file, byte-asserted in JSON.

Verification: Automated: ordered fixture.

### SPEC-022: Agent usage documentation

The repository MUST ship a `README.md` containing: what the tool is, the config schema by example, the agent usage protocol of design §7 (the AGENTS.md snippet verbatim, plus the discover → inspect → get → `run --with` flow and the rule to report missing configuration explicitly rather than guessing), the threat-model boundary (SPEC-019), the provider guidance of design §6.1/§10 (prefer `keychain`/`command` locally; `env` credentials are readable by any process inheriting the environment), and a note that the sensitive-field-name check (SPEC-020) covers string fields with matching names only — it is a guardrail, not a scanner. (§7)

Source trace: PRD-FR-003 (adoption), spec-review SUG-003.

Acceptance criteria:

- AC-022.1: README exists and contains every SPEC-022 item: tool overview, config schema example, the §7 snippet's six bullet points, the discover → inspect → get → `run --with` flow with the no-guessing rule, a threat-model section, the provider guidance, the sensitive-check guardrail caveat, a note on target-name discovery (default `inject_as` via `credential list`; per-reference `?as=` via the JSON `reference` member or `get`), and a statement that Windows behavior is specified and code-reviewed but not machine-verified in v1 (SPEC-AS-025); verified by checklist inspection at validation.

Verification: Manual: inspection recorded in validation.md.

## Edge Cases

| ID | Case | Expected behavior | Verification |
| --- | --- | --- | --- |
| EDGE-001 | Profile with only `description` (no entries) | `list` prints the profile header and an explicit "no entries" line, exit 0 | integration |
| EDGE-002 | `AGENT_CONTEXT_FILE` points at a directory | exit 2, message names the path and that it is not a file | integration |
| EDGE-003 | Config file is empty (0 bytes) | exit 2: `version` missing | integration |
| EDGE-004 | `run --with a --with a` | identical to a single `--with a` | integration |
| EDGE-005 | `run` with no `--` separator or no command after it | exit 1 usage error showing the expected form | integration |
| EDGE-006 | Command provider prints only whitespace | resolution fails: "produced no output", exit 4 | integration |
| EDGE-007 | `?as=` with invalid env name (`?as=1BAD`) | load exits 2 naming the reference | unit |
| EDGE-008 | `find` needle matching a description only | the owning entry is a match | integration |
| EDGE-009 | Offset datetime, local date, local time fields | type label `datetime`; `get` prints the TOML lexical form of each | integration |
| EDGE-010 | `get <entry>` (no field part) | exit 1 directing to `--json` or `show <entry>` | integration |
| EDGE-011 | Provider value not valid UTF-8 / contains NUL | resolution fails naming the reason, exit 4 | integration (command provider); unit (env) |
| EDGE-012 | Test-gated keychain store write fails | `credential set` exits 4 naming the platform error | integration |
| EDGE-013 | `credential set` with empty piped stdin | exit 1: empty value | integration |
| EDGE-014 | `HOME` unset, no `AGENT_CONTEXT_FILE` (Unix) | exit 2 naming the remedy (SPEC-001) | integration |
| EDGE-015 | `--profile` passed to `validate` / `credential list` / `find --all-profiles` | accepted, unused; command behaves identically | integration |
| EDGE-016 | `[profiles]` and/or `[credentials]` absent entirely | valid config; `list --profiles` prints "no profiles"; profile-requiring commands exit 3 | integration |
| EDGE-017 | Float field `inf` / `nan` | `get` prints `inf`/`nan`; JSON encodes as strings `"inf"`/`"nan"` with type `float` | integration |
| EDGE-018 | Array element string `credential://x` | ordinary data (AC-012.5 / AC-016.7); never validated, displayed verbatim, never injected | integration |
| EDGE-019 | Unknown top-level key (`defualt_profile = "work"`) | exit 2 naming the key (SPEC-002 rule 10) | integration |
| EDGE-020 | `command` credential whose `argv[0]` resolves to a directory | shallow status `command_missing` | integration |

## Dependencies

| Requirement | Dependency | Reason |
| --- | --- | --- |
| SPEC-006..011, 015 (`credential list`), 021 | SPEC-002..005 | queries need a validated config, profile, and path grammar |
| SPEC-014, SPEC-015 (`check`/`set`) | SPEC-002, SPEC-012 | providers operate on validated credential definitions |
| SPEC-016, SPEC-017 | SPEC-012, SPEC-013, SPEC-014 | injection consumes references, inject tables, and providers |

## Acceptance Matrix

| Acceptance ID | Requirement | Phase | Verification method | Status |
| --- | --- | --- | --- | --- |
| AC-001.1–4 | SPEC-001 | 1 | integration | Draft |
| AC-002.1–8 | SPEC-002 | 1 | validation tests | Draft |
| AC-003.1–2 | SPEC-003 | 1 | fixture tests | Draft |
| AC-004.1–3 | SPEC-004 | 1 | env-permutation tests | Draft |
| AC-005.1–3 | SPEC-005 | 1 | parser unit + CLI argv tests | Draft |
| AC-006.1–4 | SPEC-006 | 1 | integration | Draft |
| AC-007.1–3 | SPEC-007 | 1 | integration | Draft |
| AC-008.1–4 | SPEC-008 | 1 | byte-exact stdout tests | Draft |
| AC-009.1–5 | SPEC-009 | 1 | integration | Draft |
| AC-010.1–3 | SPEC-010 | 1 | JSON snapshot tests | Draft |
| AC-011.1–3 | SPEC-011 | 1 | integration + chmod | Draft |
| AC-012.1–5 | SPEC-012 | 1 | canary + grammar tables | Draft |
| AC-013.1–5 | SPEC-013 | 1 | validation tests + canary | Draft |
| AC-014.1–5 | SPEC-014 | 2 | test-gated store + fixture scripts | Draft |
| AC-015.1 | SPEC-015 | 1 | integration + canary | Draft |
| AC-015.2–4 | SPEC-015 | 2 | test-gated store round-trip, byte-exact | Draft |
| AC-016.1–8 | SPEC-016 | 3 | probe env-dump tests + counting provider fixture | Draft |
| AC-017.1–3 | SPEC-017 | 3 | integration, compiled helper (signal: Unix) | Draft |
| AC-018.1–2 | SPEC-018 | 1 (codes 1/2/3), 2 (code 4), 3 (127, conflict 4) | table-driven exit-code tests | Draft |
| AC-019.1, .3 | SPEC-019 | 1–3 (suite-wide) | sentinel grep + malformed-TOML test | Draft |
| AC-019.2 | SPEC-019 | 2 | unit | Draft |
| AC-020.1–6 | SPEC-020 | 1 | validation tests | Draft |
| AC-021.1 | SPEC-021 | 1 | ordered fixture | Draft |
| AC-022.1 | SPEC-022 | 3 (docs with final feature set) | manual inspection | Draft |

## Implementation Notes

- Binary and crate name: `agent-context`. Library + thin `main` per architecture.md.
- The design document's Chinese output examples are illustrative; layout may differ, content requirements above govern. All output English.
- Shallow-status strings in text output may be humanized (`available`, `not set`, `configured`, `command missing`) but JSON uses the exact enum tokens of SPEC-010.
- Startup latency (PRD-NFR-003) is satisfied structurally by ARCH-001 (single native binary, no runtime); one cold-start measurement of `list` on the example config is recorded in validation.md with a 100 ms budget on the development machine. No automated latency gate.
- Keychain test seam (SPEC-AS-019): a cargo feature (e.g. `test-keychain`) compiled only into test builds selects a file-backed store via an environment variable; release builds contain no test store code path. Division of labor: the file-backed test store serves out-of-process `assert_cmd` integration tests; the keyring-core `mock` module serves in-process unit tests of the adapter seam. Real-store behavior is covered by the one-time manual macOS round-trip recorded in validation.md.
- Parse-error rendering: `toml::de::Error`'s `Display` renders the offending source line with a caret — it MUST NOT be forwarded to output. Extract the span/position and reformat per the SPEC-002 diagnostics rule (this is the most likely accidental violation of AC-019.3).
- clap exit codes: clap's default argument-error exit code is 2, which SPEC-018 reserves for config errors. Configure/map clap errors to exit 1 (the second most likely framework-default violation).
- `--help` and `--version` are provided by clap defaults (exit 0); invoking with no subcommand prints help and exits 1 (usage).
- Secret hygiene: wrap provider-captured bytes into the secret type at the capture boundary, before trailing-newline stripping or UTF-8 validation, so no raw secret bytes are held in plain buffers during inspection.
- Crate-feature verification: re-confirm `toml` `preserve_order` and the keyring 4.x store feature set against the resolved versions as the first implementation step (plan research was against 1.1.4 / 4.1.6).

## Assumptions

- SPEC-AS-001: `get` refuses non-scalar values without `--json` (exit 1); design §5.4 only defines JSON for complex data.
- SPEC-AS-002: `find` matching is case-insensitive substring; value matching covers string scalars only (reproduces design §5.5's example; see SPEC-009).
- SPEC-AS-003: Array element addressing (`tags.0`) is not supported in v1; arrays are retrieved whole.
- SPEC-AS-004: `command` provider has no timeout in v1 and inherits stderr/stdin; those channels are outside the SPEC-019 boundary.
- SPEC-AS-005: Valid env names are `[A-Za-z_][A-Za-z0-9_]*` (POSIX portable set).
- SPEC-AS-006: The Unix permission gate accepts only modes whose permission bits are a subset of 0600.
- SPEC-AS-007: `run` target-not-executable exits 127 (shell convention).
- SPEC-AS-008: `credential set` reads from stdin when stdin is not a TTY; exactly one trailing newline is stripped on that path.
- SPEC-AS-009: `command` provider secret = captured stdout with exactly one trailing `\n`/`\r\n` stripped; values outside the credential value domain are resolution failures.
- SPEC-AS-010: TOML keys containing a double-quote character are not addressable by the v1 path grammar; such keys remain visible in `list`/`show`/JSON.
- SPEC-AS-011: `run` requires at least one `--with`; a bare wrapper invocation is a usage error rather than a silent no-op.
- SPEC-AS-012 / SPEC-AS-018: Injection dedup identity is the effective (credential, target env) pair; `inject`-table identity is (entry, key). This refines design §6.2's literal "two sources conflict" for the identical-duplicate case; distinct sources always conflict.
- SPEC-AS-013: Design §6.2's SIGINT/SIGTERM forwarding is POSIX-scoped; on Unix it is structural via `exec`, on Windows the shared-console control-event group is the platform equivalent. No extra forwarding machinery in v1.
- SPEC-AS-014: `validate` takes no `--json` in v1; on any failing `--json` invocation stdout stays empty and errors are text on stderr. JSON error envelopes are deferred.
- SPEC-AS-015: Reference scanning excludes array elements, the reserved `inject` table, and `description` keys. This narrows design §4.3's "any string value" to the cases with coherent display/injection semantics; array-borne references have none in v1.
- SPEC-AS-016: Credential values are non-empty, NUL-free, UTF-8 strings for every provider (Definitions).
- SPEC-AS-017: Non-finite floats and all four TOML datetime forms render via their TOML lexical form; JSON encodes both as strings (type tokens unchanged).
- SPEC-AS-019: Keychain integration tests use a compile-time test-gated file-backed store selected by env var. Hard isolation: the adapter compiles only under `all(feature = "test-keychain", debug_assertions)`, and enabling the feature in a release-profile build is a `compile_error!`; validation includes a negative check that the release artifact ignores `AGENT_CONTEXT_TEST_KEYCHAIN`.
- SPEC-AS-027: Design §5.9's "fields include their owning profile" is satisfied at envelope level (`profile` on `list`/entry envelopes, per-match `profile` on `find`); `Field` objects carry no `profile` member. Raw `get --json` carries none (SPEC-AS-022).
- SPEC-AS-028: Empty-string environment variable values are treated as unset (SPEC-001, SPEC-004).
- SPEC-AS-029: `env` shallow status reports an empty-but-set variable as `not_set` — consistent with SPEC-014, where an empty value is a resolution failure (recorded from T003 review M4).
- SPEC-AS-030: The Reference scanning scope's `description` exclusion applies to keys named `description` at any depth (they are never scanned as references), while SPEC-002 rule 7's reserved-key semantics still apply only at profile/entry level (recorded from T003 review M3).
- SPEC-AS-031: `credential check` and `credential set` reject `--json` with a usage error (exit 1), mirroring `validate` (recorded from T005 review S1). Trailing-newline stripping applies only to command-provider stdout (SPEC-AS-009) and the `credential set` stdin path (SPEC-AS-008); `env` and `keychain` values pass through unmodified (T005 review I5).
- SPEC-AS-020: v1 writes no log files; design §10's log-content rules are satisfied vacuously.
- SPEC-AS-021: Credential definitions have a closed schema; unknown fields there are violations (unlike the open schema under profiles). The config root schema is likewise closed (SPEC-002 rule 10).
- SPEC-AS-022: `get --json` emits the raw JSON value without a `version` envelope, adopting design §5.4 over the §5.9 envelope wording (and over PRD-NFR-003's version clause) for this one command.
- SPEC-AS-023: `show` takes an entry name and `find` takes a needle; design §5.1's dotted-path wording for those two commands is narrowed to `get` (their §5.2–§5.5 examples use entry names/needles only).
- SPEC-AS-024: Keys that are the empty string are not addressable by the path grammar (like double-quote-bearing keys, SPEC-AS-010); they remain visible in listings and JSON.
- SPEC-AS-025: Cross-OS CI (build/test matrix for macOS/Linux/Windows) is a post-v1 follow-up: the repository is local-first with no CI infrastructure. v1 verification runs on the development machine (macOS); Windows- and Linux-specific behaviors are specified here and code-reviewed but not machine-verified in v1. Recorded as an accepted risk in validation.md, surfaced to the user at acceptance.
- SPEC-AS-026: PRD-NFR-003's latency clause is discharged by ARCH-001's structural properties plus one recorded cold-start measurement (Implementation Notes); no automated latency gate exists.

## Clarifications

### Session 2026-08-21

- Q: Implementation language? -> A: Rust (applied to architecture.md ARCH-001).
- Q: CLI output language? -> A: English for all user-visible output; no i18n in v1 (applied to Scope, SPEC-018).
- Q: Threat-model strength? -> A: Accidental-leak protection is sufficient; no command allowlists or stderr capture (applied to Scope, SPEC-014, SPEC-019).

## Open Questions

| ID | Question | Blocking? | Resolution |
| --- | --- | --- | --- |
| — | none | — | — |
