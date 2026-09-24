# Local and SSH sudo execution

Status: implemented for macOS and Linux clients and Linux destinations, covering
local sudo, SSH execution, and publickey and saved-password login. The Windows
SSH client (confidential resolver and named-pipe login askpass) is deferred to a
separate implementation, and Windows builds report SSH sudo execution as
unsupported. Real-platform evidence covers arm64 Linux destinations with Linux
and macOS SSH clients; native macOS local sudo and the x86_64 release builds
remain unmeasured. Evidence and remaining gaps are recorded in
`sudo-release-evidence.md` and `sudo-compatibility.md`. This document defines
the design baseline and release criteria. It does not authorize deployment,
password collection, sudoers changes, or execution on a user's hosts.

## Outcome and scope

An agent selects a configured execution target and a command. agentenv obtains
the target's sudo password from the local credential provider only when sudo
requests it, completes authentication, and streams the command's input and
output. Password bytes never enter the agent's tool arguments or results.
The same interface supports local execution and execution over SSH. SSH login
can use existing keys or an explicitly selected locally stored password;
login authentication and sudo authentication remain separate stages.

The first complete release supports noninteractive commands, binary stdin,
separate stdout/stderr, concurrent invocations, an explicit run-as user, and
observable exit status. Commands may run for a long time; streaming does not
buffer their entire input or output. Each invocation runs one command once.

| Component | Initial supported scope |
| --- | --- |
| Local sudo | macOS and Linux with compatible sudo/askpass and conventional single-password authentication |
| SSH client | macOS, Linux, and Windows with a supported OpenSSH executable |
| SSH destination | macOS and Linux with compatible sudo and the matching unprivileged helper |
| Password storage | Existing keychain provider; command provider with restricted resolution I/O |
| SSH authentication | Explicit key/agent mode or saved-password mode; SSH-config aliases or fully explicit connection settings |
| Command interface | UTF-8 argv without NUL; executable and optional cwd are absolute paths; stdin/stdout/stderr are arbitrary bytes |

Native Windows sudo/UAC, arbitrary interactive SSH sessions, nested automatic
sudo interception, sudoedit, interactive shells, password changes, OTP/MFA,
and general PAM conversations are outside this release. An explicit shell
executable can still be the requested noninteractive command; it receives
ordinary sudoers authorization and must not be introduced implicitly.
PTY execution is a later, separate interface with terminal and resize tests.
Unsupported capabilities fail explicitly rather than switching transport.

## Existing code and integration constraints

- `src/credential/mod.rs` owns provider selection; reuse it rather than
  creating separate sudo and SSH credential stores.
- `src/credential/keychain.rs` already reads and writes platform credentials.
  `src/cli/credential.rs` provides hidden local input through `credential set`.
- `src/credential/secret.rs` prevents ordinary formatting and serialization,
  but currently uses ordinary String/Vec storage without memory zeroization.
- `src/credential/command.rs` inherits provider stdin/stderr. Its current
  behavior is unsuitable for a confidential authentication exchange.
- `src/config/model.rs` requires `inject_as` on every credential. The new
  purpose restriction below needs coordinated model, validation, writer,
  query, and CLI changes.
- `src/runner.rs` discovers credential references recursively and injects
  them into the target environment. It must refuse authentication credentials,
  including references carrying an `?as=` override, before resolving any
  provider or launching a target.
- `src/cli/mod.rs` and `src/error.rs` own command and exit-code integration.
  Add a dedicated CLI module rather than placing the execution state machine
  in the dispatch function.
- `docs/design/credential-fill.md` is a separate, unimplemented plan. Share
  its proposed restricted provider I/O seam if that implementation exists by
  the time this work starts; do not duplicate it or depend on fill delivery.
  Its proposed exit codes 8 and 11 remain reserved for filling.

Ordinary `run` continues to mean transparent environment injection. It does
not detect sudo, inspect terminal output for password prompts, or rewrite
arbitrary SSH commands. Project files remain selection-only and cannot hold
credentials or helper/SSH execution policy.

## Security and authorization model

The protected boundary is normal-workflow password delivery: configuration
contains references, credential resolution is local, and only the intended
SSH-login or sudo authentication path receives its password bytes. There is no plaintext
password in argv, environment variables, generated scripts, disk spools,
diagnostics, process transcripts, or target stdin.

This is not a sandbox against the invoking user, a malicious root process,
a compromised SSH server, a malicious credential provider, or an agent with
equivalent filesystem/process access deliberately extracting credentials.
The remote host necessarily receives the password during authentication.
Use distinct passwords for distinct accounts/hosts where possible.

OS sudoers policy remains the authorization authority. The executor invokes
sudo with the requested executable and argv directly; it never substitutes
`sudo sh -c`, a privileged agentenv helper, or `sudo env` around that command.
An account allowed arbitrary sudo effectively grants this automation that
same authority. Credential-use restrictions and user-owned target definitions
prevent mistakes; they are not an administrator-controlled command allowlist.

Existing host/tool approval boundaries continue to apply. Neither a stored
password nor a trusted `.agentenv.toml` authorizes an otherwise unauthorized
administrative action. Runtime does not edit sudoers, disable host-key checks,
install missing software, or create broad NOPASSWD rules.

The SSH configuration, known-hosts material, local provider, helper binary,
and destination OS are trusted inputs. Login startup output and wire data are
still parsed as untrusted bounded input. Owned components never log passwords;
arbitrary target output remains target output and is not universally secret-
redacted. A malicious target or server is outside this confidentiality claim.

## Architecture decision: separate authentication from command I/O

| Approach | Benefit | Reason for selection or rejection |
| --- | --- | --- |
| Environment injection plus ad hoc `sudo -S` scripts | Small implementation and no remote installation | Password shares target stdin; unsafe when sudo does not consume it; every caller reimplements quoting, failure, and output handling |
| Private password pipe plus privileged wrapper or preserved extra FD | Can separate data streams | Wrapper changes which executable sudoers authorizes; extra-FD preservation needs nondefault policy and is not portable |
| Askpass plus per-invocation broker and remote companion | Preserves real-command authorization and independent stdin | Selected; requires a small remote executable and explicit wire/lifecycle contracts |
| Persistent privileged service | Can enforce an independent authorization policy | Unnecessary root daemon, installation, and policy surface for this requirement |

Use the same unprivileged Unix execution engine locally and on the remote
host. An ephemeral Unix-domain socket connects sudo's askpass process to the
engine. The local engine resolves a configured credential; the remote engine
requests that one credential through the existing authenticated SSH session.
No credential store is installed remotely.

sudo supports askpass through `-A` and `SUDO_ASKPASS`; `-S` uses stdin. Its
ordinary command execution closes extra descriptors, with policy restrictions
on overriding that behavior. These mechanisms motivate the design above.
[sudo manual](https://raw.githubusercontent.com/sudo-project/sudo/main/docs/sudo.man.in)

The askpass implementation itself also closes extra descriptors and runs as
the invoking user. Consequently, the helper reconnects through a protected
socket instead of relying on an inherited password FD.
[sudo askpass source](https://raw.githubusercontent.com/sudo-project/sudo/main/src/tgetpass.c)

## Proposed CLI

Target names in the examples below are illustrative.

```text
agentenv sudo --with local_admin -- /usr/bin/id -u
agentenv --profile work sudo --with prod_admin -- /usr/bin/systemctl restart nginx
agentenv sudo --with prod_admin --cwd /var/lib/example -- /usr/bin/tee config.json
agentenv sudo --with prod_admin --plan --json -- /usr/bin/systemctl restart nginx
agentenv sudo --with prod_admin --check --json
```

- `--with` requires exactly one top-level target entry. Local versus SSH
  behavior is determined by that entry, not inferred from the command.
- Execution accepts one argv after `--`. There is no inline password,
  credential override, destination override, arbitrary sudo-option passthrough,
  or automatic shell interpolation. Change the target definition explicitly
  when changing its account, credential, run-as identity, or host.
- Profile and project selection retain the existing CLI precedence and trust
  notices. Target selection is explicit; operators can use `--profile` and
  `--no-project` to pin selection. `--plan` displays the resolved selection.
- `--plan` validates configuration and arguments and prints the selected
  profile, transport, endpoint, credential name, run-as user, cwd, and supplied
  command. It performs no provider resolution, SSH connection, or `ssh -G`
  execution. It is a plan, not proof of reachability or permission.
- `--check` takes no command. It checks executable/helper availability,
  identity, protocol compatibility, and broker prerequisites. Remote checks
  connect through SSH and may resolve the selected SSH login password in
  password mode, but never resolve a sudo password or run a privileged command.
  Authentication compatibility remains unknown until an actual challenge;
  `--check` must not claim successful sudo authentication.
- Execution streams raw stdout/stderr and rejects global `--json`. Structured
  output is limited to `--plan` and `--check`, which follow existing empty-
  stdout-on-error behavior. No raw protocol events appear in user output.
- Stdin belongs exclusively to the requested command. Local omission of
  `--cwd` uses the caller's cwd; remote omission uses the SSH session's initial
  directory, reported by the helper. `--cwd` is applied before spawning sudo
  and therefore must be accessible to the invoking user. Root-only cwd changes
  need a separately designed policy-aware extension.
- Connection/setup and credential-response deadlines are finite. Use
  `--connect-timeout-secs` (default 30) for connection through Ready, excluding
  time spent servicing the one allowed SSH credential response, and
  `--auth-timeout-secs` (default 60) from a validated PasswordRequest through
  its response; positive overrides are bounded at 300 seconds. Local helper
  setup uses the same setup bound. SSH password response uses the same
  credential-response bound, with at most one response; other connection time
  still consumes the setup budget. No authentication timer runs just because
  sudo has been spawned: a long-running NOPASSWD target must not be mistaken
  for stalled authentication. sudo retains its own authentication timeout.
  These timers do not prove that authentication succeeded or that the target
  started. There is no default whole-command timeout or automatic retry.

## Credential and target configuration

Use TOML and the existing `credential://` reference syntax. Add an optional
credential `usages` array. Omitted or `["environment"]` retains current
behavior; `["sudo"]`, `["ssh-password"]`, or `["ssh-password", "sudo"]`
explicitly permits those authentication consumers. Reject empty/duplicate/
unknown usages and mixing `environment` with an authentication usage. Use one
array contract so an account password can explicitly serve both stages.

For authentication credentials, forbid `inject_as` and reference `?as=` overrides. Support
keychain and command providers; reject the env provider for this use because
the intended workflow does not start with a password in inherited environment.
For environment credentials, retain required `inject_as`. `credential check`
and keychain `credential set` continue to work for every usage. No read-value
command is added.

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

The examples are fragments under an otherwise valid version-1 configuration;
the profile still needs its existing description/default-selection fields.
Local accounts and paths are illustrative, not discovered environment values.

The `kind = "sudo-target"` marker opts an entry into typed validation. Keep
unmarked entries open-schema. Validate the marked entry's description/kind/sudo
shape and closed transport-specific sudo tables at configuration load/write.
Require a credential permitting sudo use without alias query parameters.
The local variant rejects SSH fields; the SSH variant requires them. `auth_user`
must equal the actual invoking local account or remote SSH login account in
this release. `run_as` may differ and is passed explicitly to sudo.

Validate destination, user, port, absolute paths, and string bounds without
echoing candidate secrets. Host/user option-injection strings, control
characters in connection metadata, and ambiguous destinations are rejected.
The initial SSH bootstrap path accepts only absolute paths with a conservative
ASCII path grammar, without whitespace, shell metacharacters, `.`/`..` segments,
or home expansion. Argv transmitted inside the protocol has no such shell
restriction. Support a conventional POSIX-compatible remote login shell.

Extend `credential add` with repeatable `--usage` and conditional `--inject-as` rules.
The writer emits the normalized `usages` array for explicit usage selections.
Add a narrowly scoped `credential update <name> --usage <purpose> ...` operation
to replace permitted usages on an existing definition without resolving or
rewriting its stored value. Authentication usages remove `inject_as`; selecting
environment use requires an explicit `--inject-as`. Whole-file validation must
reject incompatible references atomically. Report the changed purposes and
injection availability, never a password. This permits an already saved sudo
password to gain explicit SSH use without asking the user to enter it again.
The setup sequence remains definition, hidden local input, then target entry:

```text
agentenv credential add prod_sudo --description "Production sudo password." \
  --provider keychain --service agentenv.sudo --account prod/deploy --usage sudo
agentenv credential set prod_sudo
```

Create each complete target entry in one existing `set --type json` operation,
including its description, kind, and sudo table. Individual incremental writes
that expose an incomplete typed target fail whole-file validation. This avoids
requiring a new target-management command solely for initial setup.

Query output reports usages and an absent/null injection target for authentication
credentials. Existing credential JSON fields remain present; document the new
nullable `inject_as` and additive `usages` contract. `run --with` on any entry
containing an authentication reference fails before all resolution, even in a mixed
entry and even with a requested environment-name override. It never silently
skips references. Direct filling must also reject authentication credentials if/when
that feature exists; every consuming surface enforces purpose at its boundary.

These are additive version-1 capabilities: old configurations need no rewrite,
while older binaries reject new fields through their existing closed schema.
Document the minimum feature release and upgrade-before-edit sequence. Do not
change existing stored passwords or reinterpret old credentials as authentication-use
based on their names. Purpose changes use the explicit validated update above;
no automatic conversion occurs.

## SSH configuration sources and credential matching

The target entry is the explicit binding, not a search over stored passwords.
`--with prod_admin` selects one profile entry, its SSH connection definition,
its SSH authentication reference if needed, and its separate sudo reference.
Do not match credentials by display name, IP alone, Keychain service/account
labels, or fuzzy hostname rules. Multiple aliases can explicitly reference the
same credential; multiple accounts on one host remain different targets.

The two connection modes are mutually exclusive:

| Mode | Connection source | Validation and behavior |
| --- | --- | --- |
| `ssh-config` | `host_alias` selects an OpenSSH Host name; optional absolute `config_file` selects a specific file | Let OpenSSH evaluate Host/Include/Match; do not reimplement the parser. Omit `config_file` to use normal user/system config precedence. HostName/User/Port come from that evaluation. |
| `explicit` | Required hostname, user, port, host-key binding, and authentication fields in agentenv | Invoke OpenSSH with `-F none`, supplying the connection fields explicitly; read neither user nor system SSH configuration. No alias lookup or implicit route inheritance. |

In `ssh-config` mode, execution and `--check` use a bounded `ssh -G` evaluation
with the selected config source, capture its output internally, and extract
the effective endpoint/user/port and relevant connection properties. `Match
exec` can run local commands during this evaluation; this is trusted SSH
configuration execution, not a side-effect-free parser. Never invoke it for
`--plan`, which reports the alias and labels the effective endpoint unresolved.

Compare the effective user with the target's required `auth_user` before
allowing any password lookup. Pin the evaluated HostName/User/Port in the actual
SSH launch so these fields cannot be silently reevaluated to another account.
Keep the alias for remaining config selection. Do not expose duplicate
agentenv hostname/user/port overrides in this mode: change the selected SSH
config or choose explicit mode. The helper's later login-account check remains.
SSH config and local trust files remain mutable same-user trusted inputs; this
does not claim a snapshot or protection against malicious concurrent edits.

Authentication method is always explicit in agentenv and overrides inherited
authentication preferences. Host-key controls always override ordinary SSH
config. Thus an alias supplies connection details, not permission to switch
credential, authentication method, or trusted server key. Key-mode ProxyJump
is accepted only under the per-hop policy below. Arbitrary ProxyCommand is
unsupported in this release; password mode rejects both ProxyJump and
ProxyCommand before secret resolution.

For example, this existing SSH configuration:

```sshconfig
Host prod
    HostName 203.0.113.10
    User deploy
    Port 2222
```

matches `mode = "ssh-config", host_alias = "prod"`. agentenv's
`sudo.credential` still chooses the sudo password, and `sudo.ssh.auth.credential`
chooses the SSH password when password mode is selected. No password is written
into `.ssh/config`. A user change to `User` that disagrees with `auth_user` fails
before a password is released. Hostname/address changes remain subject to the
target's pinned host-key trust; an alias string alone is not server identity.

The following complete illustrative configuration needs no `.ssh/config` and
explicitly shares one stored account password between SSH login and sudo:

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

The example host/address/account and local trust-file path are illustrative.
Save this password once using an added definition permitting both usages and
the existing hidden-input `credential set`. If the SSH and sudo passwords are
different, use two definitions with `["ssh-password"]` and `["sudo"]` and
reference each at its respective field. Never assume their values are equal
and never fall back to the other credential after failure. Sharing a definition
does not share authentication sessions or permit a second delivery in one stage.
Rotating a shared stored value changes both uses; separate references rotate
independently. Initial purpose selection is explicit at credential creation;
an existing definition can gain both purposes with:

```text
agentenv credential update prod_account --usage ssh-password --usage sudo
```

This updates metadata only and does not fetch, copy, or replace the saved value.

In explicit publickey mode, allow a typed list of absolute `identity_files`
and an explicit `use_agent` boolean in the auth table. Disable implicit default
identity files and use only the declared files and/or agent according to that
selection; verify actual OpenSSH option behavior in the compatibility milestone.
Password mode forbids these key-specific fields. Explicit mode initially has
no jump-host/ProxyCommand fields; an explicit typed multi-hop design can follow.
It must not silently consult `.ssh/config` to recover missing route information.

`-F none` is the documented OpenSSH mechanism for skipping configuration files.
This does not remove server identity checking or the need for a trusted host
key. User public-key authentication and server host-key verification are
different operations.
[OpenSSH configuration selection](https://man.openbsd.org/ssh.1)

### Publickey jump-route policy

OpenSSH implements ProxyJump by launching additional SSH processes. Its generated
command does not propagate the outer client's `-o` restrictions, including
BatchMode, authentication methods, host-key policy, and multiplexing controls.
[OpenSSH jump-command construction](https://github.com/openssh/openssh-portable/blob/master/ssh.c)
Applying those restrictions only to the final-host client does not constrain
the route.

Retain native ProxyJump only for preconfigured routes whose effective per-hop
settings already meet the required policy. This preserves existing key/agent
selection without generating a second proxy-command framework or modifying
the user's SSH files. Operators repair incompatible hop configuration explicitly.
The final destination keeps the target entry's pinned host-key binding; each
jump host needs its own verified trust material in the selected SSH configuration.
Do not reuse the final destination's HostKeyAlias or trust identity for a hop.

Before starting an owned SSH connection, evaluate every hop with bounded `ssh -G`
using the same config-file selection, hop user/port overrides, and remaining
jump chain that the supported OpenSSH client will use. Inspect the effective
settings, not only literal Host blocks. The whole route shares the existing
setup deadline and is limited to eight hops; reject cycles, malformed routes,
and any route whose actual child invocation cannot be reproduced for validation.
`--plan` still performs no `ssh -G`; it labels route policy unverified.

Every hop must have BatchMode enabled, publickey-only authentication, no
password/keyboard-interactive/other authentication fallback, strict host-key
checking, and explicit trusted known-hosts sources. Disable alternative dynamic
key sources and automatic host-key updates, connection multiplexing/reuse,
agent/X11 forwarding, and local command hooks. Configured forwarding beyond
the required ProxyJump stream must be absent or provably cleared by the child's
`-W` invocation. Trusted `Match exec` evaluation remains permitted under the
existing configuration-source contract. Reject custom ProxyCommand routes;
their arbitrary child processes cannot be covered by this policy.

S0 freezes the effective-option predicates and exact child invocation semantics
for each supported client, including Windows. S5a verifies both a conforming
multi-hop route and rejection of a nonconforming inner hop before any connecting
SSH process or credential lookup starts. If those semantics cannot be established
for a client, jump routes are unavailable on that client; direct connections remain
supported. Config files remain trusted same-user mutable inputs as documented
above; this preflight is not a filesystem snapshot. No parent-only override or
successful final-host handshake substitutes for the per-hop checks.

## Saved-password SSH login

Support two explicit methods with no automatic cross-method fallback:

| Method | OpenSSH policy | Credential handling |
| --- | --- | --- |
| `publickey` | `BatchMode=yes`, publickey-only authentication, no password/keyboard-interactive fallback | Existing configured keys/agent; no SSH login credential reference |
| `password` | `BatchMode=no`, `PreferredAuthentications=password`, `PasswordAuthentication=yes`, `PubkeyAuthentication=no`, `KbdInteractiveAuthentication=no`, `NumberOfPasswordPrompts=1` | Exactly one `ssh-password` reference through a private local askpass session |

Password mode also disables other login mechanisms and agent use. The server
must accept SSH's password authentication method; a server requiring keys,
keyboard-interactive, or multiple factors fails explicitly. A UI displaying
"Password" does not establish that the underlying SSH method is `password`.
Do not weaken server policy or infer that password mode can bypass it.

BatchMode disables password prompting. Forced askpass permits controlled local
password entry without consuming SSH stdin. These are separate options; merely
setting SSH_ASKPASS while leaving BatchMode enabled will not implement login.
[OpenSSH authentication options](https://man.openbsd.org/ssh_config.5),
[OpenSSH askpass behavior](https://man.openbsd.org/ssh.1)

For password mode, the client starts a cross-platform local
`agentenv-ssh-askpass` companion and a session broker bound to one SSH child,
target, effective account, and credential reference. Set `SSH_ASKPASS` to its
absolute executable path and `SSH_ASKPASS_REQUIRE=force` only in that child's
environment. These variables carry no password. Use owner-protected local
sockets on Unix and an equivalently scoped named-pipe ACL on Windows. The
companion has no standalone credential-name lookup or print-secret operation.

The flow is host verification, SSH password request, local credential lookup,
authentication-value validation, one private reply to OpenSSH, successful login,
remote helper handshake, then the independent sudo flow. OpenSSH captures askpass
stdout; it is never command
stdin or user stdout. SSH's wire stdin remains exclusively the remote execution
protocol. No `sshpass -p`, expect script, password environment variable, or
simulated terminal input is used.

Strict host-key verification happens before login password delivery. Key
confirmation must never be answered by the password helper. Reject
`SSH_ASKPASS_PROMPT` confirm/notification modes, unknown prompt shapes for the
tested client versions, and every request after the first. Bind prompt checking
to the expected generated user/host prompt rather than searching for the word
"password". The prompt is an additional guard; the enforced password-only
method, fixed child/session, requested account, and host-key verification own
the selection. Password expiry/change prompts fail; no new password is supplied.
Clear inherited askpass hints/debug hooks and never forward raw prompt text.
[OpenSSH askpass implementation](https://raw.githubusercontent.com/openssh/openssh-portable/master/readpass.c)

SSH lookup necessarily occurs before the remote helper's Ready message. That
handshake gates sudo password release, not SSH login authentication. A missing
helper may therefore be discovered after one successful SSH password login.
One-shot budgets are separate: one SSH response and one sudo response per
execution, even when they reference the same stored value. Do not retain an
unnecessary cleartext login password while waiting for a later sudo request.

Do not export askpass routing state to the remote environment: ensure matching
SendEnv/SetEnv entries cannot forward this metadata, and test that inherited
SSH config cannot accidentally carry it to the target. Broker access and cleanup
have the same same-user/root limitations as sudo's local broker. Password-mode
Windows support is advertised only after actual client/askpass/named-pipe tests;
an unsupported build reports the capability missing without weaker fallback.

Password authentication through jump hosts is deferred. A nested OpenSSH child
may inherit askpass settings, making a final-host password unsafe to offer to
an unbound jump-host prompt. Reject such routes in this mode. Future multi-hop
support needs separately bound host/account/credential and trust verification
for every hop; parsing prompt text is not a sufficient routing boundary.

### Password values at both authentication boundaries

The shared credential `Secret` type accepts values suitable for other consumers;
it does not establish askpass compatibility. Both the local SSH askpass reply
and the sudo PasswordResponse path must validate the complete resolved value
before sending any password bytes. Require nonempty UTF-8 without CR, LF, or
NUL, and enforce the stage's verified byte limit. Preserve all other accepted
bytes, including leading/trailing spaces; do not trim, truncate, split lines,
or count Unicode characters in place of encoded bytes.

Use a conservative product ceiling of 255 UTF-8 bytes for each stage. The SSH
limit is the smaller of that ceiling and the tested local OpenSSH askpass limit;
the sudo limit is the smaller of that ceiling and the tested destination
sudo/askpass limit. Verify any line terminator added by the companion fits the
transport limit. S0 publishes those limits independently for supported clients
and destinations. OpenSSH's askpass reader has a fixed buffer and ends the value
at CR/LF, so an unvalidated reply may silently authenticate with a different
value. [OpenSSH askpass reader](https://github.com/openssh/openssh-portable/blob/master/readpass.c)

Apply validation on every actual response, including SSH-only credentials,
shared SSH/sudo references, and SSH login followed by NOPASSWD. Never rely on
the later sudo stage to validate an earlier SSH response. A shared definition
uses each stage's own limit when that stage requests it; NOPASSWD still performs
no sudo lookup. Invalid values produce a fixed credential-resolution failure
without candidate bytes and terminate the authentication exchange. Owned
components return failure rather than a successful empty or shortened password
reply; the coordinator stops the SSH/sudo exchange instead of relying on a
particular client's treatment of askpass failure. Do not retry or substitute
another credential. Remote helpers validate sudo secret frames before askpass delivery
as well; malformed frames are protocol failures. These checks constrain the
authentication consumers without changing storage or ordinary `run` semantics.

## Local execution and authentication lifecycle

1. Select and validate the target and immutable command request. Reject an
   unsupported platform, mismatched invoking account, unsafe executable/helper
   selection, or invalid paths before resolving a credential.
2. Create a unique owner-only runtime directory and a mode-0600 Unix socket.
   Check ownership and reject symlink substitution. Bind this broker to one
   invocation, credential reference, invoking identity, and command request.
3. Start the configured trusted sudo binary directly with `-A -k -u <run_as>`,
   a fixed application prompt containing a fresh session marker and `%p`, then
   `-- <absolute executable> <args...>`. No preliminary `sudo -v`, no replay
   through `sudo -n`, and no privileged wrapper are used.
4. Set `SUDO_ASKPASS` to the packaged companion executable in askpass mode.
   Use scoped routing metadata to locate the socket; no password travels in
   environment variables. The helper defaults to askpass mode when invoked by
   sudo with its prompt argument; `--serve` is its explicit remote mode.
5. The askpass adapter validates the exact prompt marker and expanded `%p`
   against the configured authentication account. An unexpected prompt/account
   fails without fetching a password. The broker validates its session and,
   where available, Unix peer credentials. PID/UID checks are defense against
   mistakes, not proof against an adversary with the same user permissions.
6. Only that validated request triggers local resolution. Apply the shared
   authentication-value contract with the destination's sudo byte limit before
   replying; preserve accepted bytes without trimming or truncation. Return
   the value once to askpass, whose stdout is the private pipe captured by sudo,
   not the CLI's stdout.
7. Disarm the broker immediately after one password response. Any second
   challenge fails without resolving or resubmitting the password. If sudo
   never invokes sudo askpass, no sudo credential is resolved or transmitted;
   a preceding SSH login may independently have used its password.
8. Reap sudo, drain output, close IPC, dispose of secret buffers, and remove
   only this invocation's validated runtime files. Process exit and abnormal
   disconnect close pending authentication; no password-bearing disk files
   exist to recover. Stale socket directories contain no credentials.

Using `-k` with a command avoids depending on or refreshing sudo's credential
cache; policy can still permit passwordless execution. No global cache purge
is performed. This choice provides a predictable per-operation credential
lifecycle without altering unrelated sessions.
[sudo cache behavior](https://raw.githubusercontent.com/sudo-project/sudo/main/docs/sudo.man.in)

The prompt check intentionally fails closed when PAM replaces the application
prompt. It supports conventional single-password sudo, not arbitrary prompt
text recognition. Do not automatically set `passprompt_override` or change
PAM. `rootpw`, `targetpw`, or `runaspw` that request a different account's
password are unsupported and must not receive the configured login password.
An administrator-controlled multifactor setup can obscure challenge meaning;
it is outside the supported authentication contract even if a prompt looks
similar. Check and publish real platform results before claiming support.
[sudoers authentication options](https://raw.githubusercontent.com/sudo-project/sudo/main/docs/sudoers.man.in)

The engine launches with a curated environment needed for platform operation
and locale, excluding inherited provider secrets, dynamic-loader hooks, debug
hooks, and unrelated credentials. The helper uses the same discipline. sudo
still applies its own target environment policy. Do not use `sudo -E`.
Ordinary local command stdin is inherited as data; the executor does not
consume it for authentication. Both execution paths are pipe-oriented and
provide no interactive-terminal contract.

Routing metadata is a short-lived local capability. Never log it. Under unusual
sudoers environment-preservation rules, it may be visible to a target; do not
promise unconditional metadata stripping without changing the command sudoers
authorizes. A consumed capability yields no second password. In the NOPASSWD
case no password has been resolved, and a malicious same-user/root target is
outside the boundary. Verify ordinary environment policies and document this
limit instead of introducing a privileged wrapper to conceal it.

## Remote execution over SSH

The remote companion runs as the configured SSH user. It has no credential
configuration, keychain integration, setuid bit, listening network port, or
background service. It uses the same Unix engine and askpass adapter as local
execution, with password requests delegated to the local client.

Start one foreground OpenSSH session with a fixed helper invocation. Put
only the validated helper path and fixed `--serve` switch in the remote
command string. Send target argv, cwd, and execution options as protocol data;
the helper passes argv directly to sudo. OpenSSH constructs a command string
and sshd uses the login shell, so separate local argv elements are not a safe
way to transmit arbitrary remote argv.
[OpenSSH client](https://man.openbsd.org/ssh.1),
[sshd command execution](https://man.openbsd.org/sshd.8)

The client sets BatchMode according to the explicit authentication method and
forces `StrictHostKeyChecking=yes`, no PTY, no agent or X11 forwarding,
no configured port forwarding, no local command hooks, and
no reuse of a multiplexed connection. Pin user, port, HostKeyAlias, and the
configured known-hosts file. Disable alternative host-key sources and automatic
host-key updates for this route so a changed SSH alias cannot silently select a
new trusted recipient. In SSH-config publickey mode, preserve compatible key
selection and only ProxyJump routes that pass the per-hop policy before launch;
these outer-client overrides are not inherited by jump children. Password mode
refuses jump/proxy routes; explicit mode reads no SSH config. Override conflicting
RemoteCommand, stdin-null, background, and session-type options. No raw
caller-supplied SSH options are exposed.

Map options to supported OpenSSH/platform equivalents, including the null-file
path on Windows. Unsupported required options fail before password resolution.
Never downgrade security options to accommodate an old client. Host-key
enrollment is explicit, out of band, and verifies fingerprints using a trusted
source; `ssh-keyscan` output alone is not identity verification.
[OpenSSH configuration](https://man.openbsd.org/ssh_config.5)

SSH authenticates the host before the helper handshake. A fresh handshake
reports protocol/build identity, platform, actual login username/UID, initial
cwd, and supported sudo features. Validate the configured login account and
capabilities before starting sudo or resolving its password. Any SSH login
password was already delivered through the separate local askpass stage.
These self-reports supplement SSH identity;
they do not attest a malicious server or a modified same-user helper.

The client validates a bound sudo password request from this session, resolves
that local credential lazily, and returns one secret response. Password bytes are
encrypted on the network by SSH and exist briefly in remote helper/askpass/sudo
memory. They are never saved to the server's credential store or a temp file.

### Wire contract

Use a versioned binary framing layer over SSH stdin/stdout, with bounded UTF-8
JSON metadata and raw byte payloads for streams and secrets. This is transport
framing, not a new configuration format. A secret frame has a dedicated encoder
at the credential boundary; `Secret` remains non-serializable and redacted in
Debug. Explicitly zero/drop its owned buffers after delivery as practical.

| Direction | Message | Purpose |
| --- | --- | --- |
| Client to helper | Hello | Negotiate version and check/execute mode, with non-secret sudo path and expected account; contains no credential |
| Helper to client | Ready | Report identity and capabilities before execution |
| Client to helper | Start | One immutable argv/cwd/run-as/auth-user request and session identifier |
| Helper to client | PasswordRequest | One validated askpass challenge bound to that request |
| Client to helper | PasswordResponse / PasswordUnavailable | One secret response or a closed failure reason |
| Client to helper | StdinChunk / StdinEof | Target data only; EOF does not close the control channel |
| Helper to client | StdoutChunk / StderrChunk / stream EOF | Separate sudo/target streams and their completion |
| Both | WindowUpdate | Per-stream credit for bounded stdin/stdout/stderr flow |
| Client to helper | Cancel | Request cancellation without replay or a second sudo command |
| Helper to client | Result / Failure | One terminal result or infrastructure failure; no raw internal exception |

For check mode, a successful Ready is terminal and the helper closes cleanly
without accepting Start. For execution mode, Ready is followed by exactly one
Start; unexpected closure is evaluated against whether Start could have reached
the helper. Neither mode resolves a credential during Hello/Ready.

Freeze the byte layout and golden fixtures in the protocol milestone: fixed
magic/version, message type, request identifier, and unsigned length checked
before allocation. Initial bounds are 64 KiB metadata/frame payload, 32 KiB
stream chunks, and 1 MiB total queued data per direction. The advertised
sudo password limit follows the authentication-value contract: the smaller of
255 UTF-8 bytes and the destination's verified sudo/askpass limit. The SSH login
reply is a separate local channel subject to its own pre-delivery validation;
this remote frame bound does not protect it. Reject longer values rather than
truncate. These are operational bounds with boundary tests, not heuristics
that discard data.

Use separate bounded queues and explicit credits for all three data streams.
Freeze initial windows and overflow-safe credit accounting with the protocol;
reject data exceeding granted credit. Keep control and authentication scheduling
independent of a blocked target stdin writer.
Reserve control capacity so full stream queues cannot starve cancellation or
authentication. Protocol readers dispatch frames without awaiting a target
stream write; senders consume stream-specific credit before sending data and
schedule control frames fairly between bounded chunks. Read stdout/stderr
concurrently; apply backpressure rather than unbounded buffering. A stopped
output consumer can delay full output/result delivery, but must not prevent
cancellation or credential-response handling. Coordinate stream EOF, process
reaping, and Result so all output preceding completion is delivered before exit.

Reject invalid versions, unknown messages, inconsistent session identifiers,
reordered authentication, duplicate Start/Result, oversized lengths, unexpected
EOF, and duplicate password requests. No resynchronization by scanning for
magic bytes. A remote startup banner on protocol stdout is a failure before
sudo password release; SSH login may already have used its own password.
Do not silently strip it. Keep SSH stderr separate from the
framed target stderr; bound and normalize transport failures without relaying
raw helper/provider exceptions or raw protocol payloads.

### Status, cancellation, and uncertain execution

A valid Result contains the observed sudo process exit code or signal and
whether password delivery occurred. It does not infer whether a nonzero status
came from sudo or the target: sudo can return the same status as the target.
In particular, exit 1 is not reliably classifiable as a wrong password.

Return normal sudo/target exit codes unchanged. Preserve existing agentenv
preflight codes 1-5 and 127. Allocate code 9 for owned execution/protocol
failures and 10 for lost/uncertain completion; codes 8 and 11 belong to the
separate fill plan. Prefix owned stderr diagnostics with stable `sudo-execution:` reason
identifiers. Target codes can collide with these values, so numeric status
alone is not a complete classification. No command retries follow from codes.

A framed remote Result with exit 255 is a real observed remote status; SSH
exit 255 without that result is a transport failure. Missing results after
Start may mean the target ran or is still running. Report completion unknown,
even if there is no proof that the target began; never label it not executed
or automatically replay it. A received terminal result remains evidence even
if SSH subsequently reports a close error; report the close problem separately.

Cancel requests ask the engine to signal its sudo process and observe the
result. Implement platform-appropriate local signal forwarding and remote
Cancel handling, with a finite cleanup wait. Closing a connection revokes
undelivered passwords and asks the helper to stop. It does not guarantee that
detached privileged descendants stop, and an unprivileged helper cannot always
signal a privileged process. If termination cannot be confirmed, report it as
unconfirmed. There is no rollback claim. Do not issue a second privileged
`kill` command or broaden privileges during cleanup.

## Credential resolution, memory, and diagnostics

Add caller-specific resolution I/O controls at the existing provider seam.
The sudo caller disallows inherited terminal stdin, bounds command-provider
stdout, drains/discards its stderr, enforces the credential-response deadline, and
constructs safe fixed diagnostics. A provider needing user interaction must
be authenticated separately; missing or locked credentials fail explicitly.
Platform keychain dialogs may still occur, so fully unattended operation is
conditional on platform access policy. Do not promise that a stored credential
is always available without interaction.

Resolve only after valid askpass demand. Provider calls do not block the
protocol control loop: cancellation/deadlines must remain observable while a
provider or OS keychain API is slow. Use an isolated resolver subprocess where
an API cannot be interrupted; terminate and reap that owned process on timeout
without passing its stdout/stderr to the agent. Late responses after session
closure are discarded and cannot enter another invocation.

Preserve existing resolver behavior for `run`, `credential check`, and
`credential set`; new restrictions are explicit to the sudo/fill callers.
Reuse that policy if the fill work lands first. Share resolver cancellation and
late-response disposal, while keeping deadlines caller-owned: sudo uses its
credential-response budget; fill uses its remaining whole-operation budget.
Internal resolver and askpass modes require their expected session channel
and never provide a general
print-credential interface.

Use zeroizing storage for new owned password buffers where practical, audit
copies in IPC encoding and provider conversion, and suppress dependency debug
logging around secrets. Do not claim complete erasure of OS/provider/sudo
copies, immunity to a debugger, or protection from privileged memory access.
Sanitize by constructing safe errors, not by replacing known password strings
after formatting arbitrary errors. Do not persist wire captures or secret
fixtures derived from user data.

Askpass inherits sudo's stderr, which can reach the user's command output.
It therefore emits no raw prompt, account candidate, broker path/session
marker, protocol payload, or dependency exception on that stream. It reports
failure with a nonzero status and, when the broker is reachable, a closed safe
reason code. Disable debug logging in askpass. Sentinel leak tests include
malformed prompts, socket failures, and protocol failures in this process.

## Module and distribution boundaries

| Owner | Responsibility |
| --- | --- |
| `src/credential/` | Storage/resolution, usage restrictions, bounded confidential resolver mode, secret ownership |
| `src/config/` | Credential-use schema and opt-in typed sudo target validation; atomic validated writes |
| `src/sudo/` | Public execute/check/plan seam, immutable request, local Unix engine, broker, outcomes and cleanup |
| `src/sudo/ssh.rs` | Config-source selection, bounded effective-config evaluation, controlled OpenSSH invocation, identity binding and two-stage authentication |
| `src/sudo/protocol.rs` | Versioned framing, size limits, state validation, flow control |
| `src/cli/sudo.rs` | CLI translation, execution streaming, safe user-visible errors |
| `src/bin/agentenv-sudo-helper.rs` | Thin Unix askpass/serve executable using the shared engine; no store access |
| `src/bin/agentenv-ssh-askpass.rs` | Cross-platform local SSH askpass executable, owner-scoped IPC, no standalone store access |
| `tests/` and dedicated integration fixtures | Behavioral contracts, real sudo/SSH verification, leak/cancellation evidence |
| Release/update/install and shipped skill | Matching binary/helper assets, setup, support matrix, operational instructions |

Split the engine into broker/process modules only where their separate
responsibilities justify it. Do not build a generic remote automation framework
or share the environment-injection runner's execution semantics accidentally.
Use target-specific Unix bindings; Windows builds expose local sudo as
unsupported while compiling and testing the SSH client and local SSH askpass
path, including named-pipe access control.

Ship matching sudo companions with Unix release archives and the SSH askpass
companion for each supported client OS; update/install them coherently with
the main binary. If main/helper versions are incompatible,
fail before secret resolution and explain how to repair the installation;
never run stale assets as a fallback. Consider interactions with browser-helper
packaging if the fill feature is implemented concurrently.

For remote setup, publish standalone helper assets for supported Unix
architectures, with release checksums/provenance. Installation is explicit:
place the matching asset at the configured user-owned absolute path, set it
executable, verify identity/protocol with `--check`, then use execution. No
automatic upload, download, remote self-update, root installation, or runtime
dependency is required during normal use. A future installer can automate this
as a separately authorized operation. Protocol mismatch does not trigger an
upload. No remote Python/Node installation is required.

## Implementation plan

S0-S7 are implemented for macOS and Linux clients and Linux destinations.
Each hazardous task declared `security-boundary` and `external-contract` and
received independent review assurance against its final code and contracts
after verification. The Windows part of S5b (confidential resolver and SSH
password IPC) is deferred to a separate implementation, which needs the same
hazard review and native Windows evidence before Windows support is
advertised.

| Task | Owned outcome and files | Depends on | Acceptance and evidence |
| --- | --- | --- | --- |
| S0: compatibility investigation | Test-owned sudo/SSH lab and a measured platform report; no production configuration | None | Demonstrate real sudo and SSH forced askpass; freeze stage-specific byte limits, per-hop effective-option predicates and child invocation semantics, effective-config and `-F none` behavior, account markers, NOPASSWD, `-k`, close-FD behavior, and cancellation limitations; record exact versions |
| S1: schema and usage boundary | `config`, credential model, add/query surfaces, `runner` guard | S0 findings relevant to supported targets | Old configs unchanged; valid local/SSH targets and both connection/auth modes accepted; explicit multi-use references work; malformed targets and env use rejected; mixed-entry and `?as=` attempts cannot export authentication credentials; atomic write failures preserve original bytes |
| S2: confidential resolver | Provider I/O policy, secret ownership, bounded resolver process, authentication-value validation | S1 | Lazy lookup; validate each stage before reply; safe provider failures; timeout/cancel with no live resolver leak or accepted late reply; no inherited stdin or candidate text in diagnostics; existing resolver contracts pass |
| S3: local execution | `sudo` Unix engine, askpass companion, local CLI/check/plan | S1, S2 | Actual sudo password success and failure; zero password reads for NOPASSWD; binary stdin preserved; exact command-level sudoers rules still match; concurrent isolation and cleanup |
| S4: remote protocol and helper | Framing/state machine, flow control, companion serve mode | S3 | Golden protocol cases, malformed input rejection, bounded concurrent streams, cancellation during blocked input/output and password lookup; no protocol bytes reach target stdin |
| S5a: SSH connection sources and key mode | Effective-config evaluation, per-hop policy, explicit mode, controlled OpenSSH launch and platform I/O | S4 | Real alias/Include/Match resolution; user mismatch rejection; no config read in explicit mode; strict host identity and exact argv; conforming multi-hop success and unsafe inner-hop/custom-proxy rejection before connecting; exit 255 and lost-connection handling |
| S5b: SSH password mode | Local SSH askpass companion, session broker, password-only policy and two-stage lifecycle | S5a, S2 | Real password login without user keys/config; reject unsupported password bytes before reply, including SSH-only and shared/NOPASSWD cases; wrong-password one-shot behavior, method refusal, unknown key releases no secret, separate/shared references, password-mode proxy rejection, and no stdin/metadata leakage |
| S6: installation and documentation | Release/update assets, README, shipped `skills/agentenv/SKILL.md`, platform matrix | S3; final SSH instructions need S5a/S5b | Clean install/update and mismatch checks including local SSH askpass; alias/explicit setup and shared/separate passwords; explicit remote deployment guide; no automatic install during execution; documented support and rotation procedure |
| S7: integration and security review | Adversarial contract tests, current-input review findings, release evidence | S1-S6 | All release criteria below pass on advertised platforms; meaningful review findings fixed/reverified; unavailable real-platform evidence remains an explicit release gap |

S0 is a bounded feasibility milestone, not permission to weaken the interface.
If the prompt/account guard is incompatible with a supported platform, retain
the no-wrong-account/no-stdin-leak invariants and amend the design based on
measured evidence before implementing an alternative. No `-S` fallback.

After S3, local execution plus its packaging/documentation slice is independently
shippable. The complete requested feature is finished only after S5a/S5b-S7, including
real remote integration evidence. S4 must freeze protocol golden fixtures before
S5a depends on them. Shared schema/provider changes are integrated before any
parallel consumers; protocol changes are reviewed with both endpoints together.

Routine work follows implementation-first development and adds only meaningful
regression/contract tests. For any test-first exception, document the concrete
security failure, repository evidence, existing coverage gap, and narrowly
testable invariant before writing those tests; general complexity is not
sufficient reason to adopt TDD.

## Release acceptance and test matrix

Use only test-owned credentials, hosts, keys, and accounts. Real sudo tests run
in disposable Linux containers/VMs and isolated macOS accounts/VMs with explicit
lab authority; never alter the developer's sudoers or personal keychain merely
to run the suite. Fake providers/processes test edge cases but do not establish
real sudo, SSH, keychain, or OS support.

| Contract | Required cases |
| --- | --- |
| Credential confidentiality | Required-password success/failure, provider stdout/stderr failures, denied/locked keychain, debug settings, oversize/CR/LF secrets, argv/environment/artifact capture; use synthetic sentinel passwords |
| Lazy and one-shot authentication | NOPASSWD with unavailable stored secret succeeds without resolution; wrong password is never resent; duplicate request fails; cached prior sudo session does not bypass selected per-operation behavior |
| Account and destination binding | Wrong local/effective SSH user, changed host alias with untrusted key, unknown key release no password; wrong helper identity, rootpw/targetpw/runaspw requesting a different account, or unexpected PAM prompt release no sudo password |
| SSH source selection | Host aliases, Include/Match, custom config file, changed effective User, exact explicit endpoint, no default/system-config reads under `-F none`, offline plan versus connecting check |
| Publickey jump routes | Conforming multi-hop success; reject an inner hop with BatchMode off, password fallback, permissive host checking, multiplex reuse, or local hooks; unknown hop key fails without prompting; encrypted key without usable agent fails noninteractively; custom proxy, cycles, excess hops, and preflight-budget exhaustion fail before connecting; prove outer overrides alone do not pass the gate |
| SSH password login | No user public keys/config; correct/wrong/missing password; disabled password method; keyboard-interactive-only/MFA rejection; key-confirmation and password-change prompts; helper/askpass absence; no terminal/stdin fallback |
| Authentication-value fidelity | Validate SSH and sudo independently at each reply boundary; reject empty/CR/LF/NUL/invalid UTF-8 and over-limit values without candidate output; preserve spaces and Unicode; test exact byte limit and one byte over, SSH-only and shared references, and SSH followed by NOPASSWD; no successful empty/truncated reply or automatic retry |
| Two-stage credentials | Different SSH/sudo values route correctly; explicit shared reference saved once and used once per stage; NOPASSWD skips sudo lookup after SSH password login; no cross-credential fallback or routing metadata in remote env |
| Command authorization | Sudoers permits a specific absolute executable and args but denies a shell/helper; the allowed command succeeds and the disallowed command stays denied |
| Stdin and argv fidelity | Empty/binary input, leading newlines, very large streams, stdin EOF, target ignoring stdin; empty arguments, whitespace, Unicode, quotes, `$()`, semicolons, newlines and leading dashes arrive literally |
| Output and status | Independent binary stdout/stderr, high output volume, target codes 0/1/9/10/127/255, sudo rejection, missing executable, signal termination, complete output before final result |
| Lifecycle | Cancel during connect, lookup, authentication, blocked stdin/output, and target execution; disconnect after Start/password/output; no replay, no false success, no unconditional termination claim |
| IPC isolation | Simultaneous targets/credentials, wrong session/peer, symlink/socket substitution, malformed/truncated/oversized frames, duplicate/out-of-order frames, stdout banner, helper crash |
| Resource limits | Fixed memory bounds under duplex pressure, control frames not starved, resolver child reaped, broken pipes handled, no stalled invocation blocking another |
| Compatibility and packaging | Supported macOS/Linux sudo versions including policy `use_pty` variations, requiretty rejection without SSH PTY fallback, missing/mismatched helper, clean binary/helper updates, Windows SSH options and paths |

Explicitly inspect inherited target environment under default and customized
sudoers policies. Password bytes must never be present. Routing-metadata
inheritance follows the documented boundary; do not turn an unproven capability-
stripping claim into a release assertion. Test sudo I/O logging with synthetic
credentials to confirm password delivery bypasses target input recording;
external authentication plugins may have their own logging outside agentenv.

Retain current checks: `cargo fmt --check`,
`cargo clippy --all-targets -- -D warnings`,
`cargo clippy --all-targets --features test-keychain -- -D warnings`, and
`cargo test --features test-keychain` on the existing OS matrix. Add dedicated
real sudo/sshd jobs and helper artifact tests for the advertised platform matrix.
Protect tests that require privileged lab setup from accidental execution on
ordinary workstations. Test fixtures may observe their own received secret;
production code must not read back or print stored values for verification.

## Open technical questions and decision ownership

1. **Platform compatibility:** S0 measures packaged macOS sudo, supported
   Linux distributions, and OpenSSH clients including Windows forced askpass,
   account-prompt validation and local IPC. Cover PAM prompt replacement,
   accepted password bytes/length separately for local SSH askpass and destination
   sudo, per-hop child options and policy predicates, `use_pty`, effective-config
   handling, and `-F none`. Publish exact tested versions and limits;
   a version string alone is not proof of compatibility.
2. **Cancellation:** S0/S3 establish which sudo configurations forward signals
   and which cannot be controlled by the unprivileged parent. Preserve uncertain-
   completion reporting wherever termination cannot be proved.
3. **Resolver isolation:** S2 selects the smallest subprocess boundary needed
   for cancellable platform credential reads and command providers. Its secret
   IPC and output suppression are part of the security review.
4. **Unix runtime bindings:** S3 selects a maintained target-specific Rust
   facility for peer credentials, socket permissions, signals, and zeroization.
   Add project dependencies through Cargo manifests/lockfile; no global runtime
   or system package installation is implied.
5. **Release layout:** S6 chooses the companion's install path and coherent
   update sequence based on the existing installer. Failure/mismatch behavior
   is fixed above; do not add unverified binary-only fallback behavior.

The implementer can resolve these engineering choices from evidence. Any change
that sends passwords through target stdin, drops host verification, introduces
a privileged wrapper, persists remote secrets, or claims interactive support
changes the accepted design and requires an explicit design revision.
