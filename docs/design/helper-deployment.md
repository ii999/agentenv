# Remote sudo helper deployment

Status: proposed design, 2026-09-24. Extends `sudo-execution.md`, whose
"Module and distribution boundaries" section reserved this as "a future
installer ... a separately authorized operation". Nothing here is implemented.

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
absolute path, and make it executable. The release pipeline now publishes the
standalone helper for x86_64 and aarch64 Linux and both macOS architectures
with a glibc 2.28 floor, so the right bytes exist for every supported
destination; getting them there is the remaining gap.

## Outcome

One explicit command installs or upgrades the helper on one SSH target:

```bash
agentenv sudo --with prod_admin --deploy-helper [--from <file>] [--force] [--json]
```

It uses the target's existing SSH route, host-key pinning, option policy, and
login authentication; writes only the configured user-owned `helper_path`;
verifies the installed helper's identity on the destination; and finishes with
the same handshake `--check` performs. A second run with nothing to do changes
nothing and says so.

`agentenv sudo -- <command>` continues never to upload, download, or replace
the helper. A mismatch during execution still fails before the sudo password
is resolved and now names the remediation command.

## Scope and non-goals

In scope: SSH targets with Linux and macOS destinations, public-key and
saved-password login, Unix and Windows clients, first installation and
upgrade, destination architecture and libc detection, three byte sources
(local bundle, release asset, operator file), idempotent reruns.

Out of scope, and unchanged from `sudo-execution.md`: implicit deployment
during execution or in reaction to a mismatch; root or system-wide
installation; destination package managers, services, or shells other than a
POSIX `sh` with `dirname`, `mkdir`, `cat`, `chmod`, and `mv`; Windows
destinations; local transport (its helper is the bundled companion and always
matches); rotating or editing sudoers, SSH policy, or configuration; verifying
provenance attestations inside the binary (the release checksum file over
HTTPS is the same trust level `agentenv update` already relies on).

## Security model

The protected boundary is unchanged: no password in argv, environment,
scripts, files, or stdout, and no weakening of SSH host or option policy. The
new capability is remote file mutation, bounded as follows.

- **Explicit invocation only.** Deployment is a separate command line. No
  execution path, mismatch, `--check`, or `agentenv update` performs it. The
  agent skill instructs agents to run it only when the user asks to install or
  upgrade a target's helper, never as automatic remediation.
- **Same route, same policy.** The deployment sessions are built by
  `ssh::prepare` with the same pinned endpoint, known-hosts file, evaluated
  option policy, and authentication mode as execution. The only difference is
  the remote command string. `ControlMaster`, forwarding, `ProxyCommand`, and
  jump-route rules apply unchanged.
- **One writable path.** The destination writes exactly two paths: the
  configured `helper_path` and `<helper_path>.agentenv-new` beside it, in a
  directory created with the login user's default umask tightened to `077`.
  Both are substituted into the remote command only after validation: absolute,
  UTF-8, no NUL, no control characters, and no characters outside
  `[A-Za-z0-9._/+-]`. A path outside that grammar is refused, as the current
  execution command line already interpolates `helper_path` into a remote shell
  command and would break on it anyway.
- **No clobbering.** Deployment refuses when `helper_path` exists and does not
  report an `agentenv-sudo-helper` identity, is a symlink, or is not a regular
  file. `--force` reinstalls a matching helper; it does not override this rule.
  The operator fixes the path or removes the file.
- **Verified bytes.** A release asset is accepted only when its SHA-256 matches
  the entry in the same release's `SHA256SUMS` fetched through the existing
  `update::release::Client` (bundled TLS roots, bounded body sizes,
  `AGENTENV_RELEASE_BASE_URL` for mirrors). The local bundle companion is used
  only after it reports the exact expected identity locally. A `--from` file
  must be a regular file within the size bound; its acceptance is the
  destination identity check.
- **Acceptance on the destination.** The uploaded file is executed once with
  `--identity` before it replaces anything, and the output must equal
  `agentenv-sudo-helper <protocol> <client version>` byte for byte. Afterwards
  the standard Hello/Ready handshake runs through the installed path. SSH
  provides integrity for the upload; no remote digest tool is required.
- **Atomic replacement.** The new file is renamed over the old one, so a
  concurrent `--serve` session keeps its open inode and a failed upload never
  leaves a partial helper at `helper_path`. Any failure leaves either the old
  helper or nothing at the path, plus at most the temporary file, which the
  next run removes first. Rerunning is always safe.
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
prompts; the design's one-response-per-child rule holds per session.

1. **Preflight** runs a fixed remote command that prints, one per line, the
   destination platform (`uname -s`), machine (`uname -m`), the glibc version
   or `-`, and the current helper identity at `helper_path` or `-` (for an
   absent path) or `!` (present but not a helper, symlink, or not a regular
   file). Output is bounded to 4 KiB and parsed strictly; anything else,
   including a login banner on stdout, fails before any download or write.
2. **Decision.** The destination target triple follows from platform and
   machine: `Linux/x86_64`, `Linux/aarch64`, `Darwin/arm64`, `Darwin/x86_64`.
   Linux with glibc below 2.28 or without glibc (musl) is refused with the
   reason; there is no prebuilt helper for it. If the current identity already
   equals the expected identity and `--force` is absent, the command reports
   `up-to-date` and stops. If the identity line is `!`, it refuses.
3. **Source selection**, in order: `--from <file>` when given; the local bundle
   companion when the client's own target triple equals the destination's
   (Unix clients only) and it reports the expected identity; otherwise the
   standalone release asset `agentenv-sudo-helper-v<version>-<target>` for the
   client's exact version. A client built from an unreleased version has no
   asset and is told to pass `--from`.
4. **Install** runs one remote command that removes a stale temporary file,
   creates the parent directory, reads stdin into the temporary file, makes it
   `0755`, runs it with `--identity`, and renames it over `helper_path`; each
   step's failure removes the temporary file and exits with a distinct status.
   The client writes the bytes, half-closes stdin, and requires stdout to be
   exactly the expected identity line. Upload is bounded at 64 MiB.
5. **Check** is the existing check-mode handshake against the installed path
   and is the acceptance criterion; its Ready frame is part of the report.

The remote commands are fixed templates with only the two validated paths
substituted, single-quoted. They use explicit `&&`/`||` sequencing rather
than `set -e`, so every failure path is enumerated and each one removes the
temporary file.

## Interfaces

**CLI.** `--deploy-helper` conflicts with `--plan`, `--check`, `--cwd`, and a
command. `--from <path>` and `--force` require it. Text output states the
destination, source, previous and installed identities, and the check result.
`--json` renders:

```json
{
  "status": "deployed" | "up-to-date",
  "transport": "ssh",
  "helper_path": "/home/deploy/.local/libexec/agentenv-sudo-helper",
  "destination": { "platform": "linux", "machine": "aarch64", "target": "aarch64-unknown-linux-gnu", "glibc": "2.36" },
  "source": { "kind": "release" | "bundle" | "file", "name": "agentenv-sudo-helper-v0.3.0-aarch64-unknown-linux-gnu" },
  "previous": "agentenv-sudo-helper 1 0.2.0" | null,
  "installed": "agentenv-sudo-helper 1 0.3.0",
  "helper": { ...Ready... },
  "sudo_authentication": "not-attempted"
}
```

Failures are exit `9` with a `sudo-execution: helper-deploy-<reason>` line:
`preflight-unparseable`, `destination-unsupported`, `glibc-too-old`,
`path-occupied`, `source-unavailable`, `checksum-mismatch`, `upload-failed`,
`identity-mismatch`, `replace-failed`, `check-failed`. Code `10` is not used:
every interruption leaves a consistent destination and rerun is safe, so the
message says so instead. Usage errors remain exit `2`.

**Execution and check.** `helper-identity-mismatch` and `helper-handshake-
missing` messages append `run 'agentenv sudo --with <entry> --deploy-helper'
to install the matching helper`. `--check --json` is unchanged.

**Modules.** `sudo::ssh::PreparedSsh` gains a remote-command parameter so one
prepared route builds the preflight, install, and serve children; today the
`--serve` command is baked in at prepare time. `sudo::deploy` (new) owns the
preflight parser, decision table, source selection, and install session, and
returns a typed report. `update::release` gains a lookup for a named standalone
asset in a release's `SHA256SUMS`; download and digest verification are the
existing code. The CLI maps the report to text and JSON. The helper binary is
unchanged.

**Configuration.** Unchanged. `helper_path` keeps its meaning; the tightened
character grammar is enforced for deployment and reported as a warning by
`--check` for existing entries that would fail it.

## Operator and agent guidance

README "Remote helper deployment" becomes: run `--deploy-helper` once per SSH
target after installation and after every `agentenv update`; the manual asset
procedure remains documented for hosts that the client cannot reach directly
or that require a change-managed copy. `agentenv update` prints, when the
active configuration has SSH sudo targets, the list of entries whose helpers
now need redeployment; it performs no remote action.

The agent skill states: run `--deploy-helper` only when the user asks to set
up or upgrade a target; when execution fails with `helper-identity-mismatch`,
report the remediation command and stop rather than running it.

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
| H1 | `PreparedSsh` remote-command parameterization; path grammar validation; preflight session and strict parser with unit tests | `security-boundary` |
| H2 | Source selection: bundle identity check, named release asset lookup and verified download, `--from` bounds | `security-boundary` |
| H3 | Install session, atomic replacement, acceptance handshake, CLI and JSON report, remediation hints in mismatch messages | `security-boundary`, `external-contract` |
| H4 | Lab coverage, README, skill, `sudo-execution.md` cross-reference, release evidence entry | none |

Each hazardous milestone needs independent review assurance against its final
code and contracts, as the sudo-execution milestones did.

## Acceptance and test matrix

Deterministic tests: path grammar acceptance and refusal; preflight parsing
for every platform/machine/glibc/identity combination including banner and
truncated output; decision table (`up-to-date`, `--force`, `!`, unsupported,
musl, glibc floor); source order and the unreleased-version message; remote
command templates rendered against a fake `ssh` that records argv and stdin.

`tests/sudo_execution_lab` additions against the real loopback sshd:

- First installation into an absent directory; the installed file is `0755`,
  user-owned, and `--check` succeeds.
- Upgrade over a fixture helper reporting an older version; the report shows
  previous and installed identities; a `--serve` session started before the
  upgrade completes normally.
- Rerun after success reports `up-to-date` and leaves mtime unchanged;
  `--force` reinstalls.
- `helper_path` occupied by a non-helper file, a symlink, and a directory:
  refused, unchanged.
- Connection cut during upload: `helper_path` unchanged, only the temporary
  file present, next run succeeds and removes it.
- Banner on stdout: failure before any write.
- Saved-password login: three askpass replies, no password bytes on the wire
  outside SSH authentication, the sudo credential never resolved.
- Windows client to the Linux destination with the release source, once the
  Windows lab has a release mirror fixture.

`sudo-release-evidence.md` records which destination architectures and libc
versions received a real deployment before the feature is advertised.

## Open questions

1. Whether `--deploy-helper` should accept several `--with` entries or an
   `--all` selector for fleets, or whether a shell loop is enough for the
   intended single-operator use. Recommendation: single target first.
2. Whether `agentenv update` should offer to run deployments interactively
   after a successful local update. Recommendation: print the list only;
   remote mutation stays behind its own explicit command.
3. Whether to verify release provenance attestations in-process, which would
   add a Sigstore verifier dependency. Recommendation: not in this design; the
   checksum path matches `agentenv update`, and operators who need
   attestation verification use `gh attestation verify` with `--from`.
