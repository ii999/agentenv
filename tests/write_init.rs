//! Integration coverage for `agentenv init` (change 002: SPEC-004).

mod helpers;

use std::fs;

use helpers::{assert_exit, assert_mentions, run_ac};
use tempfile::TempDir;

#[test]
fn init_creates_a_valid_file_and_names_the_next_step() {
    // AC-004.1 + AC-004.3: the parent chain is created as needed.
    let dir = TempDir::new().expect("a temp dir");
    let config = dir
        .path()
        .join("nested")
        .join("agentenv")
        .join("config.toml");
    let run = run_ac(&config, &[], &["init"]);
    assert_exit(&run, 0, "init creates the file");
    assert_mentions(&run, "config.toml", "stdout names the created path");
    assert_mentions(&run, "agentenv set", "stdout names the next step");
    assert!(config.is_file(), "the file exists");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let bits = fs::metadata(&config)
            .expect("metadata")
            .permissions()
            .mode()
            & 0o777;
        assert_eq!(bits, 0o600, "the file is created with exactly 0600");
    }
    let validate = run_ac(&config, &[], &["validate"]);
    assert_exit(&validate, 0, "the created file validates");
}

#[test]
fn init_refuses_an_existing_file() {
    // AC-004.2.
    let dir = TempDir::new().expect("a temp dir");
    let config = dir.path().join("config.toml");
    let first = run_ac(&config, &[], &["init"]);
    assert_exit(&first, 0, "the first init succeeds");
    let before = fs::read_to_string(&config).expect("the file reads");

    let second = run_ac(&config, &[], &["init"]);
    assert_exit(&second, 2, "a second init is refused");
    assert_mentions(&second, "config.toml", "the refusal names the path");
    let after = fs::read_to_string(&config).expect("the file reads");
    assert_eq!(before, after, "the file is untouched");
}

#[test]
fn init_reports_an_uncreatable_parent_directory() {
    // AC-004.4: a regular file blocks the directory chain.
    let dir = TempDir::new().expect("a temp dir");
    let blocker = dir.path().join("blocked");
    fs::write(&blocker, b"in the way").expect("the blocking file is written");
    let config = blocker.join("config.toml");
    let run = run_ac(&config, &[], &["init"]);
    assert_exit(&run, 2, "an uncreatable parent is exit 2");
    assert_mentions(&run, "blocked", "the diagnostic names the path");
}

#[cfg(unix)]
#[test]
fn init_refuses_a_dangling_symlink() {
    // EDGE-013.
    let dir = TempDir::new().expect("a temp dir");
    let link = dir.path().join("config.toml");
    std::os::unix::fs::symlink(dir.path().join("gone.toml"), &link)
        .expect("the symlink is created");
    let run = run_ac(&link, &[], &["init"]);
    assert_exit(&run, 2, "a dangling symlink is refused");
    assert_mentions(&run, "symlink", "the diagnostic explains the state");
}

#[test]
fn init_refuses_a_directory_path() {
    let dir = TempDir::new().expect("a temp dir");
    let run = run_ac(dir.path(), &[], &["init"]);
    assert_exit(&run, 2, "a directory path is refused");
    assert_mentions(&run, "directory", "the diagnostic states the conflict");
}

#[test]
fn init_rejects_the_global_json_flag() {
    // AC-001.7 applies to every write command.
    let dir = TempDir::new().expect("a temp dir");
    let config = dir.path().join("config.toml");
    let run = run_ac(&config, &[], &["init", "--json"]);
    assert_exit(&run, 1, "init rejects --json");
    assert!(!config.exists(), "nothing is created");
}
