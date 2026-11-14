# Packaged release verification

`verify.py` requires Python 3.11 or newer and uses only the standard library.
It serves a packaged archive through a loopback release mirror, invokes the
platform's real installer without a pinned version, forces a self-update, and
reinstalls the pinned version. It checks executable identities after replacement.

On macOS, it creates a disposable Keychain, isolates helper state, checks native
signatures, stores and checks a synthetic credential, and verifies that helper
bytes, public receipts, and signing identity survive update and reinstall.
Production-artifact runs change the default Keychain only in a disposable
GitHub Actions runner and restore it before deleting the fixture.

```bash
python tests/release_lab/verify.py \
  --archive artifacts/agentenv-v0.5.0-aarch64-apple-darwin.tar.gz \
  --tag v0.5.0 --target aarch64-apple-darwin
```

The archive name, tag, and target identify the artifact under test. A separate
macOS fixture contains only the CLI and requires credential reads and writes
to fail with an explicit helper-installation error. Neither operation may
create helper state. Every fixture has an isolated Keychain and data directory.

Local macOS runs use an archive built with `--features test-support` and pass
`--debug-keychain`. The tool verifies that the archived CLI exposes the debug
fixture command before enabling this mode. It selects its disposable Keychain
through the debug hook and leaves the user's default selection unchanged.
This mode verifies behavior; production-artifact acceptance belongs to the
GitHub runner gate without debug hooks.

The release workflow runs this gate for both macOS architectures, both Linux
architectures, and Windows before publishing. Manual branch runs use the same
gate. The partition-aware login-Keychain continuity test remains separate in
`tests/macos_keychain_lab`: a fresh disposable Keychain does not establish that
login-Keychain partition contract.
