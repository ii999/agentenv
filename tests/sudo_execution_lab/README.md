# Built application sudo lab

This lab builds all three matching executables and runs them against real sudo
and OpenSSH inside a disposable Debian container. Accounts, passwords, keys,
provider scripts, and sudoers rules are synthetic fixtures. The container has no
external network and exposes no ports. Host accounts, keychains, and sudoers are
untouched.

Prerequisites: Python 3, Cargo, and a running Docker engine. Build the base image
with `./tests/sudo_lab/run.sh`, then run:

```sh
python3 tests/sudo_execution_lab/run.py
python3 tests/sudo_execution_lab/run.py --ssh
python3 tests/sudo_execution_lab/run.py --release
```

The runner vendors exactly `Cargo.lock`'s dependencies into a temporary directory
and builds offline inside the pinned Rust image. Its Docker context contains an
explicit repository-source allowlist and those dependency sources; it excludes
Git state, user configuration, credential stores, and workflow artifacts.

Checks cover lazy password resolution, exact sudoers authorization, one-shot
wrong-password behavior, invalid provider values, binary streams, literal argv,
observed exit codes, concurrent isolation, and signal forwarding. Output contains
only fixed check names, statuses, and non-secret measurements. A failing check
returns a nonzero status. `--build-only` builds without executing the fixtures.

The SSH matrix starts loopback-only sshd instances and a test SSH agent. It
covers file/agent selection, separate and shared login/sudo credentials, lazy
NOPASSWD behavior, setup/authentication deadlines, host-key refusal, helper
availability, binary input/output over 2 MiB, literal argv and status 255,
concurrency, signals to the CLI and to its whole process group, and connection loss
after command start. A real two-hop
route is checked alongside rejected unsafe hop policies and account mismatches.

The release matrix varies sudoers policy: default and preserved environments,
sudo I/O logging, `!use_pty`, `requiretty`, and `rootpw`/`targetpw`/`runaspw`.
It inspects the target environment and I/O logs for password bytes and checks
signal termination and a missing executable. Its SSH cases refuse servers
without the password method or with keyboard-interactive only, a missing local
askpass companion, and an expired-password change without answering it.

Successful Linux results do not establish native macOS local sudo, Windows,
other sudo versions, or other PAM policies. The separate `tests/sudo_macos_lab`
fixture exercises the native macOS SSH client against this Linux destination.
