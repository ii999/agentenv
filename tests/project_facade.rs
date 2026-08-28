//! End-to-end library tests for the project trust facade (SPEC-003, SPEC-005).

use std::fs;
use std::path::{Path, PathBuf};

use agentenv::error::AppError;
use agentenv::project::{allow, resolve, revoke, ProjectContext, UntrustedReason};
use tempfile::TempDir;

mod helpers;
use helpers::STATE_BASE_ENV;

const VALID: &[u8] = b"version = 1\nprofile = \"work\"\n";

fn tree() -> TempDir {
    TempDir::new().expect("a temporary tree is available")
}

fn environment(state: &Path) -> impl Fn(&str) -> Option<String> + '_ {
    move |name| match name {
        name if name == STATE_BASE_ENV => Some(state.display().to_string()),
        _ => None,
    }
}

fn no_environment(_: &str) -> Option<String> {
    None
}

fn project_file(tree: &TempDir, contents: &[u8]) -> PathBuf {
    let directory = tree.path().join("repo/nested");
    fs::create_dir_all(&directory).expect("project directory is created");
    let file = tree.path().join("repo/.agentenv.toml");
    fs::write(&file, contents).expect("project file is written");
    file
}

fn cwd(tree: &TempDir) -> PathBuf {
    tree.path().join("repo/nested")
}

fn expected_path(file: &Path) -> PathBuf {
    file.canonicalize()
        .expect("project file has a canonical path")
}

#[test]
fn lifecycle_approves_invalidates_and_revokes_a_project_file() {
    let tree = tree();
    let file = project_file(&tree, VALID);
    let state = tree.path().join("state");
    let env = environment(&state);
    let directory = cwd(&tree);

    match resolve(&directory, &env).expect("new project state resolves") {
        ProjectContext::Untrusted {
            path,
            reason: UntrustedReason::New,
            meta: Some(meta),
        } => {
            assert_eq!(path, expected_path(&file));
            assert_eq!(meta.pin.expect("pin is parsed").name, "work");
        }
        other => panic!("expected a new untrusted project, got {other:?}"),
    }

    let first = allow(&directory, &env).expect("valid project can be approved");
    assert_eq!(first.path, expected_path(&file));
    assert!(!first.already_current, "first approval creates a record");

    match resolve(&directory, &env).expect("approved project resolves") {
        ProjectContext::Trusted { path, meta } => {
            assert_eq!(path, expected_path(&file));
            assert_eq!(meta.pin.expect("pin is preserved").name, "work");
        }
        other => panic!("expected a trusted project, got {other:?}"),
    }

    let second = allow(&directory, &env).expect("re-approving current bytes succeeds");
    assert!(second.already_current, "matching approval is reported");

    fs::write(&file, b"version = 1\nprofile = \"work\"\n\n").expect("project file is changed");
    assert!(matches!(
        resolve(&directory, &env).expect("changed project resolves"),
        ProjectContext::Untrusted {
            reason: UntrustedReason::Changed,
            ..
        }
    ));

    let revoked = revoke(&directory, &env).expect("changed project can be revoked");
    assert!(revoked.record_existed, "the stale record is removed");
    assert!(
        !revoke(&directory, &env)
            .expect("a second revoke succeeds")
            .record_existed
    );
}

#[test]
fn invalid_content_outranks_a_stale_approval_and_cannot_be_allowed() {
    let tree = tree();
    let file = project_file(&tree, VALID);
    let state = tree.path().join("state");
    let env = environment(&state);
    let directory = cwd(&tree);
    allow(&directory, &env).expect("valid file is approved first");

    fs::write(&file, b"version = \"wrong\"\n").expect("project becomes invalid");
    match resolve(&directory, &env).expect("invalid state is reported, not rejected") {
        ProjectContext::Untrusted {
            reason: UntrustedReason::Invalid(violations),
            meta: None,
            ..
        } => assert!(!violations.is_empty(), "validation violations are retained"),
        other => panic!("expected invalid to outrank changed, got {other:?}"),
    }

    let error = allow(&directory, &env).expect_err("invalid content cannot be approved");
    assert_eq!(error.exit_code(), 2);
    assert!(matches!(error, AppError::Config(_)));

    let revoked = revoke(&directory, &env).expect("invalid file can still be revoked by path");
    assert!(revoked.record_existed, "the original approval is removed");
}

#[test]
fn missing_state_base_is_unavailable_for_reads_and_an_error_for_mutations() {
    let tree = tree();
    project_file(&tree, VALID);
    let directory = cwd(&tree);

    match resolve(&directory, &no_environment).expect("read path degrades to untrusted") {
        ProjectContext::Untrusted {
            reason: UntrustedReason::StateUnavailable(message),
            ..
        } => {
            assert!(message.contains(STATE_BASE_ENV));
            assert!(message.contains("HOME"));
        }
        other => panic!("expected unavailable state, got {other:?}"),
    }

    let error = allow(&directory, &no_environment)
        .expect_err("approval needs a state base and fails explicitly");
    assert_eq!(error.exit_code(), 2);
    assert!(matches!(error, AppError::Config(_)));
    let error = revoke(&directory, &no_environment)
        .expect_err("revocation needs a state base and fails explicitly");
    assert_eq!(error.exit_code(), 2);
}

#[test]
fn mutations_require_a_discovered_project_file() {
    let tree = tree();
    let state = tree.path().join("state");
    let env = environment(&state);

    assert!(matches!(
        resolve(tree.path(), &env).expect("missing project is a normal context"),
        ProjectContext::None
    ));
    let allow_error = allow(tree.path(), &env).expect_err("allow requires a project file");
    let revoke_error = revoke(tree.path(), &env).expect_err("revoke requires a project file");
    for error in [allow_error, revoke_error] {
        assert_eq!(error.exit_code(), 5);
        assert!(matches!(error, AppError::ProjectTrust(_)));
        assert!(error.to_string().contains(".agentenv.toml"));
    }
}

#[test]
fn corrupt_trust_store_is_propagated_as_a_configuration_error() {
    let tree = tree();
    project_file(&tree, VALID);
    let state = tree.path().join("state");
    let store = state.join("agentenv/trust.toml");
    fs::create_dir_all(store.parent().expect("store has a parent"))
        .expect("state directory is created");
    fs::write(&store, "not a trust store").expect("corrupt store is written");
    let env = environment(&state);

    let error = resolve(&cwd(&tree), &env).expect_err("corrupt state never degrades to empty");
    assert_eq!(error.exit_code(), 2);
    assert!(matches!(error, AppError::Config(_)));
    assert!(error.to_string().contains(&store.display().to_string()));
}

#[cfg(unix)]
#[test]
fn approval_uses_the_canonical_path_through_a_symlinked_ancestor() {
    let tree = tree();
    let file = project_file(&tree, VALID);
    let real_repo = tree.path().join("repo");
    let linked_repo = tree.path().join("linked-repo");
    std::os::unix::fs::symlink(&real_repo, &linked_repo).expect("symlinked ancestor is created");
    let linked_cwd = linked_repo.join("nested");
    let state = tree.path().join("state");
    let env = environment(&state);

    let outcome = allow(&linked_cwd, &env).expect("symlinked project is approved");
    assert_eq!(outcome.path, expected_path(&file));
    match resolve(&cwd(&tree), &env).expect("real spelling sees the same approval") {
        ProjectContext::Trusted { path, .. } => assert_eq!(path, expected_path(&file)),
        other => panic!("expected trust through either spelling, got {other:?}"),
    }
}
