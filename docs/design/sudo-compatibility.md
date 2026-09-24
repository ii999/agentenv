# Sudo execution compatibility evidence

Status: measured Linux S0 evidence. This report supports the implementation
baseline in `sudo-execution.md`; it does not establish macOS or Windows support.

## Reproduction boundary

`tests/sudo_lab/run.sh` builds a pinned `postgres:17` image and runs a disposable
Linux lab with `--network none`. The fixture starts sshd only on container
loopback and publishes no ports. It creates only synthetic in-container users,
passwords, host keys, authorized keys, and sudoers policy. It does not read or
change host accounts, sudoers, keychains, SSH configuration, or credentials.

The successful verification run used:

| Component | Measured version |
| --- | --- |
| Docker server | 29.5.2, Linux arm64, kernel 6.8.0-117-generic |
| Base image | `postgres:17@sha256:67f41722b7a8cbdb868a44a4995c846eddfdc2973bccb291ce937dce88ad5675` |
| Container OS | Debian GNU/Linux 13.6 (trixie), arm64 |
| sudo | 1.9.16p2, Debian package `1.9.16p2-3+deb13u2` |
| OpenSSH client/server | client reports OpenSSH_10.0p2; Debian packages `1:10.0p1-7+deb13u4` |
| PAM modules | Debian package `1.7.0-5` |
| Python fixture runtime | 3.13.5 |

The run completed 28 probes with 28 passing. The generated safe evidence is
`.dev/artifacts/work/sudo-execution/scratch/sudo-lab/evidence.json`. Evidence
includes status, counts, hashes, option values, and synthetic profile labels,
never credential bytes. A caller can choose another artifact directory with
`run.sh --output-dir PATH`.

## Authentication values and prompts

The independently measured sudo and SSH stage limit for this product remains
255 UTF-8 bytes. Both stages successfully authenticated a 255-byte credential
when the helper emitted those 255 bytes plus one LF terminator. Both also
accepted a 256-byte credential plus LF on these exact builds. Therefore 255 is
a conservative product boundary rather than the underlying implementation's
observed maximum. The lab did not search for the clients' absolute maximum
beyond 256 bytes because values above the product boundary must be rejected.

Both sudo and OpenSSH accepted an askpass reply terminated by EOF with no line
terminator. Both stopped at an embedded LF or CR: a reply made from the correct
password, followed by CR or LF and then an incorrect suffix, authenticated as
the prefix. The implementation must validate the complete resolved value and
reject CR, LF, NUL, invalid UTF-8, empty values, and values over 255 encoded
bytes before invoking either reply path. It must not trim, split, or truncate.

Real `sudo -A -k -p '[agentenv:S0:%p:prompt]'` expanded `%p` to the invoking
account (`labuser`), called askpass once, and executed the authorized target as
EUID 0. This supports exact comparison of a fresh session marker and expanded
account in the conventional single-password configuration tested here.

OpenSSH forced askpass worked with `SSH_ASKPASS_REQUIRE=force`, no PTY, and
binary data on SSH stdin. `SSH_ASKPASS_PROMPT` was unset for the password prompt
on this build. It was also unset when `StrictHostKeyChecking=ask` invoked
askpass for an unknown-host confirmation. Consequently, prompt type is not a
portable discriminator on this supported Linux client. Owned execution must
force `StrictHostKeyChecking=yes` and bind the helper to the exact expected
password prompt, account, host, child, and one-shot session. Any other prompt
shape must fail before credential lookup even when `SSH_ASKPASS_PROMPT` is
absent.

## sudo execution behavior

- An unavailable askpass profile was never invoked for a matching NOPASSWD
  command; the command succeeded as EUID 0. Credential resolution must remain
  demand-driven.
- Two otherwise identical commands with `-k` caused exactly two askpass calls.
  This confirms per-invocation authentication on the tested global timestamp
  policy without a separate cache purge.
- A sudoers rule naming `/opt/sudo-lab/bin/allowed fixed` allowed that direct
  executable and argument, rejected a changed argument, and rejected a shell
  wrapper. The engine must pass the requested executable and argv directly.
- Askpass authentication did not consume command stdin. A 266-byte payload
  containing every byte value, NUL, `0xff`, and a leading newline reached the
  target unchanged by SHA-256, while target stdout and stderr stayed separate.
- An inherited descriptor 9 was closed in both the askpass process and the
  authorized target with default sudo close-from behavior. The broker design
  cannot depend on preserving an inherited password FD.

## SSH configuration and jump routes

OpenSSH `ssh -G -F <file>` resolved a Host alias, Include file, and Match block
to the expected hostname, user, port, and matched option. `ssh -G -F none` did
not apply a hostile `$HOME/.ssh/config`; explicit hostname, user, and port won.
With `IdentityFile=none`, a declared identity path, `IdentitiesOnly=yes`, and
`IdentityAgent=none`, `ssh -G` retained exactly the two identity-file entries
in command-line order. Explicit mode should use these controls and continue to
pin host-key sources separately.

For the tested OpenSSH client, each native ProxyJump hop must be evaluated with
its own `ssh -G` call and must satisfy these effective predicates:

| Property | Required effective value |
| --- | --- |
| Batch mode | `batchmode yes` |
| Authentication | `preferredauthentications publickey`, `passwordauthentication no`, `kbdinteractiveauthentication no` |
| Host verification | `stricthostkeychecking true`, one explicit `userknownhostsfile`, `globalknownhostsfile none`, `updatehostkeys false` |
| Key sources | `identitiesonly yes`, explicit identity file(s) and/or an allowed agent, with the fixture using `identityagent none` and one explicit key |
| Multiplexing | `controlmaster false`; OpenSSH 10.0p2 omits `controlpath` from `-G` output when disabled |
| Forwarding/hooks | `forwardagent no`, `forwardx11 no`, `permitlocalcommand no`; no `localforward`, `remoteforward`, or `dynamicforward` entries |

A one-hop route satisfying those predicates completed real public-key
authentication through the jump host to the final host. Changing only the
inner hop to `BatchMode no` was detectable by `ssh -G` before connection.

The native child invocation observed in OpenSSH debug output was:

```text
ssh -F /run/sudo-lab/jump_config -vv -W '[127.0.0.1]:2222' jump
```

Outer command-line `-o BatchMode=yes` and `-o PasswordAuthentication=no`
options did not appear in that child invocation. Outer restrictions therefore
cannot establish hop safety. The selected configuration must make every hop
safe independently, custom ProxyCommand remains unsupported, and password mode
must reject an effective ProxyJump or ProxyCommand before credential lookup.

## Cancellation

Sending SIGTERM to sudo terminated and reaped the tested foreground privileged
target. A privileged descendant that created a new session and ignored SIGTERM
remained alive after its sudo parent was reaped; the fixture root sent SIGKILL
only to clean up that disposable process after recording the result. Production
cancellation can request termination and report a confirmed observed result,
but cannot claim that detached privileged descendants stopped. When termination
or a terminal result cannot be confirmed, completion remains uncertain and the
command must not be replayed.

## Remaining compatibility gaps

This evidence is limited to one Debian arm64 sudo/PAM/OpenSSH combination in a
Linux container. It does not establish support for macOS sudo, macOS OpenSSH,
Windows OpenSSH or named pipes, other Linux distributions, other sudo/PAM
policies, `use_pty`, `requiretty`, `rootpw`, `targetpw`, `runaspw`, PAM prompt
replacement, MFA, or authentication plugins. It also does not measure the
absolute askpass implementation maximum above 256 bytes.

The lab covers a conforming one-hop route, one unsafe inner-hop predicate, and
the native child construction. Cycles, more than eight hops, custom proxy
commands, encrypted keys, agent-backed routes, per-hop budget exhaustion,
unknown jump-host keys, all forwarding forms, and concurrent configuration
changes remain later S5a/S7 integration evidence. Remote protocol cancellation,
disconnect ambiguity, and platform-specific process signaling remain S4/S5
work. None of these unmeasured cases is supported by inference from this report.
