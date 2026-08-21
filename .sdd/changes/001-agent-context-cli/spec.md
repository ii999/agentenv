# Implementation Specification: agent-context CLI

## Source Artifacts

- Change ID: 001-agent-context-cli
- PRD: prd.md
- Architecture: architecture.md
- Design contract: design-source.md (user-approved design document, Chinese; §ref numbers below cite it)
- Review state: revised per spec-review.md round 1 (CRIT-001..004, IMP-001..012, MIN-001..006, SUG-001..003)
- Current specs: none (greenfield)

## Scope

### In Scope

- TOML config loading, profile selection, open-schema entries with mandatory descriptions (§3–§4).
- Query commands `list`, `show`, `get`, `find` with text and stable JSON output (§5).
- `validate` with the full §5.7/§8 rule set and exit codes.
- Credential references (`credential://name[?as=ENV]`), providers `env` / `keychain` / `command`, shallow status display (§4.3, §5.6, §6.1).
- `credential list` / `check` / `set` (§5.8).
- `run --with` injection of credentials and `inject`-table values with conflict detection and transparent process semantics (§4.4, §6.2).
- Security invariant: no secret value in any output agent-context itself writes (§10; boundary defined in SPEC-019).
- README documenting the agent usage protocol (§7).

### Out of Scope

- GUI, cloud sync, config-file writing, guessing missing fields, printing plaintext credentials (§11).
- Defense against a launched target process deliberately reading its own environment (§10 threat model).
- Output written by external processes agent-context launches (the `run` target; a `command` provider's inherited stderr): those channels belong to the external tool. Accidental-leak protection covers agent-context's own output only (user decision 2026-08-21).
- Localization: all CLI output is English (Clarifications, 2026-08-21).

## Definitions

- **Query commands** (the no-resolution set): `list`, `list --profiles`, `show`, `get`, `find`, `credential list`, `validate`. None of these may resolve a secret, execute a provider command, read a credential store, or perform network I/O.
- **Resolving commands**: `credential check`, `credential set`, `run`. These may resolve or store secrets but never print them.
- **Scalar**: TOML string, integer, float, boolean, or datetime. **Injectable scalar**: string, integer, float, or boolean (datetime excluded, §4.4).
- **Scalar-to-string conversion** (used by `get` text output and env injection): strings verbatim; integers in decimal; floats via shortest round-trip decimal representation; booleans as `true`/`false`; datetimes in RFC 3339 (text output only — datetimes are not injectable).

## Command–Profile Matrix

| Command | Requires active profile | Notes |
| --- | --- | --- |
| `list`, `list <entry>`, `show`, `get`, `find` | Yes | resolution order per SPEC-004 |
| `find --all-profiles` | No | searches every profile |
| `list --profiles` | No | this is the recovery command when profile resolution fails |
| `validate` | No | validates the whole file |
| `credential list` / `check` / `set` | No | credentials are global (top-level `credentials` table) |
| `run` | Yes | entries are looked up in the active profile |

`--profile` is a global flag: commands in the "No" rows accept it and do not use it (documented in `--help` per command).

## Phase Map

| Phase | Name | Priority | Objective | Depends on | Independent test |
| --- | --- | --- | --- | --- | --- |
| Phase 1 | Config core & queries | P1 (MVP) | Load/validate config, select profile, run all query commands with text+JSON and exit codes 0/1/2/3. Credential fields shown as references with shallow status computed from config metadata, `getenv`, and PATH lookup only — no provider resolution machinery exists yet | None | Against fixture TOML files: every query command returns the documented output and every Phase-1 error path (exit 1/2/3) returns its code; a canary proves no external process ran |
| Phase 2 | Credential providers | P2 | `Provider` seam with env/keychain/command adapters; `credential check` / `set`; exit 4 paths; `Secret` no-leak type | Phase 1 | With a mock keychain store, a fake command script, and env vars: `check` reports success/failure per provider, `set` round-trips exact bytes, sentinel secrets never appear in agent-context's output |
| Phase 3 | Injection runner | P3 | `run --with` builds a conflict-checked injection plan and launches the target transparently; exit 127 path | Phase 1, 2 | A probe target dumps its env to a file: injected names present with exact values, precedence and conflict rules observed, stdio/exit codes pass through |

## Requirements

### SPEC-001: Config file location

The CLI MUST read its configuration from, in priority order: the `AGENT_CONTEXT_FILE` environment variable if set; else `$XDG_CONFIG_HOME/agent-context/context.toml` if `XDG_CONFIG_HOME` is set (Unix); else `~/.config/agent-context/context.toml` (Unix) / `%APPDATA%\agent-context\context.toml` (Windows). If the home/config base directory cannot be determined (e.g. `HOME` unset and no override), exit 2 naming the missing variable. (§3)

Source trace: PRD-FR-001.

Acceptance criteria:

- AC-001.1: GIVEN `AGENT_CONTEXT_FILE=/tmp/x.toml` pointing at a valid file, WHEN any query command runs, THEN that file is used even if a default-path file exists.
- AC-001.2: GIVEN no config file at the resolved path, WHEN any command runs, THEN exit 2 with a message containing the resolved path.
- AC-001.3: GIVEN `HOME` and `AGENT_CONTEXT_FILE` both unset (Unix test harness), WHEN `list` runs, THEN exit 2 naming `HOME` or `AGENT_CONTEXT_FILE` as remedies.

Verification: Automated: integration tests with temp dirs and scrubbed env.

### SPEC-002: Strict core validation on load

Every command MUST refuse to operate on a config that fails core validation, reporting **all** violations (not only the first), each with its config path, and exiting 2. Core rules (§8):

1. `version` is a supported integer (only `1`).
2. `default_profile`, when present, names an existing profile.
3. Every profile and every entry has a non-empty string `description`.
4. Every credential has `description`, a `provider` in the set {`env`, `keychain`, `command`} (any other value is a violation), `inject_as`, and its provider-specific required fields (`env`: `name`; `keychain`: `service`, `account`; `command`: non-empty `argv` array of strings).
5. Every `credential://` reference satisfies the strict grammar of SPEC-012 and resolves to a defined credential.
6. `inject_as`, `?as=` values, and `inject`-table keys are valid environment variable names (`[A-Za-z_][A-Za-z0-9_]*`).
7. Reserved keys: at entry level, `inject` (if present) MUST be a table satisfying SPEC-013; an entry-level `inject` of any other TOML type is a violation. An empty `inject` table is valid and injects nothing. Tables nested deeper than entry level may freely use the names `inject` and `description` as ordinary fields.
8. Sensitive-field-name rule (SPEC-020) — enforced here, so a config with a suspected plaintext secret is unusable by every command, not only flagged by `validate`.

Source trace: PRD-FR-006, PRD-NFR-001.

Acceptance criteria:

- AC-002.1: GIVEN a config whose only defect is `profiles.work.llm` lacking `description`, WHEN `list` runs, THEN exit 2 and the message contains `profiles.work.llm`.
- AC-002.2: GIVEN `version = 2`, WHEN any command runs, THEN exit 2 with a message naming the supported version.
- AC-002.3: GIVEN an entry field `credential = "credential://missing"`, WHEN `list` runs, THEN exit 2 and the message contains `missing`.
- AC-002.4: GIVEN a config with three independent violations (missing description, unknown provider `vault`, entry-level `inject = "x"`), WHEN `get llm.endpoint` runs, THEN exit 2 and all three config paths appear in the error output.
- AC-002.5: GIVEN `api_key = "sk-live-123"` in an entry, WHEN `get <that entry>.api_key` runs, THEN exit 2 (the value is never printed) with the SPEC-020 message.

Verification: Automated: table-driven validation tests.

### SPEC-003: Open schema

Users MUST be able to add arbitrary entries and fields (strings, integers, floats, booleans, datetimes, arrays, sub-tables) under a profile; new fields appear in `list`, `show`, `get`, and `find` with no CLI change. (§4.2)

Source trace: PRD-FR-001.

Acceptance criteria:

- AC-003.1: GIVEN a fixture entry with one field of each TOML type plus a nested sub-table, WHEN `list <entry>` and `show <entry>` run, THEN every field (including nested ones, with dotted paths) appears with its correct type label.
- AC-003.2: GIVEN the same fixture, WHEN `get <entry>.<nested>.<field>` runs, THEN the stored value is returned.

Verification: Automated: fixture round-trip tests.

### SPEC-004: Profile selection

The active profile MUST be resolved as: `--profile` flag, else `AGENT_CONTEXT_PROFILE` env var, else `default_profile` from the file. If the result names no existing profile, or nothing is configured, commands requiring a profile (see Command–Profile Matrix) MUST exit 3 listing available profile names and suggesting `agent-context list --profiles`. (§4.1)

Source trace: PRD-FR-002.

Acceptance criteria:

- AC-004.1: GIVEN `default_profile = "work"` and `AGENT_CONTEXT_PROFILE=personal`, WHEN `list` runs, THEN the `personal` profile is listed.
- AC-004.2: GIVEN the same, WHEN `list --profile work` runs, THEN the `work` profile is listed (flag beats env var).
- AC-004.3: GIVEN `AGENT_CONTEXT_PROFILE=nope`, WHEN `list` runs, THEN exit 3 and the message lists the defined profile names; WHEN `list --profiles` runs, THEN exit 0 (recovery path works without an active profile).

Verification: Automated: env-permutation tests.

### SPEC-005: Path grammar

Path arguments are dot-separated segments; the first segment is an entry name. A segment MAY be wrapped in double quotes to contain dots or spaces: a quoted segment starts at `"`, ends at the next `"`, and may contain any character except `"` (keys containing a double quote are not addressable in v1 — SPEC-AS-010). No escape sequences exist inside or outside quotes. Documentation examples MUST show shell-safe invocation (single-quoted argument: `agent-context get 'server."my.field"'`). An unknown path MUST exit 3 with a message naming the failing path and suggesting `agent-context list <entry>`. (§5.1)

Source trace: PRD-FR-003.

Acceptance criteria:

- AC-005.1: GIVEN a field literally named `my.field` under entry `server`, WHEN the CLI receives the argument `server."my.field"` (argv-level, no shell), THEN its value is returned.
- AC-005.2: GIVEN entry `llm` without field `region`, WHEN `get llm.region` runs, THEN exit 3 and the message contains `llm.region` and `list llm`.
- AC-005.3: GIVEN an unterminated quote in the path argument, WHEN `get` runs, THEN exit 1 with a grammar error message.

Verification: Automated: unit tests on the path parser + CLI integration via argv.

### SPEC-006: `list`

`list` MUST show every entry of the active profile with its description, and each of the entry's **top-level** fields with its name and type label — nested sub-tables (including `inject`) appear as a single field of type `table` without recursion. `list <entry>` MUST show one entry and recurse into nested tables using dotted paths. `list --profiles` MUST show every profile with its description and mark the default. Credential-reference fields show shallow status per SPEC-012. All listings follow config-file order (SPEC-021). (§5.2)

Source trace: PRD-FR-003.

Acceptance criteria:

- AC-006.1: GIVEN the design document's example config, WHEN `list` runs, THEN output contains entries `llm`, `ci`, `kubernetes` with their descriptions, top-level field names, and type labels, and the `llm.inject` table appears as one `table` line without its members.
- AC-006.2: WHEN `list --profiles` runs, THEN output contains `work` marked as default and `personal`.
- AC-006.3: GIVEN `list nosuch`, THEN exit 3 listing available entry names.
- AC-006.4: WHEN `list llm` runs, THEN `inject.OPENAI_BASE_URL` and `inject.OPENAI_MODEL` appear as dotted-path fields.

Verification: Automated: golden-output integration tests (required tokens per line).

### SPEC-007: `show`

`show <entry>` MUST print the entry description and every field recursively with dotted paths and values, with two exceptions: credential references show only the credential name and shallow status (never a secret, never the raw value's resolution); the `inject` table is rendered once, as `ENVNAME ← field.path` pairs (not additionally as dotted fields). Arrays render as their JSON encoding on one line; non-`inject` nested tables render as their fields with dotted paths. `show nosuch` exits 3 listing available entry names. (§5.3)

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
- AC-008.4: GIVEN boolean, integer, float, and datetime fixture fields, WHEN `get` runs on each, THEN output follows the scalar-to-string conversion table (`true`, `42`, `1.5`, RFC 3339).

Verification: Automated: byte-exact stdout assertions.

### SPEC-009: `find`

`find <needle>` MUST case-insensitively substring-match entry names, field names (any depth), and descriptions within the active profile, printing each match's path plus its scalar value, description (entry matches), or credential name + shallow status (reference matches). On zero matches: stdout empty, stderr `No matches for '<needle>'`, exit 0. `--all-profiles` widens the search to every profile, labels each match with its profile, and requires no active profile. (§5.5)

Source trace: PRD-FR-003.

Acceptance criteria:

- AC-009.1: WHEN `find llm` runs on the example config, THEN matches include the `llm` entry and `llm.endpoint`, and no match line contains a resolved secret.
- AC-009.2: WHEN `find LLM --all-profiles` runs with no `default_profile` configured, THEN matches from both `work` and `personal` appear, each labeled with its profile, exit 0.
- AC-009.3: WHEN `find zzz-no-match` runs, THEN stdout is empty, stderr contains `No matches`, exit 0; with `--json`, stdout is an envelope with an empty `matches` array.

Verification: Automated: integration tests.

### SPEC-010: JSON output contract

All query commands accept `--json`; JSON is the only stdout content. Two output classes exist:

**Raw value** — `get <path> --json` prints exactly the JSON encoding of the addressed value: scalar → JSON scalar (datetime → RFC 3339 string), array → JSON array, table → JSON object, credential reference → JSON string of the stored reference. No envelope, no `version` (recorded deviation from the envelope rule; design §5.4 defines `get` as raw-value retrieval).

**Envelope** — every other JSON-producing command prints one object carrying `"version"` (config version) plus:

- `list --json`: `{"version", "profile", "entries": [{"name", "description", "fields": [Field...]}]}` — fields recursive (dotted paths), file order.
- `list --profiles --json`: `{"version", "profiles": [{"name", "description", "default": bool}]}`.
- `show <entry> --json`: `{"version", "profile", "name", "description", "fields": [Field...]}` (the `inject` table appears as ordinary `table`/member fields here; the `←` form is text-only sugar).
- `find --json`: `{"version", "matches": [{"profile", "path", "kind": "entry"|"field", "type"?, "value"?, "description"?, "credential"?}]}`.
- `credential list --json`: `{"version", "credentials": [{"name", "description", "provider", "status"}]}`.

A `Field` is `{"path", "type", "value"}` with `type` ∈ `string|integer|float|boolean|datetime|array|table|credential_ref`; `table` fields enumerate members as further `Field`s rather than carrying a `value`; `credential_ref` fields carry `{"path", "type": "credential_ref", "credential": {"name", "provider", "status"}}` and never a `value` or the reference's `?as=` target resolution. `status` ∈ `available|not_set|configured|command_missing`. These shapes are a frozen compatibility contract; changes must be additive. (§5.9)

Source trace: PRD-NFR-003, PRD-NFR-001.

Acceptance criteria:

- AC-010.1: WHEN `list --json` runs on the example config, THEN output parses, carries `version: 1` and `profile: "work"`, and the `llm.credential` field object has `credential.{name,provider,status}` and no `value`.
- AC-010.2: WHEN `get ci.tags --json` runs, THEN stdout is exactly the bare JSON array; WHEN `get llm.credential --json` runs, THEN stdout is the JSON string of the reference.
- AC-010.3: Snapshot tests lock all six shapes above (five envelopes + raw value); any shape change fails the suite.

Verification: Automated: JSON snapshot tests.

### SPEC-011: `validate`

`validate` MUST run all SPEC-002 core rules plus the Unix file-permission check (fail when the config file mode grants any group/other read or write bit; 0600 and 0400 pass) and MUST report every failure with its config path, exiting 2 on any failure, 0 when clean. `validate` requires no active profile and is a query command (no resolution). (§5.7)

Source trace: PRD-FR-006.

Acceptance criteria:

- AC-011.1: GIVEN a config with two independent violations, WHEN `validate` runs, THEN both are reported and exit is 2.
- AC-011.2: GIVEN a clean config with mode 0644 on Unix, WHEN `validate` runs, THEN the failure names the file and `0600`.
- AC-011.3: GIVEN a clean 0600 config, THEN exit 0.

Verification: Automated: integration tests with `chmod` in temp dirs.

### SPEC-012: Credential references and shallow status

Any string value beginning with `credential://` anywhere under an entry (including nested sub-tables) MUST parse against the exact grammar `credential://<name>` or `credential://<name>?as=<ENV>` where `<name>` matches `[A-Za-z0-9_-]+` and `<ENV>` is a valid env name. Anything else beginning with `credential://` — empty name, unknown or duplicated query parameters, wrong-case `?As=`, trailing garbage — is a load-time violation (exit 2, SPEC-002 rule 5): a malformed reference is never silently treated as an ordinary string and never silently falls back to the credential's `inject_as`.

Query commands MUST display references with a shallow status computed without resolving: `env` → `available`/`not_set` via variable presence; `keychain` → `configured` (no store read); `command` → `configured`/`command_missing` via PATH lookup (or existence check for an absolute path) of `argv[0]`. (§4.3, §5.6)

Source trace: PRD-FR-004, PRD-NFR-001.

Acceptance criteria:

- AC-012.1: GIVEN a reference nested two tables deep, WHEN `list <entry>` runs, THEN it is displayed as a credential reference.
- AC-012.2: GIVEN a `command` credential whose `argv[0]` is a canary script that creates a file when executed, WHEN every query command runs, THEN status logic reports `configured` and the canary file does not exist afterward.
- AC-012.3: GIVEN an `env` credential with the variable set, THEN status is `available`; unset, `not_set`.
- AC-012.4: GIVEN `credential://company_llm?As=X`, `credential://`, and `credential://name?x=1` in three fixtures, WHEN any command runs, THEN each exits 2 citing the malformed reference.

Verification: Automated: canary no-execution test; grammar table test.

### SPEC-013: `inject` tables

An entry MAY contain a reserved `inject` sub-table mapping environment variable names to field paths within the same entry. Beyond SPEC-002 rule 7 (must be a table), validation MUST reject: keys that are not valid env names; values that are not strings; paths that do not resolve within the entry; paths resolving to anything other than an injectable scalar (string/integer/float/boolean — datetime and all non-scalars rejected); paths resolving to credential references (credentials inject via `inject_as`/`?as=` only, message directs there). (§4.4)

Source trace: PRD-FR-005.

Acceptance criteria:

- AC-013.1: GIVEN `inject = { OPENAI_BASE_URL = "endpoint" }` where `endpoint` is a string, WHEN `validate` runs, THEN exit 0.
- AC-013.2: GIVEN inject values pointing at an array field and at a datetime field, WHEN `validate` runs, THEN exit 2 naming each inject key and path.
- AC-013.3: GIVEN an inject value pointing at a credential-reference field, WHEN `validate` runs, THEN exit 2 with a message directing to `inject_as`.

Verification: Automated: validation tests.

### SPEC-014: Credential providers

`env` resolution reads the named variable, failing when unset. `keychain` resolution reads the platform credential store (macOS Keychain / Windows Credential Manager / Linux secret-service) by `service` + `account`, failing when the item is missing; on systems with no usable store the error names the provider and suggests `command`/`env`. `command` resolution executes `argv` directly via the OS process API — agent-context itself never constructs a shell invocation or passes the command through a shell; the content of a user-authored `argv` is the config author's choice and is not policed. The provider command's stdout is captured as the secret (exactly one trailing `\n` or `\r\n` stripped); stdin and stderr are inherited so interactive password-manager auth can complete; resolution fails on non-zero exit, empty/whitespace-only output, or output that is not valid UTF-8 or contains NUL. Resolution failure is always explicit (exit 4 at CLI level) and never substitutes another credential. (§6.1, §10)

Source trace: PRD-FR-004.

Acceptance criteria:

- AC-014.1: GIVEN `COMPANY_LLM_TOKEN` unset, WHEN `credential check company_llm` runs, THEN exit 4 and the message names the variable without printing any value.
- AC-014.2: GIVEN a fake command-provider script printing a sentinel secret, WHEN `credential check` runs, THEN success is reported and the sentinel does not appear in agent-context's stdout or stderr.
- AC-014.3: GIVEN a command provider whose `argv` includes the literal argument `$HOME`, and a script that records its argv, WHEN resolved, THEN the script receives the literal string `$HOME` (no shell expansion occurred).
- AC-014.4: GIVEN a mock keychain store containing the item, WHEN `check` runs, THEN success; with the item absent, exit 4 naming service and account.
- AC-014.5: GIVEN a command provider script exiting 1, and another printing only `\n`, WHEN each is checked, THEN exit 4 with distinct messages (non-zero exit vs empty output).

Verification: Automated: keyring mock store; fixture scripts.

### SPEC-015: `credential list` / `check` / `set`

`credential list` MUST print every credential definition with provider type and shallow status (query command — no resolution), and supports `--json` (shape in SPEC-010). `credential check <name>` MUST perform a real resolution and report success or the concrete failure reason, never printing the secret; an undefined name exits 3 listing defined credential names. `credential set <name>` MUST work only for `keychain` credentials: with a TTY it prompts and reads without echo; without a TTY it reads stdin to EOF, stripping exactly one trailing `\n`/`\r\n`; an empty value (after stripping) exits 1. The exact remaining bytes are stored. Store write failures exit 4 naming the platform error. For `env`/`command` credentials it exits 1 explaining those are externally managed. (§5.8)

Source trace: PRD-FR-007.

Acceptance criteria:

- AC-015.1: WHEN `credential list` runs on the example config, THEN both credentials appear with provider labels and statuses, and a command-provider canary proves no resolution ran.
- AC-015.2: GIVEN the piped value `hunter2\n`, WHEN `credential set openai_personal` runs against the mock store, THEN the stored bytes are exactly `hunter2`, a following `check` succeeds, and no output channel ever contained the value.
- AC-015.3: WHEN `credential set company_llm` (env provider) runs, THEN exit 1 and the message says env credentials are managed externally.
- AC-015.4: WHEN `credential check nosuch` runs, THEN exit 3 listing the defined credential names.

Verification: Automated: mock-store round-trip with byte assertion.

### SPEC-016: `run --with` injection plan

`run --with <entry>... -- <cmd> [args...]` MUST: require at least one `--with` and a command after `--` (else exit 1 usage error showing the expected form); resolve each named entry in the active profile (exit 3 if absent); recursively collect its credential references and evaluate its `inject` table (scalar-to-string conversion); and build the full target-env mapping BEFORE resolving any secret. Conflict rule: after deduplicating identical injection sources — repeated `--with` entries, and identical (env name, credential name, `?as=`) triples — two remaining injections targeting the same env name fail with exit 4, naming both sources (credential names and/or `entry.inject` keys), without launching the target or resolving any provider. Precedence: injected variables override same-named variables inherited from agent-context's own environment; inherited variables are never conflicts. `?as=` overrides the credential's `inject_as` for that reference. An entry with no references and no `inject` table injects nothing and still runs the target. (§6.2)

Source trace: PRD-FR-005.

Acceptance criteria:

- AC-016.1: GIVEN entry `llm` (credential → `OPENAI_API_KEY`, inject → `OPENAI_BASE_URL`, `OPENAI_MODEL`), WHEN `run --with llm -- <probe>` runs, THEN the probe's env contains those three names with the expected values (assertion restricted to the injected names).
- AC-016.2: GIVEN two `--with` entries whose distinct credentials both target `OPENAI_API_KEY`, WHEN `run` executes, THEN exit 4, the message names both credentials, and a canary proves no provider command ran.
- AC-016.3: GIVEN `credential://company_llm?as=LLM_API_KEY`, WHEN `run --with llm -- <probe>` runs, THEN the probe sees `LLM_API_KEY` and no injected `OPENAI_API_KEY`.
- AC-016.4: GIVEN `OPENAI_API_KEY=inherited` in agent-context's own env and entry `llm` injecting it, WHEN `run` executes, THEN the probe sees the injected value, not `inherited`.
- AC-016.5: GIVEN two entries referencing the same credential with the same target name, WHEN `run --with a --with b -- <probe>` executes, THEN no conflict: the value is injected once.
- AC-016.6: WHEN `run -- true` (zero `--with`) runs, THEN exit 1 usage error.

Verification: Automated: probe writing its env to a temp file.

### SPEC-017: `run` process transparency

After a successful injection plan, the target's stdio MUST be the caller's stdio, uncaptured and unbuffered, and the exit status observed by the caller MUST be the target's own. Platform semantics: on Unix the CLI replaces itself with the target (`exec`), so signal delivery and exit/signal status (observed as 128+N by the shell) are structural; on Windows the CLI spawns the target in the same console (sharing the console control-event group, so Ctrl-C reaches the target), waits, and exits with the target's exit code — no additional forwarding machinery is claimed in v1. Pre-launch failures use SPEC-018 exit codes; a target that cannot be executed exits 127 naming the command. agent-context writes nothing to stdout/stderr on the happy path. (§6.2)

Source trace: PRD-FR-005.

Acceptance criteria:

- AC-017.1: WHEN `run --with ci -- <helper emitting "out" on stdout, "err" on stderr, exiting 7>` runs, THEN stdout is `out`, stderr is `err`, exit code 7, no wrapper output (helper is a cross-platform test binary).
- AC-017.2: WHEN the target does not exist, THEN exit 127 and the message names the missing command.
- AC-017.3: (Unix) WHEN the target kills itself with SIGTERM, THEN the observed exit status is 143.

Verification: Automated: integration tests with a compiled helper; signal case Unix-only.

### SPEC-018: Exit codes and error messages

The CLI MUST use: 0 success; 1 usage/argument errors (bad flags, path grammar errors, `get` non-scalar without `--json`, `credential set` on non-keychain or empty value, `run` without `--with`/command); 2 config-file errors (missing file/base dir, parse, any SPEC-002 violation, permission check); 3 name-resolution failures (unknown profile, entry, path, or credential name); 4 credential resolution failure, store write failure, or injection conflict; 127 target-not-executable (`run` only). Every error message MUST name the failing thing and a next action (a command to run or a config path to edit), in English. (§9)

Source trace: PRD-NFR-002.

Acceptance criteria:

- AC-018.1: A table-driven test exercises at least one failure per exit code available in the phase under test (Phase 1: 1, 2, 3; Phase 2 adds 4; Phase 3 adds 127 and injection-conflict 4) and asserts code plus a required message token.
- AC-018.2: No error path prints a secret value (enforced suite-wide by AC-019.1).

Verification: Automated.

### SPEC-019: No-secret invariant (security)

No output that agent-context itself writes — stdout, stderr, or any file, on any command, in success or failure — may contain a resolved secret value. `credential check` and `run` resolve secrets but never print them. Enforcement boundary: channels owned by external processes that agent-context launches (the `run` target's stdio; a `command` provider's inherited stderr and stdin) are outside this invariant — they belong to the external tool and are covered by the documented threat model (accidental-leak protection; Scope). The invariant is enforced structurally (secret type without `Display`/`Serialize`, constructed only inside the credential module, per ARCH-005) and by tests. (§10)

Source trace: PRD-NFR-001.

Acceptance criteria:

- AC-019.1: A suite-wide assertion greps every captured agent-context stdout/stderr from every integration test for planted sentinel secret values (distinct high-entropy strings per provider) and fails on any hit.
- AC-019.2: (Phase 2) The secret type implements neither `Display` nor `Serialize`, and its `Debug` prints a redaction marker (unit-asserted).

Verification: Automated: sentinel grep + unit test.

### SPEC-020: Sensitive field names

A field whose name exactly equals `token`, `password`, `secret`, `api_key`, or `private_key`, or ends with `_token`, `_password`, `_secret`, `_api_key`, or `_private_key`, and holds a string that is not a `credential://` reference, is a core validation violation (SPEC-002 rule 8; every command exits 2). Non-string values and names like `token_endpoint` are unaffected. (§8)

Source trace: PRD-NFR-001.

Acceptance criteria:

- AC-020.1: GIVEN `api_key = "sk-live-123"`, WHEN `validate` runs, THEN exit 2 naming the path and suggesting a `credential://` reference (and per AC-002.5, `get` refuses identically).
- AC-020.2: GIVEN `github_token = "credential://gh"` (defined credential), THEN valid.
- AC-020.3: GIVEN `token_endpoint = "https://x"` and `use_token = true`, THEN both valid.

Verification: Automated: validation tests.

### SPEC-021: Presentation order

`list`, `show`, and every JSON array in SPEC-010 envelopes MUST present profiles, entries, and fields in config-file order. (ARCH-002; design §5.2 examples)

Source trace: PRD-FR-003.

Acceptance criteria:

- AC-021.1: GIVEN a fixture whose entries and fields are deliberately non-alphabetical, WHEN `list` and `list --json` run, THEN order matches the file, byte-asserted in JSON.

Verification: Automated: ordered fixture.

### SPEC-022: Agent usage documentation

The repository MUST ship a `README.md` containing: what the tool is, the config schema by example, and the agent usage protocol of design §7 (the AGENTS.md snippet verbatim, plus the discover → inspect → get → `run --with` flow and the rule to report missing configuration explicitly rather than guessing), and a statement of the threat model boundary (SPEC-019). (§7)

Source trace: PRD-FR-003 (adoption), spec-review SUG-003.

Acceptance criteria:

- AC-022.1: README exists and contains the §7 snippet's six bullet points and a threat-model section; verified by inspection at validation.

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
| EDGE-009 | Datetime field | type label `datetime`; `get` prints RFC 3339 | integration |
| EDGE-010 | `get <entry>` (no field part) | exit 1 directing to `--json` or `show <entry>` | integration |
| EDGE-011 | Provider command output not valid UTF-8 / contains NUL | resolution fails naming the reason, exit 4 | integration |
| EDGE-012 | Mock keychain store write fails | `credential set` exits 4 naming the platform error | integration |
| EDGE-013 | `credential set` with empty piped stdin | exit 1: empty value | integration |
| EDGE-014 | `HOME` unset, no `AGENT_CONTEXT_FILE` (Unix) | exit 2 naming the remedy (SPEC-001) | integration |
| EDGE-015 | `--profile` passed to `validate` / `credential list` | accepted, unused; command behaves identically | integration |

## Dependencies

| Requirement | Dependency | Reason |
| --- | --- | --- |
| SPEC-006..011, 021 | SPEC-002..005 | queries need a validated config, profile, and path grammar |
| SPEC-014, SPEC-015 | SPEC-002, SPEC-012 | providers operate on validated credential definitions |
| SPEC-016, SPEC-017 | SPEC-012, SPEC-013, SPEC-014 | injection consumes references, inject tables, and providers |

## Acceptance Matrix

| Acceptance ID | Requirement | Phase | Verification method | Status |
| --- | --- | --- | --- | --- |
| AC-001.1–3 | SPEC-001 | 1 | integration | Draft |
| AC-002.1–5 | SPEC-002 | 1 | validation tests | Draft |
| AC-003.1–2 | SPEC-003 | 1 | fixture tests | Draft |
| AC-004.1–3 | SPEC-004 | 1 | env-permutation tests | Draft |
| AC-005.1–3 | SPEC-005 | 1 | parser unit + CLI argv tests | Draft |
| AC-006.1–4 | SPEC-006 | 1 | integration | Draft |
| AC-007.1–3 | SPEC-007 | 1 | integration | Draft |
| AC-008.1–4 | SPEC-008 | 1 | byte-exact stdout tests | Draft |
| AC-009.1–3 | SPEC-009 | 1 | integration | Draft |
| AC-010.1–3 | SPEC-010 | 1 (raw `get` + envelopes), extended in 2 (`credential list`) | JSON snapshot tests | Draft |
| AC-011.1–3 | SPEC-011 | 1 | integration + chmod | Draft |
| AC-012.1–4 | SPEC-012 | 1 | canary + grammar tables | Draft |
| AC-013.1–3 | SPEC-013 | 1 | validation tests | Draft |
| AC-014.1–5 | SPEC-014 | 2 | mock store + fixture scripts | Draft |
| AC-015.1–4 | SPEC-015 | 2 | mock-store round-trip, byte-exact | Draft |
| AC-016.1–6 | SPEC-016 | 3 | probe env-dump tests | Draft |
| AC-017.1–3 | SPEC-017 | 3 | integration, compiled helper (signal: Unix) | Draft |
| AC-018.1–2 | SPEC-018 | 1 (codes 1/2/3), 2 (code 4), 3 (127, conflict 4) | table-driven exit-code tests | Draft |
| AC-019.1 | SPEC-019 | 1–3 (suite-wide) | sentinel grep | Draft |
| AC-019.2 | SPEC-019 | 2 | unit | Draft |
| AC-020.1–3 | SPEC-020 | 1 | validation tests | Draft |
| AC-021.1 | SPEC-021 | 1 | ordered fixture | Draft |
| AC-022.1 | SPEC-022 | 3 (docs with final feature set) | manual inspection | Draft |

## Implementation Notes

- Binary and crate name: `agent-context`. Library + thin `main` per architecture.md.
- The design document's Chinese output examples are illustrative; layout may differ, content requirements above govern. All output English.
- Shallow-status strings in text output may be humanized (`available`, `not set`, `configured`, `command missing`) but JSON uses the exact enum tokens of SPEC-010.
- Startup latency (PRD-NFR-003) is satisfied structurally by ARCH-001 (native binary, no runtime); one cold-start measurement of `list` on the example config is recorded in validation.md with a 100 ms budget on the development machine. No automated latency gate.

## Assumptions

- SPEC-AS-001: `get` refuses non-scalar values without `--json` (exit 1) instead of inventing an unspecified text encoding; design §5.4 only defines JSON for complex data.
- SPEC-AS-002: `find` matching is case-insensitive substring; the design doc does not specify case rules and agent needles are unpredictable.
- SPEC-AS-003: Array element addressing (`tags.0`) is not supported in v1; arrays are retrieved whole. The design doc's path grammar addresses keys only.
- SPEC-AS-004: `command` provider has no timeout in v1 and inherits stderr/stdin so interactive password-manager auth (e.g. `op`) can complete; the provider's inherited channels are outside the SPEC-019 boundary.
- SPEC-AS-005: Valid env names are `[A-Za-z_][A-Za-z0-9_]*` (POSIX portable set).
- SPEC-AS-006: The Unix permission gate accepts only files with no group/other permission bits for read or write (0600, 0400).
- SPEC-AS-007: `run` target-not-executable exits 127 (shell convention); the design doc's table does not cover this case.
- SPEC-AS-008: `credential set` reads from stdin when stdin is not a TTY, enabling scripted setup while remaining no-echo interactively; exactly one trailing newline is stripped on the stdin path.
- SPEC-AS-009: `command` provider secret = captured stdout with exactly one trailing `\n`/`\r\n` stripped; empty, whitespace-only, non-UTF-8, or NUL-containing output is a resolution failure. The design doc does not define the output contract.
- SPEC-AS-010: TOML keys containing a double-quote character are not addressable by the v1 path grammar (no escape syntax); such keys remain visible in `list`/`show`/JSON.
- SPEC-AS-011: `run` requires at least one `--with`; a bare wrapper invocation adds nothing and likely indicates a mistake, so it is a usage error rather than a silent no-op wrapper.
- SPEC-AS-012: Identical injection triples (env name, credential, `?as=`) are deduplicated rather than conflicting; distinct sources targeting one name always conflict.

## Clarifications

### Session 2026-08-21

- Q: Implementation language? -> A: Rust (applied to architecture.md ARCH-001).
- Q: CLI output language? -> A: English for all user-visible output; no i18n in v1 (applied to Scope, SPEC-018).
- Q: Threat-model strength? -> A: Accidental-leak protection is sufficient; no command allowlists or stderr capture (applied to Scope, SPEC-014, SPEC-019).

## Open Questions

| ID | Question | Blocking? | Resolution |
| --- | --- | --- | --- |
| — | none | — | — |
