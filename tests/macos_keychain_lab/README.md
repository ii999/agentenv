# Native macOS Keychain lab

The integration test in `continuity.rs` exercises the real install script and
self-update path using a loopback release server. It requires macOS, Xcode
command-line tools, and an unlocked partitioned login Keychain.

```bash
cargo test --features test-support --locked --test update \
  macos_login_keychain::installer_and_update_preserve_login_keychain_authorization \
  -- --ignored --exact --test-threads=1 --nocapture
```

The test creates one uniquely named `agentenv-test-update-*` generic-password
item with synthetic data in the login Keychain. It reads the item before the
update, verifies the presence of a code-hash partition, and disables Keychain
interaction throughout credential operations. It installs a content-different
rebuild through `update --force`; this uses the ordinary update implementation
without requesting helper replacement. It verifies changed CLI bytes,
unchanged active-helper bytes, and continued credential availability.

The negative controls are an unrelated executable and an explicit upgrade to
a content-different helper using the same certificate. Both must be refused
without authorization UI. Cleanup restores the exact original helper and
removes only the test's synthetic login item. The lab does not lock, reset, or
change settings on the login Keychain.

Signing keys, public receipts, installed executables, download fixtures, and
the signing Keychain belong to temporary test directories. The native default
Keychain lookup retains the real login home while `XDG_DATA_HOME` directs the
helper to the test installation. Ordinary native tests instead select their
disposable Keychain explicitly with debug-only test controls.
