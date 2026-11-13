# macOS Keychain access

## Identity and process boundaries

The macOS Keychain provider uses an on-demand `agentenv-keychain-helper`.
The helper performs native generic-password reads and writes, lock-state
inspection, and system unlock requests. It runs once per request and exits;
there is no daemon, network listener, or launch agent.

The current login-Keychain implementation supplements ordinary application
access controls with a partition record. Self-signed executable partitions
include a code-directory hash. Retaining a certificate and designated
requirement therefore does not by itself preserve access when executable
contents change. The active helper's exact signed file remains unchanged
across ordinary CLI installs and updates. Main executable changes do not
change the process that accesses credentials.

The private IPC endpoint is an inherited connected Unix-domain socket on file
descriptor 3. The helper validates the socket family and peer user before
accepting a request. Request metadata and secret payloads are framed
separately; metadata serialization cannot serialize a `Secret`. The native
Keychain data-length boundary is retained, including Unicode and multiline
values. Both sides validate external frames and wrap secret buffers in
zeroizing containers. The helper receives no secret in argv, its environment,
stdout, stderr, or files. Its environment retains only process and locale
settings needed by macOS, plus debug-only test controls in test builds.

The helper checks the parent socket for closure while Security.framework is
waiting. Cancellation of the parent terminates the helper instead of leaving
an orphaned authorization window. Confidential resolver process-group
cancellation remains responsible for the full resolver subtree.

The permission boundary is the logged-in user. Other processes running as that
user can already invoke agentenv with a chosen consumer and edit user-owned
credential definitions. The helper does not authenticate the calling
application or claim isolation from arbitrary same-user execution.

## Local signing identity

Each user installation has a native, nonextractable RSA signing private key
and a retained self-signed code-signing certificate. The key permits signing
by the enrolling executable and `/usr/bin/codesign`. It is sensitive and does
not permit decryption, derivation, or unwrapping. Other programs receive no
direct private-key access. An authorized same-user caller can still invoke
codesign, consistent with the existing user-level permission boundary.

The private key never leaves the Keychain. A public certificate encoder asks
the native key to sign certificate data; it does not generate or export a
plaintext private key. The certificate contains digital-signature usage,
non-CA constraints, and a critical code-signing extended usage. No system
trust roots or Keychain partition lists are changed.

An owner-only `signing.toml` receipt records a random enrollment serial before
key creation and the exact certificate before certificate import. Pending
enrollment can recover with its original key. A deleted certificate can be
restored from its saved bytes after matching the public key. Missing enrolled
private keys, conflicting keys, and mismatched recovery metadata fail
explicitly. A new same-named certificate would not preserve identity.

Staged executable signatures bind a fixed role identifier to the certificate
leaf hash. Signature verification checks executable contents and the exact
designated requirement using public data only. It works while the signing
Keychain is locked. The source CLI signs separate staged copies and never
modifies its running executable inode.

## Installation and updates

User data lives under `XDG_DATA_HOME/agentenv/macos-keychain`, with
`HOME/.local/share` as the default data root. The directory and receipts are
owner-only. An installation lock serializes enrollment, signing, and helper
replacement; a credential operation holds a shared lock through its helper
exchange.

The install script verifies the release archive checksum and executable
identities, copies the bundle to a separate staging directory, and invokes
the downloaded CLI to prepare local signatures. The updater performs the
same preparation after download verification and before replacing installed
executables. Existing bundle rollback remains responsible for restoring
companions if main-executable replacement fails.

The release's helper candidate is installed beside the CLI. It is separate
from the active credential helper in the user data directory. A compatible,
verified active helper is preserved byte-for-byte during ordinary updates
and reinstalls. The receipt pins its SHA-256, signing identity, and protocol.
Runtime access verifies the receipt and signature before starting it.

Credential operations acquire an existing verified helper under the shared
installation lock. Missing, invalid, and partial installations fail before any
credential request is sent. Credential reads and writes do not enroll identities
or install executables. The dedicated helper alone exposes its private serving
entry; the main CLI does not provide a substitute server.

Installation and explicit setup require a dedicated helper source. They open
and copy the selected file once into private staging, validate its helper
identity, and sign only the staged inode. Missing or invalid source files fail
explicitly. The local signing identity and helper are created only through
installation, update preparation, or explicit setup.

`agentenv keychain setup` enrolls a missing installation or verifies an existing
one. `--from` selects an explicit candidate. `--replace-helper` authorizes an
intentional replacement and reports its reauthorization consequence. A modified
active helper is not silently replaced during a credential read or ordinary
update. Helper compatibility changes require an explicit upgrade decision;
maintainers must not preserve an implementation that a security fix requires
replacing.

Switching an existing credential from direct CLI access to the helper can
require a one-time item authorization. A genuine helper executable update
can require authorization again. A local signature is not an Apple Developer
ID signature and does not provide Gatekeeper notarization.

## Lock state and diagnostics

Diagnostic commands read state without resolving credentials, enrolling an
identity, or displaying a password window. Keychain lock state, graphical
session access, and process interaction permission are separate observations.
Settings that cannot be read while locked remain unavailable in the report.
An overridden home directory can affect native default-Keychain selection;
a missing default Keychain is distinguished from a missing credential.

Before a credential operation, the helper checks the Keychain's lock state.
When locked and interaction is available, it requests `SecKeychainUnlock`
without providing a password. macOS owns the unlock window and password input.
After successful unlock the original operation continues. If locking races the
first operation, one authentication-failed operation may resume after a
confirmed unlock. Item denial on an unlocked Keychain does not trigger a
lock/unlock cycle. Cancellation and unavailable UI remain explicit failures.

Structured failures carry an operation, reason, optional OSStatus, and available
session/lock observations. They contain no provider output or credential
values. These diagnostics survive the confidential resolver, filling, and
authentication boundaries; arbitrary command-provider failures retain their
existing redaction. A lost store acknowledgement reports an unconfirmed save
because native storage may already have completed.

## Verification

Native signing and helper tests use disposable Keychains, independent
bootstrap processes, synthetic credentials, and explicit cleanup. They cover
identity recovery, nonextractability, signature and helper integrity checks,
long-value round trips, lock diagnostics, unavailable interaction, and lost
store acknowledgements.

A fresh disposable Keychain can use an older format that lacks partition
checks. It cannot establish login-Keychain authorization continuity. The
explicit native lab runs the actual installer and updater against a loopback
release fixture, creates and reads one uniquely named synthetic login item,
and confirms that the item contains a code-hash partition. It changes the main
executable while preserving the active helper, reads the item with UI disabled,
and checks unrelated-code refusal. An intentional content-changing helper
upgrade is a negative control. The original helper removes the synthetic item
after the experiment.

Release publication depends on a packaged-artifact gate for every target.
The gate serves the actual production archive through a loopback release
mirror, runs latest installation, forced self-update, and pinned reinstallation,
and checks executable identities. macOS runs also check active-helper bytes,
public receipts, signing identity, and synthetic credential access. A separate
incomplete installation must refuse credential reads and writes without
creating helper state or substituting the CLI. Manual branch
workflows run these same gates without publishing. A post-publication smoke
test exercises the public download path and actual update.

The lab's signing identity and installation state remain disposable. The login
Keychain itself is never locked, reset, or reconfigured by the lab. System
unlock-window verification uses a separate disposable Keychain.
