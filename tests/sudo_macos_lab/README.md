# Native macOS SSH client lab

This fixture runs the built macOS client against the Linux application lab.
The destination is a disposable container published only on a random localhost
port. Its accounts, passwords, client key, host key, and sudoers are synthetic.
The host uses temporary configuration and key files; it does not run local sudo
or modify user credentials, keychains, SSH configuration, or known-hosts files.

Build `agentenv-sudo-execution:lab` with `tests/sudo_execution_lab/run.py`, then:

```sh
cargo build --bins --features test-keychain
python3 tests/sudo_macos_lab/run.py
```

The runner checks key and password login, readiness, lazy NOPASSWD behavior,
remote sudo authentication, and binary input larger than 2 MiB. It emits fixed
case names, statuses, and lookup counts, and removes its temporary container and
host files on completion. Host-key trust comes directly from the owned fixture's
public key, without trusting a network key scan.

This is SSH-client evidence. It does not establish native macOS sudo or Windows
support. A failing case returns a nonzero status.
