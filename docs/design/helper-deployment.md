# Remote sudo helper deployment

Status: implemented, 2026-09-24 (branch `helper-deployment`). Extends
`sudo-execution.md`, whose "Module and distribution boundaries" section
reserved this as "a future installer ... a separately authorized operation".
Sections below describe the shipped behaviour; "Evidence" records what has and
has not run against a real destination.

## Problem

`agentenv sudo` over SSH requires `agentenv-sudo-helper` at the configured
`helper_path` on the destination, and `client::validate_ready` accepts the
helper only when its `build` equals the client's `CARGO_PKG_VERSION` exactly.
Every `agentenv update` therefore invalidates every SSH target until an
operator redeploys the helper by hand. The failure is discovered late, as
`helper-identity-mismatch` on the next execution, after a successful SSH login
and possibly after one SSH password delivery.

Manual deployment is also where mistakes happen: the operator has to pick the
asset for the destination's architecture and libc, verify its checksum, copy
it through a separately authorized path, place it at the exact configured
absolute path, and make it executable. The release pipeline publishes the
standalone helper for x86_64 and aarch64 Linux and both macOS architectures
with a glibc 2.28 floor, so the right bytes exist for every supported
destination; getting them there was the remaining gap.

## Outcome

One explicit command installs or upgrades the helper on one SSH target:

```bash
agentenv sudo --with prod_admin --deploy-helper [--from <file>] [--force] [--json]
```

It uses the target's existing SSH route, host-key pinning, option policy, and
login authentication; writes only the configured user-owned `helper_path` and
one temporary file beside it; verifies the installed helper's identity on the
destination; and finishes with the same handshake `--check` performs. A second
run with nothing to do changes nothing and says so.

`agentenv sudo -- <command>` continues never to upload, download, or replace
the helper. A mismatch during execution still fails before the sudo password
is resolved and names the remediation command.

## Scope and non-goals

In scope: SSH targets with Linux and macOS destinations, public-key and
saved-password login, Unix and Windows clients, first installation and
upgrade, destination architecture and libc detection, three byte sources
(operator file, local bundle, release asset), idempotent reruns.

Out of scope, and unchanged from `sudo-execution.md`: implicit deployment
during execution or in reaction to a mismatch; root or system-wide
installation; destination package managers, services, or shells other than a
POSIX `sh` with `uname`, `dirname`, `mkdir`, `cat`, `wc`, `chmod`, `mv` and
`rm`; Windows destinations; local transport (its helper is the bundled
companion and always matches); rotating or editing sudoers, SSH policy, or
configuration; verifying provenance attestations inside the binary (the
release checksum file over HTTPS is the same trust level `agentenv update`
already relies on).

## Security model

The protected boundary is unchanged: no password in argv, environment,
scripts, files, or stdout, and no weakening of SSH host or option policy. The
new capability is remote file mutation, bounded as follows.

- **Explicit invocation only.** Deployment is a separate command line. No
  execution path, mismatch, `--check`, or `agentenv update` performs it. The
  agent skill instructs agents to run it only when the user asks to install or
  upgrade a target's helper, never as automatic remediation.
- **Same route, same policy.** The deployment sessions come from the same
  `ssh::prepare` result as execution: the same pinned endpoint, known-hosts
  file, evaluated option policy, curated environment, and authentication mode.
  `PreparedSsh::command_for` replaces only the final remote-command argument.
  `ControlMaster`, forwarding, `ProxyCommand`, and jump-route rules apply
  unchanged. The acceptance check prepares the route once more, exactly as
  `--check` does.
- **One writable path.** The destination writes exactly two paths: the
  configured `helper_path` and `<helper_path>.agentenv-new.<pid>` beside it
  (stale `<helper_path>.agentenv-new*` files are removed first), under
  `umask 077` (a created parent directory is `0700`; the helper itself is made
  `0755`). The path is passed to both remote commands as a positional shell
  parameter, never spliced into script text, and only after validation:
  absolute, UTF-8, at most 1024 bytes, characters from `[A-Za-z0-9._/+-]`, no
  empty, `.` or `..` component, no trailing slash, and the file name
  `agentenv-sudo-helper` (the serving helper relaunches the helper beside
  itself under that name, so any other name cannot serve). A path outside the
  grammar is refused as `helper-deploy-invalid-helper-path`, and `--check`
  warns about it on stderr for existing entries.
- **No clobbering.** Deployment refuses when `helper_path` exists and does not
  report an `agentenv-sudo-helper` identity, is a symlink, or is not a regular
  file. The identity rule is decided at preflight; the install command itself
  refuses a symlink or a non-regular file at its start and again immediately
  before the rename, so a directory or symlink appearing during the upload is
  refused rather than receiving the file, and after the rename the path must
  be a regular file or the result is `replace-failed`. A foreign regular file
  placed at the path between preflight and rename is overwritten; that is the
  same-user threat the model excludes below. `--force` reinstalls a matching
  helper; it does not override this rule. The operator fixes the path or
  removes the file.
- **Verified bytes.** A release asset is accepted only when its SHA-256 matches
  the entry for exactly that asset name in the same release's `SHA256SUMS`
  fetched through the existing `update::release::Client` (bundled TLS roots,
  bounded body sizes and body timeout, `AGENTENV_RELEASE_BASE_URL` for
  mirrors); a malformed digest or a duplicated entry is refused. The local
  bundle companion is used only when the destination's target triple equals
  the client's own and it reports the exact expected identity locally. A
  `--from` file must be a regular file of 1 byte to 64 MiB; its acceptance is
  the destination identity check.
- **Acceptance on the destination.** The uploaded file must have the announced
  byte count, is executed once with `--identity` before it replaces anything,
  and its output (ignoring trailing newlines) must equal
  `agentenv-sudo-helper <protocol> <client version>`. Afterwards the standard
  Hello/Ready handshake runs through the installed path. SSH provides
  integrity for the upload; no remote digest tool is required.
- **Atomic replacement.** The verified file is renamed over the old one, so a
  concurrent `--serve` session keeps its open inode and a failed upload never
  leaves a partial helper at `helper_path`. Any failure leaves either the old
  helper, the newly verified helper (only when the rename had already
  happened), or nothing at the path, plus at most the temporary file, which
  the next run removes first. Rerunning is always safe and every failure
  message says so. Two deployments to the same path at the same time use
  different temporary names (the remote shell's pid, unique within one PID
  namespace; hosts sharing a `helper_path` over a shared filesystem are not
  supported), and the later one removes the earlier one's file as stale: a
  run whose file vanished fails (`upload-failed` or `replace-failed`) and
  never moves the other's upload, so the path only ever receives a verified
  file, and a rerun finishes the job. A
  directory appearing in the instant between the last check and the rename
  can leave the verified file inside it; that is the excluded same-user
  threat, and `--check` reports the path as occupied.
- **No new remote privilege.** The helper gains no upgrade or write operation;
  it remains a stdin/stdout protocol server. Deployment does not need an
  existing helper, so it works for first installation and after an
  incompatible protocol change.

What this does not defend against is unchanged: a malicious destination, a
compromised SSH server, or a same-user process on the destination can already
replace the helper; the identity self-report is not an attestation.

## Sessions and flow

Deployment runs up to three SSH sessions. With saved-password login each
session is one askpass reply, so a full deployment answers three SSH password
prompts; the one-response-per-child rule holds per session. The sudo credential
is never resolved.

Each session is `ssh <route> -T sh -c '<script>' <name> '<param>'...`. The two
scripts are fixed templates containing no `'`, `\` or `!`, and the parameters
are restricted to the same characters, so the command line means the same
thing under `sh`, `bash`, `dash`, `ksh`, `zsh`, `csh`, `tcsh` and `fish` login
shells. The client writes the session's stdin completely and half-closes it
(immediately when there is nothing to send), reads stdout to 4 KiB and fails
the session if more arrives, discards stderr, and ends the session at its
deadline (the setup timeout plus one second per 128 KiB of upload) or on
cancellation. A destination that exits before consuming the upload is
classified by its exit status, not by the broken pipe. ssh exiting 255 during
the preflight session before any output is a connection, host-key or login
failure and is reported as `ssh-connect-failed`, not as a deployment failure;
the same status during the install session, whether the login failed or the
connection ended mid-upload, is `session-failed`, because the path may already
have changed, as is an install session that ends without any exit status. An
executable that never returns from `--identity` (a foreign program at the
path during preflight, or an uploaded `--from` file during install) keeps its
session open until the deadline and then keeps running on the destination,
where its output is not bounded; the client reports `session-failed`. For
preflight nothing is written; for install the remote script can still finish
on its own, and the next run or `--check` shows the result.

1. **Preflight** prints four lines: `uname -s`, `uname -m`, `glibc <X.Y>` or
   `none`, and `absent`, `occupied` (present but a symlink, not a regular
   file, or not answering `--identity` with a helper identity) or
   `helper <identity>`. Only the `helper ` line may carry more text, because
   it relays whatever the existing executable printed; a foreign answer there
   is `occupied`. Anything else, including a login banner on stdout or text
   after a bare keyword, fails as `preflight-unparseable` before any download
   or write.
2. **Decision.** The destination target triple follows from platform and
   machine: `Linux/x86_64`, `Linux/aarch64`, `Darwin/arm64`, `Darwin/x86_64`;
   anything else is `destination-unsupported`. Linux without glibc (musl) is
   `destination-unsupported` and glibc below 2.28 is `glibc-too-old`; there is
   no prebuilt helper for them. `occupied` is `path-occupied`. If the current
   identity already equals the expected identity and `--force` is absent, the
   command reports `up-to-date` and stops before touching any source.
3. **Source selection**, in order: `--from <file>` when given; the local
   bundle companion when the destination's target equals the client's build
   target and the companion reports the expected identity within ten seconds;
   otherwise the standalone release asset
   `agentenv-sudo-helper-v<version>-<target>` for the client's exact version.
   A client built from an unreleased version, an unreachable release host, or
   a missing asset is `source-unavailable` and the message says to pass
   `--from`. A digest mismatch is `checksum-mismatch`. Cancellation is
   observed while the source is being read, probed or downloaded.
4. **Install** runs one remote command with the path, the expected identity
   and the byte count as parameters. It removes stale temporary files (exit
   21 on failure), creates the parent directory (22), refuses a symlink or a
   non-regular file at the path (23), writes stdin to the temporary file under
   `set -C` and checks the byte count (24), makes it `0755` (25), runs it with
   `--identity` and compares (26), repeats the symlink and regular-file check
   (23), renames it over `helper_path` and requires a regular file there
   afterwards (27), and echoes the identity last. Every failure removes the
   temporary file. The client requires exit 0 and stdout equal to the
   expected identity line; the statuses map to `replace-failed` (21, 22, 27),
   `path-occupied` (23), `upload-failed` (24, 25, other), `identity-mismatch`
   (26) and `session-failed` (255 or no status: the connection ended or the
   session reached its deadline).
5. **Check** is the existing check-mode handshake against the installed path
   and is the acceptance criterion; its Ready frame is part of the report. A
   failure there is `check-failed` and names the deployment status it follows.

## Interfaces

**CLI.** `--deploy-helper` conflicts with `--plan`, `--check`, `--cwd`, and a
command, and requires an SSH target. `--from <path>` and `--force` require it.
Text output states the destination, source, previous and installed
identities, and the check result. `--json` renders:

```json
{
  "status": "deployed" | "up-to-date",
  "transport": "ssh",
  "helper_path": "/home/deploy/.local/libexec/agentenv-sudo-helper",
  "destination": { "platform": "linux", "machine": "aarch64", "target": "aarch64-unknown-linux-gnu", "glibc": "2.36" },
  "source": { "kind": "release" | "bundle" | "file", "name": "agentenv-sudo-helper-v0.3.0-aarch64-unknown-linux-gnu" } | null,
  "previous": "agentenv-sudo-helper 1 0.2.0" | null,
  "installed": "agentenv-sudo-helper 1 0.3.0",
  "helper": { ...Ready... },
  "sudo_authentication": "not-attempted"
}
```

`source` is `null` and `previous` equals `installed` for `up-to-date`.

Deployment failures are exit `9` with a `sudo-execution: helper-deploy-<reason>`
line: `invalid-helper-path`, `preflight-unparseable`,
`destination-unsupported`, `glibc-too-old`, `path-occupied`,
`source-unavailable`, `checksum-mismatch`, `upload-failed`,
`identity-mismatch`, `replace-failed`, `check-failed`, `session-failed` (a
session could not be started, kept within its deadline, or read), and
`cancelled`. Every message ends by stating that `helper_path` holds the
previous helper, the newly verified one, or nothing, so rerunning is safe.
Code `10` is not used. Failures that precede or surround deployment keep their
existing vocabulary and exit codes: route preparation and policy refusals,
`ssh-connect-failed`, askpass companion failures, a missing credential
definition (exit 3) and a login credential failure (exit 4). Usage errors,
including flag conflicts, exit `1` as elsewhere in the CLI.

**Execution and check.** `helper-identity-mismatch` and
`helper-handshake-missing` messages append `to install the helper matching
this build, run: agentenv --profile=<profile> sudo --with=<entry>
--deploy-helper`. The profile is named so the command reaches the same target
however the profile was selected; names are quoted for the path grammar and
the shell, and the `--flag=value` form keeps names that start with `-`
working. `--check --json`
stdout is unchanged; `--check` adds a stderr warning when the target's
`helper_path` is outside the deployment grammar.

**Update.** After a successful `agentenv update`, the text report lists one
deployment command per SSH sudo target in the configuration the executable
would load, and the JSON report carries them as `helper_redeployments`
(`profile`, `entry`, `command`). No remote action is taken; a missing or
invalid configuration yields an empty list and is `agentenv validate`'s
business.

**Modules.** `sudo::ssh::PreparedSsh` keeps its route arguments and exposes
`command_for(remote_command)` beside the serve `command()`, so one prepared
route builds the preflight, install and serve children. `sudo::deploy` owns
the path grammar, the remote command templates, the preflight parser, the
decision table, the `SessionRunner` and `HelperSource` seams, the source
implementations and the typed report; `sudo::client` provides the SSH session
runner with the same authentication, process-group and stderr discipline as
execution, wraps the source so cancellation is observed, and runs the
acceptance check. `update::release::Client` gained `fetch_asset` (a named
standalone asset from `SHA256SUMS`) and `download_asset` with a caller-chosen
size bound; `agentenv update` uses the same code. The CLI maps the report to
text and JSON. The helper binary is unchanged.

**Configuration.** Unchanged. `helper_path` keeps its meaning; the tightened
grammar is enforced for deployment and reported as a warning by `--check`.

## Operator and agent guidance

README "Remote helper deployment" says: run `--deploy-helper` once per SSH
target after installation and after every `agentenv update`; the manual asset
procedure remains documented for hosts that the client cannot reach directly
or that require a change-managed copy, and `--from` deploys such a copy.

The agent skill states: run `--deploy-helper` only when the user asks to set
up or upgrade a target; when execution fails with `helper-identity-mismatch`
or `helper-handshake-missing`, report the remediation command and stop rather
than running it.

## Alternatives considered

- **Deploy implicitly when the handshake reports a mismatch.** Rejected. It
  turns a credential-use command into a remote installer under an agent's
  control, contradicts the accepted rule that execution mutates nothing beyond
  the sudo command, and would let any destination trigger a deployment by
  answering with a wrong version.
- **Copy with `scp` or `sftp`.** Rejected. A second OpenSSH client binary with
  its own option handling would need its own route and policy validation to
  stay pinned to the same endpoint, the SFTP subsystem may be disabled, and the
  file still needs a separate command to be made executable and verified.
- **An `Upgrade` frame served by the old helper.** Rejected. It cannot install
  a first helper or recover from an incompatible protocol, and it hands the
  helper a write capability it does not have today.
- **Leave deployment to configuration management.** Remains fully supported
  through the standalone asset and `--from`, but does not fit the single-user,
  per-workstation deployments this tool targets, where the mismatch is
  discovered by the agent mid-task.

## Milestones and hazards

| Milestone | Outcome | Hazards |
| --- | --- | --- |
| H1 | `PreparedSsh` remote-command parameterization; path grammar validation; remote command templates; preflight parser and decision table with unit tests | `security-boundary` |
| H2 | Source selection: `--from` bounds, bundle identity check, named release asset lookup and verified download | `security-boundary` |
| H3 | SSH session runner, install session, acceptance handshake, CLI and JSON report, remediation hints, connection-failure classification | `security-boundary`, `external-contract` |
| H4 | Lab coverage, README, skill, this document, `sudo-execution.md` cross-reference, release evidence entry | none |

Each hazardous milestone received independent review against its final code
and contracts, as the sudo-execution milestones did.

## Evidence

Deterministic tests (`cargo test --features test-keychain`): path grammar
acceptance and refusal including the file-name rule; remote command templates
and their parameter order; strict preflight parsing for every
platform/machine/glibc/identity combination including banner, truncated and
forged output; the decision table (`up-to-date`, `--force`, `occupied`,
unsupported, musl, glibc floor); install status classification; `--from`
bounds including a FIFO; bundle preference and fallback; named release asset
lookup, checksum, malformed and duplicate refusal against a loopback release
server; the rendered remote commands executed under every login shell
installed on the test host (fresh install `0755` in a `0700` directory, wrong
identity, truncated upload, non-program, stale temporary file, forged
`absent`, symlink and directory occupation, a directory or symlink swapped in
during the upload, two installs running at once); route reuse with only the remote
command replaced; cancellation while a source is pending; and, through a fake
`ssh` that runs the remote command locally, the whole CLI flow (usage
refusals, wrong source, first install, up-to-date, `--force`, upgrade over an
older helper, remediation text, `ssh-connect-failed` without remediation, a
connection lost during the install session, install failures that precede the
upload, the `--check` grammar warning) plus
the `agentenv update` reminder.

`tests/sudo_execution_lab/run.py --ssh` against the real loopback sshd
(Debian 13, OpenSSH 10.0p2, aarch64, glibc 2.41) records nine deployment
cases: first installation from the bundle into an absent directory (`0755`,
user-owned, no temporary file, no credential lookup), execution through the
deployed helper, `up-to-date` rerun with unchanged mtime, `--force`
reinstall, upgrade over an older helper with `--from`, a wrong source refused
by the destination's identity check, an occupied path refused, a login shell
that prints on stdout refused before any write, and saved-password login
using three sessions and no sudo lookup.

Not yet measured on a real destination: the release-asset source (the lab has
no network; the loopback release server covers it deterministically), an
x86_64 or macOS destination, a `--serve` session that outlives an upgrade, a
connection cut during upload, and the Windows client. These stay in
`sudo-release-evidence.md` as gaps until recorded.

## Open questions

1. Whether `--deploy-helper` should accept several `--with` entries or an
   `--all` selector for fleets, or whether the `agentenv update` list and a
   shell loop are enough for the intended single-operator use. Decision:
   single target first.
2. Whether `agentenv update` should offer to run deployments interactively
   after a successful local update. Decision: print the list only; remote
   mutation stays behind its own explicit command.
3. Whether to verify release provenance attestations in-process, which would
   add a Sigstore verifier dependency. Decision: not in this design; the
   checksum path matches `agentenv update`, and operators who need
   attestation verification use `gh attestation verify` with `--from`.
