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

## Installation

Each [GitHub release](https://github.com/ii999/agentenv/releases) ships
prebuilt binaries and a `SHA256SUMS` checksum file for:

| Platform | Archive |
| --- | --- |
| macOS (Apple silicon) | `agentenv-<tag>-aarch64-apple-darwin.tar.gz` |
| macOS (Intel) | `agentenv-<tag>-x86_64-apple-darwin.tar.gz` |
| Linux (x86_64, glibc) | `agentenv-<tag>-x86_64-unknown-linux-gnu.tar.gz` |
| Windows (x86_64) | `agentenv-<tag>-x86_64-pc-windows-msvc.zip` |

### Install script

The scripts download the archive for the current platform over HTTPS,
verify its SHA-256 checksum, and install the matching `agentenv`,
`agentenv-sudo-helper`, and `agentenv-ssh-askpass` bundle plus the agent skill.
They validate the complete bundle before replacing installed executables.

On macOS and Linux:

```bash
curl -fsSL https://raw.githubusercontent.com/ii999/agentenv/main/install.sh | bash
```

The binary lands in `~/.local/bin` and the agent skill in
`~/.agents/skills/agentenv`. Pass `--claude-skills` to also install the
skill to `~/.claude/skills` for Claude Code, `--no-skill` to install the
executable bundle only, `--version <tag>` to pin a release, and `--dir <path>` to
change the binary directory (`AGENTENV_VERSION` and `AGENTENV_INSTALL_DIR`
work the same way). When piping, place options after `bash -s --`:

```bash
curl -fsSL https://raw.githubusercontent.com/ii999/agentenv/main/install.sh \
    | bash -s -- --claude-skills
```

On Windows (PowerShell 7+):

```powershell
Invoke-RestMethod https://raw.githubusercontent.com/ii999/agentenv/main/install.ps1 `
    -OutFile install.ps1
pwsh -File install.ps1
```

The binary lands in `%LOCALAPPDATA%\Programs\agentenv` and the agent skill
in `~\.agents\skills\agentenv`. The matching switches are `-ClaudeSkills`,
`-NoSkill`, `-Version <tag>`, and `-InstallDir <path>`.

### Manual download

```bash
base=https://github.com/ii999/agentenv/releases/download/v0.2.0
curl -fsSLO "$base/agentenv-v0.2.0-aarch64-apple-darwin.tar.gz"
curl -fsSLO "$base/SHA256SUMS"
shasum -a 256 --check --ignore-missing SHA256SUMS
tar -xzf agentenv-v0.2.0-aarch64-apple-darwin.tar.gz
install -m 755 agentenv-v0.2.0-aarch64-apple-darwin/agentenv* ~/.local/bin/
```

Substitute the archive name for your platform from the table above. On Linux,
`sha256sum --check --ignore-missing SHA256SUMS` performs the same
verification.

### Build from source

With a Rust toolchain installed:

```bash
cargo install --path .
```

### Update

An installed binary updates itself:

```bash
agentenv update
```

`update` resolves the latest release from its `SHA256SUMS` file, downloads
the archive for the running binary's platform, verifies the checksum, runs
all three downloaded executables to confirm their identities and release
version, and then replaces the bundle in place. Every installed copy of the agent skill
(`~/.agents/skills/agentenv` and `~/.claude/skills/agentenv`) is refreshed
from the same release; a copy that was never installed stays absent.

- `agentenv update --check` reports the installed and latest versions
  without changing anything; `--json` renders the report as JSON.
- `--version <tag>` installs a specific release, including an older one.
  Without it, `update` refuses to move to a release older than the
  installed version.
- `--force` reinstalls the current version, which repairs a damaged skill
  directory.
- `--no-skill` updates the executable bundle while leaving installed agent
  skills unchanged.

`update` refuses binaries installed by cargo or Homebrew and names the
matching upgrade command instead. `AGENTENV_RELEASE_BASE_URL` points it at
a release mirror. A missing or mismatched companion is never bypassed: rerun
`agentenv update --force` or the installer to repair the complete bundle.

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
2. A non-empty `AGENTENV_PROFILE`
3. The `profile` pin in a trusted project file
4. `default_profile` in the file

The same order applies to reads, `run`, `set`, and `unset`. Creating a profile
is deliberately separate: `--create-profile` still requires an explicit
`--profile <NAME>` and does not use a project pin.

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

### Project-scoped configuration

`agentenv` can discover one checked-in `.agentenv.toml`: it starts in the
working directory and walks toward the filesystem root, using the nearest
regular file. The file is selection-only; it cannot define values, credentials,
or injection mappings.

```toml
version = 1
profile = "work"

[requires.llm]
reason = "Run the application with its configured LLM."
fields = ["model"]
```

The file must use `version = 1`. It may include a non-empty `profile` and
`[requires.<entry>]` tables. Each requirement has a non-empty `reason`; when
present, `fields` lists one or more non-empty entry-relative paths. The file is
limited to 64 KiB and may not contain `credential://` references.

Project files are inert until approved for their exact contents. Approval is
stored in your user state directory, outside the repository; any edit makes the
file untrusted again. Review and manage the discovered file with:

```bash
agentenv project status
agentenv project status --json
agentenv project allow
agentenv project revoke
```

`status` reports discovery, trust, the profile pin, and declared requirements.
`allow` validates and approves the current file; `revoke` removes its approval.
An untrusted file produces one stderr notice for ordinary commands and has no
effect on profile selection.

Most failing JSON commands leave stdout empty. `agentenv project status --json`
is the exception: it writes its JSON report to stdout for status `5`
(untrusted, invalid, or unavailable project trust) and status `6`
(requirements are unsatisfied or cannot be checked). A status `2`
infrastructure failure still leaves stdout empty.

Use `--no-project`, or set `AGENTENV_NO_PROJECT` to a non-empty value, to skip
discovery for an ordinary invocation. The bypass does not apply to `agentenv
project status`, `agentenv project allow`, or `agentenv project revoke`; those
commands always discover the nearest project file.

## Privileged command execution

`sudo` executes one configured target once. Passwords are resolved only through
the authentication channel; never put a password in command arguments,
environment variables, chat, redirected command stdin, or a config value.

```bash
agentenv sudo --with local_admin -- /usr/bin/id -u
agentenv --profile work sudo --with prod_admin -- /usr/bin/systemctl restart nginx
agentenv sudo --with prod_admin --cwd /var/lib/example -- /usr/bin/tee config.json
agentenv sudo --with prod_admin --plan --json -- /usr/bin/systemctl restart nginx
agentenv sudo --with prod_admin --check --json
```

`--plan` performs no credential lookup, SSH evaluation, or connection.
`--check` checks local prerequisites or the remote helper handshake without
requesting a sudo password. Execution uses pipes and no PTY, so interactive
shells, terminal programs, password changes, MFA conversations, and sudoers
`requiretty` policies are unsupported. Local execution refuses a terminal as
stdin; redirect it from a file, a pipe, or `/dev/null`.

### Credentials and targets

Declare each authentication purpose explicitly. `credential update` changes
metadata only; it does not read, copy, or replace the stored value.

```bash
agentenv credential add prod_account \
  --description "Production deploy password." \
  --provider keychain --service agentenv.sudo --account prod/deploy \
  --usage sudo --usage ssh-password
agentenv credential set prod_account
agentenv credential update prod_account --usage sudo --usage ssh-password
```

`credential set` uses hidden local input. Run it directly at a terminal. Do not
pass its value as an argument or environment variable. Authentication
credentials have no `inject_as`; the env provider and `?as=` overrides are not
allowed for authentication use.

Create a complete typed target atomically with `set --type json`. A local
target uses `transport = "local"`. An SSH target uses either an `ssh-config`
alias or a fully explicit hostname, user, and port; both modes also bind a
host-key alias, known-hosts file, remote helper path, and explicit public-key
or saved-password authentication method. The sudo and SSH login references are
independent. They may name one definition that permits both usages, but each
stage requests and validates it separately. Rotating that shared stored value
changes both stages; separate definitions rotate independently.

Host-key material must be enrolled out of band after verifying its fingerprint
with a trusted source. `ssh-keyscan` output alone does not verify server
identity. The selected sudoers rule must authorize the actual target executable
and arguments because agentenv calls that executable directly through sudo; it
does not replace it with a privileged shell or wrapper.

### Remote helper deployment

Remote execution requires the matching unprivileged
`agentenv-sudo-helper` on the destination. Unix release jobs publish it as a
standalone checksummed asset as well as inside the archive. Verify the release
checksum, copy the helper through an independently authorized deployment path,
place it at the absolute user-owned `helper_path`, make it executable, and then
run the configured check:

```bash
agentenv sudo --with prod_admin --check --json
```

The helper runs as the SSH login user and needs no root installation, service,
credential store, Python, or Node.js. agentenv never uploads, downloads,
self-updates, or installs it during execution. A missing or mismatched helper
fails before the sudo password is resolved; repair the deployment explicitly.

### Authentication and failure limits

Each SSH-login and sudo password must be complete UTF-8 of at most 255 encoded
bytes, nonempty, and contain no CR, LF, or NUL. Values are preserved exactly,
including leading and trailing spaces; they are never trimmed or truncated.
One challenge is answered at most once per stage. There is no automatic retry.
Exit code `10` means agentenv could not confirm how the command ended, locally
or over SSH: for example, the connection was lost after the request may have
reached the helper, sudo did not exit after a forwarded cancellation signal,
or output could not be completed. The command may have run; never replay it
automatically. Exit code `9` is an owned execution, protocol, or helper failure.
Both carry a `sudo-execution:` prefix on stderr, which distinguishes them from a
target that itself exits with `9` or `10`. A cancelled run reports the status
that sudo or the remote helper actually observed. A reported target status,
including `255`, remains the target's status when it arrived in a valid remote
result; a later `sudo-execution:` warning such as `ssh-close-failed`,
`output-interrupted`, or `remote-protocol-after-result` does not replace it.

Current compatibility evidence is deliberately limited and is recorded in
`docs/design/sudo-release-evidence.md`:

- Linux destinations were measured only on arm64 Debian 13 with sudo 1.9.16p2,
  using debug builds. The repository labs run 18 local, 37 SSH, and 16 policy
  and refusal cases there. The shipped x86_64 Linux asset has no recorded run.
- The native macOS OpenSSH client passed 8 cases on arm64 against that Linux
  destination. The x86_64 macOS client and macOS local sudo remain unverified.
- Windows builds include the native OpenSSH client boundary, a private named-pipe
  credential resolver and one-shot SSH askpass. Use native `ssh.exe` from Windows
  OpenSSH; MSYS/Cygwin clients and Windows ProxyJump routes are rejected. Native
  Windows local sudo/UAC elevation remains outside this implementation. See
  `docs/design/windows-port.md` for tests and the compatibility boundary.

## Credential filling

`credential fill` delivers one credential into a destination without printing
it. The agent keeps using its own browser automation to navigate and submit;
agentenv only writes the value into the one field the agent names. This
release ships the Chrome DevTools Protocol (CDP) backend, which attaches to an
existing Chromium-family browser through its loopback remote debugging port.
Windows also includes focused desktop input through UI Automation and native
Unicode input. The Playwright backend and non-Windows desktop adapters are not
included; `--capabilities` reports what the installed binary can do.

```bash
agentenv credential fill --capabilities --json
agentenv credential fill portal_password --backend cdp \
  --endpoint http://127.0.0.1:9222 \
  --page-url https://portal.example/login --selector '#password' --json
agentenv credential fill portal_password --backend cdp \
  --endpoint http://127.0.0.1:9222 \
  --page-url https://portal.example/login --page-match origin-path \
  --frame-selector 'iframe#auth' --selector 'input[name=password]' --json
```

The browser must expose a remote debugging port on loopback, for example
Chrome, Chromium, or Edge started with `--remote-debugging-port=9222`. Chrome
and Chromium 136 and later refuse remote debugging on the default profile, so
start the browser with a dedicated `--user-data-dir` as well. Browser
automation tools that already drive a Chromium through CDP can pass their
endpoint directly. Remote endpoints and endpoint authentication are not
supported.

Targeting is strict. `--page-url` must match exactly one open page across all
browser contexts; `--page-match origin-path` ignores the query string and
fragment, which login and single-sign-on pages change between reads;
`--context-index` disambiguates duplicate pages in different contexts, where
`0` is the default context. Each `--frame-selector` must match exactly one
iframe in its parent frame; without any, the main frame is used and child
frames are never searched implicitly. `--selector` must match exactly one
visible, enabled, editable text control: an `input` of a text-like type, a
`textarea`, or a contenteditable element. Open shadow roots are searched;
labels are not retargeted to their controls, and date, color, checkbox, and
file inputs are refused. The existing content is replaced, and the page sees
the same `focus`, `beforeinput`, and `input` events a person typing would
produce, so reactive frameworks observe the change.

Before any credential is resolved, agentenv connects, checks the protocol
version, selects the page, frame chain, and element, and fails if DevTools
is open on that page, because a DevTools recorder's state cannot be read.
Chromium tracing, screencasts, screenshots, and Playwright tracing, HAR, or
video started by another client are not observable and must be kept off by
the automation host during a fill. Only then is the credential resolved,
through the same confidential resolver the sudo channel uses, and the target
is validated again immediately before one `Input.insertText`. A page that
navigated, a frame that was removed, or a selector that became ambiguous in
between fails rather than filling a different element. The connection is then
closed; the browser, its pages, and other clients' sessions are left as they
were.

Only credentials that permit `environment` usage can be filled; sudo and SSH
passwords are refused before resolution. The value must be a single line of
UTF-8 of at most 8,192 bytes, preserved exactly including spaces; control
characters, the U+2028/U+2029 line separators, and the U+202A–U+202E and
U+2066–U+2069 bidirectional controls are rejected as `value-unsupported`. A
command provider runs confidentially (no stdin, stderr discarded) and one
trailing newline is removed from its output; an empty or non-UTF-8 value is
a credential error. The success result is only `{"version":1,"backend":"cdp","effect":"field-filled"}`.
agentenv never reads the field back, takes no screenshot, and does not submit
the form; the effect says the replacement was delivered, not that a login
succeeded, and a later screenshot by another tool can still show a visible
API-key field.

Failures leave stdout empty and write one stderr line
`credential-fill: <reason>: <message>`. Exit code `8` means nothing was
changed: `backend-unavailable`, `connect-failed`, `version-unsupported`,
`recording-conflict`, `context-absent`, `page-absent`, `page-ambiguous`,
`frame-absent`, `frame-ambiguous`, `frame-invalid`, `target-absent`,
`target-ambiguous`, `target-hidden`, `target-disabled`, `target-readonly`,
`target-unfillable`, `target-changed`, `value-unsupported`, `timeout`, or
`cancelled` (SIGINT, SIGTERM, or SIGHUP) before insertion. Exit code `11`
means the destination may have changed: `timeout`, `cancelled`, or
`delivery-failed` during insertion, or `cleanup-unconfirmed` after a
delivered value. Inspect the field before retrying an `11`; never
retry automatically. The whole operation, including credential lookup and any
keychain authorization dialog, runs under one deadline of `--timeout-ms`
(default 30,000, at most 300,000).

### Windows focused input

Windows builds support `env`, Windows Credential Manager (`keychain`), and
`command` providers for CDP and desktop filling. Keychain and command lookup
use a cancellable child process and private named pipe, not CLI stdout.

```powershell
agentenv credential fill --capabilities --json
agentenv credential fill portal_password --backend desktop --expect-pid 12345 --json
```

Prepare an empty, focused input in that process first. The desktop adapter
requires an enabled, visible, writable UI Automation Edit control, rechecks
its identity after lookup, and refuses changed focus or held modifier keys.
It inserts literal Unicode once without using the clipboard, clearing the
field, switching windows, reading its value, or pressing Enter. Success is
`input-sent`, not a claim of login success or verified field content.

Desktop capabilities report `compiled: true` with `available: null`: target
and permission availability can only be checked for the actual invocation.
An interactive Windows desktop is required. Secure desktop/UAC prompts and
input into higher-integrity applications are not supported. A remaining OS
focus race exists between the final check and input dispatch; keep the target
focused while running the operation. Exit 11 means input may have occurred;
inspect the destination before any manual retry. See
`docs/design/windows-port.md` for build and validation instructions.

## Agent usage protocol

A full agent skill ships in `skills/agentenv/` and in every release archive;
it covers discovery, reading, credential-injected runs, writes, and the
no-secret rules in one document. The install scripts place it in
`~/.agents/skills/agentenv` by default, and `--claude-skills`
(`-ClaudeSkills` on Windows) also installs it to `~/.claude/skills/agentenv`
for Claude Code. To wire it up manually, copy the directory to your
runtime's skills location — `~/.claude/skills/agentenv/` for all projects or
`.claude/skills/agentenv/` inside one project.

For runtimes without skill support, projects can place this block in
`AGENTS.md`:

```md
User environment information is available through `agentenv`.

- Run `agentenv project status --json` first to discover project state. Its
  report may be written to stdout with exit status `5` or `6`.
- A project file is the nearest regular `.agentenv.toml`; it contains only
  `version = 1`, an optional profile pin, and optional requirements. Approve
  its exact contents with `agentenv project allow` and remove approval with
  `agentenv project revoke`.
- Profile selection is `--profile`, non-empty `AGENTENV_PROFILE`, a trusted
  project pin, then `default_profile`. This applies to reads, `run`, `set`,
  and `unset`; `--create-profile` still needs an explicit `--profile`.
- Use `--no-project` or non-empty `AGENTENV_NO_PROJECT` to bypass ordinary
  discovery. The bypass does not apply to `agentenv project` subcommands.
- Run `agentenv list --json` to discover available configuration after checking
  project state.
- Run `agentenv show <name> --json` before using an unfamiliar entry.
- Use `agentenv get <path>` to retrieve ordinary values.
- Use `agentenv run --with <entry> -- <command>` when credentials are required.
- Use `agentenv set <path> <value>` to record configuration the user asks you
  to save; define credentials with `agentenv credential add <name> ...` before
  referencing them, and never write a secret value into the file.
- Never print, log, persist, or summarize resolved credentials.
- Status `2` also covers project-file validation errors; status `5` reports
  project trust-state failures and status `6` reports unchecked or unsatisfied
  project requirements.
- Keep non-secret settings in `.env`; for Docker Compose, inject credentials
  with `agentenv run --with llm -- docker compose up`, never an `env_file:`
  containing secrets.
- Report missing configuration or credentials explicitly.
```

Use the commands in this order:

1. Discover project state with `agentenv project status --json`. Read its
   stdout report even when it exits `5` or `6`; an exit `2` leaves stdout empty.
2. Discover available profiles and entries with `agentenv list --json`.
3. Inspect an unfamiliar entry with `agentenv show <name> --json`.
4. Read an ordinary scalar with `agentenv get <path>`; use `--json` for
   an array or table.
5. When a target needs credentials, use
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
agentenv project status
agentenv project status --json
agentenv project allow
agentenv project revoke
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
creates the profile with that description and never uses a project pin. `unset`
removes one field or table. Write commands have no `--json` output mode.

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
agentenv credential add <name> --description "<text>" --provider keychain \
    --service <service> --account <account> --usage sudo [--usage ssh-password]
agentenv credential update <name> --usage <environment|sudo|ssh-password> \
    [--usage <purpose> ...] [--inject-as <ENV>]
agentenv credential fill --capabilities --json
agentenv credential fill <name> --backend cdp --endpoint <http://127.0.0.1:port> \
    --page-url <url> [--page-match exact|origin-path] [--context-index <n>] \
    [--frame-selector <css> ...] --selector <css> [--timeout-ms <ms>] --json
```

Maintenance commands are:

```bash
agentenv update
agentenv update --check
agentenv update --check --json
agentenv update --version <tag>
agentenv update --force
agentenv update --no-skill
```

`update` installs the latest GitHub release over the running binary and
refreshes installed agent skills; see [Update](#update).

`credential list` performs only a shallow status check and does not read a
secret store or execute a provider command. `credential check` resolves one
credential and reports availability without printing its value. Run
`credential set` directly to enter a keychain credential at its hidden terminal
prompt; do not put the value in argv, an environment variable, chat, a command
pipe, or a file. Environment and command credentials are managed by their external
systems and cannot be set by this command. `credential add` writes a
credential definition to the config file — never a value; define the
credential first, then reference it from entries (`credential://<name>`) and,
for the keychain provider, store its value with `credential set`.
`credential update` replaces only the permitted-purpose metadata and validates
the whole file before writing; it never resolves or changes the saved value.
`credential fill` delivers a credential into a browser field; see
[Credential filling](#credential-filling).
`credential list --json` reports an additive `usages` array and always includes
`inject_as`, which is `null` for authentication-only definitions.

## Providers

Prefer `keychain` or `command` for local development:

- `keychain` uses the platform credential store: Keychain on macOS, Credential
  Manager on Windows, and a secret-service implementation such as GNOME
  Keyring or KWallet on Linux.
- `command` executes `argv` directly through the operating system, without a
  shell. For ordinary environment injection, its standard output supplies the
  credential, one trailing newline is stripped, and the command inherits stdin
  and stderr. For `sudo` or `ssh-password` use, the provider is confidential
  and noninteractive: stdin and stderr are closed, stdout is bounded, and its
  complete output is validated without stripping a newline. An authentication
  provider must emit only the password bytes, with no CR, LF, or NUL. For
  `credential fill`, the provider is confidential and noninteractive in the
  same way, but one trailing newline is stripped as for environment injection.
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

### Pure runs

`run --pure` launches the target with a curated minimal environment instead
of the full inherited one. The child environment is built in three layers —
a fixed platform base, then explicitly kept variables, then the planned
injections — and contains nothing else. Stray variables in the calling shell,
including exported tokens meant for other tools, never reach the target.

```bash
agentenv run --pure --with llm -- llm-client request
agentenv run --pure --keep AWS_REGION --with llm -- deploy-tool sync
```

The base is a closed list per platform; no name is carried by prefix or
pattern, and names unset in the parent are not synthesized:

- Unix-like systems: `PATH`, `HOME`, `TMPDIR`, `TERM`, `USER`, `LOGNAME`,
  `SHELL`, `LANG`, `TZ`, `XDG_CONFIG_HOME`, `XDG_STATE_HOME`,
  `AGENTENV_FILE`, `AGENTENV_PROFILE`, `AGENTENV_NO_PROJECT`, and the locale
  variables `LC_ALL`, `LC_COLLATE`, `LC_CTYPE`, `LC_MESSAGES`, `LC_MONETARY`,
  `LC_NUMERIC`, `LC_TIME`, `LC_ADDRESS`, `LC_IDENTIFICATION`,
  `LC_MEASUREMENT`, `LC_NAME`, `LC_PAPER`, `LC_TELEPHONE`.
- Windows: `PATH`, `PATHEXT`, `SystemRoot`, `SystemDrive`, `windir`,
  `ComSpec`, `TEMP`, `TMP`, `USERPROFILE`, `HOMEDRIVE`, `HOMEPATH`,
  `APPDATA`, `LOCALAPPDATA`, `ProgramData`, `ProgramFiles`,
  `ProgramFiles(x86)`, `ProgramW6432`, `CommonProgramFiles`,
  `CommonProgramFiles(x86)`, `CommonProgramW6432`, `ALLUSERSPROFILE`,
  `PUBLIC`, `COMPUTERNAME`, `USERNAME`, `USERDOMAIN`, `OS`,
  `NUMBER_OF_PROCESSORS`, `PROCESSOR_ARCHITECTURE`, `AGENTENV_FILE`,
  `AGENTENV_PROFILE`, `AGENTENV_NO_PROJECT`.

The `AGENTENV_*` names and the platform configuration locations are carried
so that a nested `agentenv` call inside a pure target resolves the same
configuration file and profile as the caller.

`--keep <NAME>` carries one additional parent variable and can be repeated.
Names are exact — `[A-Za-z_][A-Za-z0-9_]*`, the same grammar as `inject_as` —
so platform names outside it, such as Windows names containing parentheses,
are covered by the base only. `--keep` without `--pure`, an invalid name, or
an empty name is a usage error (exit `1`) detected before configuration
loading. A kept name that is unset in the parent is reported on standard
error and the run continues without it; the report concerns inheritance only
and appears even when an injection supplies the same name. Injections
override kept and base variables alike.

TLS and proxy configuration — `SSL_CERT_FILE`, `SSL_CERT_DIR`, `HTTP_PROXY`,
`HTTPS_PROXY`, `NO_PROXY`, and their lowercase forms — is deliberately not in
the base, because proxy URLs can embed credentials. On machines that need
them, carry them explicitly:

```bash
agentenv run --pure --keep HTTPS_PROXY --keep NO_PROXY --with llm -- llm-client request
```

Under `--pure`, a non-absolute target is resolved against the child
environment's `PATH`; the platform's additional search locations (such as the
Windows system directories) remain in effect.

### Docker Compose pairing

Keep non-secret defaults, such as ports and image tags, in `.env`. Supply
credentials only through `agentenv run`, then let Compose pass through the
already-injected variable:

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

Compose can also use `${OPENAI_API_KEY}` interpolation. Do not put secrets in
an `env_file:`; that pattern copies credentials into a file and is the
anti-pattern this workflow avoids.

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

`run --pure` is an environment filter for the launched target, never a
sandbox. The pure target still runs as the same user, in the same working
directory, with inherited standard streams and file descriptors, full
filesystem access — including the user's `agentenv` configuration and the
platform credential store — and unrestricted network access. Credential
resolution is unchanged by `--pure`: a `command` provider's subprocess keeps
the full parent environment.

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
| `2` | Configuration-file error, including validation, project-file validation, corrupt project trust state, or Unix permission failure |
| `3` | Unknown profile, entry, field path, or credential name |
| `4` | Credential resolution/store failure or injection conflict |
| `5` | Project trust-state failure: `status` found an untrusted, invalid, or unavailable project file, or `allow`/`revoke` found no project file |
| `6` | Project requirements are unsatisfied or cannot be checked by `status` |
| `7` | `update` could not resolve, download, verify, or install a release, or replaced the binary but could not refresh an agent skill |
| `8` | `credential fill` failed before anything was changed (stderr prefix `credential-fill: <reason>:`) |
| `9` | `sudo` owned execution, protocol, or helper failure (stderr prefix `sudo-execution:`) |
| `10` | `sudo` completion could not be confirmed; the command may have run (stderr prefix `sudo-execution: completion-unconfirmed:`) |
| `11` | `credential fill` failed after the destination may have changed; inspect it before retrying (stderr prefix `credential-fill: <reason>:`) |
| `127` | `run` target could not be executed |

## License

This project is licensed under the [MIT License](LICENSE).
