mod helpers;

use agentenv::config::{Config, CredentialUsage, SshAuth, SshConnection, SudoTransport};
use helpers::{assert_exit, assert_mentions, read_config, run_ac, staged_config};

fn no_env(_: &str) -> Option<String> {
    None
}

const VALID_EXPLICIT: &str = r#"version = 1
default_profile = "work"

[profiles.work]
description = "Work."

[credentials.account]
description = "Remote account password."
provider = "keychain"
service = "agentenv.accounts"
account = "prod/deploy"
usages = ["ssh-password", "sudo"]

[profiles.work.admin]
description = "Production administrator."
kind = "sudo-target"

[profiles.work.admin.sudo]
transport = "ssh"
credential = "credential://account"
auth_user = "deploy"
run_as = "root"
sudo_path = "/usr/bin/sudo"

[profiles.work.admin.sudo.ssh]
mode = "explicit"
hostname = "203.0.113.10"
user = "deploy"
port = 2222
host_key_alias = "agentenv-prod"
known_hosts_file = "/etc/ssh/known_hosts"
helper_path = "/home/deploy/.local/libexec/agentenv-sudo-helper"

[profiles.work.admin.sudo.ssh.auth]
method = "password"
credential = "credential://account"
"#;

#[test]
fn explicit_password_target_loads_as_closed_typed_model() {
    let (_dir, path) = staged_config(VALID_EXPLICIT);
    let config = Config::load(Some(&path), &no_env).expect("valid target loads");
    let account = config.credential("account").expect("credential exists");
    assert!(account.permits(CredentialUsage::Sudo));
    assert!(account.permits(CredentialUsage::SshPassword));
    assert_eq!(account.inject_as, None);
    let target = config
        .sudo_target(config.profile("work").unwrap(), "admin")
        .unwrap();
    let SudoTransport::Ssh(ssh) = target.transport else {
        panic!("SSH target expected")
    };
    assert!(matches!(
        ssh.connection,
        SshConnection::Explicit { port: 2222, .. }
    ));
    assert!(matches!(ssh.auth, SshAuth::Password { .. }));
}

#[test]
fn authentication_usage_and_target_boundaries_are_enforced() {
    for (replacement, expected) in [
        ("provider = \"env\"\nname = \"PASSWORD\"", "env provider"),
        (
            "provider = \"keychain\"\nservice = \"s\"\naccount = \"a\"\ninject_as = \"LEAK\"",
            "inject_as is forbidden",
        ),
    ] {
        let config = VALID_EXPLICIT.replace(
            "provider = \"keychain\"\nservice = \"agentenv.accounts\"\naccount = \"prod/deploy\"",
            replacement,
        );
        let (_dir, path) = staged_config(&config);
        let run = run_ac(&path, &[], &["validate"]);
        assert_exit(&run, 2, expected);
        assert_mentions(&run, expected, expected);
    }

    let alias = VALID_EXPLICIT.replacen(
        "credential = \"credential://account\"",
        "credential = \"credential://account?as=PASSWORD\"",
        1,
    );
    let (_dir, path) = staged_config(&alias);
    let run = run_ac(&path, &[], &["validate"]);
    assert_exit(&run, 2, "sudo aliases are rejected");
    assert_mentions(
        &run,
        "cannot use '?as='",
        "diagnostic explains the boundary",
    );
}

#[test]
fn helper_path_and_closed_tables_reject_unsafe_or_unknown_fields() {
    let config = VALID_EXPLICIT.replace(
        "helper_path = \"/home/deploy/.local/libexec/agentenv-sudo-helper\"",
        "helper_path = \"/home/deploy/helper path\"\nunknown = true",
    );
    let (_dir, path) = staged_config(&config);
    let run = run_ac(&path, &[], &["validate"]);
    assert_exit(&run, 2, "unsafe SSH target");
    assert_mentions(
        &run,
        "conservative ASCII path",
        "helper grammar is enforced",
    );
    assert_mentions(&run, "unknown field", "SSH table is closed");
}

#[test]
fn client_paths_accept_windows_drive_and_unc_forms() {
    let drive_known_hosts = VALID_EXPLICIT.replace(
        "known_hosts_file = \"/etc/ssh/known_hosts\"",
        r"known_hosts_file = 'C:\Users\operator\.ssh\known_hosts'",
    );
    let (_dir, path) = staged_config(&drive_known_hosts);
    Config::load(Some(&path), &no_env).expect("a drive-letter known-hosts path is valid");

    let ssh_config = VALID_EXPLICIT
        .replace(
            "mode = \"explicit\"\nhostname = \"203.0.113.10\"\nuser = \"deploy\"\nport = 2222",
            r#"mode = "ssh-config"
host_alias = "prod"
config_file = '\\fileserver\ssh\config'"#,
        )
        .replace(
            "known_hosts_file = \"/etc/ssh/known_hosts\"",
            r"known_hosts_file = 'C:\Users\operator\.ssh\known_hosts'",
        )
        .replace(
            "method = \"password\"\ncredential = \"credential://account\"",
            "method = \"publickey\"",
        );
    let (_dir, path) = staged_config(&ssh_config);
    Config::load(Some(&path), &no_env).expect("an UNC SSH config path is valid");

    let identity = VALID_EXPLICIT
        .replace(
            "known_hosts_file = \"/etc/ssh/known_hosts\"",
            r"known_hosts_file = '\\fileserver\ssh\known_hosts'",
        )
        .replace(
            "method = \"password\"\ncredential = \"credential://account\"",
            r#"method = "publickey"
identity_files = ['C:\Users\operator\.ssh\id_ed25519']
use_agent = false"#,
        );
    let (_dir, path) = staged_config(&identity);
    Config::load(Some(&path), &no_env).expect("a drive-letter identity path is valid");
}

#[test]
fn destination_and_remote_helper_paths_use_unix_shell_safe_grammars() {
    for sudo_path in [
        r"sudo_path = 'C:\Windows\System32\sudo.exe'",
        r"sudo_path = '\\server\share\sudo'",
    ] {
        let config = VALID_EXPLICIT.replace("sudo_path = \"/usr/bin/sudo\"", sudo_path);
        let (_dir, path) = staged_config(&config);
        let run = run_ac(&path, &[], &["validate"]);
        assert_exit(&run, 2, "non-POSIX sudo path");
        assert_mentions(&run, "absolute POSIX destination path", "sudo path grammar");
    }

    for helper_path in [
        r"helper_path = '/home/deploy/helper\name'",
        r#"helper_path = "/home/deploy/helper\u001bname""#,
    ] {
        let config = VALID_EXPLICIT.replace(
            "helper_path = \"/home/deploy/.local/libexec/agentenv-sudo-helper\"",
            helper_path,
        );
        let (_dir, path) = staged_config(&config);
        let run = run_ac(&path, &[], &["validate"]);
        assert_exit(&run, 2, "unsafe remote helper path");
        assert_mentions(
            &run,
            "control characters, backslashes",
            "helper path grammar",
        );
    }
}

#[test]
fn usage_update_is_atomic_when_an_environment_reference_would_break() {
    let source = r#"version = 1
default_profile = "work"
[profiles.work]
description = "Work."
[profiles.work.tool]
description = "Tool."
credential = "credential://shared"
[credentials.shared]
description = "Shared."
provider = "keychain"
service = "s"
account = "a"
inject_as = "TOKEN"
"#;
    let (_dir, path) = staged_config(source);
    let before = read_config(&path);
    let run = run_ac(
        &path,
        &[],
        &["credential", "update", "shared", "--usage", "sudo"],
    );
    assert_exit(&run, 1, "incompatible metadata update");
    assert_mentions(
        &run,
        "environment injection",
        "reference is identified by purpose",
    );
    assert_eq!(
        read_config(&path),
        before,
        "a refused update preserves every byte"
    );
}

#[test]
fn adding_an_authentication_usage_does_not_reclassify_existing_auth_references() {
    let source = r#"version = 1
default_profile = "work"
[profiles.work]
description = "Work."
[profiles.work.metadata]
description = "Metadata."
reference = "credential://account"
[credentials.account]
description = "Account password."
provider = "keychain"
service = "s"
account = "a"
usages = ["sudo"]
"#;
    let (_dir, path) = staged_config(source);
    let run = run_ac(
        &path,
        &[],
        &[
            "credential",
            "update",
            "account",
            "--usage",
            "sudo",
            "--usage",
            "ssh-password",
        ],
    );
    assert_exit(&run, 0, "an auth-only definition gains another auth usage");
    assert!(
        read_config(&path).contains("usages = [\"sudo\", \"ssh-password\"]"),
        "the metadata update is persisted"
    );
}

#[test]
fn ordinary_run_rejects_authentication_references_before_resolution_even_with_alias() {
    let source = r#"version = 1
default_profile = "work"
[profiles.work]
description = "Work."
[profiles.work.mixed]
description = "Mixed."
ordinary = "value"
credential = "credential://auth?as=EXPORTED_PASSWORD"
[profiles.work.array]
description = "Array."
values = ["ordinary", "credential://auth"]
[credentials.auth]
description = "Authentication only."
provider = "command"
argv = ["/definitely/not/a/provider"]
usages = ["sudo"]
"#;
    let (_dir, path) = staged_config(source);
    let run = run_ac(
        &path,
        &[],
        &["run", "--with", "mixed", "--", "/usr/bin/true"],
    );
    assert_exit(&run, 4, "authentication reference cannot be exported");
    assert_mentions(
        &run,
        "restricted to authentication use",
        "failure occurs at the usage boundary",
    );
    let array = run_ac(
        &path,
        &[],
        &["run", "--with", "array", "--", "/usr/bin/true"],
    );
    assert_exit(
        &array,
        4,
        "authentication reference in an otherwise legacy-unscanned array",
    );
}

#[test]
fn credential_add_and_query_apply_conditional_injection_contract() {
    let source = "version = 1\n";
    let (_dir, path) = staged_config(source);
    let added = run_ac(
        &path,
        &[],
        &[
            "credential",
            "add",
            "admin",
            "--description",
            "Administrator password.",
            "--provider",
            "keychain",
            "--service",
            "agentenv.sudo",
            "--account",
            "local/admin",
            "--usage",
            "sudo",
        ],
    );
    assert_exit(&added, 0, "authentication credential add");
    let file = read_config(&path);
    assert!(
        file.contains("usages = [\"sudo\"]"),
        "explicit usage is persisted: {file}"
    );
    assert!(
        !file.contains("inject_as"),
        "authentication credentials have no injection target: {file}"
    );

    let listed = run_ac(&path, &[], &["credential", "list", "--json"]);
    assert_exit(&listed, 0, "authentication credential query");
    assert_mentions(
        &listed,
        "\"inject_as\":null",
        "query reports an absent injection target",
    );
    assert_mentions(
        &listed,
        "\"usages\":[\"sudo\"]",
        "query reports permitted usages",
    );
}

#[test]
fn complete_local_target_can_be_created_with_one_json_set() {
    let source = r#"version = 1
default_profile = "work"
[profiles.work]
description = "Work."
[credentials.admin]
description = "Administrator password."
provider = "keychain"
service = "agentenv.sudo"
account = "local/admin"
usages = ["sudo"]
"#;
    let (_dir, path) = staged_config(source);
    let target = r#"{"description":"Local administrator.","kind":"sudo-target","sudo":{"transport":"local","credential":"credential://admin","auth_user":"operator","run_as":"root","sudo_path":"/usr/bin/sudo"}}"#;
    let set = run_ac(&path, &[], &["set", "admin", target, "--type", "json"]);
    assert_exit(&set, 0, "one complete target write");
    let config = Config::load(Some(&path), &no_env).expect("written target validates");
    let target = config
        .sudo_target(config.profile("work").unwrap(), "admin")
        .unwrap();
    assert!(matches!(target.transport, SudoTransport::Local));
}

#[test]
fn ssh_config_publickey_target_uses_the_selected_ssh_configuration() {
    let config = VALID_EXPLICIT
        .replace(
            "mode = \"explicit\"\nhostname = \"203.0.113.10\"\nuser = \"deploy\"\nport = 2222",
            "mode = \"ssh-config\"\nhost_alias = \"prod\"\nconfig_file = \"/etc/ssh/ssh_config\"",
        )
        .replace(
            "method = \"password\"\ncredential = \"credential://account\"",
            "method = \"publickey\"",
        );
    let (_dir, path) = staged_config(&config);
    let loaded = Config::load(Some(&path), &no_env).expect("ssh-config publickey target loads");
    let target = loaded
        .sudo_target(loaded.profile("work").unwrap(), "admin")
        .unwrap();
    let SudoTransport::Ssh(ssh) = target.transport else {
        panic!("SSH target expected")
    };
    assert!(matches!(ssh.connection, SshConnection::Config { .. }));
    assert!(matches!(
        ssh.auth,
        SshAuth::PublicKey {
            use_agent: false,
            ..
        }
    ));
}
