//! Nonconnecting SSH connection-policy preparation.
//!
//! `prepare` evaluates trusted OpenSSH configuration under one bounded setup
//! deadline, validates every selected route, and returns an immutable command
//! description. It never opens a network connection or resolves a credential.

use std::collections::{HashMap, HashSet};
use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::Duration;

use tokio::io::{AsyncRead, AsyncReadExt};
use tokio::process::Command;
use tokio::time::{timeout_at, Instant};

use crate::config::{CredentialRef, SshAuth, SshConnection, SshTarget, SudoTarget, SudoTransport};
use crate::error::AppError;

const MAX_CONFIG_OUTPUT: usize = 1024 * 1024;
const MAX_DIAGNOSTIC_OUTPUT: usize = 64 * 1024;
const MAX_JUMP_HOPS: usize = 8;
const REMOTE_SWITCH: &str = " --serve";

/// Authentication policy frozen into a prepared SSH invocation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PreparedAuth {
    PublicKey,
    Password {
        credential: CredentialRef,
        /// Exact OpenSSH 10.0 password prompt measured by S0. The trailing
        /// space is significant.
        expected_prompt: String,
    },
}

/// A validated, nonconnecting SSH invocation plan.
#[derive(Debug, Clone)]
pub struct PreparedSsh {
    pub effective_host: String,
    pub effective_user: String,
    pub effective_port: u16,
    pub auth: PreparedAuth,
    pub ssh_executable: PathBuf,
    pub arguments: Vec<OsString>,
    environment: Vec<(OsString, OsString)>,
}

impl PreparedSsh {
    /// Rebuilds the exact owned SSH child. Callers may add invocation-scoped
    /// askpass routing variables to the returned command before spawning it.
    pub fn command(&self) -> Command {
        let mut command = Command::new(&self.ssh_executable);
        command
            .args(&self.arguments)
            .env_clear()
            .envs(self.environment.iter().cloned())
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true);
        command
    }
}

/// Resolves and validates an SSH route without connecting to it.
pub async fn prepare(target: &SudoTarget, timeout: Duration) -> Result<PreparedSsh, AppError> {
    if timeout.is_zero() || timeout > Duration::from_secs(300) {
        return Err(owned(
            "invalid-timeout",
            "SSH setup timeout must be greater than zero and at most 300 seconds",
        ));
    }
    let SudoTransport::Ssh(ssh) = &target.transport else {
        return Err(owned(
            "invalid-target",
            "the selected sudo target does not use SSH transport",
        ));
    };
    let executable = find_ssh_executable()?;
    let environment = curated_environment();
    let deadline = Instant::now() + timeout;
    #[cfg(windows)]
    {
        // MSYS/Cygwin process ids, paths and askpass launch conventions are
        // different. This adapter targets native Win32 OpenSSH only.
        let version = run_bounded(
            &executable,
            &environment,
            &[OsString::from("-V")],
            deadline,
            1024,
        )
        .await?;
        if !String::from_utf8_lossy(&version.stderr).starts_with("OpenSSH_for_Windows_") {
            return Err(owned(
                "ssh-client-unsupported",
                "Windows execution requires native Win32 OpenSSH, not an MSYS/Cygwin client",
            ));
        }
    }

    let (destination, initial) = match &ssh.connection {
        SshConnection::Config {
            host_alias,
            config_file,
        } => {
            let mut arguments = config_source_arguments(config_file.as_deref());
            arguments.push(OsString::from(host_alias));
            let effective = evaluate(
                &executable,
                &environment,
                arguments,
                deadline,
                "ssh-config-evaluation",
            )
            .await?;
            (host_alias.clone(), effective)
        }
        SshConnection::Explicit {
            hostname,
            user,
            port,
        } => {
            let mut effective = EffectiveConfig::default();
            effective.insert_one("hostname", hostname.clone());
            effective.insert_one("user", user.clone());
            effective.insert_one("port", port.to_string());
            (hostname.clone(), effective)
        }
    };

    let host = required_single(&initial, "hostname", "effective-host")?;
    let user = required_single(&initial, "user", "effective-user")?;
    let port = parse_port(required_single(&initial, "port", "effective-port")?)?;
    if user != target.auth_user {
        return Err(owned(
            "account-mismatch",
            "the effective SSH user does not match the target authentication account",
        ));
    }

    let proxy_jump = effective_route(&initial, "proxyjump");
    let proxy_command = effective_route(&initial, "proxycommand");
    if proxy_command.is_some() {
        return Err(owned(
            "proxy-command-unsupported",
            "custom SSH ProxyCommand routes are unsupported",
        ));
    }

    match &ssh.auth {
        SshAuth::Password { .. } if proxy_jump.is_some() => {
            return Err(owned(
                "password-route-unsupported",
                "SSH password authentication does not support proxy or jump routes",
            ));
        }
        SshAuth::PublicKey { .. } => {
            if let Some(route) = proxy_jump.as_deref() {
                validate_jump_route(
                    &executable,
                    &environment,
                    ssh,
                    route,
                    (&host, &user, port),
                    deadline,
                )
                .await?;
            }
        }
        _ => {}
    }

    let mut evaluation_arguments = connection_arguments(ssh, &destination);
    append_security_options(&mut evaluation_arguments, ssh);
    pin_endpoint(&mut evaluation_arguments, &host, &user, port);
    evaluation_arguments.push(OsString::from(&destination));
    let final_effective = evaluate(
        &executable,
        &environment,
        evaluation_arguments.clone(),
        deadline,
        "ssh-option-validation",
    )
    .await?;
    validate_final_route(&final_effective, ssh, proxy_jump.as_deref())?;
    validate_pinned_endpoint(&final_effective, &host, &user, port)?;
    validate_final_policy(&final_effective, ssh)?;
    validate_export_policy(
        &final_effective,
        matches!(ssh.auth, SshAuth::Password { .. }),
    )?;

    let helper = ssh.helper_path.to_str().ok_or_else(|| {
        owned(
            "invalid-helper-path",
            "the configured remote helper path is not valid UTF-8",
        )
    })?;
    let mut arguments = evaluation_arguments;
    arguments.insert(arguments.len() - 1, OsString::from("-T"));
    arguments.push(OsString::from(format!("{helper}{REMOTE_SWITCH}")));

    let auth = match &ssh.auth {
        SshAuth::PublicKey { .. } => PreparedAuth::PublicKey,
        SshAuth::Password { credential } => PreparedAuth::Password {
            credential: credential.clone(),
            expected_prompt: format!("{user}@{}'s password: ", ssh.host_key_alias),
        },
    };
    Ok(PreparedSsh {
        effective_host: host,
        effective_user: user,
        effective_port: port,
        auth,
        ssh_executable: executable,
        arguments,
        environment,
    })
}

fn connection_arguments(ssh: &SshTarget, _destination: &str) -> Vec<OsString> {
    let mut arguments = match &ssh.connection {
        SshConnection::Config { config_file, .. } => {
            config_source_arguments(config_file.as_deref())
        }
        SshConnection::Explicit { .. } => config_source_arguments(Some(Path::new("none"))),
    };
    if let SshConnection::Explicit { .. } = &ssh.connection {
        push_option(&mut arguments, "IdentityFile", "none");
        if let SshAuth::PublicKey {
            identity_files,
            use_agent,
        } = &ssh.auth
        {
            for identity in identity_files {
                push_option(
                    &mut arguments,
                    "IdentityFile",
                    identity.to_string_lossy().as_ref(),
                );
            }
            if *use_agent {
                push_option(&mut arguments, "IdentitiesOnly", "no");
            } else {
                push_option(&mut arguments, "IdentitiesOnly", "yes");
                push_option(&mut arguments, "IdentityAgent", "none");
            }
        }
    }
    arguments
}

fn config_source_arguments(config_file: Option<&Path>) -> Vec<OsString> {
    match config_file {
        Some(path) => vec![OsString::from("-F"), path.as_os_str().to_owned()],
        None => Vec::new(),
    }
}

fn append_security_options(arguments: &mut Vec<OsString>, ssh: &SshTarget) {
    for (key, value) in [
        ("StrictHostKeyChecking", "yes"),
        (
            "UserKnownHostsFile",
            ssh.known_hosts_file.to_string_lossy().as_ref(),
        ),
        ("GlobalKnownHostsFile", "none"),
        ("UpdateHostKeys", "no"),
        ("VerifyHostKeyDNS", "no"),
        ("KnownHostsCommand", "none"),
        ("ControlMaster", "no"),
        ("ControlPath", "none"),
        ("ControlPersist", "no"),
        ("ForwardAgent", "no"),
        ("ForwardX11", "no"),
        ("ForwardX11Trusted", "no"),
        ("PermitLocalCommand", "no"),
        ("ClearAllForwardings", "yes"),
        ("ExitOnForwardFailure", "yes"),
        ("RequestTTY", "no"),
        ("StdinNull", "no"),
        ("ForkAfterAuthentication", "no"),
        ("SessionType", "default"),
        ("RemoteCommand", "none"),
        ("GatewayPorts", "no"),
        ("Tunnel", "no"),
        ("EnableEscapeCommandline", "no"),
        ("SecurityKeyProvider", "none"),
        ("PKCS11Provider", "none"),
        ("SendEnv", "-*"),
    ] {
        push_option(arguments, key, value);
    }
    match &ssh.auth {
        SshAuth::PublicKey { .. } => {
            push_option(arguments, "BatchMode", "yes");
            push_option(arguments, "PreferredAuthentications", "publickey");
            push_option(arguments, "PubkeyAuthentication", "yes");
            push_option(arguments, "PasswordAuthentication", "no");
            push_option(arguments, "KbdInteractiveAuthentication", "no");
        }
        SshAuth::Password { .. } => {
            push_option(arguments, "BatchMode", "no");
            push_option(arguments, "PreferredAuthentications", "password");
            push_option(arguments, "PubkeyAuthentication", "no");
            push_option(arguments, "PasswordAuthentication", "yes");
            push_option(arguments, "KbdInteractiveAuthentication", "no");
            push_option(arguments, "NumberOfPasswordPrompts", "1");
            push_option(arguments, "IdentityAgent", "none");
        }
    }
    push_option(arguments, "GSSAPIAuthentication", "no");
    push_option(arguments, "HostbasedAuthentication", "no");
    push_option(arguments, "HostKeyAlias", &ssh.host_key_alias);
}

fn pin_endpoint(arguments: &mut Vec<OsString>, host: &str, user: &str, port: u16) {
    push_option(arguments, "HostName", host);
    arguments.push(OsString::from("-l"));
    arguments.push(OsString::from(user));
    arguments.push(OsString::from("-p"));
    arguments.push(OsString::from(port.to_string()));
}

fn push_option(arguments: &mut Vec<OsString>, key: &str, value: &str) {
    arguments.push(OsString::from("-o"));
    arguments.push(OsString::from(format!(
        "{key}={}",
        quote_config_value(value)
    )));
}

fn quote_config_value(value: &str) -> String {
    if !value.is_empty()
        && value.chars().all(|character| {
            character.is_ascii_alphanumeric()
                || matches!(
                    character,
                    '-' | '_' | '.' | '/' | ':' | '@' | '%' | '+' | '*' | ',' | '[' | ']'
                )
        })
    {
        return value.to_owned();
    }
    let escaped = value.replace('\\', "\\\\").replace('"', "\\\"");
    format!("\"{escaped}\"")
}

#[derive(Debug, Default, Clone)]
struct EffectiveConfig(HashMap<String, Vec<String>>);

impl EffectiveConfig {
    fn parse(output: &[u8]) -> Result<Self, AppError> {
        let text = std::str::from_utf8(output).map_err(|_| {
            owned(
                "ssh-config-invalid",
                "OpenSSH effective configuration was not valid UTF-8",
            )
        })?;
        let mut config = Self::default();
        for line in text.lines() {
            let Some((key, value)) = line.split_once(char::is_whitespace) else {
                continue;
            };
            let value = value.trim_start();
            if !value.is_empty() {
                config
                    .0
                    .entry(key.to_ascii_lowercase())
                    .or_default()
                    .push(value.to_owned());
            }
        }
        Ok(config)
    }

    fn insert_one(&mut self, key: &str, value: String) {
        self.0.insert(key.to_owned(), vec![value]);
    }

    fn values(&self, key: &str) -> &[String] {
        self.0.get(key).map(Vec::as_slice).unwrap_or(&[])
    }

    fn first(&self, key: &str) -> Option<&str> {
        self.values(key).first().map(String::as_str)
    }
}

async fn evaluate(
    executable: &Path,
    environment: &[(OsString, OsString)],
    mut arguments: Vec<OsString>,
    deadline: Instant,
    reason: &'static str,
) -> Result<EffectiveConfig, AppError> {
    arguments.insert(0, OsString::from("-G"));
    let output = run_bounded(
        executable,
        environment,
        &arguments,
        deadline,
        MAX_CONFIG_OUTPUT,
    )
    .await?;
    if !output.status.success() {
        return Err(owned(
            reason,
            "OpenSSH could not evaluate the required connection policy",
        ));
    }
    EffectiveConfig::parse(&output.stdout)
}

struct BoundedOutput {
    status: std::process::ExitStatus,
    stdout: Vec<u8>,
    #[allow(dead_code)]
    stderr: Vec<u8>,
}

async fn run_bounded(
    executable: &Path,
    environment: &[(OsString, OsString)],
    arguments: &[OsString],
    deadline: Instant,
    stdout_limit: usize,
) -> Result<BoundedOutput, AppError> {
    let mut child = Command::new(executable);
    child
        .args(arguments)
        .env_clear()
        .envs(environment.iter().cloned())
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    #[cfg(windows)]
    child.creation_flags(windows_sys::Win32::System::Threading::CREATE_NO_WINDOW);
    let mut child = child.spawn().map_err(|_| {
        owned(
            "ssh-unavailable",
            "the configured OpenSSH client could not be started",
        )
    })?;
    let stdout = child.stdout.take().expect("piped stdout");
    let stderr = child.stderr.take().expect("piped stderr");
    let mut readers = ReaderTasks {
        stdout: tokio::spawn(read_bounded(stdout, stdout_limit)),
        stderr: tokio::spawn(read_bounded(stderr, MAX_DIAGNOSTIC_OUTPUT)),
    };
    let status = match timeout_at(deadline, child.wait()).await {
        Ok(Ok(status)) => status,
        Ok(Err(_)) => {
            let _ = child.start_kill();
            let _ = child.wait().await;
            readers.abort_and_join().await;
            return Err(owned("ssh-probe-failed", "OpenSSH probe failed"));
        }
        Err(_) => {
            let _ = child.kill().await;
            let _ = child.wait().await;
            readers.abort_and_join().await;
            return Err(owned(
                "ssh-setup-timeout",
                "OpenSSH policy evaluation exceeded the setup timeout",
            ));
        }
    };
    let first = timeout_at(deadline, async {
        tokio::select! {
            result = &mut readers.stdout => FirstReader::Stdout(result),
            result = &mut readers.stderr => FirstReader::Stderr(result),
        }
    })
    .await;
    let (stdout, stderr) = match first {
        Ok(FirstReader::Stdout(result)) => {
            let stdout = match reader_task_result(result, "OpenSSH stdout reader failed") {
                Ok(output) => output,
                Err(error) => {
                    abort_reader_task(&mut readers.stderr).await;
                    return Err(error);
                }
            };
            let stderr = match timeout_at(deadline, &mut readers.stderr).await {
                Ok(result) => reader_task_result(result, "OpenSSH stderr reader failed")?,
                Err(_) => {
                    abort_reader_task(&mut readers.stderr).await;
                    return Err(owned(
                        "ssh-setup-timeout",
                        "OpenSSH policy evaluation exceeded the setup timeout",
                    ));
                }
            };
            (stdout, stderr)
        }
        Ok(FirstReader::Stderr(result)) => {
            let stderr = match reader_task_result(result, "OpenSSH stderr reader failed") {
                Ok(output) => output,
                Err(error) => {
                    abort_reader_task(&mut readers.stdout).await;
                    return Err(error);
                }
            };
            let stdout = match timeout_at(deadline, &mut readers.stdout).await {
                Ok(result) => reader_task_result(result, "OpenSSH stdout reader failed")?,
                Err(_) => {
                    abort_reader_task(&mut readers.stdout).await;
                    return Err(owned(
                        "ssh-setup-timeout",
                        "OpenSSH policy evaluation exceeded the setup timeout",
                    ));
                }
            };
            (stdout, stderr)
        }
        Err(_) => {
            readers.abort_and_join().await;
            return Err(owned(
                "ssh-setup-timeout",
                "OpenSSH policy evaluation exceeded the setup timeout",
            ));
        }
    };
    Ok(BoundedOutput {
        status,
        stdout,
        stderr,
    })
}

enum FirstReader {
    Stdout(Result<Result<Vec<u8>, AppError>, tokio::task::JoinError>),
    Stderr(Result<Result<Vec<u8>, AppError>, tokio::task::JoinError>),
}

struct ReaderTasks {
    stdout: tokio::task::JoinHandle<Result<Vec<u8>, AppError>>,
    stderr: tokio::task::JoinHandle<Result<Vec<u8>, AppError>>,
}

impl ReaderTasks {
    async fn abort_and_join(&mut self) {
        self.stdout.abort();
        self.stderr.abort();
        let _ = tokio::join!(&mut self.stdout, &mut self.stderr);
    }
}

impl Drop for ReaderTasks {
    fn drop(&mut self) {
        self.stdout.abort();
        self.stderr.abort();
    }
}

fn reader_task_result(
    result: Result<Result<Vec<u8>, AppError>, tokio::task::JoinError>,
    detail: &'static str,
) -> Result<Vec<u8>, AppError> {
    result.map_err(|_| owned("ssh-probe-failed", detail))?
}

async fn abort_reader_task(task: &mut tokio::task::JoinHandle<Result<Vec<u8>, AppError>>) {
    task.abort();
    let _ = task.await;
}

async fn read_bounded(
    mut reader: impl AsyncRead + Unpin,
    limit: usize,
) -> Result<Vec<u8>, AppError> {
    let mut output = Vec::new();
    let mut chunk = [0_u8; 8192];
    loop {
        let read = reader
            .read(&mut chunk)
            .await
            .map_err(|_| owned("ssh-probe-failed", "OpenSSH probe output could not be read"))?;
        if read == 0 {
            return Ok(output);
        }
        if output.len().saturating_add(read) > limit {
            return Err(owned(
                "ssh-probe-output-limit",
                "OpenSSH probe output exceeded its allowed size",
            ));
        }
        output.extend_from_slice(&chunk[..read]);
    }
}

fn required_single(
    config: &EffectiveConfig,
    key: &str,
    reason: &'static str,
) -> Result<String, AppError> {
    config
        .first(key)
        .filter(|value| !value.is_empty())
        .map(str::to_owned)
        .ok_or_else(|| {
            owned(
                reason,
                "OpenSSH omitted a required effective connection field",
            )
        })
}

fn parse_port(value: String) -> Result<u16, AppError> {
    value
        .parse::<u16>()
        .ok()
        .filter(|port| *port > 0)
        .ok_or_else(|| {
            owned(
                "effective-port",
                "OpenSSH returned an invalid effective port",
            )
        })
}

fn effective_route(config: &EffectiveConfig, key: &str) -> Option<String> {
    config
        .first(key)
        .filter(|value| !is_none(value))
        .map(str::to_owned)
}

fn validate_pinned_endpoint(
    config: &EffectiveConfig,
    host: &str,
    user: &str,
    port: u16,
) -> Result<(), AppError> {
    if config.first("hostname") != Some(host)
        || config.first("user") != Some(user)
        || config.first("port") != Some(port.to_string().as_str())
    {
        return Err(owned(
            "endpoint-pin-failed",
            "OpenSSH did not retain the pinned endpoint and account",
        ));
    }
    Ok(())
}

fn validate_final_policy(config: &EffectiveConfig, ssh: &SshTarget) -> Result<(), AppError> {
    for (key, expected) in [
        ("stricthostkeychecking", true),
        ("updatehostkeys", false),
        ("verifyhostkeydns", false),
        ("forwardagent", false),
        ("forwardx11", false),
        ("permitlocalcommand", false),
        ("stdinnull", false),
        ("forkafterauthentication", false),
    ] {
        require_boolean(config, key, expected)?;
    }
    if !matches!(config.first("controlmaster"), Some("no" | "false")) {
        return Err(policy_error("connection multiplexing was not disabled"));
    }
    if !matches!(config.first("controlpersist"), Some("no" | "false"))
        || !disabled_or_omitted(config, "controlpath")
    {
        return Err(policy_error("connection reuse was not disabled"));
    }
    if config.first("globalknownhostsfile") != Some("none") {
        return Err(policy_error("global known-hosts sources were not disabled"));
    }
    if !disabled_or_omitted(config, "knownhostscommand") {
        return Err(policy_error(
            "alternate known-hosts commands were not disabled",
        ));
    }
    let configured_known_hosts = ssh.known_hosts_file.to_string_lossy();
    if config.values("userknownhostsfile") != [configured_known_hosts.as_ref()] {
        return Err(policy_error("the selected known-hosts file was not pinned"));
    }
    if !disabled_or_omitted(config, "securitykeyprovider")
        || !disabled_or_omitted(config, "pkcs11provider")
    {
        return Err(policy_error(
            "alternative dynamic key providers were not disabled",
        ));
    }
    for (key, expected) in [
        ("clearallforwardings", true),
        ("exitonforwardfailure", true),
        ("requesttty", false),
        ("gatewayports", false),
        ("tunnel", false),
        ("enableescapecommandline", false),
    ] {
        require_boolean(config, key, expected)?;
    }
    if config.first("sessiontype") != Some("default")
        || config
            .first("remotecommand")
            .is_some_and(|value| !is_none(value))
        || config.first("hostkeyalias") != Some(ssh.host_key_alias.as_str())
    {
        return Err(policy_error(
            "OpenSSH did not retain the required session and host identity policy",
        ));
    }
    let expected_auth = match ssh.auth {
        SshAuth::PublicKey { .. } => "publickey",
        SshAuth::Password { .. } => "password",
    };
    if config.first("preferredauthentications") != Some(expected_auth) {
        return Err(policy_error(
            "the explicit SSH authentication method was not retained",
        ));
    }
    match &ssh.auth {
        SshAuth::PublicKey {
            identity_files,
            use_agent,
        } => {
            require_boolean(config, "batchmode", true)?;
            require_boolean(config, "pubkeyauthentication", true)?;
            require_boolean(config, "passwordauthentication", false)?;
            require_boolean(config, "kbdinteractiveauthentication", false)?;
            if matches!(ssh.connection, SshConnection::Explicit { .. }) {
                require_boolean(config, "identitiesonly", !use_agent)?;
                if !use_agent && !disabled_or_omitted(config, "identityagent") {
                    return Err(policy_error("the SSH agent was not disabled"));
                }
                let identity_values = config.values("identityfile");
                if identity_values.first().map(String::as_str) != Some("none")
                    || identity_values.len() != identity_files.len() + 1
                    || !identity_files
                        .iter()
                        .zip(&identity_values[1..])
                        .all(|(expected, actual)| expected.to_string_lossy().as_ref() == *actual)
                {
                    return Err(policy_error(
                        "the selected SSH identity files were not retained",
                    ));
                }
            }
        }
        SshAuth::Password { .. } => {
            require_boolean(config, "batchmode", false)?;
            require_boolean(config, "pubkeyauthentication", false)?;
            require_boolean(config, "passwordauthentication", true)?;
            require_boolean(config, "kbdinteractiveauthentication", false)?;
            if config.first("numberofpasswordprompts") != Some("1") {
                return Err(policy_error("SSH password prompts were not limited to one"));
            }
        }
    }
    Ok(())
}

fn validate_final_route(
    config: &EffectiveConfig,
    ssh: &SshTarget,
    validated_jump: Option<&str>,
) -> Result<(), AppError> {
    if effective_route(config, "proxycommand").is_some() {
        return Err(owned(
            "proxy-command-unsupported",
            "the final SSH policy selected a custom ProxyCommand",
        ));
    }
    let final_jump = effective_route(config, "proxyjump");
    if matches!(ssh.auth, SshAuth::Password { .. }) && final_jump.is_some() {
        return Err(owned(
            "password-route-unsupported",
            "SSH password authentication does not support proxy or jump routes",
        ));
    }
    if final_jump.as_deref() != validated_jump {
        return Err(policy_error(
            "the final SSH options changed the validated jump route",
        ));
    }
    Ok(())
}

fn validate_export_policy(config: &EffectiveConfig, password: bool) -> Result<(), AppError> {
    for value in config.values("sendenv") {
        for pattern in value.split_ascii_whitespace() {
            if pattern.starts_with('-') {
                continue;
            }
            let safe = pattern == "LANG" || pattern == "TZ" || pattern.starts_with("LC_");
            if !safe || reserved_environment_name(pattern) {
                return Err(policy_error("SSH SendEnv could export broker metadata"));
            }
        }
    }
    for value in config.values("setenv") {
        for assignment in value.split_ascii_whitespace() {
            let name = assignment
                .split_once('=')
                .map_or(assignment, |(name, _)| name);
            if password || reserved_environment_name(name) {
                return Err(policy_error("SSH SetEnv could export broker metadata"));
            }
        }
    }
    Ok(())
}

fn reserved_environment_name(name: &str) -> bool {
    let name = name.to_ascii_uppercase();
    name.starts_with("AGENTENV_SSH_")
        || matches!(
            name.as_str(),
            "SSH_ASKPASS" | "SSH_ASKPASS_REQUIRE" | "SSH_ASKPASS_PROMPT"
        )
}

fn require_boolean(config: &EffectiveConfig, key: &str, expected: bool) -> Result<(), AppError> {
    let actual = config.first(key).and_then(parse_boolean);
    if actual != Some(expected) {
        return Err(policy_error(
            "OpenSSH did not retain a required safety option",
        ));
    }
    Ok(())
}

fn parse_boolean(value: &str) -> Option<bool> {
    match value.to_ascii_lowercase().as_str() {
        "yes" | "true" => Some(true),
        "no" | "false" => Some(false),
        _ => None,
    }
}

fn is_none(value: &str) -> bool {
    value.eq_ignore_ascii_case("none") || value.eq_ignore_ascii_case("\"none\"")
}

fn disabled_or_omitted(config: &EffectiveConfig, key: &str) -> bool {
    config.first(key).is_none_or(is_none)
}

fn single_config_word(config: &EffectiveConfig, key: &str) -> Option<String> {
    let values = config.values(key);
    if values.len() != 1 {
        return None;
    }
    let words = config_words(&values[0])?;
    (words.len() == 1).then(|| words[0].clone())
}

fn config_words(value: &str) -> Option<Vec<String>> {
    let mut words = Vec::new();
    let mut word = String::new();
    let mut quoted = false;
    let mut escaped = false;
    let mut present = false;
    for character in value.chars() {
        if escaped {
            word.push(character);
            escaped = false;
            present = true;
        } else if character == '\\' {
            escaped = true;
            present = true;
        } else if character == '"' {
            quoted = !quoted;
            present = true;
        } else if character.is_whitespace() && !quoted {
            if present {
                words.push(std::mem::take(&mut word));
                present = false;
            }
        } else {
            word.push(character);
            present = true;
        }
    }
    if escaped || quoted {
        return None;
    }
    if present {
        words.push(word);
    }
    Some(words)
}

#[derive(Debug, Clone)]
struct JumpSpec {
    raw: String,
    host: String,
    user: Option<String>,
    port: Option<u16>,
}

async fn validate_jump_route(
    executable: &Path,
    environment: &[(OsString, OsString)],
    ssh: &SshTarget,
    route: &str,
    final_endpoint: (&str, &str, u16),
    deadline: Instant,
) -> Result<(), AppError> {
    ensure_jump_client_supported(executable, environment, deadline).await?;
    let hops = parse_jump_route(route)?;
    let mut resolved = vec![None; hops.len()];
    let mut trust_identities = HashSet::new();
    trust_identities.insert(ssh.host_key_alias.to_ascii_lowercase());
    let mut next = (final_endpoint.0.to_owned(), final_endpoint.2);
    for index in (0..hops.len()).rev() {
        let hop = &hops[index];
        let mut arguments = jump_config_source(ssh)?;
        if index > 0 {
            arguments.extend([
                OsString::from("-J"),
                OsString::from(
                    hops[..index]
                        .iter()
                        .map(|item| item.raw.as_str())
                        .collect::<Vec<_>>()
                        .join(","),
                ),
            ]);
        }
        if let Some(user) = &hop.user {
            arguments.extend([OsString::from("-l"), OsString::from(user)]);
        }
        if let Some(port) = hop.port {
            arguments.extend([OsString::from("-p"), OsString::from(port.to_string())]);
        }
        arguments.extend([
            OsString::from("-W"),
            OsString::from(format!("[{}]:{}", next.0, next.1)),
            OsString::from(&hop.host),
        ]);
        let config = evaluate(
            executable,
            environment,
            arguments,
            deadline,
            "jump-policy-evaluation",
        )
        .await?;
        let expected_jump = (index > 0).then(|| {
            hops[..index]
                .iter()
                .map(|item| item.raw.as_str())
                .collect::<Vec<_>>()
                .join(",")
        });
        if effective_route(&config, "proxyjump").as_deref() != expected_jump.as_deref() {
            return Err(policy_error(
                "a jump child selected an unvalidated nested route",
            ));
        }
        if effective_route(&config, "proxycommand").is_some() {
            return Err(owned(
                "proxy-command-unsupported",
                "a jump child selected a custom ProxyCommand",
            ));
        }
        validate_hop_policy(&config)?;
        validate_export_policy(&config, false)?;
        let endpoint = (
            required_single(&config, "hostname", "jump-host")?,
            required_single(&config, "user", "jump-user")?,
            parse_port(required_single(&config, "port", "jump-port")?)?,
        );
        let trust_identity = config
            .first("hostkeyalias")
            .filter(|value| !is_none(value))
            .unwrap_or(&endpoint.0);
        record_jump_trust_identity(&mut trust_identities, trust_identity)?;
        next = (endpoint.0.clone(), endpoint.2);
        resolved[index] = Some(endpoint);
    }

    let mut identities = HashSet::new();
    identities.insert(format!(
        "{}@{}:{}",
        final_endpoint.1, final_endpoint.0, final_endpoint.2
    ));
    for endpoint in resolved.into_iter().flatten() {
        if !identities.insert(format!("{}@{}:{}", endpoint.1, endpoint.0, endpoint.2)) {
            return Err(owned("jump-cycle", "the SSH jump route contains a cycle"));
        }
    }
    Ok(())
}

fn record_jump_trust_identity(
    identities: &mut HashSet<String>,
    identity: &str,
) -> Result<(), AppError> {
    if identities.insert(identity.to_ascii_lowercase()) {
        Ok(())
    } else {
        Err(owned(
            "jump-trust-identity",
            "an SSH jump hop reuses a destination trust identity",
        ))
    }
}

fn jump_config_source(ssh: &SshTarget) -> Result<Vec<OsString>, AppError> {
    match &ssh.connection {
        SshConnection::Config { config_file, .. } => {
            Ok(config_source_arguments(config_file.as_deref()))
        }
        SshConnection::Explicit { .. } => Err(owned(
            "jump-route-unsupported",
            "explicit SSH targets do not support jump routes",
        )),
    }
}

async fn ensure_jump_client_supported(
    executable: &Path,
    environment: &[(OsString, OsString)],
    deadline: Instant,
) -> Result<(), AppError> {
    if !cfg!(target_os = "linux") {
        return Err(owned(
            "jump-client-unsupported",
            "native ProxyJump is supported only on the measured Linux OpenSSH client",
        ));
    }
    let output = run_bounded(
        executable,
        environment,
        &[OsString::from("-V")],
        deadline,
        1024,
    )
    .await?;
    let version = String::from_utf8_lossy(&output.stderr);
    if !version.contains("OpenSSH_10.0") {
        return Err(owned(
            "jump-client-unsupported",
            "native ProxyJump requires the measured OpenSSH 10.0 client semantics",
        ));
    }
    Ok(())
}

fn parse_jump_route(route: &str) -> Result<Vec<JumpSpec>, AppError> {
    let parts: Vec<&str> = route.split(',').collect();
    if parts.is_empty() || parts.len() > MAX_JUMP_HOPS || parts.iter().any(|part| part.is_empty()) {
        return Err(owned(
            "jump-route-invalid",
            "SSH ProxyJump must contain between one and eight valid hops",
        ));
    }
    parts.into_iter().map(parse_jump_spec).collect()
}

fn parse_jump_spec(raw: &str) -> Result<JumpSpec, AppError> {
    let (user, host_port) = raw
        .rsplit_once('@')
        .map_or((None, raw), |(user, rest)| (Some(user.to_owned()), rest));
    let (host, port) = if let Some(bracketed) = host_port.strip_prefix('[') {
        let (host, suffix) = bracketed.split_once(']').ok_or_else(|| {
            owned(
                "jump-route-invalid",
                "SSH ProxyJump contains an invalid IPv6 hop",
            )
        })?;
        let port = suffix.strip_prefix(':').map(parse_jump_port).transpose()?;
        if !suffix.is_empty() && port.is_none() {
            return Err(owned(
                "jump-route-invalid",
                "SSH ProxyJump contains invalid syntax",
            ));
        }
        (host.to_owned(), port)
    } else if host_port.matches(':').count() == 1 {
        let (host, port) = host_port.rsplit_once(':').expect("one colon");
        (host.to_owned(), Some(parse_jump_port(port)?))
    } else {
        (host_port.to_owned(), None)
    };
    if host.is_empty()
        || host.starts_with('-')
        || host.chars().any(|ch| ch.is_control() || ch.is_whitespace())
        || user.as_deref().is_some_and(|value| {
            value.is_empty()
                || value
                    .chars()
                    .any(|ch| ch.is_control() || ch.is_whitespace())
        })
    {
        return Err(owned(
            "jump-route-invalid",
            "SSH ProxyJump contains invalid host or account metadata",
        ));
    }
    Ok(JumpSpec {
        raw: raw.to_owned(),
        host,
        user,
        port,
    })
}

fn parse_jump_port(value: &str) -> Result<u16, AppError> {
    value
        .parse::<u16>()
        .ok()
        .filter(|port| *port > 0)
        .ok_or_else(|| {
            owned(
                "jump-route-invalid",
                "SSH ProxyJump contains an invalid port",
            )
        })
}

fn validate_hop_policy(config: &EffectiveConfig) -> Result<(), AppError> {
    for (key, expected) in [
        ("batchmode", true),
        ("pubkeyauthentication", true),
        ("passwordauthentication", false),
        ("kbdinteractiveauthentication", false),
        ("gssapiauthentication", false),
        ("hostbasedauthentication", false),
        ("stricthostkeychecking", true),
        ("updatehostkeys", false),
        ("verifyhostkeydns", false),
        ("identitiesonly", true),
        ("forwardagent", false),
        ("forwardx11", false),
        ("forwardx11trusted", false),
        ("permitlocalcommand", false),
    ] {
        require_boolean(config, key, expected)?;
    }
    if config.first("preferredauthentications") != Some("publickey")
        || !matches!(config.first("controlmaster"), Some("no" | "false"))
        || !matches!(config.first("controlpersist"), Some("no" | "false"))
        || !disabled_or_omitted(config, "controlpath")
        || config.first("globalknownhostsfile") != Some("none")
        || !disabled_or_omitted(config, "knownhostscommand")
    {
        return Err(policy_error(
            "a jump hop does not meet the public-key route policy",
        ));
    }
    let known_hosts = single_config_word(config, "userknownhostsfile");
    if known_hosts.as_deref().is_none_or(is_none) {
        return Err(policy_error(
            "a jump hop lacks one explicit known-hosts source",
        ));
    }
    let files = config.values("identityfile");
    let agent = config
        .first("identityagent")
        .filter(|value| !is_none(value));
    if !files.iter().any(|value| !is_none(value)) && agent.is_none() {
        return Err(policy_error(
            "a jump hop lacks an explicit key or allowed agent",
        ));
    }
    if !disabled_or_omitted(config, "securitykeyprovider")
        || !disabled_or_omitted(config, "pkcs11provider")
    {
        return Err(policy_error(
            "a jump hop enables an alternative key provider",
        ));
    }
    if config.values("localforward").len()
        + config.values("remoteforward").len()
        + config.values("dynamicforward").len()
        > 0
        && config.first("clearallforwardings").and_then(parse_boolean) != Some(true)
    {
        return Err(policy_error("a jump hop retains configured forwarding"));
    }
    Ok(())
}

fn find_ssh_executable() -> Result<PathBuf, AppError> {
    let path = std::env::var_os("PATH").ok_or_else(|| {
        owned(
            "ssh-unavailable",
            "PATH is unavailable, so OpenSSH cannot be located",
        )
    })?;
    for directory in std::env::split_paths(&path) {
        for name in ssh_names() {
            let candidate = directory.join(name);
            if candidate.is_file() {
                return std::fs::canonicalize(candidate).map_err(|_| {
                    owned(
                        "ssh-unavailable",
                        "the OpenSSH executable path could not be resolved",
                    )
                });
            }
        }
    }
    Err(owned(
        "ssh-unavailable",
        "a supported OpenSSH executable was not found on PATH",
    ))
}

#[cfg(windows)]
fn ssh_names() -> &'static [&'static str] {
    &["ssh.exe"]
}

#[cfg(not(windows))]
fn ssh_names() -> &'static [&'static str] {
    &["ssh"]
}

fn curated_environment() -> Vec<(OsString, OsString)> {
    std::env::vars_os()
        .filter(|(name, _)| {
            let Some(name) = name.to_str() else {
                return false;
            };
            #[cfg(windows)]
            let normalized = name.to_ascii_uppercase();
            #[cfg(windows)]
            let name = normalized.as_str();
            matches!(
                name,
                "PATH" | "HOME" | "USER" | "LOGNAME" | "LANG" | "TZ" | "SSH_AUTH_SOCK"
            ) || name.starts_with("LC_")
                || cfg!(windows)
                    && matches!(
                        name,
                        "SYSTEMROOT"
                            | "WINDIR"
                            | "USERPROFILE"
                            | "HOMEDRIVE"
                            | "HOMEPATH"
                            | "APPDATA"
                            | "LOCALAPPDATA"
                            | "TEMP"
                            | "TMP"
                            | "PROGRAMDATA"
                    )
        })
        .collect()
}

fn policy_error(detail: &'static str) -> AppError {
    owned("ssh-policy-rejected", detail)
}

fn owned(reason: &'static str, detail: &'static str) -> AppError {
    AppError::SudoExecution(format!("{reason}: {detail}"))
}

#[cfg(test)]
mod tests {
    use super::{
        config_words, parse_boolean, parse_jump_route, quote_config_value,
        record_jump_trust_identity, validate_export_policy, validate_hop_policy, EffectiveConfig,
    };
    use std::collections::HashSet;

    #[test]
    fn config_values_are_quoted_for_openssh() {
        assert_eq!(quote_config_value("none"), "none");
        assert_eq!(
            quote_config_value(r#"C:\A "quoted" path"#),
            r#""C:\\A \"quoted\" path""#
        );
    }

    #[test]
    fn boolean_normalization_accepts_openssh_forms() {
        assert_eq!(parse_boolean("yes"), Some(true));
        assert_eq!(parse_boolean("true"), Some(true));
        assert_eq!(parse_boolean("no"), Some(false));
        assert_eq!(parse_boolean("false"), Some(false));
    }

    #[test]
    fn jump_parser_bounds_and_splits_route() {
        let route = parse_jump_route("first,alice@[2001:db8::1]:2222").unwrap();
        assert_eq!(route.len(), 2);
        assert_eq!(route[1].user.as_deref(), Some("alice"));
        assert_eq!(route[1].host, "2001:db8::1");
        assert_eq!(route[1].port, Some(2222));
        assert!(parse_jump_route("a,b,c,d,e,f,g,h,i").is_err());
    }

    #[test]
    fn effective_config_word_parser_preserves_one_quoted_path() {
        assert_eq!(
            config_words(r#""/tmp/known hosts""#),
            Some(vec!["/tmp/known hosts".to_owned()])
        );
        assert_eq!(
            config_words("/tmp/first /tmp/second"),
            Some(vec!["/tmp/first".to_owned(), "/tmp/second".to_owned()])
        );
        assert_eq!(config_words(r#""unterminated"#), None);
    }

    #[test]
    fn hop_policy_rejects_authentication_fallback() {
        let config = EffectiveConfig::parse(
            br#"batchmode yes
pubkeyauthentication yes
passwordauthentication no
kbdinteractiveauthentication no
gssapiauthentication yes
hostbasedauthentication no
stricthostkeychecking yes
updatehostkeys no
identitiesonly yes
forwardagent no
forwardx11 no
forwardx11trusted no
permitlocalcommand no
preferredauthentications publickey
controlmaster false
controlpersist no
globalknownhostsfile none
userknownhostsfile /tmp/known_hosts
identityfile /tmp/id
"#,
        )
        .unwrap();
        assert!(validate_hop_policy(&config).is_err());
    }

    #[test]
    fn hop_policy_rejects_alternative_trust_sources() {
        let base = br#"batchmode yes
pubkeyauthentication yes
passwordauthentication no
kbdinteractiveauthentication no
gssapiauthentication no
hostbasedauthentication no
stricthostkeychecking yes
updatehostkeys no
verifyhostkeydns no
identitiesonly yes
forwardagent no
forwardx11 no
forwardx11trusted no
permitlocalcommand no
preferredauthentications publickey
controlmaster false
controlpersist no
globalknownhostsfile none
userknownhostsfile /tmp/known_hosts
identityfile /tmp/id
"#;
        let safe = EffectiveConfig::parse(base).unwrap();
        assert!(validate_hop_policy(&safe).is_ok());

        let dns = String::from_utf8(base.to_vec())
            .unwrap()
            .replace("verifyhostkeydns no", "verifyhostkeydns yes");
        let dns = EffectiveConfig::parse(dns.as_bytes()).unwrap();
        assert!(validate_hop_policy(&dns).is_err());

        let mut command = base.to_vec();
        command.extend_from_slice(b"knownhostscommand /usr/bin/true\n");
        let command = EffectiveConfig::parse(&command).unwrap();
        assert!(validate_hop_policy(&command).is_err());
    }

    #[test]
    fn jump_trust_identity_rejects_final_and_duplicate_aliases() {
        let mut identities = HashSet::from(["agentenv-prod".to_owned()]);
        assert!(record_jump_trust_identity(&mut identities, "jump-one").is_ok());
        assert!(record_jump_trust_identity(&mut identities, "AGENTENV-PROD").is_err());
        assert!(record_jump_trust_identity(&mut identities, "Jump-One").is_err());
    }

    #[test]
    fn export_policy_rejects_broker_metadata_case_insensitively() {
        let config = EffectiveConfig::parse(b"sendenv agentenv_ssh_session\n").unwrap();
        assert!(validate_export_policy(&config, false).is_err());
        let config = EffectiveConfig::parse(b"sendenv LANG LC_* TZ\n").unwrap();
        assert!(validate_export_policy(&config, false).is_ok());
    }
}
