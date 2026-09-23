# Privileged execution with agentenv

Use this reference only for typed `sudo-target` entries. The target binds the
transport, account, credential references, sudo executable, and SSH trust
policy. Do not convert an ordinary entry into a target unless the user asks to
configure privileged execution.

## Run, inspect, and check

```text
agentenv sudo --with local_admin -- /usr/bin/id -u
agentenv --profile work sudo --with prod_admin -- /usr/bin/systemctl restart nginx
agentenv sudo --with prod_admin --cwd /var/lib/example -- /usr/bin/tee config.json
agentenv sudo --with prod_admin --plan --json -- /usr/bin/systemctl restart nginx
agentenv sudo --with prod_admin --check --json
```

The executable and optional cwd are absolute destination paths. Everything
after `--` is one argv, without implicit shell interpolation. `--plan` does not
resolve a credential, evaluate SSH config, or connect. `--check` checks the
helper and transport prerequisites and does not test sudo authentication.

Never request or place a password in chat, argv, environment variables,
configuration values, command stdin, or a generated file. Do not use `sudo -S`,
`sshpass`, Expect, or a PTY as a fallback. Execution is pipe-based and does not
support interactive programs, `requiretty`, password changes, MFA, or arbitrary
PAM conversations. Local execution refuses a terminal as stdin, so run it with
stdin redirected from a file, a pipe, or `/dev/null` when the tool harness
provides a terminal.

## Define authentication credentials

```text
agentenv credential add prod_account --description "Production deploy password." --provider keychain --service agentenv.sudo --account prod/deploy --usage sudo --usage ssh-password
agentenv credential set prod_account
agentenv credential update prod_account --usage sudo --usage ssh-password
```

Ask the user to run `credential set` at their terminal for its hidden input.
`credential update` changes purpose metadata only; it does not resolve, copy,
or replace the stored value. Authentication definitions cannot use the env
provider, `inject_as`, or a reference with `?as=`.

An authentication-purpose command provider must be noninteractive. agentenv
closes its stdin and stderr, bounds stdout, and validates the complete output
without removing a trailing newline. Configure it to emit only the complete
password bytes, with no CR, LF, or NUL. This differs from ordinary environment
injection, where command providers inherit stdin/stderr and strip one trailing
newline.

SSH login and sudo are separate authentication stages. Prefer separate stored
values. One definition may be shared only when it explicitly permits both
`ssh-password` and `sudo`; each stage still validates and delivers it once.
Rotating a shared definition changes both uses.

Each delivered value must be nonempty, complete UTF-8 of at most 255 encoded
bytes, and contain no CR, LF, or NUL. Preserve every other byte represented by
the UTF-8 value, including leading and trailing spaces. Never trim, truncate,
retry, or substitute a different credential.

## Configure and operate SSH targets

SSH-config mode selects an existing alias. Explicit mode supplies hostname,
user, and numeric port and reads no SSH config. Both require a verified
known-hosts file, a stable host-key alias, an absolute remote helper path, and
an explicit public-key or password method. Verify host fingerprints through a
trusted channel; `ssh-keyscan` alone is not identity verification.

The matching unprivileged `agentenv-sudo-helper` must already exist at the
configured remote path. Deployment is a separate user-authorized operation:
verify the standalone release asset checksum, upload it through the chosen
deployment mechanism, set it executable, and run `sudo --check`. Never install,
upload, update, or replace the remote helper implicitly. A mismatch fails
closed without releasing the sudo password.

Sudoers must grant the actual requested executable and arguments. Do not
broaden policy, add NOPASSWD, or substitute a privileged shell or wrapper.

## Interpret completion

Return target exit codes unchanged. Code `9` identifies an owned execution,
protocol, or helper failure. Code `10` means agentenv could not confirm how the
command ended, locally or over SSH: a lost connection, sudo not exiting after a
forwarded cancellation signal, or incomplete output. The command may have run;
never retry a code-10 operation automatically. Owned failures carry a
`sudo-execution:` stderr prefix, which distinguishes them from a target that
itself exits with `9` or `10`. A cancelled run reports the observed status. A
valid remote result containing status `255` is still an observed target status,
and a later `sudo-execution:` warning does not replace a reported status.

Real compatibility evidence is incomplete. Linux destinations were measured
only on arm64 Debian 13 with sudo 1.9.16p2, where the repository labs cover local
sudo, SSH key, agent, and password stages, streams, statuses, cancellation,
disconnects, sudoers policy variations, and refusal cases. The native macOS
OpenSSH client passed its lab against that destination. macOS local sudo
remains unverified.
All Windows SSH execution is unavailable because the native client boundary is
not implemented; confidential password IPC and native Windows OpenSSH behavior
are also unverified. Native Windows local sudo is unsupported.
