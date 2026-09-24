---
name: agentenv
description: >-
  Read and edit the user's local environment configuration and run commands
  with injected credentials through the agentenv CLI. Use when a task needs
  user-specific configuration such as LLM endpoints, models, kubernetes
  contexts, or CI settings; when a command requires an API key or other
  secret; when the user asks to save configuration or register a credential;
  when a configured local or SSH sudo target must run a privileged command;
  when a login form or API-key field in a browser you are automating needs
  a stored credential; or when project instructions mention agentenv.
  Credentials are delivered through bounded consumer-specific channels and
  are never printed.
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
- After `credential fill` writes a value into a browser field, do not read
  that field back, evaluate its value, or take a screenshot that shows it.
  The fill result is the only thing you may report.

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

Add `--pure` to launch the target with a curated minimal environment: a
fixed platform base (PATH, HOME, locale, `AGENTENV_*`, and similar
process-critical names), plus variables carried explicitly with repeatable
`--keep <NAME>`, plus the injections — nothing else from the calling shell.
A kept name unset in the parent is reported on stderr and the run
continues. `--pure` filters the target's environment only; it is not a
sandbox, and nested `agentenv` calls inside the target still resolve the
same configuration. Credential resolution is unchanged by `--pure`: a
`command` provider's subprocess keeps the full parent environment.

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
agentenv credential set <name>           # store a keychain value at a hidden prompt
```

Order for a new credential: `credential add` the definition, then for the
keychain provider store the value with `credential set`, then reference it
from an entry field as `credential://<name>`. Env and command credentials
get their values from their external systems; `credential set` does not
apply to them.

`credential set` reads the value from a hidden terminal prompt, so prefer
asking the user to run it themselves. Never ask them to send the value in
chat or place it in argv, an environment variable, a command pipe, or a file.

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

## Filling a credential into a browser field

Use `credential fill` when a page you are driving needs a stored credential
in one input field. It writes the value through the browser's debugging
protocol; you keep navigating and submitting with your own browser tool.

```bash
agentenv credential fill --capabilities --json
agentenv credential fill <name> --backend cdp --endpoint http://127.0.0.1:<port> \
    --page-url <exact page URL> --selector '<css of the one input>' --json
```

Steps:

1. Confirm `--capabilities` lists the `cdp` backend as available. Your
   browser must expose a loopback Chrome DevTools Protocol endpoint (a
   Chromium started with `--remote-debugging-port` and a non-default
   `--user-data-dir`, or the endpoint your browser tool already uses). If
   there is none, say so; do not launch a browser or paste the value.
2. Navigate to the page with your browser tool, then pass the page's exact
   URL and a CSS selector that matches only the target input. Add
   `--page-match origin-path` when the URL carries volatile query
   parameters, `--frame-selector` for each iframe on the way to the field,
   and `--context-index` if the same URL is open in several contexts.
3. Close DevTools on that tab and keep tracing, screencasts, HAR, and video
   recording off while filling; agentenv fails only on the open DevTools
   window because the other recorders cannot be detected.
4. Run the command. Success prints only
   `{"version":1,"backend":"cdp","effect":"field-filled"}`; the field's
   previous content was replaced and the page received normal input events.
   Continue with your own tool to submit.

Only credentials that permit `environment` usage can be filled; the value
must be a single line of at most 8,192 bytes without control, line-separator,
or bidirectional-control characters (`value-unsupported` otherwise). A
command provider runs without a terminal and its one trailing newline is
stripped; empty, NUL, or non-UTF-8 provider output is a credential error
(exit 4). The whole operation, including a keychain authorization dialog on
first access, runs under `--timeout-ms` (default 30,000); raise it when the
user must click through such a dialog. `credential fill` does not verify the
field, submit the form, or log in; report the effect, not a success you did
not observe.

Failure reasons on stderr (`credential-fill: <reason>: <message>`). Exit 8,
nothing changed: `backend-unavailable`, `permission`, `connect-failed`,
`version-unsupported`, `recording-conflict`, `context-absent`, `page-absent`,
`page-ambiguous`, `frame-absent`, `frame-ambiguous`, `frame-invalid`,
`target-absent`, `target-ambiguous`, `target-hidden`, `target-disabled`,
`target-readonly`, `target-unfillable`, `target-changed`,
`value-unsupported`, `timeout`, `cancelled`. Exit 11, the field may hold the
value: `timeout`, `cancelled`, or `delivery-failed` after the value was sent,
and `cleanup-unconfirmed` after a delivered value.

## Privileged execution

When the request uses a configured `sudo-target`, read
[references/sudo-execution.md](references/sudo-execution.md) before planning,
checking, executing, creating, or rotating that target. It contains the exact
local/SSH commands, authentication-purpose workflow, one-shot failure rules,
and current platform limitations. Ordinary `run --with` must not consume an
authentication credential.

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
| 7 | `update` failed, or replaced the binary but left an agent skill unrefreshed | Relay the diagnostic; rerun `agentenv update --force` once the cause is fixed |
| 8 | `credential fill` failed before anything was changed (`credential-fill: <reason>:`) | Fix what the reason names: the endpoint, page URL, frame chain, selector, an open DevTools window, or the credential's value; a `cancelled` or `timeout` reason may simply be retried |
| 9 | Owned sudo execution, protocol, or helper failure | Relay the diagnostic; repair the named prerequisite before retrying |
| 10 | Sudo completion could not be confirmed, locally or over SSH | Do not retry; report that the command may have run |
| 11 | `credential fill` failed after the field may have changed | Do not retry automatically; tell the user the field may hold a partial or complete value and let them inspect it |
| 127 | `run` target could not be executed | The target command is missing, not agentenv |

Diagnostics never echo secret values, so it is safe to relay them to the
user verbatim.

### Windows desktop credential filling

Use `credential fill NAME --backend desktop --expect-pid PID` only after your
desktop automation has focused an empty intended input in that process. Do
not steal focus while filling. Windows requires a writable UI Automation Edit
control and no held modifier keys. It inserts text without clearing or
submitting; `input-sent` does not establish login success. Never read back a
credential to verify it. Capabilities reports desktop runtime availability
as unknown until the actual target is checked. Exit 11 is uncertain mutation:
never retry automatically. Prefer CDP when the browser exposes a supported
loopback debugging endpoint. Windows `keychain` and `command` credentials use
the confidential resolver; no environment-value workaround is needed.
