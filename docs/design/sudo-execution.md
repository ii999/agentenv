# Local and SSH sudo execution

## Outcome and scope

An agent selects a configured execution target and a command. `agentenv` obtains
the target's sudo password from the local credential provider only when `sudo`
requests it, completes authentication, and streams the command's input and
output. Password bytes never enter the agent's tool arguments or results.
The same interface supports local execution on Unix and remote execution over
SSH from macOS, Linux, and Windows clients to Unix destinations. SSH login can
use existing keys or an explicitly selected locally stored password; login
authentication and sudo authentication remain separate stages.

The execution interface supports noninteractive commands, binary stdin,
separate stdout and stderr streams, concurrent invocations, an explicit run-as
user, and observable exit status. Commands may run for a long time; streaming
does not buffer their entire input or output. Each invocation runs one command
once.

| Component | Supported scope |
| --- | --- |
| Local sudo | macOS and Linux (`#[cfg(unix)]`) with compatible `sudo`/askpass and conventional single-password authentication; unavailable on Windows (`unsupported-platform`) |
| SSH client | macOS, Linux, and Windows (native Win32 `OpenSSH_for_Windows_`; MSYS/Cygwin SSH clients are rejected with `ssh-client-unsupported`) |
| SSH jump routes (`ProxyJump`) | `ssh-config` publickey mode on Linux clients with `OpenSSH_10.0`; rejected on other clients (`jump-client-unsupported`) and in `password` mode (`password-route-unsupported`); `explicit` mode reads no SSH configuration and has no jump route; custom `ProxyCommand` routes are rejected in every mode (`proxy-command-unsupported`) |
| SSH destination | macOS (`Darwin/arm64`, `Darwin/x86_64`) and Linux (`Linux/x86_64`, `Linux/aarch64` with glibc >= 2.28) with compatible `sudo` and the matching unprivileged `agentenv-sudo-helper` binary |
| Password storage | Keychain provider and command provider with confidential resolution I/O in an isolated resolver subprocess |
| SSH authentication | Explicit `publickey` mode or saved `password` mode through `agentenv-ssh-askpass`; `ssh-config` aliases or `explicit` connection settings |
| Command interface | UTF-8 argv without NUL; executable and optional `--cwd` are absolute POSIX paths; stdin, stdout, and stderr carry arbitrary bytes |

Native Windows `sudo` or UAC elevation, Windows remote helper execution,
arbitrary interactive SSH sessions, nested automatic sudo interception,
`sudoedit`, interactive shells, password changes, OTP/MFA, and general PAM
conversations are outside scope. An explicit shell executable can still be the
requested noninteractive command; it receives ordinary sudoers authorization
and is never introduced implicitly. Unsupported capabilities fail explicitly
with stable reason codes rather than switching transport. See
[sudo compatibility](sudo-compatibility.md) for measured platform behavior,
[helper deployment](helper-deployment.md) for remote companion installation,
[Windows port](windows-port.md) for Windows client validation, and
[sudo release evidence](sudo-release-evidence.md) for the dated verification
record.

## Codebase integration and boundaries

- `src/credential/mod.rs` owns provider selection across `env`, `keychain`, and
  `command` providers, exposing ordinary resolution for `run` and confidential
  resolution (`resolve_confidential`) for authentication and fill callers.
- `src/credential/keychain.rs` reads and writes platform credentials.
  `src/cli/credential.rs` provides hidden local input through `credential set`
  and purpose management through `credential add` and `credential update`.
- `src/credential/secret.rs` stores secret bytes in zeroizing memory
  (`Zeroizing<String>` and `Zeroizing<Vec<u8>>`), omits `Display` and
  `Serialize`, redacts `Debug`, and enforces stage-specific validation:
  authentication secrets allow 1 to 255 UTF-8 bytes without CR, LF, or NUL, and
  fill secrets allow 1 to 8,192 UTF-8 bytes of single-line text.
- `src/credential/command.rs` runs confidential command providers with null
  stdin, discarded stderr, and a fixed zeroized capture buffer bounded by the
  confidential ceiling plus overflow detection. Authentication stages preserve
  raw output bytes without stripping trailing newlines (`line_oriented = false`).
- `src/credential/resolver.rs` and `src/credential/resolver/windows.rs` run
  keychain and command resolution in an isolated child process invoked via
  `--agentenv-resolver`, enforcing stage-specific usage gates (`Sudo`,
  `SshPassword`, and `Fill`), caller-owned deadlines, process-group or Job
  Object termination on cancellation, and late-reply disposal.
- `src/config/model.rs` defines `CredentialUsage` (`environment`, `sudo`,
  `ssh-password`), requires `inject_as` only when `usages` is `["environment"]`,
  forbids `inject_as` on authentication credentials, and validates
  `kind = "sudo-target"` entries and their transport-specific `sudo` tables on
  load and write.
- `src/runner.rs` inspects every credential reference across all selected
  entries before resolving any provider or spawning a child process. Any
  reference to an authentication credential—including one carrying an `?as=`
  override—fails preflight as a configuration error (exit code 2).
- `src/cli/sudo.rs` and `src/error.rs` own CLI dispatch and exit-code mapping.
  Exit code 9 reports owned `sudo-execution:` preflight, authentication, or
  protocol failures before target start, and exit code 10 reports lost or
  uncertain completion after execution may have started. Exit codes 8 and 11
  belong to [credential filling](credential-fill.md).

Ordinary `run` continues to perform transparent environment injection. It does
not detect `sudo`, inspect terminal output for password prompts, or rewrite
arbitrary SSH commands. Project files (`.agentenv.toml`) remain selection-only
and cannot define credentials or helper/SSH execution policy.

## Security and authorization model

The protected boundary is normal-workflow password delivery: configuration
contains references, credential resolution is local, and only the intended
SSH-login or sudo authentication path receives its password bytes. Plaintext
passwords never appear in argv, environment variables, generated scripts, disk
spools, diagnostics, process transcripts, or target stdin.

This mechanism is not a sandbox against the invoking user, a malicious root
process, a compromised SSH server, a malicious credential provider, or an agent
with equivalent filesystem or process access deliberately extracting
credentials. The remote host necessarily receives the password during
authentication. Operators should use distinct passwords for distinct accounts
and hosts where possible.

OS sudoers policy remains the authorization authority. The executor invokes
`sudo` with the requested executable and argv directly; it never substitutes
`sudo sh -c`, a privileged `agentenv` wrapper, or `sudo env` around that
command. An account allowed arbitrary `sudo` effectively grants this automation
that same authority. Credential-use restrictions and user-owned target
definitions prevent accidental misuse; they are not an administrator-controlled
command allowlist.

Host and tool approval boundaries continue to apply. Neither a stored password
nor a trusted `.agentenv.toml` authorizes an otherwise unauthorized
administrative action. The runtime never edits `sudoers`, disables host-key
checks, installs missing system software, or creates `NOPASSWD` rules.

The SSH configuration, known-hosts material, local provider, helper binary,
and destination OS are trusted inputs. Login startup output and wire data are
parsed as untrusted bounded input. Owned components never log passwords;
arbitrary target output remains target output and is not secret-redacted. A
malicious target or server is outside this confidentiality boundary.

## Architecture decision: separate authentication from command I/O

| Approach | Benefit | Reason for selection or rejection |
| --- | --- | --- |
| Environment injection plus ad hoc `sudo -S` scripts | Small implementation and no remote installation | Password shares target stdin; unsafe when `sudo` does not consume it; every caller reimplements quoting, failure, and output handling |
| Private password pipe plus privileged wrapper or preserved extra FD | Separates data streams | Wrapper changes which executable `sudoers` authorizes; extra-FD preservation needs nondefault `closefrom_override` policy and is not portable |
| Askpass plus per-invocation broker and remote companion | Preserves real-command authorization and independent stdin | Selected; uses a small unprivileged companion executable and explicit wire and lifecycle contracts |
| Persistent privileged service | Enforces an independent authorization policy | Introduces an unnecessary root daemon, installation surface, and policy engine |

Local Unix execution and remote SSH execution share the same unprivileged Unix
execution engine. An ephemeral Unix-domain socket connects `sudo`'s askpass
process to the engine. The local engine resolves a configured credential
through the local confidential resolver; the remote engine requests that one
credential over the existing authenticated SSH session. No credential store is
installed on the remote host.

`sudo` supports askpass through `-A` and `SUDO_ASKPASS`, whereas `-S` reads
from standard input. Ordinary command execution in `sudo` closes descriptors
above stderr unless overridden by policy, and `sudo`'s askpass child also closes
extra descriptors and runs as the invoking user. Connecting the askpass helper
back to the broker through a mode-`0600` Unix socket in a mode-`0700` runtime
directory avoids relying on inherited file descriptors.
[sudo manual](https://raw.githubusercontent.com/sudo-project/sudo/main/docs/sudo.man.in),
[sudo askpass source](https://raw.githubusercontent.com/sudo-project/sudo/main/src/tgetpass.c)

## CLI contract

Target names in the examples below are illustrative.

```text
agentenv sudo --with local_admin -- /usr/bin/id -u
agentenv --profile work sudo --with prod_admin -- /usr/bin/systemctl restart nginx
agentenv sudo --with prod_admin --cwd /var/lib/example -- /usr/bin/tee config.json
agentenv sudo --with prod_admin --plan --json -- /usr/bin/systemctl restart nginx
agentenv sudo --with prod_admin --check --json
agentenv sudo --with prod_admin --deploy-helper --json
agentenv sudo --with prod_admin --deploy-helper --from ./dist/agentenv-sudo-helper --force
```

- `--with <ENTRY>` is required and selects exactly one top-level
  `kind = "sudo-target"` entry in the resolved profile. Local versus SSH
  behavior comes from that target entry rather than the command.
- Execution accepts one command argv after `--`. The executable path and
  optional `--cwd <PATH>` must be absolute POSIX paths (`/` prefix, valid
  UTF-8, no NUL bytes). There is no inline password, credential override,
  destination override, arbitrary sudo-option passthrough, or automatic shell
  interpolation.
- Profile and project selection follow the standard CLI precedence and trust
  notices (`--profile`, `--no-project`).
- `--plan` requires a command after `--` and conflicts with `--check` and
  `--deploy-helper`. It validates configuration and command arguments and
  prints `profile`, `target`, `transport`, `endpoint` (`local`,
  `ssh-config:<alias> (unresolved)`, or `<user>@<hostname>:<port>`),
  `route_policy` (`local`, `unverified` for `ssh-config`, or `explicit` for
  `explicit`), `credential`, `auth_user`, `run_as`, `cwd` (`null` in JSON or
  `remote initial directory (unresolved)` in text when omitted on an SSH
  target), and `command`. It performs no provider resolution, SSH connection, or
  `ssh -G` evaluation.
- `--check` takes no command and rejects `--cwd`, conflicting with `--plan` and
  `--deploy-helper`. Locally, it verifies that the invoking Unix username
  matches `auth_user`, that `sudo_path` and the companion
  `agentenv-sudo-helper` are safe executables (regular non-symlink files owned
  by root or the invoking user, executable, and not group- or world-writable),
  that the companion's `--identity` line matches the main binary's build, and
  that the private runtime directory and socket can be created. Over SSH, it
  evaluates the route, connects (resolving the SSH login password once if
  `method = "password"`), and completes the `Hello`/`Ready` check handshake, in
  which the remote helper runs the same account, `sudo_path`, and companion
  checks before answering. It emits a `sudo-check:` warning on stderr if
  `helper_path` lies outside the `--deploy-helper` path grammar. `--check`
  never resolves the sudo password or runs a privileged command.
- `--deploy-helper` takes no command and rejects `--cwd`, conflicting with
  `--plan` and `--check`. It requires an SSH target and supports optional
  `--from <PATH>` (one local helper file of 1 byte to 64 MiB to upload instead
  of the bundled companion or the release asset; requires `--deploy-helper`)
  and `--force` (reinstalls even when the destination already reports this
  build's identity; requires `--deploy-helper`). `--force` never overrides the
  refusal of an occupied `helper_path`: a symlink, directory, or file that does
  not report a helper identity fails with `helper-deploy-path-occupied`.
  Deployment runs a read-only preflight over the target's prepared SSH route
  (`uname -s`, `uname -m`, `getconf GNU_LIBC_VERSION`, and the state of
  `helper_path`), selects the helper bytes locally (a downloaded release asset
  must match its entry in the release's `SHA256SUMS`), streams them into a
  sibling temporary file whose byte count and `--identity` output are checked
  on the destination, renames it over `helper_path`, and runs the `--check`
  handshake as acceptance. Full mechanics and reason codes are specified in
  [helper-deployment.md](helper-deployment.md).
- Execution streams raw binary stdout and stderr and rejects global `--json`
  (`sudo execution does not support --json; use --plan --json, --check --json or --deploy-helper --json`).
  Structured JSON output is supported on `--plan`, `--check`, and
  `--deploy-helper`, which leave stdout empty on error.
- Local execution refuses a terminal on stdin (`std::io::stdin().is_terminal()`
  fails with exit code 1) because `sudo` runs in its own process group
  (`process_group(0)`) and would be stopped by the kernel if a child attempted
  to read from the controlling terminal. Callers that do not stream data pipe
  `/dev/null` into stdin.
- When `--cwd` is omitted locally, `sudo` inherits the caller's current
  directory. When `--cwd` is omitted over SSH, the remote helper uses the SSH
  session's initial directory reported in `Ready`. Because `--cwd` is applied before spawning `sudo`, it must be
  accessible to the invoking unprivileged user.
- Connection setup and credential-response deadlines are finite and configurable:
  - `--connect-timeout-secs <SECONDS>` (default `30`, range `1..=300`) bounds
    setup through `Ready`, excluding time spent waiting inside the local
    credential resolver subprocess.
  - `--auth-timeout-secs <SECONDS>` (default `60`, range `1..=300`) bounds each
    credential resolution and response stage (one budget for the SSH login
    password request if used, and one budget from a validated sudo
    `PasswordRequest` through `PasswordResponse`).
  No authentication timer runs merely because `sudo` has spawned, so a
  long-running `NOPASSWD` command is never mistaken for stalled authentication.
  There is no default whole-command execution timeout or automatic retry.

## Credential and target configuration

Configuration uses version-1 TOML and `credential://` references. The optional
`usages` array on a credential definition controls which subsystems may resolve
it:

- Omitted or `["environment"]` permits ordinary environment injection (`run`)
  and [credential filling](credential-fill.md), and requires `inject_as`.
- `["sudo"]`, `["ssh-password"]`, or `["ssh-password", "sudo"]` permits the
  named authentication stage(s) and forbids `inject_as` and reference `?as=`
  overrides.
- Empty arrays, duplicate entries, unknown usage names, and mixing
  `environment` with an authentication usage are rejected at configuration load
  and write.
- Authentication credentials must use the `keychain` or `command` provider; the
  `env` provider is rejected because authentication secrets must not start in
  the inherited process environment.

```toml
[credentials.local_sudo]
description = "Local administrator account password."
provider = "keychain"
service = "agentenv.sudo"
account = "local/operator"
usages = ["sudo"]

[credentials.prod_sudo]
description = "Production deploy account sudo password."
provider = "keychain"
service = "agentenv.sudo"
account = "prod/deploy"
usages = ["sudo"]

[profiles.work.local_admin]
description = "Local administrative commands."
kind = "sudo-target"

[profiles.work.local_admin.sudo]
transport = "local"
credential = "credential://local_sudo"
auth_user = "operator"
run_as = "root"
sudo_path = "/usr/bin/sudo"

[profiles.work.prod_admin]
description = "Production administrative commands over SSH."
kind = "sudo-target"

[profiles.work.prod_admin.sudo]
transport = "ssh"
credential = "credential://prod_sudo"
auth_user = "deploy"
run_as = "root"
sudo_path = "/usr/bin/sudo"

[profiles.work.prod_admin.sudo.ssh]
mode = "ssh-config"
host_alias = "prod"
host_key_alias = "agentenv-prod"
known_hosts_file = "/Users/operator/.ssh/agentenv_known_hosts"
helper_path = "/home/deploy/.local/libexec/agentenv-sudo-helper"

[profiles.work.prod_admin.sudo.ssh.auth]
method = "publickey"
```

Setting `kind = "sudo-target"` on a profile entry opts it into closed-schema
validation. Unmarked profile entries remain open-schema environment tables. A
`sudo-target` entry allows only `description`, `kind`, and `sudo`, and its
`sudo.credential` reference must target a defined credential whose `usages`
include `sudo` without `?as=` query parameters. The `local` transport rejects
an `ssh` subtable; the `ssh` transport requires it. `auth_user` must match the
invoking local Unix username or remote SSH login account; `run_as` may differ
and is passed to `sudo -u`.

Configuration validation applies three path grammars, each limited to 4,096
bytes without control characters:

- `sudo_path` is a destination executable: it must start with `/`; spaces and
  other printable characters are accepted. At execution time the local engine
  (or the remote helper) additionally requires a regular, non-symlink,
  executable file owned by root or the invoking user and not group- or
  world-writable (`sudo-unavailable` otherwise).
- `helper_path` on SSH targets is inserted unquoted into the remote login-shell
  command `<helper_path> --serve`, so it must start with `/`, be ASCII, and
  contain no backslash, whitespace, shell metacharacter
  (`` ' " ` $ ; & | < > * ? ( ) [ ] { } ! ``), or `.`/`..` segment. The
  stricter `--deploy-helper` grammar additionally limits the path to 1,024
  bytes of `[A-Za-z0-9._/+-]`, forbids empty segments and a trailing slash, and
  requires the file name `agentenv-sudo-helper`, because the serving helper
  starts the `agentenv-sudo-helper` beside itself as `sudo`'s askpass program.
  `--check` warns about a configured path outside that grammar.
- `known_hosts_file`, `config_file`, and `identity_files` are files read by the
  local SSH client. Each must be non-empty and absolute in a portable sense: a
  `/` prefix, a drive-letter path, or a UNC path is accepted regardless of the
  client platform.

Host aliases, hostnames, usernames, `run_as`, and `host_key_alias` values must
be non-empty, at most 255 bytes, and free of a leading `-`, whitespace, and
control characters. Host names and aliases additionally reject `@`, `/`, and
`\`; account names reject `@`, `:`, `/`, and `\`.

`credential add` accepts repeatable `--usage <environment|sudo|ssh-password>`
flags with conditional `--inject-as` validation, and `credential update <name>
--usage ... [--inject-as ...]` updates permitted usages on an existing
credential definition without reading or rewriting its stored secret. Whole-file
validation rejects any change that would leave an existing profile entry or
`sudo-target` referencing a credential with incompatible usages.

```text
agentenv credential add prod_sudo --description "Production sudo password." \
  --provider keychain --service agentenv.sudo --account prod/deploy --usage sudo
agentenv credential set prod_sudo
```

Target entries are written atomically in a single `set --type json` operation
containing `description`, `kind`, and `sudo`. `credential list` and query
commands report `usages` and a nullable `inject_as` (`null` for authentication
credentials).

## SSH configuration sources and credential matching

The selected `sudo-target` entry explicitly binds the SSH connection source,
optional SSH login credential, and sudo credential. Credentials are never
matched by display name, IP address, keychain labels, or hostname heuristics.

The two SSH connection modes are mutually exclusive:

| Mode | Connection source | Validation and behavior |
| --- | --- | --- |
| `ssh-config` | `host_alias` selects an OpenSSH `Host` entry; optional absolute `config_file` selects a specific file | Evaluates `Host`/`Include`/`Match` through `ssh -G` (`-F <config_file>` when set); extracts effective `hostname`, `user`, `port`, and route policy. |
| `explicit` | Required `hostname`, `user`, `port` (an integer from 1 through 65535; there is no default), `host_key_alias`, `known_hosts_file`, `helper_path`, and `auth` in `agentenv` | Invokes OpenSSH with `-F none` and explicit flags; reads neither user nor system SSH config files. |

In `ssh-config` mode, execution, `--check`, and `--deploy-helper` run a bounded
`ssh -G` evaluation before connecting. Because OpenSSH `Match exec` directives
can run local commands during `ssh -G`, this step is never executed during
`--plan`, which reports `ssh-config:<alias> (unresolved)` and
`route_policy = "unverified"`.

`agentenv` compares the effective `user` from `ssh -G` against `auth_user`
before resolving any password (`account-mismatch`), and pins `-l <user>`,
`-p <port>`, and `-o HostName=<hostname>` on the spawned `ssh` command while
passing
`<host_alias>` as the destination token so remaining `ssh_config` directives
continue to apply. In `explicit` mode, `user` must equal `auth_user` at
configuration validation time.

The authentication method is always explicit in `sudo.ssh.auth` and overrides
inherited SSH preferences. Host-key verification is always pinned via
`StrictHostKeyChecking=yes`, `HostKeyAlias=<host_key_alias>`,
`UserKnownHostsFile=<known_hosts_file>`, `GlobalKnownHostsFile=none`,
`UpdateHostKeys=no`, `VerifyHostKeyDNS=no`, and `KnownHostsCommand=none`.

The following configuration uses `explicit` mode and shares one stored account
password across both SSH login and `sudo`:

```toml
version = 1
default_profile = "work"

[profiles.work]
description = "Work machines."

[credentials.prod_account]
description = "Deploy account password for the production host."
provider = "keychain"
service = "agentenv.accounts"
account = "prod/deploy"
usages = ["ssh-password", "sudo"]

[profiles.work.prod_admin]
description = "Production administrative commands."
kind = "sudo-target"

[profiles.work.prod_admin.sudo]
transport = "ssh"
credential = "credential://prod_account"
auth_user = "deploy"
run_as = "root"
sudo_path = "/usr/bin/sudo"

[profiles.work.prod_admin.sudo.ssh]
mode = "explicit"
hostname = "203.0.113.10"
user = "deploy"
port = 2222
host_key_alias = "agentenv-prod"
known_hosts_file = "/Users/operator/.config/agentenv/known_hosts"
helper_path = "/home/deploy/.local/libexec/agentenv-sudo-helper"

[profiles.work.prod_admin.sudo.ssh.auth]
method = "password"
credential = "credential://prod_account"
```

When SSH login and `sudo` use different passwords, configure two credential
definitions (`usages = ["ssh-password"]` and `usages = ["sudo"]`) and reference
each at its respective field. `agentenv` never falls back from one credential
to the other, and sharing a definition still enforces separate one-shot budgets
for SSH login and `sudo`.

In `ssh-config` mode, `method = "publickey"` accepts no further fields; keys and
agent use come from the evaluated SSH configuration. In `explicit` mode,
`publickey` requires `identity_files` (an array of absolute paths, empty when
only the agent is used) and an explicit boolean `use_agent`, with at least one
non-empty `identity_files` entry or `use_agent = true`. The client passes
`IdentityFile=none` followed by each configured file; `use_agent = true` adds
`IdentitiesOnly=no`, and `use_agent = false` adds `IdentitiesOnly=yes` and
`IdentityAgent=none`. `password` mode requires `credential` (with
`ssh-password` usage) and accepts no other field.
[OpenSSH configuration selection](https://man.openbsd.org/ssh.1)

### Publickey jump-route policy

OpenSSH implements `ProxyJump` by spawning child `ssh` processes (`ssh -W
'[host]:port' ...`) that do not inherit the outer client's `-o` command-line
restrictions such as `BatchMode`, authentication methods, host-key policy, or
multiplexing controls.
[OpenSSH jump-command construction](https://github.com/openssh/openssh-portable/blob/master/ssh.c)

`agentenv` permits native `ProxyJump` only when all of the following hold:

1. The target uses `mode = "ssh-config"` and `method = "publickey"`. A jump
   route with `method = "password"` fails with `password-route-unsupported`.
   `explicit` mode runs OpenSSH with `-F none`, so no configuration can supply
   a jump route there.
2. The local client is Linux and `ssh -V` reports `OpenSSH_10.0`, where child
   jump-command construction is verified
   ([sudo compatibility](sudo-compatibility.md)). Any other client fails with
   `jump-client-unsupported` before connecting.
3. Before launching `ssh`, `agentenv` evaluates every hop of the jump chain
   (one to eight hops; another count or malformed hop syntax fails with
   `jump-route-invalid`) with `ssh -G`, using the same config file, the hop's
   user and port overrides, the `-W` forwarding target, and the remaining `-J`
   chain that OpenSSH constructs. Every hop must already satisfy this effective
   policy in SSH configuration; a violation fails with `ssh-policy-rejected`:
   - `batchmode yes`, `pubkeyauthentication yes`, `passwordauthentication no`,
     `kbdinteractiveauthentication no`, `gssapiauthentication no`,
     `hostbasedauthentication no`, `identitiesonly yes`, and
     `preferredauthentications publickey`
   - `stricthostkeychecking yes`, `updatehostkeys no`, `verifyhostkeydns no`,
     `globalknownhostsfile none`, no `knownhostscommand`, and exactly one
     explicit `userknownhostsfile`
   - an explicit identity file or identity agent, and no security-key or
     PKCS#11 provider
   - `forwardagent no`, `forwardx11 no`, `forwardx11trusted no`,
     `permitlocalcommand no`, `controlmaster no`, `controlpersist no`, no
     `controlpath`, and either no configured local, remote, or dynamic
     forwardings or `clearallforwardings yes`
   - no `SendEnv` or `SetEnv` entry that could export the askpass routing
     variables, and no nested route other than the remaining validated chain

A custom `ProxyCommand` on the destination or on any hop fails with
`proxy-command-unsupported`. A hop endpoint (`<user>@<hostname>:<port>`) that
repeats another hop or the final destination fails with `jump-cycle`, and a
reused effective trust identity (`hostkeyalias`, or the hop's hostname, compared
with the destination's `host_key_alias`) fails with `jump-trust-identity`. All
of these checks complete before any network connection or secret resolution
starts.

## Saved-password SSH login

SSH authentication supports two explicit methods with no cross-method fallback:

| Method | OpenSSH options enforced | Credential handling |
| --- | --- | --- |
| `publickey` | `BatchMode=yes`, `PubkeyAuthentication=yes`, `PreferredAuthentications=publickey`, `PasswordAuthentication=no`, `KbdInteractiveAuthentication=no` | Configured keys/agent; no SSH login credential reference |
| `password` | `BatchMode=no`, `PreferredAuthentications=password`, `PasswordAuthentication=yes`, `PubkeyAuthentication=no`, `KbdInteractiveAuthentication=no`, `NumberOfPasswordPrompts=1`, `IdentityAgent=none` (plus `IdentityFile=none` in `explicit` mode) | One `ssh-password` credential delivered through a private local `agentenv-ssh-askpass` session |

Both methods also set `GSSAPIAuthentication=no` and
`HostbasedAuthentication=no`.

`BatchMode=yes` disables password prompting, whereas `BatchMode=no` combined
with `SSH_ASKPASS` and `SSH_ASKPASS_REQUIRE=force` routes password reads to the
askpass companion without using terminal or standard input. The server must
accept the SSH `password` authentication method; servers requiring
`keyboard-interactive` or multi-factor prompts fail explicitly.
[OpenSSH authentication options](https://man.openbsd.org/ssh_config.5),
[OpenSSH askpass behavior](https://man.openbsd.org/ssh.1)

In `password` mode, the client starts a single-use local askpass broker bound
to the spawned `ssh` child, target, effective account, and credential
reference. Before opening the broker it requires the companion
`agentenv-ssh-askpass` executable beside `agentenv` to print exactly
`agentenv-ssh-askpass 1 <version>` for `--identity`. It then sets
`SSH_ASKPASS` to that companion, `SSH_ASKPASS_REQUIRE=force`, and
`DISPLAY=agentenv:0` in the child's environment and removes any inherited
`SSH_ASKPASS_PROMPT`:

- **Unix askpass IPC (`src/sudo/ssh_askpass.rs`):** The broker creates a
  mode-`0700` runtime directory and a mode-`0600` Unix domain socket, passing
  the socket path (`AGENTENV_SSH_SOCKET`) and a 192-bit random session marker
  encoded as 48 hexadecimal characters (`AGENTENV_SSH_SESSION`) to `ssh`. On
  connection, the broker verifies the peer UID against the current process
  UID, and the companion reports its parent PID, which must equal the spawned
  `ssh` PID.
- **Windows askpass IPC (`src/sudo/ssh_askpass/windows.rs`, `src/windows.rs`):**
  The broker creates a single-instance, local-only named pipe
  (`\\.\pipe\agentenv-ssh-<pid>-<nonce>`) with `PIPE_REJECT_REMOTE_CLIENTS` and
  a protected DACL granting access only to the current user SID
  (`D:P(A;;GA;;;<sid>)`), passing the pipe name (`AGENTENV_SSH_PIPE`), the
  broker PID (`AGENTENV_SSH_PARENT`), and the session marker
  (`AGENTENV_SSH_SESSION`) to `ssh`. `agentenv-ssh-askpass.exe` opens the pipe
  with `SECURITY_ANONYMOUS` impersonation. The broker verifies the client PID
  (`GetNamedPipeClientProcessId`), checks that the client's canonical executable
  image (`QueryFullProcessImageNameW`) matches the companion
  `agentenv-ssh-askpass.exe`, and verifies that the client's parent PID equals
  the spawned `ssh.exe` PID before accepting the request. After writing the
  reply frame, the broker waits for a single `0x06` acknowledgement byte before
  closing the pipe so Windows does not tear down the pipe while the client is
  still reading.

Strict host-key verification happens inside OpenSSH before `ssh` invokes
`SSH_ASKPASS`. The askpass companion refuses any invocation with
`SSH_ASKPASS_PROMPT` set (OpenSSH sets it for confirmation and notification
prompts), the broker accepts only one connection (the Unix broker removes its
socket after the first request, and the Windows pipe has a single instance),
and the broker requires the session marker and the exact prompt string
`"<user>@<host_key_alias>'s password: "` (OpenSSH formats the password prompt with `HostKeyAlias` when that option is
pinned). Host-key confirmation prompts, passphrase prompts, and password-change
prompts never match and never trigger credential lookup.
[OpenSSH askpass implementation](https://raw.githubusercontent.com/openssh/openssh-portable/master/readpass.c)

SSH login lookup necessarily occurs before the remote helper's `Ready`
handshake, which gates sudo password release. One-shot budgets are independent:
at most one SSH login password response and at most one sudo password response
per invocation. The askpass routing variables (`SSH_ASKPASS`,
`SSH_ASKPASS_REQUIRE`, and `AGENTENV_SSH_*`) never reach the remote host:
`-o SendEnv=-*` clears inherited `SendEnv` patterns, and the effective
configuration from `ssh -G` is rejected (`ssh-policy-rejected`) when a
remaining `SendEnv` pattern is anything other than `LANG`, `TZ`, or `LC_*`, when
a `SetEnv` entry names a reserved variable, or, in `password` mode, when any
`SetEnv` entry is present.

### Password values at both authentication boundaries

Both the local SSH askpass broker and the sudo `PasswordResponse` path validate
the resolved `Secret` with `Secret::validate_authentication` before transmitting
any bytes:

- The value must be non-empty UTF-8 containing no CR (`\r`), LF (`\n`), or NUL
  (`\0`) bytes.
- All other UTF-8 bytes—including leading and trailing spaces—are preserved
  without trimming or normalization.
- Both stages enforce a maximum length of 255 UTF-8 bytes
  (`SUDO_PASSWORD_LIMIT = 255`, matching the measured destination `sudo` PAM
  buffer ceiling of 255 bytes and staying well within OpenSSH's 1,024-byte
  `read_passphrase` buffer; see [sudo compatibility](sudo-compatibility.md)).

Validation runs on every actual password request, including SSH-only
credentials, shared `["ssh-password", "sudo"]` credentials, and SSH password
login followed by `NOPASSWD` sudo. Invalid values fail with exit code 4
(credential error) without echoing candidate bytes and immediately terminate
the authentication exchange. The remote helper also validates the received
`PasswordResponse` frame before writing it to the sudo askpass socket.

## Local execution and authentication lifecycle

1. **Preflight:** Select and validate the target and command request. Verify
   that the platform is Unix (`unsupported-platform` on Windows), that the
   real and effective UIDs agree and the invoking Unix username matches
   `auth_user` (`account-mismatch`, or `account-unavailable` when the account
   cannot be read), that `sudo_path` is a safe executable (`sudo-unavailable`),
   and that the companion `agentenv-sudo-helper` beside `agentenv` is a safe
   executable (`helper-missing`) whose `--identity` output is exactly
   `agentenv-sudo-helper <protocol> <version>` for this build
   (`helper-mismatch`, or `helper-timeout` when it does not answer within
   `--connect-timeout-secs`). A requested `--cwd` must be an existing directory
   (`invalid-cwd`).
2. **Broker setup:** Create a unique mode-`0700` runtime directory
   (`agentenv-sudo-*` in the platform temporary directory, `TMPDIR` or `/tmp`)
   and bind a mode-`0600` Unix domain socket (`askpass.sock`). Verify the
   ownership, type, and permissions of both (`broker-setup`) and generate a
   192-bit random session marker encoded as 48 hexadecimal characters.
3. **Spawn `sudo`:** Launch `sudo_path` in its own process group
   (`process_group(0)`) with `-A -k -u <run_as> -p
   'agentenv sudo [<session>] password for %p:' -- <executable> <args...>`.
   No preliminary `sudo -v`, `sudo -n`, or wrapper binary is used. `-k` ignores
   and avoids updating `sudo`'s cached timestamp while still allowing
   `NOPASSWD` rules to succeed without a prompt.
   [sudo cache behavior](https://raw.githubusercontent.com/sudo-project/sudo/main/docs/sudo.man.in)
4. **Curated environment:** Start from an empty environment, set
   `PATH=/usr/bin:/bin:/usr/sbin:/sbin`, and copy only `HOME`, `LANG`,
   `LC_ALL`, `LC_CTYPE`, and `TZ` when they are set. Add
   `SUDO_ASKPASS=<companion helper path>`, `AGENTENV_SUDO_SOCKET` (the socket
   path), and `AGENTENV_SUDO_SESSION` (the session marker). Every other
   inherited variable, including dynamic-loader variables, debug hooks, and
   inherited secrets, is absent. `sudo` then applies its own `env_reset`
   policy to the target command (`sudo -E` is never used).
5. **Prompt and peer verification:** When `sudo` requires a password, it runs
   `agentenv-sudo-helper` with the expanded prompt argument. The helper
   requires `AGENTENV_SUDO_SOCKET`, `AGENTENV_SUDO_SESSION`, and a prompt that
   starts with `agentenv sudo [<session>] password for ` and ends with `:`
   before connecting, then sends the session marker and prompt over the Unix
   socket. The broker verifies the peer UID (`getpeereid` / `SO_PEERCRED`,
   `peer-rejected`), the session marker, and the exact expected prompt
   `agentenv sudo [<session>] password for <auth_user>:`. If PAM or
   `rootpw`/`targetpw`/`runaspw` replaces the prompt or requests a different
   account's password, the broker refuses the request and fails with
   `authentication-rejected` without resolving the credential. A malformed
   request fails with `broker-invalid`, and a request that does not arrive
   within `--connect-timeout-secs` after the helper connects fails with
   `helper-timeout`.
   [sudoers authentication options](https://raw.githubusercontent.com/sudo-project/sudo/main/docs/sudoers.man.in)
6. **Lazy, one-shot resolution:** The broker accepts one connection. After a
   valid request it removes the socket file, so a second askpass invocation
   (for example after a rejected password) cannot connect and `sudo` receives
   no further password. Only then does the local coordinator invoke the
   confidential resolver within `--auth-timeout-secs`; a provider failure,
   timeout, or invalid value refuses the helper and fails with exit code 4. If
   `sudo` satisfies policy via `NOPASSWD`, no credential lookup occurs.
7. **Output and cleanup:** Standard input, stdout, and stderr are passed to
   `sudo` directly and stream independently. A broker failure terminates the
   `sudo` process group as described under cancellation below. On completion,
   the engine reaps `sudo`, drops secret buffers, and removes the socket and
   runtime directory. Because `agentenv-sudo-helper` in askpass mode inherits
   `sudo`'s stderr, it writes nothing to stderr and reports a failure only
   through its exit status.

## Remote execution over SSH

The remote `agentenv-sudo-helper` runs as the unprivileged SSH login user. It
has no local credential configuration, keychain integration, setuid bit,
listening network port, or background daemon. It runs the same Unix `sudo` and
askpass socket engine as local execution, delegating any validated askpass
challenge over the framed SSH protocol to the local client.

The client spawns one foreground OpenSSH session with `-T`, the pinned
`HostName`, `-l <user>`, and `-p <port>`, the authentication options above, and
a fixed policy set: host-key pinning (`StrictHostKeyChecking=yes`,
`HostKeyAlias`, `UserKnownHostsFile`, `GlobalKnownHostsFile=none`,
`UpdateHostKeys=no`, `VerifyHostKeyDNS=no`, `KnownHostsCommand=none`); no
multiplexing (`ControlMaster=no`, `ControlPath=none`, `ControlPersist=no`);
no forwarding or local hooks (`ForwardAgent=no`, `ForwardX11=no`,
`ForwardX11Trusted=no`, `PermitLocalCommand=no`, `ClearAllForwardings=yes`,
`ExitOnForwardFailure=yes`, `GatewayPorts=no`, `Tunnel=no`,
`EnableEscapeCommandline=no`); no alternative key providers
(`SecurityKeyProvider=none`, `PKCS11Provider=none`); a plain session
(`RequestTTY=no`, `StdinNull=no`, `ForkAfterAuthentication=no`,
`SessionType=default`, `RemoteCommand=none`); and `SendEnv=-*`. The same
arguments are first evaluated with `ssh -G`, and the session is refused when
OpenSSH did not retain the pinned endpoint (`endpoint-pin-failed`) or a
required option (`ssh-policy-rejected`). The remote command is the single
fixed string `<helper_path> --serve`. Target argv, optional `--cwd`, `run_as`,
and `auth_user` travel inside the binary wire protocol over SSH stdin/stdout
rather than through the remote login shell. `ssh` runs in its own process
group, so a terminal signal reaches only `agentenv`, which forwards it as
`Cancel`.
[OpenSSH client](https://man.openbsd.org/ssh.1),
[sshd command execution](https://man.openbsd.org/sshd.8)

On `Hello`, the helper runs the same preflight as local execution against the
SSH login account: the account must equal `auth_user`, `sudo_path` must be a
safe executable, the `agentenv-sudo-helper` beside the serving helper must
report this build's identity, and the runtime directory must be creatable. A
failed preflight answers `Hello` with a `Failure` frame instead of `Ready`,
which the client reports as `helper-handshake-invalid`. A successful preflight
answers with `Ready`. A first frame of another kind, a `Ready` whose request
ID differs from `Hello`'s, or `Ready` metadata that does not decode fails with
`helper-handshake-invalid`. A decoded `Ready` is accepted only when `build`
equals the client version, `protocol` is `1`, `platform` is `linux` or
`macos`, `auth_user` equals the target's `auth_user`, `password_limit` is
between 1 and 255, `cwd` is an absolute path without NUL, and `features`
includes `one-shot-auth`, `binary-streams`, `credits`, and `cancel`; any other
decoded `Ready` fails with `helper-identity-mismatch`. These checks
complete before the client sends `Start` or resolves any sudo credential. When
`helper-identity-mismatch` or `helper-handshake-missing` occurs, the
diagnostic ends with the remediation command
`agentenv --profile=<profile> sudo --with=<entry> --deploy-helper`.

### Wire contract

The client and `agentenv-sudo-helper --serve` communicate over SSH stdin and
stdout using a versioned binary framing layer (`src/sudo/protocol.rs`,
`src/sudo/transport.rs`):

- **Frame header (20 bytes):** 4-byte ASCII magic `AGEP`, `u16` big-endian
  protocol version (`1`), `u16` message kind (`1..=16`), `u64` nonzero request
  ID, and `u32` payload length validated before allocation. The client draws
  one random nonzero request ID per invocation and uses it on every frame,
  including `Hello`; the helper adopts the ID of `Hello` for the rest of the
  session.
- **Payload limits:** 64 KiB (`65,536` bytes) for JSON metadata frames, 32 KiB
  (`32,768` bytes) for `StdinChunk`, `StdoutChunk`, and `StderrChunk` frames,
  and 255 bytes for `PasswordResponse` frames. `PasswordUnavailable` and the
  three `*Eof` frames must have empty payloads. Metadata payloads are JSON
  objects whose decoders reject unknown fields.
- **Flow control:** Each data stream (`stdin`, `stdout`, `stderr`) begins with
  an initial credit window of 128 KiB (`131,072` bytes, also the maximum credit
  window). Senders decrement credit by chunk byte length before transmitting
  and pause data chunks when credit is exhausted; receivers issue
  `WindowUpdate` frames as chunks are written to their sink. Control frames use
  a separate priority queue so blocked stream writers cannot starve
  `PasswordRequest`, `PasswordResponse`, `WindowUpdate`, `Cancel`, `Result`, or
  `Failure`.

| Direction | Message (`u16` kind) | Purpose |
| --- | --- | --- |
| Client to helper | `Hello` (`1`) | Select the mode and supply the helper's preflight inputs: `mode` (`check` or `execute`), `sudo_path`, `auth_user`, `setup_timeout_secs`, and `auth_timeout_secs` |
| Helper to client | `Ready` (`2`) | Report `build` (the helper's release version), `protocol`, `platform`, `auth_user`, `uid`, `cwd` (the session's initial directory), `password_limit` (`255`), and `features` |
| Client to helper | `Start` (`3`) | Send one immutable execution request: `executable`, `arguments`, `cwd` (or `null`), `run_as`, and `auth_user`, which must equal the `Hello` value |
| Helper to client | `PasswordRequest` (`4`) | Forward one validated askpass challenge with the `auth_user` it is for |
| Client to helper | `PasswordResponse` (`5`) / `PasswordUnavailable` (`6`) | Deliver the password as the raw payload (1 to 255 bytes), or report with an empty payload that no password will be sent |
| Client to helper | `StdinChunk` (`7`) / `StdinEof` (`8`) | Stream target stdin bytes and EOF without closing the SSH control channel |
| Helper to client | `StdoutChunk` (`9`) / `StdoutEof` (`10`) / `StderrChunk` (`11`) / `StderrEof` (`12`) | Stream target stdout and stderr independently and signal per-stream completion |
| Both | `WindowUpdate` (`13`) | Grant incremental byte credit (`stream` and `bytes`, up to the 131,072-byte window) for `stdin`, `stdout`, or `stderr` |
| Client to helper | `Cancel` (`14`) | Forward the received termination `signal` to the running `sudo` process group without replay |
| Helper to client | `Result` (`15`) / `Failure` (`16`) | Report `exit_code`, `signal`, and `password_delivered`, or a `Failure` whose `reason` is `preflight`, `protocol`, `credential`, or `completion_unknown` |

In `check` mode, `Ready` is terminal: the helper closes its output and the
client requires end of stream next (`helper-check-invalid` otherwise). In
`execute` mode, `Ready` is followed by exactly one `Start`. Any startup banner
on remote stdout fails frame verification before `Ready`
(`helper-handshake-invalid`), and a stream that ends before `Ready` is
`helper-handshake-missing`, or `ssh-connect-failed` when `ssh` exits with
status 255 (a connection, host-key, or login failure); neither releases the
sudo password. The `ssh` process's own stderr is read continuously through a
4 KiB buffer and discarded: it is never relayed as target stderr, retained,
or parsed, so diagnostics come from exit statuses and protocol state. Target
stderr arrives only in `StderrChunk` frames.

### Status, cancellation, and uncertain execution

A terminal `Result` frame carries `exit_code`, optional `signal`, and
`password_delivered`. `agentenv` returns the observed `sudo` or target exit
code unchanged (converting a terminating signal `sig` to `128 + sig` when
`exit_code` is absent). Because `sudo` itself can exit with status 1 on
authentication rejection or policy refusal and a target command can also exit 1,
`agentenv` never guesses whether exit 1 means a wrong password.

Owned failures preserve existing `agentenv` preflight exit codes (`1` usage,
`2` configuration, `3` unknown credential, `4` credential resolution) and use
two sudo-specific codes:

- **Exit code 9:** A failure whose outcome is known: preflight, SSH login,
  helper handshake, deployment, or protocol failure before `Start`, or a
  `Failure` frame after `Start` that confirms the command did not run
  (`remote-preflight-failed` or `remote-protocol-failed`). Stderr prints
  `sudo-execution: <reason>: <detail>`. A `Failure` frame with reason
  `credential` is a credential error (exit code 4).
- **Exit code 10:** Completion is unconfirmed: the target command may have
  started or may still be running. Stderr prints
  `sudo-execution: completion-unconfirmed: [<reason>: ]<detail>`. Over SSH,
  every such case reports `remote-result-missing`: a lost connection, protocol
  violation, failed password delivery, timeout, or unanswered cancellation
  once `Start` may have reached the helper, and a `Failure` frame with reason
  `completion_unknown`. The local engine reports an unconfirmed outcome
  without a further reason code when it cannot observe, signal, or reap `sudo`.
  The command is never replayed automatically.

The reason codes are listed in `src/sudo/error.rs`; deployment failures carry
the `helper-deploy-` prefix described in
[helper-deployment.md](helper-deployment.md). When a remote `Result` was
observed but the session did not end cleanly, `agentenv` still returns the
observed status and adds a warning line to stderr:
`sudo-execution: ssh-close-failed: ...`,
`sudo-execution: output-interrupted: ...`, or
`sudo-execution: remote-protocol-after-result: ...`.

Signal cancellation (`SIGINT`, `SIGTERM`, `SIGHUP` on Unix; `Ctrl+C` /
`Ctrl+Break` on Windows) aborts pending credential resolution. A signal that
arrives before `Start` is sent, or before the local `sudo` process exists,
ends the invocation without running the command and exits with `128 + signal`.
After that point the two transports differ:

- **Local:** The engine forwards the received signal to the `sudo` process
  group and waits up to 5 seconds. If `sudo` exits, its observed status is
  returned. Otherwise the engine sends `SIGKILL` to the process group, waits up
  to another 5 seconds, and exits with code 10, because the privileged target
  may outlive `sudo`. If the signal cannot be delivered at all, the engine
  waits up to 5 seconds for `sudo` to exit on its own and otherwise exits with
  code 10. A broker failure ends `sudo` the same way, starting with `SIGTERM`.
- **SSH:** The client queues `Cancel` with the signal number and keeps reading
  for a terminal frame for up to 10 seconds (5 seconds when `Cancel` could not
  be queued within one second). The helper forwards the signal through the
  same local engine. A `Result` received in time is returned as the observed
  status; otherwise the invocation exits with code 10
  (`remote-result-missing`).

Because an unprivileged parent or helper cannot always signal a setuid `sudo`
process or its privileged descendants, `agentenv` reports termination as
unconfirmed rather than issuing a privileged `kill` command.

## Credential resolution, memory, and diagnostics

When a valid SSH login askpass request or sudo `PasswordRequest` arrives,
`agentenv` resolves the referenced credential through `credential::resolver`
within `--auth-timeout-secs`:

- **Unix resolver (`src/credential/resolver.rs`):** Spawns the current
  `agentenv` executable with `--agentenv-resolver` in its own process group
  (`process_group(0)`), passing one end of an unnamed `UnixStream::pair()` as
  file descriptor 3 with null stdin, stdout, and stderr, and a scrubbed
  environment. Both parent and child verify peer UID (`getpeereid` /
  `SO_PEERCRED`) before exchanging frames. On timeout or cancellation, the
  parent sends `SIGKILL` to the negative PID (process group) and reaps the
  child.
- **Windows resolver (`src/credential/resolver/windows.rs`, `src/windows.rs`):**
  Creates a local-only, single-instance named pipe
  (`\\.\pipe\agentenv-resolver-<pid>-<nonce>`) with a current-user-only DACL
  (`D:P(A;;GA;;;<sid>)`), spawns `agentenv.exe --agentenv-resolver` with
  `CREATE_NO_WINDOW` and null stdio, and assigns the child to a kill-on-close
  Job Object (`JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE`) before sending the request
  frame. Both parent and child verify peer and parent PIDs over the named pipe,
  and the child waits for a `0x06` acknowledgement byte before closing the pipe.
  Dropping or terminating the Job Object kills the resolver process tree on
  timeout or cancellation.

Internal `--agentenv-resolver` and `agentenv-ssh-askpass` modes require their
bound session channels and never expose a standalone print-secret CLI. Secret
buffers use `Zeroizing` storage and are dropped immediately after framing;
errors use fixed safe classifications so candidate password bytes never enter
diagnostics.

## Module and distribution boundaries

| Module or binary | Responsibility |
| --- | --- |
| `src/credential/` | Provider dispatch, `Secret` zeroization and stage validation, confidential command capture, and the cancellable subprocess resolver (`resolver.rs`, `resolver/windows.rs`) |
| `src/config/` | `CredentialUsage` schema, nullable `inject_as`, `kind = "sudo-target"` validation, and atomic whole-file writes |
| `src/sudo/` | Local check and execution entry points, shared outcomes and reason codes (`error.rs`), local Unix engine (`unix.rs`), remote helper server (`remote.rs`), askpass channel framing (`askpass_channel.rs`), and the SSH client session runner for execution, `--check`, and `--deploy-helper` (`client.rs`) |
| `src/sudo/ssh.rs` and `src/sudo/ssh_askpass.rs` | `ssh -G` evaluation, jump-chain verification, controlled OpenSSH invocation, and local SSH askpass broker (`ssh_askpass/windows.rs` on Windows) |
| `src/sudo/deploy.rs` | `--deploy-helper` path grammar, remote preflight and decision, helper source selection (operator file, bundled companion, or release asset verified against `SHA256SUMS`), and the install command with destination byte-count and identity checks; `client.rs` runs the sessions and the acceptance handshake |
| `src/sudo/protocol.rs` and `src/sudo/transport.rs` | `AGEP` v1 binary framing, payload limits, credit-window accounting, and priority frame queues |
| `src/cli/sudo.rs` | CLI flag validation, `--plan` / `--check` / `--deploy-helper` formatting, and execution streaming |
| `src/bin/agentenv-sudo-helper.rs` | Unix companion binary implementing `--identity`, askpass mode (default), and remote `--serve` mode; no credential-store access |
| `src/bin/agentenv-ssh-askpass.rs` | Cross-platform local SSH askpass companion executable; no credential-store access |

Unix release archives package `agentenv`, `agentenv-sudo-helper`, and
`agentenv-ssh-askpass` together; Windows archives package `agentenv.exe` and
`agentenv-ssh-askpass.exe`. `agentenv update` installs and rolls back companion
binaries atomically alongside the main binary.

Remote helper installation is explicit: operators either place the matching
`agentenv-sudo-helper` binary at `helper_path` and verify it with `--check`, or
run `agentenv sudo --with <entry> --deploy-helper` as specified in
[helper-deployment.md](helper-deployment.md). Normal execution never uploads or
updates remote binaries automatically.

## Verification and test matrix

Verification uses only test-owned credentials, hosts, keys, and accounts.
Real `sudo` and `sshd` suites run inside disposable containers or dedicated lab
environments and refuse to run unless their explicit lab environment guards are
set.

| Contract | Verified cases |
| --- | --- |
| Credential confidentiality | Required-password success and failure, provider stdout/stderr failures, denied/locked keychain, debug environment variables, oversize and CR/LF/NUL secrets, and sentinel leak checks across stdout, stderr, and artifacts |
| Lazy and one-shot authentication | `NOPASSWD` with an unavailable stored secret succeeds without resolution; wrong password is never resent; duplicate askpass requests fail; `-k` ignores prior cached sudo timestamps |
| Account and destination binding | Local and remote `auth_user` mismatch, changed host alias with untrusted host key, unknown host key, helper build mismatch, and `rootpw`/`targetpw`/`runaspw` or custom PAM prompts fail before releasing a sudo password |
| SSH source selection | `ssh-config` aliases, `Include`/`Match` directives, custom `config_file`, changed effective `User`, `explicit` mode with `-F none`, offline `--plan` versus connecting `--check` |
| Publickey jump routes | Conforming multi-hop route succeeds on Linux `OpenSSH_10.0`; inner hops with `BatchMode=no`, password fallback, permissive host checking, multiplex reuse, or local command hooks, as well as custom `ProxyCommand`, cycles, and >8 hops, fail before connecting |
| SSH password login | Login without public keys or user config; correct, wrong, and missing password; server password-method refusal; `keyboard-interactive` rejection; `SSH_ASKPASS_PROMPT=confirm` and unexpected prompts rejected without lookup |
| Authentication-value fidelity | Independent 255-byte UTF-8 and CR/LF/NUL validation at both SSH askpass and sudo `PasswordResponse` boundaries; exact preservation of spaces and Unicode; SSH-only, shared, and SSH-followed-by-`NOPASSWD` flows |
| Command authorization and stream fidelity | Direct command execution matches command-specific `sudoers` rules while disallowed commands stay denied; binary stdin, independent binary stdout/stderr, special characters in argv, and exit codes `0`, `1`, `9`, `10`, `127`, `255` |
| Lifecycle, IPC, and resource limits | Signal cancellation during connect, lookup, authentication, and target execution; lost SSH connection after `Start` exits 10; concurrent invocations, symlink/socket substitution refusal, malformed/oversized frames, and 128 KiB stream credit backpressure |
| Helper deployment | `--deploy-helper` clean install, idempotent re-deploy, upgrade, `--from`, `--force` reinstall of a matching helper, refusal of an occupied path, and path-grammar validation ([helper-deployment.md](helper-deployment.md)) |

Continuous integration in `.github/workflows/ci.yml` and the repository lab
suites verify these contracts:

- **`lint` job (`ubuntu-latest`):** Runs `cargo fmt --check`,
  `cargo clippy --all-targets -- -D warnings`, and
  `cargo clippy --all-targets --features test-support -- -D warnings`.
- **`test` job (`ubuntu-latest`, `ubuntu-24.04-arm`, `macos-latest`, `windows-latest`):**
  Runs `cargo test --features test-support` across all platforms (covering
  config/schema validation, `runner` authentication-credential rejection, wire
  protocol framing and flow control, confidential resolver lifecycle, and
  CLI contracts). On `windows-latest`, it also runs Clippy for both feature
  sets and executes `python tests/windows_lab/ssh.py`, which drives the real
  `agentenv.exe` and `agentenv-ssh-askpass.exe` binaries with native
  `OpenSSH_for_Windows` against a loopback Paramiko SSH server and simulated
  remote helper ([windows-port.md](windows-port.md)).
- **`sudo-integration` job (`ubuntu-latest`):** Runs the disposable Debian 13
  container labs (`--network none`):
  - `bash tests/sudo_lab/run.sh`: verifies raw `sudo` and OpenSSH askpass, `-k`,
    `NOPASSWD`, FD closing, and `ssh -G` / `ProxyJump` behavior recorded in
    [sudo-compatibility.md](sudo-compatibility.md).
  - `python3 tests/sudo_execution_lab/run.py`: verifies local `agentenv sudo`
    execution, `--check`, `--plan`, binary streaming, concurrency, and
    cancellation.
  - `/opt/sudo-lab/verify_ssh.py` (`python3 tests/sudo_execution_lab/run.py --ssh`):
    verifies SSH `publickey` and `password` modes, `ssh-config` and `explicit`
    modes, multi-hop `ProxyJump` preflight, and `--deploy-helper` against a
    real loopback `sshd`.
  - `/opt/sudo-lab/verify_release.py` (`python3 tests/sudo_execution_lab/run.py --release`):
    verifies `sudoers` policy variations (`use_pty`, `!use_pty`, `requiretty`,
    `rootpw`, `targetpw`, `runaspw`, custom `passprompt`, `sudo_log_input`),
    SSH refusal cases, and mid-flight cancellation and disconnect behavior.
- **`tests/sudo_macos_lab/run.py`:** Exercises the native macOS OpenSSH client
  and `agentenv` binaries against the containerized Linux `sshd` destination
  bound to `127.0.0.1`.

The dated execution results, verified package versions, and remaining
unmeasured platform combinations (such as native macOS local `sudo` and the
`x86_64` release targets) are recorded in
[sudo-release-evidence.md](sudo-release-evidence.md).
