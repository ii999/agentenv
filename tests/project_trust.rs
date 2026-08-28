//! Trust-store contract for project files (SPEC-003).
//!
//! These tests drive the store through the production filesystem adapter in a
//! temporary tree. The cases that need a scheduled failure — an interrupted
//! commit — live beside the code in `src/project/trust.rs`, where the
//! `StoreFs` seam can be substituted; everything observable through a real
//! filesystem lives here.

use std::fs;
use std::path::PathBuf;

use agentenv::error::AppError;
use agentenv::project::trust::{fingerprint, RealFs, TrustStore};
use tempfile::TempDir;

const PINNED: &[u8] = b"version = 1\nprofile = \"work\"\n";
const BARE: &[u8] = b"version = 1\n";

fn tree() -> TempDir {
    TempDir::new().expect("a temporary directory is available")
}

/// The store location inside a test tree. Nothing creates it; the store is
/// expected to create its own directory when it first saves.
fn store_file(tree: &TempDir) -> PathBuf {
    tree.path()
        .join("state")
        .join("agentenv")
        .join("trust.toml")
}

/// Writes a project file at `relative` and returns its canonical path, which
/// is the trust identity the store is keyed by.
fn project_file(tree: &TempDir, relative: &str, content: &[u8]) -> PathBuf {
    let path = tree.path().join(relative);
    fs::create_dir_all(path.parent().expect("the project file has a parent"))
        .expect("the project directory is created");
    fs::write(&path, content).expect("the project file is written");
    path.canonicalize()
        .expect("the project file has a canonical path")
}

/// Unwraps the exit-2 configuration error every trust-store failure uses and
/// renders its violations for message assertions.
fn config_error_text(error: AppError) -> String {
    assert_eq!(error.exit_code(), 2, "store failures exit 2: {error}");
    match error {
        AppError::Config(violations) => violations
            .iter()
            .map(ToString::to_string)
            .collect::<Vec<_>>()
            .join("; "),
        other => panic!("expected a configuration error, got {other:?}"),
    }
}

#[test]
fn a_fingerprint_changes_with_any_byte_change() {
    // AC-003.3 at the store level: whitespace counts as a change.
    assert_eq!(
        fingerprint(PINNED),
        fingerprint(b"version = 1\nprofile = \"work\"\n"),
        "identical bytes fingerprint identically"
    );
    assert_ne!(
        fingerprint(PINNED),
        fingerprint(b"version = 1\n\nprofile = \"work\"\n"),
        "an added blank line changes the fingerprint"
    );
    assert_ne!(
        fingerprint(PINNED),
        fingerprint(b"version = 1\nprofile = \"work\""),
        "a dropped trailing newline changes the fingerprint"
    );
    assert_ne!(
        fingerprint(PINNED),
        fingerprint(BARE),
        "different content fingerprints differently"
    );

    let hex = fingerprint(BARE);
    assert_eq!(hex.len(), 64, "a hex SHA-256 digest is 64 characters");
    assert!(
        hex.chars().all(|character| character.is_ascii_hexdigit()),
        "the fingerprint is hex-encoded: {hex}"
    );
}

#[test]
fn lookup_is_keyed_by_the_canonical_path() {
    let tree = tree();
    let approved = project_file(&tree, "repo/.agentenv.toml", PINNED);
    let other = project_file(&tree, "elsewhere/.agentenv.toml", PINNED);

    let mut store = TrustStore::load(&store_file(&tree), &RealFs)
        .expect("a missing store loads as an empty store");
    store.allow(&approved, PINNED);

    assert_eq!(
        store.lookup(&approved),
        Some(fingerprint(PINNED).as_str()),
        "the approved path resolves to the approved fingerprint"
    );
    assert_eq!(
        store.lookup(&other),
        None,
        "identical content at another path is not approved"
    );
}

#[cfg(unix)]
#[test]
fn lookup_matches_a_path_reached_through_a_symlinked_ancestor() {
    // AC-003.7 at the store level.
    let tree = tree();
    let approved = project_file(&tree, "real/.agentenv.toml", PINNED);
    std::os::unix::fs::symlink(tree.path().join("real"), tree.path().join("link"))
        .expect("the symlinked ancestor is created");
    let through_link = tree
        .path()
        .join("link/.agentenv.toml")
        .canonicalize()
        .expect("the symlinked spelling canonicalizes");

    let mut store = TrustStore::load(&store_file(&tree), &RealFs)
        .expect("a missing store loads as an empty store");
    store.allow(&approved, PINNED);

    assert_eq!(
        store.lookup(&through_link),
        Some(fingerprint(PINNED).as_str()),
        "trust follows the canonical path, not the spelling it was reached through"
    );
}

#[test]
fn a_missing_store_loads_as_empty() {
    let tree = tree();
    let path = store_file(&tree);
    assert!(!path.exists(), "the test tree starts without a store");

    let store = TrustStore::load(&path, &RealFs).expect("a missing store is an empty store");
    assert_eq!(
        store.lookup(&tree.path().join("repo/.agentenv.toml")),
        None,
        "an empty store approves nothing"
    );
}

#[test]
fn a_corrupt_store_names_the_path_and_a_next_action() {
    // AC-003.8: never silently treated as empty.
    let tree = tree();
    let path = store_file(&tree);
    fs::create_dir_all(path.parent().expect("the store path has a parent"))
        .expect("the state directory is created");
    fs::write(&path, b"this is not a trust store\n").expect("the corrupt store is written");

    let error = TrustStore::load(&path, &RealFs)
        .expect_err("a store that exists but cannot be parsed is an error");
    let text = config_error_text(error);
    assert!(
        text.contains(&path.display().to_string()),
        "the error names the store path: {text}"
    );
    assert!(
        text.contains("agentenv project allow"),
        "the error states a next action: {text}"
    );
}

#[cfg(unix)]
#[test]
fn saving_creates_the_store_with_owner_only_permissions() {
    // AC-003.9.
    use std::os::unix::fs::PermissionsExt;

    let tree = tree();
    let path = store_file(&tree);
    let approved = project_file(&tree, "repo/.agentenv.toml", PINNED);

    let mut store =
        TrustStore::load(&path, &RealFs).expect("a missing store loads as an empty store");
    store.allow(&approved, PINNED);
    store.save(&path, &RealFs).expect("the store saves");

    let mode = fs::metadata(&path)
        .expect("the saved store exists")
        .permissions()
        .mode();
    assert_eq!(
        mode & 0o777,
        0o600,
        "the store is readable and writable by its owner only"
    );
}

#[test]
fn revoking_one_approval_preserves_the_other() {
    // AC-003.10, and AC-003.13 at the store level: revoke is path-only, so it
    // never consults the content it removes.
    let tree = tree();
    let path = store_file(&tree);
    let alpha = project_file(&tree, "alpha/.agentenv.toml", PINNED);
    let beta = project_file(&tree, "beta/.agentenv.toml", BARE);

    let mut store =
        TrustStore::load(&path, &RealFs).expect("a missing store loads as an empty store");
    store.allow(&alpha, PINNED);
    store.allow(&beta, BARE);
    store.save(&path, &RealFs).expect("the store saves");

    let mut reloaded = TrustStore::load(&path, &RealFs).expect("the saved store parses");
    assert_eq!(
        reloaded.lookup(&alpha),
        Some(fingerprint(PINNED).as_str()),
        "the first approval survives the second"
    );
    assert_eq!(
        reloaded.lookup(&beta),
        Some(fingerprint(BARE).as_str()),
        "the second approval is recorded"
    );

    assert!(
        reloaded.revoke(&alpha),
        "revoking a recorded path reports that an approval existed"
    );
    assert!(
        !reloaded.revoke(&alpha),
        "a second revoke reports that no approval existed"
    );
    reloaded
        .save(&path, &RealFs)
        .expect("the store saves again");

    let after = TrustStore::load(&path, &RealFs).expect("the store still parses after a revoke");
    assert_eq!(
        after.lookup(&alpha),
        None,
        "the revoked approval is gone from the saved store"
    );
    assert_eq!(
        after.lookup(&beta),
        Some(fingerprint(BARE).as_str()),
        "the untouched approval is preserved"
    );
}

#[test]
fn approval_binds_the_snapshot_it_was_given() {
    // AC-003.12 at the store level: approval binds the bytes handed to
    // `allow`, so content that replaces them afterwards is untrusted.
    let tree = tree();
    let path = store_file(&tree);
    let approved = project_file(&tree, "repo/.agentenv.toml", PINNED);

    let mut store =
        TrustStore::load(&path, &RealFs).expect("a missing store loads as an empty store");
    store.allow(&approved, PINNED);
    store.save(&path, &RealFs).expect("the store saves");

    fs::write(&approved, BARE).expect("the project file is replaced after approval");

    let reloaded = TrustStore::load(&path, &RealFs).expect("the saved store parses");
    let recorded = reloaded
        .lookup(&approved)
        .expect("the approval is recorded for the canonical path");
    let on_disk = fs::read(&approved).expect("the replaced project file reads");
    assert_eq!(
        recorded,
        fingerprint(PINNED),
        "the recorded fingerprint is the approved snapshot"
    );
    assert_ne!(
        recorded,
        fingerprint(&on_disk),
        "the replacement content no longer matches the approval"
    );
}
