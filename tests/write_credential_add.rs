//! Integration coverage for `agentenv credential add` (change 002: SPEC-005).

mod helpers;

use helpers::{assert_exit, assert_mentions, read_config, run_ac, staged_config};

const BASE_CONFIG: &str = r#"version = 1
default_profile = "work"

[profiles.work]
description = "Work environment."

[profiles.work.llm]
description = "LLM entry."
endpoint = "https://llm.example.com/v1"

[credentials.existing]
description = "Already defined."
provider = "env"
name = "EXISTING_TOKEN"
inject_as = "EXISTING"
"#;

#[test]
fn add_defines_each_provider_kind() {
    // AC-005.1.
    let (_dir, config) = staged_config(BASE_CONFIG);

    let env = run_ac(
        &config,
        &[],
        &[
            "credential",
            "add",
            "env_cred",
            "--description",
            "Env credential.",
            "--provider",
            "env",
            "--env-var",
            "MY_TOKEN_VAR",
            "--inject-as",
            "OPENAI_API_KEY",
        ],
    );
    assert_exit(&env, 0, "an env credential adds");

    let keychain = run_ac(
        &config,
        &[],
        &[
            "credential",
            "add",
            "kc_cred",
            "--description",
            "Keychain credential.",
            "--provider",
            "keychain",
            "--service",
            "agentenv",
            "--account",
            "me",
            "--inject-as",
            "KC_KEY",
        ],
    );
    assert_exit(&keychain, 0, "a keychain credential adds");

    let command = run_ac(
        &config,
        &[],
        &[
            "credential",
            "add",
            "cmd_cred",
            "--description",
            "Command credential.",
            "--provider",
            "command",
            "--argv",
            "op",
            "--argv",
            "read",
            "--argv",
            "op://Vault/item/token",
            "--inject-as",
            "CMD_KEY",
        ],
    );
    assert_exit(&command, 0, "a command credential adds");

    let list = run_ac(&config, &[], &["credential", "list", "--json"]);
    assert_exit(&list, 0, "the definitions list");
    for needle in ["env_cred", "kc_cred", "cmd_cred", "OPENAI_API_KEY"] {
        assert_mentions(&list, needle, "credential list shows the definition");
    }
    // credential list's v1 shape omits provider fields; the file itself is
    // the verification surface for them.
    let file = read_config(&config);
    for needle in [
        "name = \"MY_TOKEN_VAR\"",
        "service = \"agentenv\"",
        "account = \"me\"",
        "argv = [\"op\", \"read\", \"op://Vault/item/token\"]",
    ] {
        assert!(file.contains(needle), "provider fields are written: {file}");
    }
    let validate = run_ac(&config, &[], &["validate"]);
    assert_exit(&validate, 0, "the mutated file validates");
}

#[test]
fn add_refuses_a_duplicate_name() {
    // AC-005.2.
    let (_dir, config) = staged_config(BASE_CONFIG);
    let before = read_config(&config);
    let run = run_ac(
        &config,
        &[],
        &[
            "credential",
            "add",
            "existing",
            "--description",
            "d",
            "--provider",
            "env",
            "--env-var",
            "X",
            "--inject-as",
            "X",
        ],
    );
    assert_exit(&run, 1, "a duplicate name is refused");
    assert_mentions(&run, "existing", "the refusal names the credential");
    assert_eq!(before, read_config(&config), "the file is unchanged");
}

#[test]
fn keychain_add_hints_at_credential_set() {
    // AC-005.3.
    let (_dir, config) = staged_config(BASE_CONFIG);
    let run = run_ac(
        &config,
        &[],
        &[
            "credential",
            "add",
            "kc2",
            "--description",
            "d",
            "--provider",
            "keychain",
            "--service",
            "agentenv",
            "--account",
            "acct",
            "--inject-as",
            "K2",
        ],
    );
    assert_exit(&run, 0, "the keychain credential adds");
    assert_mentions(
        &run,
        "credential set kc2",
        "stdout hints at storing the value",
    );
}

#[test]
fn provider_flag_mismatches_are_usage_errors() {
    // AC-005.4 (mismatched) and missing required provider flags.
    let (_dir, config) = staged_config(BASE_CONFIG);
    let before = read_config(&config);

    let mismatched = run_ac(
        &config,
        &[],
        &[
            "credential",
            "add",
            "c1",
            "--description",
            "d",
            "--provider",
            "env",
            "--env-var",
            "X",
            "--argv",
            "op",
            "--inject-as",
            "X",
        ],
    );
    assert_exit(&mismatched, 1, "--argv with the env provider");
    assert_mentions(&mismatched, "--argv", "the refusal names the flag");

    let missing = run_ac(
        &config,
        &[],
        &[
            "credential",
            "add",
            "c2",
            "--description",
            "d",
            "--provider",
            "keychain",
            "--service",
            "agentenv",
            "--inject-as",
            "X",
        ],
    );
    assert_exit(&missing, 1, "keychain without --account");
    assert_mentions(&missing, "--account", "the refusal names the flag");
    assert_eq!(before, read_config(&config), "the file is unchanged");
}

#[test]
fn invalid_names_and_targets_are_usage_errors() {
    // AC-005.5 plus an invalid --inject-as.
    let (_dir, config) = staged_config(BASE_CONFIG);
    let before = read_config(&config);

    let bad_name = run_ac(
        &config,
        &[],
        &[
            "credential",
            "add",
            "my cred",
            "--description",
            "d",
            "--provider",
            "env",
            "--env-var",
            "X",
            "--inject-as",
            "X",
        ],
    );
    assert_exit(&bad_name, 1, "an invalid credential name");
    assert_mentions(&bad_name, "A-Za-z0-9_-", "the refusal names the pattern");

    let bad_target = run_ac(
        &config,
        &[],
        &[
            "credential",
            "add",
            "c3",
            "--description",
            "d",
            "--provider",
            "env",
            "--env-var",
            "X",
            "--inject-as",
            "1bad",
        ],
    );
    assert_exit(&bad_target, 1, "an invalid --inject-as target");
    assert_eq!(before, read_config(&config), "the file is unchanged");
}

#[test]
fn added_credentials_are_immediately_referenceable() {
    // The credential add -> set reference ordering from SPEC-002.
    let (_dir, config) = staged_config(BASE_CONFIG);
    let add = run_ac(
        &config,
        &[],
        &[
            "credential",
            "add",
            "fresh",
            "--description",
            "d",
            "--provider",
            "env",
            "--env-var",
            "FRESH_TOKEN",
            "--inject-as",
            "FRESH",
        ],
    );
    assert_exit(&add, 0, "the credential adds");
    let reference = run_ac(
        &config,
        &[],
        &["set", "llm.credential", "credential://fresh"],
    );
    assert_exit(&reference, 0, "the new credential is referenceable");
}

#[test]
fn add_rejects_the_global_json_flag() {
    // AC-001.7 applies to every write command.
    let (_dir, config) = staged_config(BASE_CONFIG);
    let run = run_ac(
        &config,
        &[],
        &[
            "credential",
            "add",
            "cx",
            "--description",
            "d",
            "--provider",
            "env",
            "--env-var",
            "X",
            "--inject-as",
            "X",
            "--json",
        ],
    );
    assert_exit(&run, 1, "credential add rejects --json");
}
