# Windows sudo client and credential filling

This implementation extends the native Windows build. It does not change
configuration or credential-store formats and does not implement local UAC
privilege elevation or a Windows sudo server. A Windows client talks to the
existing matching Unix remote helper. Keep `agentenv.exe` and the matching
`agentenv-ssh-askpass.exe` companion in the same installed bundle.

## Boundaries

* SSH: native `OpenSSH_for_Windows` only; direct public-key/agent and saved
  password authentication retain strict pinned host verification, `ssh -G`
  policy validation, independent login/sudo lookups and no implicit retries.
  MSYS/Cygwin `ssh`, custom ProxyCommand and Windows ProxyJump are rejected.
  Ctrl+C and Ctrl+Break become protocol cancellation; observed remote results
  retain their status and unconfirmed completion remains exit 10.
* Credential resolution: one local named pipe with a protected current-user
  DACL, remote clients rejected, first-instance-only creation, and OS peer-PID
  verification. Child clients use anonymous security QoS to prevent pipe-server
  impersonation. There are no secret temporary files, argv values, environment
  values or resolver stdout. The job is assigned before any provider request,
  so dropping the operation kills the resolver and provider descendants.
* SSH askpass: additionally binds the actual helper image, its parent SSH PID,
  exact expected prompt and invocation marker. A single connected instance
  permits one response. The helper writes only to SSH's dedicated askpass
  stdout. Explicit response-consumption acknowledgements precede pipe closure.
* Filling: CDP keeps its existing targeting and mutation semantics. Windows
  desktop filling validates foreground PID, focus identity and a writable UIA
  Edit control. A dedicated MTA worker queries metadata only and never receives
  a credential. After revalidation, the operation performs a single Unicode
  `SendInput` batch itself; no detached worker can type after cancellation.
  Buffers holding UTF-16 input events are zeroized on drop.

The threat boundary remains normal-workflow confidentiality, not protection
against deliberate reads by the same user, an administrator or the destination.
A compromised trusted executable or provider is outside it. Pipe routing names
and process identifiers are not advertised as secret bearer credentials.

## Desktop limitations

The caller prepares a focused empty field. The adapter neither reads its value
nor verifies emptiness, uses the clipboard, clears it, presses Enter, clicks or
changes applications. Success is `input-sent`, not verified text or successful
login. UIA ValuePattern availability and read-only metadata are required; custom
or inaccessible controls fail closed. Held Ctrl/Alt/Shift/Windows keys are
rejected. An interactive desktop at a compatible integrity level is required;
there is no UAC/UIPI/secure-desktop bypass. Native input cannot atomically bind
focus checking to event dispatch: a small OS focus race remains. Keep the
application focused during delivery. Partial/failed input after the mutation
gate is exit 11, never an automatic retry.

UIA calls may block in another application. Only the metadata-only worker can
outlive the operation; it has no credential and cannot input anything. Cleanup
has its existing two-second grace; incomplete cleanup is explicitly reported.
Capabilities distinguishes compiled desktop support from unknown per-target
runtime availability (`available: null`).

## Build and validation

Build natively with the Rust MSVC toolchain:

```powershell
cargo build --release --bins
cargo test --features test-keychain
cargo clippy --all-targets -- -D warnings
cargo clippy --all-targets --features test-keychain -- -D warnings
```

`tests/windows_credentials.rs` exercises the real Windows named-pipe/Job Object
boundary, provider errors, 255/8192-byte limits, Unicode, cancellation of provider
descendants, timeout, direct invocation refusal and bound askpass. Its native
Credential Manager case creates and deletes only a uniquely named synthetic
entry; existing credentials are never read or modified.

The two Windows labs require a debug build containing `test-probe.exe`:

```powershell
python -m pip install paramiko==4.0.0
python tests/windows_lab/ssh.py
pwsh -NoProfile -STA -File tests/windows_lab/desktop.ps1
```

Paramiko is test-only and is not a product dependency. The SSH lab binds only
loopback and uses ephemeral generated keys and synthetic authentication. It
checks native Windows OpenSSH public-key and password login, one-attempt bad
password refusal, pinned-host refusal before credential lookup and the helper
readiness protocol. Its server is a protocol fixture, not a real Unix sudo
server. The existing isolated Linux sudo/SSH labs remain the evidence for the
actual Unix helper and sudo policy behavior.

The WinForms lab is the destination owner: it reads back only its own synthetic
fixture text to check Unicode delivery. Production fill never reads back. Cases
cover text/password inputs, read-only and non-input controls, wrong PID, and
focus changes while credential lookup is pending. A headless service session
cannot establish interactive desktop support; run this fixture on an interactive
Windows desktop. CI failures must be reported, not converted into success.

Cross-compilation is useful for API/type/lint checking but is not runtime
Windows evidence. CI results and the exact native SSH version printed by the
lab determine the measured compatibility; no claim is made for every Windows
or OpenSSH version. Earlier release evidence remains historical and is not
retroactively changed by this implementation.
