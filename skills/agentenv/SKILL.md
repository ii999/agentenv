---
name: agentenv
description: >-
  Read and edit the user's local environment configuration and run commands
  with injected credentials through the agentenv CLI. Use when a task needs
  user-specific configuration such as LLM endpoints, models, kubernetes
  contexts, or CI settings; when a command requires an API key or other
  secret; when the user asks to save configuration or register a credential;
  or when project instructions mention agentenv. Credentials are injected
  into target processes and are never printed.
---

# agentenv

`agentenv` reads a local TOML configuration file that holds ordinary values
and references to credentials. Credential values live in an environment
variable, the platform credential store, or an external command — never in
the file. The CLI reads values, edits the file with validated
format-preserving writes, and launches target processes with selected
values injected into a temporary environment.

Confirm the CLI is available with `command -v agentenv` before relying on
it. If it is missing, say so and stop; do not reconstruct its behavior by
reading the config file directly.

## Hard rules

- Never print, log, persist, or summarize a resolved credential value. Do
  not launch `agentenv run` with a target chosen to reveal its own
  environment — `env`, `printenv`, `set`, `sh -c 'echo $VAR'`, or writing
  the environment to a file are all violations.
- Never write a secret value into the config file. `set` is for ordinary
  values; `credential add` writes a definition only; `credential set` is
  the single path that stores a value, and it stores it in the platform
  credential store, not the file.
- If a profile, entry, field, or credential is missing, report that
  explicitly. Do not guess a field name, silently switch profiles, or
  substitute another credential.
- Read the config through the CLI, not by opening the TOML file, so
  profile selection, credential indirection, and validation apply.

## Reading configuration

Work top-down; skip steps you have already done in this session.

1. `agentenv project status --json` — discover the nearest project file and
   its trust state before reading profile-dependent configuration. Its report
   is written to stdout even when it exits `5` or `6`; an exit `2` leaves
   stdout empty.
2. `agentenv list --json` — discover profiles and entries.
   `agentenv list --profiles` lists profile names without selecting one.
3. `agentenv show <entry> --json` — inspect an unfamiliar entry before
   using it. Credential fields appear as a `reference` member
   (`credential://<name>` or `credential://<name>?as=<ENV>`), never as a
   value.
4. `agentenv get <path>` — read one ordinary scalar. Use `--json` for an
   array or table. Reading a credential field returns the reference
   string, not the secret.
5. `agentenv find <needle> --json` — search entry names, field names,
   descriptions, and string values when you do not know where something
   lives. Add `--all-profiles` to search every profile.

Paths are dot-separated field segments within the selected profile, for
example `llm.model`. Quote a segment containing punctuation or spaces:
`servers."my host".port`. The profile is not part of the path; select it
with `--profile <NAME>` (precedence: `--profile`, then `AGENTENV_PROFILE`,
then the trusted project-file pin, then `default_profile` in the file). This
applies to reads, `run`, `set`, and `unset`. `--create-profile` requires an
explicit `--profile` and never uses the pin.

## Project-scoped configuration

`agentenv` discovers the nearest regular `.agentenv.toml` while walking from
the working directory toward the filesystem root. The file has a closed,
selection-only schema: `version = 1`, an optional non-empty `profile`, and
optional `[requires.<entry>]` tables with a non-empty `reason` and optional
entry-relative `fields` that, when present, are non-empty. It is limited to
64 KiB and cannot contain
values, credential definitions, `inject` tables, or `credential://` strings.

Project files are inert until their exact contents are approved. Use
`agentenv project status`, `agentenv project allow`, and `agentenv project
revoke` to inspect, approve, and remove approval. Approval is kept in the user
state directory, outside the repository; editing the file makes it untrusted
again. An untrusted file affects no ordinary command except for one stderr
notice.

Use `--no-project`, or a non-empty `AGENTENV_NO_PROJECT`, to bypass discovery
for a command outside the `project` group. The bypass never applies to
`project status`, `project allow`, or `project revoke`, which always discover
the nearest file.

`project status --json` deliberately writes its report to stdout with exit
status `5` for an unavailable, invalid, or untrusted project state and with
exit status `6` when requirements are unsatisfied or cannot be checked. This
is the exception to the usual empty-stdout-on-failing-JSON rule. Exit `2`
covers configuration errors, including project-file validation errors, and
leaves stdout empty for this command.

## Running a command that needs credentials

Use `run` with one or more `--with <entry>` flags and the target after
`--`:

```bash
agentenv run --with llm -- llm-client request
agentenv run --with llm --with kubernetes -- deploy-tool sync
```

Each `--with` entry contributes its credential references and its `inject`
table (which maps environment names to ordinary fields of that entry). A
credential injects under its `inject_as` name unless the reference carries
a `?as=<ENV>` override. To learn which environment names a target will
receive, read `inject_as` from `agentenv credential list --json` and the
`inject` table and `reference` members from `agentenv show <entry> --json`.

The target's stdin/stdout/stderr pass through unchanged and its exit
status is returned. Conflicting injections (two sources targeting one
environment name) abort with exit code 4 before anything resolves or
launches.

## Writing configuration

Use these when the user asks to record configuration:

```bash
agentenv init                            # create the config file; refuses to overwrite
agentenv set <path> <value>              # write one value; creates intermediate tables
agentenv set <path> <value> --type int|float|bool|json
agentenv set llm.model gpt-5 --description "Default LLM."   # creates the entry too
agentenv set <path> <value> --profile dev --create-profile "Dev profile."
agentenv unset <path>                    # remove one field or table
```

Every entry needs a description; creating a new top-level entry requires
`--description` on the same `set`. An unknown profile is an error unless
you pass `--create-profile` with an explicit `--profile`. Every write is
whole-file validated first and refused if the result would be invalid, so
a rejected write leaves the file byte-identical — report the diagnostic
rather than retrying variations blindly.

Validation refuses plaintext values in string fields named (or suffixed)
`token`, `password`, `secret`, `api_key`, or `private_key`: such fields
must hold a `credential://` reference. When the user hands you a secret to
save, define and store it as a credential instead (next section), then
reference it.

## Managing credentials

```bash
agentenv credential list --json          # definitions + shallow status; resolves nothing
agentenv credential check <name>         # resolves one credential; reports availability only
agentenv credential add <name> --description "<text>" --provider env \
    --env-var <NAME> --inject-as <ENV>
agentenv credential add <name> --description "<text>" --provider keychain \
    --service <service> --account <account> --inject-as <ENV>
agentenv credential add <name> --description "<text>" --provider command \
    --argv <arg> [--argv <arg> ...] --inject-as <ENV>
agentenv credential set <name>           # store a keychain value (hidden prompt / stdin)
```

Order for a new credential: `credential add` the definition, then for the
keychain provider store the value with `credential set`, then reference it
from an entry field as `credential://<name>`. Env and command credentials
get their values from their external systems; `credential set` does not
apply to them.

`credential set` reads the value from a hidden terminal prompt, so prefer
asking the user to run it themselves. If they hand the value to you
instead, pipe it via stdin without echoing it into the transcript or shell
history.

Prefer `keychain` or `command` providers for local use; `env` exposes the
value to every process inheriting the environment and suits CI.

## Docker Compose pairing

Keep non-secret settings in `.env`. Inject credentials only for the process
that needs them, then let Compose receive the injected variable:

```bash
agentenv run --with llm -- docker compose up
```

```yaml
services:
  app:
    image: alpine:3.20
    environment:
      - OPENAI_API_KEY
```

`${OPENAI_API_KEY}` interpolation is also supported by Compose. Do not use an
`env_file:` containing secrets; it persists credentials in a file.

## Exit codes and errors

| Code | Meaning | Typical response |
| ---: | --- | --- |
| 0 | Success | — |
| 1 | Usage or argument error | Fix the invocation |
| 2 | Config-file error, including project-file validation, corrupt trust state, or Unix permission bits not ⊆ 0600 | Run `agentenv validate`; report the diagnostic |
| 3 | Unknown profile, entry, field path, or credential | Re-check with `list`/`find`; report what is missing |
| 4 | Credential resolution failure or injection conflict | `credential check <name>`; report, do not substitute |
| 5 | Project trust-state failure | Run `agentenv project status`; use `allow` or `revoke` as indicated |
| 6 | Project requirements unsatisfied or uncheckable (`project status` only) | Read the status report and repair the reported requirement or profile selection |
| 127 | `run` target could not be executed | The target command is missing, not agentenv |

Diagnostics never echo secret values, so it is safe to relay them to the
user verbatim.
