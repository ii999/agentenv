# Sudo execution release evidence

Status: S7 evidence map for `sudo-execution.md`. This report records which
release-matrix contracts have real-platform evidence, which rest on
deterministic tests only, and which are unmeasured. An unmeasured case is a
release gap. It is not supported by inference from a related passing case.

## Evidence sources

| Source | What runs | Environment |
| --- | --- | --- |
| `tests/sudo_lab/run.sh` | S0 measurements of real sudo and OpenSSH behavior | Disposable Debian 13 container, `--network none`; see `sudo-compatibility.md` |
| `tests/sudo_execution_lab/run.py` | Built `agentenv` local sudo matrix (18 cases) | Same container family; sudo 1.9.16p2, default `use_pty` |
| `tests/sudo_execution_lab/run.py --ssh` | Built client, helper, and askpass companion over real sshd, including explicit helper deployment (46 cases) | Loopback-only sshd inside the container; OpenSSH 10.0p2 client and server; glibc 2.41 destination |
| `tests/sudo_execution_lab/run.py --release` | sudoers policy variations, SSH refusal cases, and passwordless remote cancellation (16 cases) | Same container; release-specific sudoers and additional loopback sshd instances |
| `tests/sudo_macos_lab/run.py` | Native macOS client against the Linux destination (8 cases) | macOS 27.0 arm64, OpenSSH_10.3p1 (LibreSSL 3.3.6); destination published only on 127.0.0.1 |
| `cargo test --features test-keychain` | Protocol, transport, client, resolver, config, askpass, and local-engine contracts | Fake sudo, SSH, and provider processes; no real authentication |
| `tests/windows_credentials.rs` and `tests/windows_lab/ssh.py` (CI `windows-latest`) | Native named-pipe resolver and bound askpass boundary; native `OpenSSH_for_Windows` client and askpass against a loopback protocol fixture | GitHub `windows-latest`; the fixture stands in for the remote helper, so no Unix sudo is involved |

The Linux and macOS lab runs recorded here used Docker 29.5.2 on arm64 and
debug builds (stripped, without debuginfo, since 2026-09-24) with the
`test-keychain` feature, not release artifacts. All
accounts, passwords, keys, host keys, and sudoers rules are synthetic and belong
to disposable containers or temporary directories. The CI `sudo-integration`
job runs the three Linux lab entry points on `ubuntu-latest` (amd64), again
with debug builds. Its run for `ef4875f` on 2026-09-24 passed
(<https://github.com/ii999/agentenv/actions/runs/36028642446/job/107731482816>)
and is the amd64 Linux evidence for the rows below; the `windows-latest` job
of the same run passed the Windows fixtures listed above.

## Contract coverage

| Contract | Real-platform evidence | Deterministic tests | Gaps |
| --- | --- | --- | --- |
| Credential confidentiality | Required-password success/failure; provider failure, newline, and oversize values rejected with status 4; the failing provider's sentinel text absent from output; sentinel values absent from checked lab output; password absent from the target environment under default and `!env_reset` policies and from sudo I/O logs | Resolver has no stdin and discards stderr; secrets redacted in Debug; password frames use a dedicated validated encoder; provider failures and invalid values fail with status 4 and without the provider's sentinel text | Denied or locked platform keychain; provider debug settings; inspection of process argv while a password is in flight; absence of newline and oversize candidate bytes from output is not asserted |
| Lazy and one-shot authentication | NOPASSWD skips lookup locally and over SSH; wrong password never resent; `-k` forces per-invocation authentication (S0) | Second sudo challenge fails without a second resolution; a duplicate PasswordRequest after Start reports completion unknown | None on the measured platform |
| Account and destination binding | Effective SSH user mismatch, unknown host key, a missing remote helper (against a password-requiring command), and `rootpw`/`targetpw`/`runaspw` prompts release no password | A wrong sudo prompt releases nothing; login askpass rejects a wrong prompt or parent process | PAM prompt replacement on a real system; a wrong invoking local account and a wrong helper identity (build, protocol, platform, or account mismatch in Ready or `--identity`) are enforced in code without a targeted test; an untrusted host key reached through `ssh-config` mode, or a user config that weakens host-key checking, is overridden in code without a targeted test |
| SSH source selection | S0: raw `ssh -G` resolves Alias, Include, and Match, and `ssh -G -F none` ignores a hostile user config. agentenv: explicit endpoint; `ssh-config` mode with a custom `config_file` over a real two-hop route; alias user mismatch rejected | Alias endpoint pinned after `ssh -G`; `--plan` performs no provider, SSH, or `ssh -G` call | Include/Match and default user/system configuration precedence through agentenv against a real server; explicit mode with a hostile user config |
| Publickey jump routes | Conforming two-hop route succeeds; unsafe inner-hop authentication, duplicate trust alias, and password-mode jump routes rejected before lookup; S0 shows outer `-o` options are absent from the native jump child, and the gate evaluates each hop's own effective configuration | Hop policy rejects authentication fallback and alternative host-key sources; the jump parser rejects more than eight hops | Cycles, custom ProxyCommand in publickey mode, unknown jump-host key, encrypted key without an agent, preflight budget exhaustion, an inner hop with BatchMode off or permissive host-key checking, and multiplexing, forwarding, and local-command hop variants are enforced in code without a targeted test |
| SSH password login | Correct, wrong, missing, newline, and oversize login values; server with the password method disabled; keyboard-interactive-only server; missing local askpass companion; expired password not answered | Prompt must match the pinned user and host; confirmation hints and non-UTF-8 prompts fail silently | Multi-factor server policies; no lab run has a terminal attached, so the absence of a terminal password fallback is enforced by `SSH_ASKPASS_REQUIRE=force` in code without a targeted test |
| Authentication-value fidelity | S0: raw sudo and OpenSSH askpass accept 255-byte values; agentenv rejects newline and 256-byte provider values at both stages before any reply | Exact-limit and one-over boundaries, CR/LF/NUL/empty/invalid UTF-8, preserved spaces and Unicode | None on the measured platform |
| Two-stage credentials | Separate and shared references resolve once per requesting stage; NOPASSWD skips sudo lookup after SSH password login; no cross-stage retry | Stage-specific resolver purposes; SSH exports of askpass routing metadata are rejected before connection | None on the measured platform |
| Command authorization | An exact sudoers rule allows the configured argv and denies a changed argument; S0 shows a shell wrapper is denied | Requests carry absolute executable and argv only | None on the measured platform |
| Stdin and argv fidelity | Binary stdin over 2 MiB; empty, whitespace, Unicode, quote, `$()`, semicolon, newline, and leading-dash arguments arrive literally; early exit with unread stdin | Stdin data or EOF before Start is rejected | Empty stdin is not asserted |
| Output and status | Independent binary stdout/stderr over 2 MiB each; codes 0, 1, 9, 10, 127, 255 locally and over SSH; locally, signal termination reports 137 and a missing executable fails without output | Result requires both output EOFs; duplicate or post-terminal frames are reported separately while the observed status is kept | Signal termination and a missing executable over SSH, outside cancellation |
| Lifecycle | SIGTERM forwarding locally and over SSH, with the target confirmed gone whenever a signal status is reported; SIGINT delivered to the whole process group, as a terminal or harness would, still reaches the remote target as Cancel and reports 130; cancelling a passwordless remote target reports its observed signal; SSH disconnect after start reports completion unknown (10) | Cancellation before and during helper setup; sudo that survives the forwarded signal reports completion unknown; Cancel after exit bounds output held open by a descendant; a queued Result survives a failed Cancel write; post-Start local failures report completion unknown; resolver and login-askpass cancellation during lookup; Cancel delivered while target stdin or output is blocked | Cancellation during a slow real keychain lookup; no lab assertion that a disconnected command was started only once; remote cancellation during SSH connect or login or while a sudo password request is pending, and disconnect after a password reply or after output, have no targeted test |
| IPC isolation | Four concurrent local and four concurrent remote invocations each succeed; a login-time banner on protocol stdout fails before any sudo lookup for a password-requiring command | Mismatched session identifiers and malformed, truncated, oversized, duplicate, and out-of-order frames; a helper stream closing right after Start reports completion unknown | Per-invocation output and credential separation under concurrency; wrong local broker peer and symlink or socket substitution are enforced in code without a targeted test; a helper crash before Ready or after a password reply |
| Resource limits | Output over 2 MiB per stream | Bounded credit over more than 1 MiB; thousands of one-byte frames within the byte window; control not starved by blocked streams; resolver child reaped on timeout; a broken local stdout reports completion unknown | A stalled invocation blocking another is not tested; broken pipes in a real lab |
| Helper deployment (`helper-deployment.md`) | First installation from the bundle into an absent directory (`0755`, user-owned, no temporary file, no credential lookup); execution through the deployed helper; `up-to-date` rerun with unchanged mtime; `--force` reinstall; upgrade over an older helper with `--from`; a wrong source refused by the destination identity check; an occupied path refused; a login-shell banner refused before any write; saved-password login uses three sessions and no sudo lookup | Path grammar and file-name rule; remote command templates executed under every installed login shell (fresh install, wrong identity, truncated upload, non-program, stale temporary file, forged `absent`, symlink and directory occupation); strict preflight parsing; decision table; install status classification; `--from` bounds; bundle identity gate; named release asset lookup, checksum, malformed and duplicate refusal against a loopback release server; cancellation of a pending source; route reuse; CLI flow through a fake `ssh` including `ssh-connect-failed` without remediation and install failures that precede the upload; the `agentenv update` reminder | Release-asset source against a real destination (the lab has no network); x86_64 and macOS destinations; a `--serve` session that outlives an upgrade; a connection cut during upload; Windows client |
| Compatibility and packaging | sudo 1.9.16p2 with and without `use_pty`; locally, `requiretty` rejected without a PTY fallback; native macOS OpenSSH client | Update installs both companions with the main binary and refuses missing or mismatched companions without replacing it | Native macOS sudo; other Linux distributions and sudo versions; Windows OpenSSH client and named-pipe askpass against a real destination; `requiretty` over SSH (no PTY is requested, enforced in code without a targeted test) |

Under `!env_reset`, the lab observed the local askpass routing variables in
the target environment. This is the documented boundary in the design: the
routing metadata is a consumed, password-free capability, and sudoers
environment policy controls whether a target sees it.

## Release gaps

These gaps block advertising the affected platforms. They do not affect the
measured arm64 and amd64 Linux destinations and the arm64 Linux, amd64 Linux,
and arm64 macOS client combinations above.

1. **Windows SSH client against a real destination.** The Windows confidential
   resolver, named-pipe login askpass, and native OpenSSH client are implemented
   (`windows-port.md`) and measured in CI against a loopback protocol fixture
   only. No Windows client has run against a real Linux destination with sudo.
   Windows local sudo and UAC elevation remain out of scope.
2. **Native macOS sudo.** Local execution on macOS compiles and passes fake-sudo
   tests, but real macOS sudo, PAM, and prompt behavior have not run in an
   isolated macOS account or VM. The macOS evidence above covers only the SSH
   client role.
3. **Other destinations.** Only Debian 13 sudo 1.9.16p2 with its default PAM
   stack has been measured. Other distributions, sudo versions, PAM prompt
   replacement, multi-factor policies, and authentication plugins are
   unmeasured.
4. **Real keychain behavior.** The labs use command providers. Locked or denied
   platform keychains and slow keychain lookups under cancellation have not run
   against a real keychain.
5. **Untested adversaries.** The cases the table marks as "enforced in code
   without a targeted test" have neither real nor deterministic coverage.
   Excess jump hops have a deterministic parser test only.
6. **Shipped build and x86_64 macOS.** Every recorded run used debug builds
   with `test-keychain`; the arm64 runs are local, the amd64 Linux run is the CI
   `sudo-integration` job recorded above. No release artifact has a recorded
   lab run on any target, and the x86_64 macOS client remains unmeasured.
7. **Helper deployment from a release.** Deployment has installed the helper
   from the local bundle and from a file on the arm64 Linux destination only.
   The release-asset source is verified against a loopback release server, not
   against GitHub Releases, and no x86_64 or macOS destination has received a
   deployment. Record a real release-sourced deployment per destination
   target before advertising `--deploy-helper` for it.
