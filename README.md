# agentenv

`agentenv` is a local-first command-line interface for browsing and editing a
user's environment configuration and using credentials without printing them.
It reads a TOML file, exposes ordinary values as text or JSON, edits the file
through validated, format-preserving writes, and can launch a target process
with selected configuration and credential values in a temporary environment.

The configuration file contains ordinary values and references to credentials.
Credential values live in an environment variable, a platform credential
store, or an external password-manager command. `agentenv` does not write
credential values to the TOML file; write commands add definitions and
ordinary values only.

## Configuration

By default, the configuration file is:

- Unix-like systems: `$XDG_CONFIG_HOME/agentenv/config.toml`, or
  `~/.config/agentenv/config.toml` when `XDG_CONFIG_HOME` is unset.
- Windows: `%APPDATA%\agentenv\config.toml`.

Set `AGENTENV_FILE` to use another file. On Unix-like systems, the file's
permission bits must be a subset of `0600`; run `agentenv validate` after
creating or changing the file.

Profile selection uses this precedence:

1. `--profile <NAME>`
2. `AGENTENV_PROFILE`
3. `default_profile` in the file

The schema is open for fields under profile entries. Each profile and each
entry needs a non-empty `description`. The reserved `inject` table maps target
environment-variable names to scalar field paths in the same entry.

### Example

```toml
version = 1
default_profile = "work"

[profiles.work]
description = "Day-to-day development environment for company projects."

[profiles.work.llm]
description = "Default LLM for company projects."
endpoint = "https://llm.example.com/v1"
model = "company-model"
credential = "credential://company_llm"

[profiles.work.llm.inject]
OPENAI_BASE_URL = "endpoint"
OPENAI_MODEL = "model"

[profiles.work.ci]
description = "Labels used when submitting CI jobs for company projects."
tags = ["linux", "self-hosted"]

[profiles.work.kubernetes]
description = "Kubernetes staging environment used during development."
context = "company-staging"
namespace = "developer-tools"

[profiles.personal]
description = "Environment for personal projects."

[profiles.personal.llm]
description = "Default public LLM for personal projects."
endpoint = "https://api.openai.com/v1"
model = "gpt-5"
credential = "credential://openai_personal"

[credentials.company_llm]
description = "Access credential for the company LLM."
provider = "env"
name = "COMPANY_LLM_TOKEN"
inject_as = "OPENAI_API_KEY"

[credentials.openai_personal]
description = "Access credential for the personal OpenAI account."
provider = "keychain"
service = "agentenv"
account = "openai-personal"
inject_as = "OPENAI_API_KEY"
```

Credential references have one of these forms:

```text
credential://<name>
credential://<name>?as=<ENV>
```

`<name>` uses letters, digits, `_`, and `-`; `<ENV>` must be a valid
environment-variable name. A `?as=` value overrides the credential's default
`inject_as` only for that reference. Ordinary reads return the reference, not
the credential value.

Entry paths use dot-separated segments, with double quotes for a segment that
contains punctuation or spaces. A profile name is selected with `--profile`
and is not part of the path. Arrays are read as whole values with `get --json`.

## Agent usage protocol

Projects can place this block in `AGENTS.md`:

```md
User environment information is available through `agentenv`.

- Run `agentenv list --json` to discover available configuration.
- Run `agentenv show <name> --json` before using an unfamiliar entry.
- Use `agentenv get <path>` to retrieve ordinary values.
- Use `agentenv run --with <entry> -- <command>` when credentials are required.
- Use `agentenv set <path> <value>` to record configuration the user asks you
  to save; define credentials with `agentenv credential add <name> ...` before
  referencing them, and never write a secret value into the file.
- Never print, log, persist, or summarize resolved credentials.
- Report missing configuration or credentials explicitly.
```

Use the commands in this order:

1. Discover available profiles and entries with `agentenv list --json`.
2. Inspect an unfamiliar entry with `agentenv show <name> --json`.
3. Read an ordinary scalar with `agentenv get <path>`; use `--json` for
   an array or table.
4. When a target needs credentials, use
   `agentenv run --with <entry> -- <command> [args...]`.

If the requested profile, entry, field, or credential is missing, report that
fact explicitly. Do not guess a field name, silently switch profiles, or
substitute another credential.

### Finding injection target names

Use `agentenv credential list --json` to see each credential's default
environment target in its `inject_as` member. For an entry that contains a
credential reference, inspect the `reference` member of its JSON output from
`agentenv list --json` or `agentenv show <name> --json`. You can also
read the raw reference directly with `agentenv get <path>`.

For example, a reference of
`credential://company_llm?as=LLM_API_KEY` injects `LLM_API_KEY` for that use,
even when the credential definition's `inject_as` is `OPENAI_API_KEY`.

## CLI reference

The main query commands are:

```bash
agentenv list --json
agentenv list --profiles
agentenv list <entry> --json
agentenv show <entry> --json
agentenv get <path>
agentenv get <path> --json
agentenv find <needle> --json
agentenv find <needle> --all-profiles
agentenv validate
```

`list`, `show`, `get`, and `find` support `--profile <NAME>` and `--json`.
`list --profiles` lists profiles without selecting one. `find` searches entry
names, field names, descriptions, and string values; `--all-profiles` searches
every profile. `get --json` emits the raw JSON value for its path.

Write commands are:

```bash
agentenv init
agentenv set <path> <value>
agentenv set <path> <value> --type int|float|bool|json
agentenv set <path> <value> --description "<entry description>"
agentenv set <path> <value> --profile <name> --create-profile "<profile description>"
agentenv unset <path>
```

`init` creates the config file at the resolved path (0600 on Unix-like
systems) and refuses to touch an existing file. `set` writes exactly one
value at a profile-scoped path, creating missing intermediate tables;
`--type` selects the TOML type (`json` accepts arrays and objects, written
as inline tables). `--description` also writes the description of the entry
named by the first path segment, which is how a new entry is created in one
command. An unknown profile is an error from every selection source;
`--create-profile <text>` together with an explicit `--profile <name>`
creates the profile with that description. `unset` removes one field or
table. Write commands have no `--json` output mode.

Every write is validated as a whole file before anything touches disk: a
mutation whose result would not pass `agentenv validate`'s schema rules is
refused and the file stays byte-identical. Writes preserve comments, blank
lines, and key order, replace the file atomically, and keep its permission
bits. There is no file locking; concurrent writers are not coordinated, and
the last writer wins.

Credential commands are:

```bash
agentenv credential list --json
agentenv credential check <name>
agentenv credential set <name>
agentenv credential add <name> --description "<text>" --provider env \
    --env-var <NAME> --inject-as <ENV>
agentenv credential add <name> --description "<text>" --provider keychain \
    --service <service> --account <account> --inject-as <ENV>
agentenv credential add <name> --description "<text>" --provider command \
    --argv <arg> [--argv <arg> ...] --inject-as <ENV>
```

`credential list` performs only a shallow status check and does not read a
secret store or execute a provider command. `credential check` resolves one
credential and reports availability without printing its value. `credential
set` accepts a keychain credential from a hidden terminal prompt or standard
input. Environment and command credentials are managed by their external
systems and cannot be set by this command. `credential add` writes a
credential definition to the config file — never a value; define the
credential first, then reference it from entries (`credential://<name>`) and,
for the keychain provider, store its value with `credential set`.

## Providers

Prefer `keychain` or `command` for local development:

- `keychain` uses the platform credential store: Keychain on macOS, Credential
  Manager on Windows, and a secret-service implementation such as GNOME
  Keyring or KWallet on Linux.
- `command` executes `argv` directly through the operating system, without a
  shell. Its standard output supplies the credential; the provider strips one
  trailing newline. Standard input and standard error remain available to the
  external command for interactive authentication.
- `env` is useful for CI and already-managed shells. Its value is readable by
  any process that inherits the environment, including an agent process, so it
  is a weaker choice for local use.

For example, a command provider can delegate to an existing password manager:

```toml
[credentials.production_llm]
description = "Production LLM credential."
provider = "command"
argv = ["op", "read", "op://Engineering/Production LLM/token"]
inject_as = "OPENAI_API_KEY"
```

## Running with injected values

Use at least one `--with` entry and place the target after `--`:

```bash
agentenv run --with llm -- llm-client request
agentenv run --with llm --with kubernetes -- deploy-tool sync
```

Before launching the target, `run` builds and checks the complete injection
plan. It collects credential references and values declared by the entry's
`inject` table. Credential references use `inject_as` or their `?as=` override;
`inject` values are converted from strings, integers, floats, or booleans to
environment strings.

Repeated identical credential-and-target pairs are deduplicated. A credential
used with two different target names is resolved once and injected under both
names. Distinct sources targeting the same environment name are an injection
conflict: `run` reports the conflict with exit code 4, does not resolve a
provider, and does not launch the target. Injected variables override matching
variables inherited from the `agentenv` process; inherited variables by
themselves are never conflicts.

The target receives its normal standard input, output, and error streams.
`agentenv` does not capture or rewrite them, and the target's exit status
is returned to the caller. A target that cannot be executed returns exit code
`127`.

## Safety and threat model

`agentenv` itself never prints a resolved credential to standard output or
standard error and writes no log files. Provider-captured candidate bytes are
also excluded from diagnostics, including when a provider exits unsuccessfully
or returns an invalid value. Configuration diagnostics do not echo TOML source
lines or open-schema field values; command-provider diagnostics identify only
`argv[0]`.

This protects against accidental leaks from the CLI. It does not defend
against a malicious local process. The launched `run` target can read the
credentials intentionally placed in its own environment. The target's
standard output and error are external-process output and pass through
unchanged; they are outside the CLI's no-secret invariant. A `command`
provider's inherited standard input and standard error are also owned by that
external command and outside the invariant.

`credential set` is the deliberate exception: it writes the entered value to
the selected platform credential store. No other command prints or persists
that value.

The write commands (`init`, `set`, `unset`, `credential add`) widen the CLI's
surface deliberately: the config file, which was always hand-editable, is now
also CLI-writable. A caller able to invoke `agentenv` can rewrite injection
topology — point an entry's `credential` reference at a different defined
credential, add a `?as=` override, or edit `inject` tables — without ever
touching a secret value. The mitigations are that every write is
whole-file-validated, secret values never enter the TOML (the sensitive-field
guardrail refuses plaintext secrets and `credential set` remains the only
value-storage path), all changes are visible in the file for review, and
`run` still refuses conflicting injection targets. Write-command diagnostics
follow the same no-echo rule as the rest of the CLI: they name paths, never
the user-supplied or stored values.

### Sensitive-field guardrail

Validation checks string fields whose names are exactly `token`, `password`,
`secret`, `api_key`, or `private_key`, or end in one of those suffixes. Such a
field must contain a `credential://` reference. The check covers matching
field names in nested profile data, including tables inside arrays; it does
not inspect every string for secret-like contents. It is a guardrail against
common plaintext credential fields, not a secret scanner. The reserved
`inject` table is excluded because its keys are environment-variable targets.

## Exit statuses

Commands use these statuses:

| Status | Meaning |
| ---: | --- |
| `0` | Success |
| `1` | Usage or argument error |
| `2` | Configuration-file error, including validation or Unix permission failure |
| `3` | Unknown profile, entry, field path, or credential name |
| `4` | Credential resolution/store failure or injection conflict |
| `127` | `run` target could not be executed |

## Windows support

Windows behavior is specified and code-reviewed, but not machine-verified in
v1. The documented behavior uses `%APPDATA%` for the default configuration
path, Windows Credential Manager for `keychain`, case-insensitive environment
variable conflict checks, and a shared console when `run` waits for the target.
