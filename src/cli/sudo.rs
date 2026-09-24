use std::io::IsTerminal;
use std::path::PathBuf;
use std::time::Duration;

use agentenv::config::{Config, Profile, SudoTransport};
use agentenv::credential::resolver::{self, ResolutionStage};
use agentenv::error::AppError;
use agentenv::path::single_entry_name;
use agentenv::sudo::{self, ExecutionRequest, LocalOptions, ProcessIo, SUDO_PASSWORD_LIMIT};
use clap::Args;

use super::Output;

#[derive(Debug, Args)]
pub struct SudoArgs {
    /// Configured sudo target entry.
    #[arg(long = "with", value_name = "ENTRY")]
    pub target: String,
    /// Working directory applied before sudo starts.
    #[arg(long, value_name = "PATH")]
    pub cwd: Option<PathBuf>,
    /// Print the resolved request without contacting a provider or host.
    #[arg(long, conflicts_with = "check")]
    pub plan: bool,
    /// Check the endpoint, helper compatibility, and execution prerequisites.
    #[arg(long, conflicts_with = "plan")]
    pub check: bool,
    /// Setup deadline in seconds.
    #[arg(long, default_value_t = 30, value_parser = clap::value_parser!(u64).range(1..=300))]
    pub connect_timeout_secs: u64,
    /// Credential response deadline in seconds.
    #[arg(long, default_value_t = 60, value_parser = clap::value_parser!(u64).range(1..=300))]
    pub auth_timeout_secs: u64,
    /// Absolute executable and its arguments, after `--`.
    #[arg(last = true, value_name = "COMMAND")]
    pub command: Vec<String>,
}

pub(crate) fn execute(
    config: &Config,
    profile: &Profile,
    args: SudoArgs,
    json: bool,
) -> Result<Output, AppError> {
    let entry = single_entry_name(&args.target)?;
    let target = config.sudo_target(profile, &entry).ok_or_else(|| {
        AppError::Usage(format!(
            "'{entry}' is not a configured sudo target in profile '{}'",
            profile.name
        ))
    })?;
    let request = request(&args)?;
    if args.plan {
        let request = request.ok_or_else(command_required)?;
        return render_plan(profile, &entry, &target, &request, json);
    }
    if args.check {
        if request.is_some() {
            return Err(AppError::Usage(
                "sudo --check does not accept a command".to_owned(),
            ));
        }
        if args.cwd.is_some() {
            return Err(AppError::Usage(
                "sudo --check does not accept --cwd".to_owned(),
            ));
        }
        return check(config, &target, &args, json);
    }
    if json {
        return Err(AppError::Usage(
            "sudo execution does not support --json; use --plan --json or --check --json"
                .to_owned(),
        ));
    }
    let request = request.ok_or_else(command_required)?;
    run(config, target, request, &args)
}

fn request(args: &SudoArgs) -> Result<Option<ExecutionRequest>, AppError> {
    let Some((executable, arguments)) = args.command.split_first() else {
        return Ok(None);
    };
    Ok(Some(ExecutionRequest::new(
        PathBuf::from(executable),
        arguments.to_vec(),
        args.cwd.clone(),
    )?))
}

fn command_required() -> AppError {
    AppError::Usage(
        "sudo execution and --plan require an absolute command after '--'; --check takes no command"
            .to_owned(),
    )
}

fn local_options(
    target: &agentenv::config::SudoTarget,
    args: &SudoArgs,
) -> Result<LocalOptions, AppError> {
    let executable = std::env::current_exe().map_err(|_| {
        AppError::SudoExecution(
            "helper-missing: could not determine the agentenv executable path".to_owned(),
        )
    })?;
    Ok(LocalOptions {
        sudo_path: target.sudo_path.clone(),
        helper_path: sudo::companion_path(&executable)?,
        auth_user: target.auth_user.clone(),
        run_as: target.run_as.clone(),
        setup_timeout: Duration::from_secs(args.connect_timeout_secs),
        auth_timeout: Duration::from_secs(args.auth_timeout_secs),
    })
}

fn runtime() -> Result<tokio::runtime::Runtime, AppError> {
    tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .map_err(|_| {
            AppError::SudoExecution(
                "runtime-unavailable: could not initialize sudo execution".to_owned(),
            )
        })
}

fn check(
    config: &Config,
    target: &agentenv::config::SudoTarget,
    args: &SudoArgs,
    json: bool,
) -> Result<Output, AppError> {
    match &target.transport {
        SudoTransport::Local => {
            let options = local_options(target, args)?;
            runtime()?.block_on(sudo::check_local(&options))?;
            let stdout = if json {
                serde_json::to_string(&serde_json::json!({
                    "status": "ready",
                    "transport": "local",
                    "helper_protocol": sudo::PROTOCOL_VERSION,
                    "sudo_authentication": "not-attempted"
                }))
                .expect("sudo check JSON is serializable")
                    + "\n"
            } else {
                "Local sudo prerequisites are ready; sudo authentication was not attempted.\n"
                    .to_owned()
            };
            Ok(Output::success(stdout, String::new()))
        }
        SudoTransport::Ssh(_) => remote(config, target, None, args, json),
    }
}

fn run(
    config: &Config,
    target: agentenv::config::SudoTarget,
    request: ExecutionRequest,
    args: &SudoArgs,
) -> Result<Output, AppError> {
    if !matches!(target.transport, SudoTransport::Local) {
        return remote(config, &target, Some(request), args, false);
    }
    // sudo runs in its own process group, so a target reading a controlling
    // terminal would be stopped by the OS instead of receiving input.
    if std::io::stdin().is_terminal() {
        return Err(AppError::Usage(
            "local sudo execution passes stdin to the command as data and cannot use a terminal; redirect stdin from a file, a pipe, or /dev/null".to_owned(),
        ));
    }
    let credential = config
        .credential(&target.credential.name)
        .ok_or_else(|| {
            AppError::NotFound(format!(
                "sudo credential '{}' is not defined",
                target.credential.name
            ))
        })?
        .clone();
    let options = local_options(&target, args)?;
    let executable = std::env::current_exe().map_err(|_| {
        AppError::SudoExecution(
            "resolver-unavailable: could not determine the agentenv executable path".to_owned(),
        )
    })?;
    let auth_timeout = options.auth_timeout;
    let runtime = runtime()?;
    let (cancel, cancellation) = sudo::cancellation_channel();
    super::signals::install_signal_forwarder(&runtime, cancel, signal_error)?;
    let outcome = runtime.block_on(sudo::execute_local(
        request,
        options,
        ProcessIo::inherit(),
        cancellation,
        || async {
            resolver::resolve(
                &executable,
                &credential,
                ResolutionStage::Sudo,
                SUDO_PASSWORD_LIMIT,
                auth_timeout,
            )
            .await
        },
    ))?;
    Ok(Output {
        stdout: String::new(),
        stderr: String::new(),
        status: outcome.shell_status(),
    })
}

fn remote(
    config: &Config,
    target: &agentenv::config::SudoTarget,
    request: Option<ExecutionRequest>,
    args: &SudoArgs,
    json: bool,
) -> Result<Output, AppError> {
    let runtime = runtime()?;
    let (cancel, cancellation) = sudo::cancellation_channel();
    super::signals::install_signal_forwarder(&runtime, cancel, signal_error)?;
    let result = runtime.block_on(sudo::client::execute(
        config,
        target,
        request,
        Duration::from_secs(args.connect_timeout_secs),
        Duration::from_secs(args.auth_timeout_secs),
        cancellation,
    ));
    runtime.shutdown_timeout(Duration::from_millis(100));
    let outcome = result?;
    if let Some(execution) = outcome.execution {
        let mut stderr = String::new();
        if outcome.close_failed {
            stderr.push_str("sudo-execution: ssh-close-failed: remote completion was observed, but SSH did not close cleanly\n");
        }
        if outcome.output_interrupted {
            stderr.push_str("sudo-execution: output-interrupted: remote completion was observed, but output delivery was cancelled\n");
        }
        if outcome.protocol_after_result {
            stderr.push_str("sudo-execution: remote-protocol-after-result: remote completion was observed, but the helper sent invalid data afterward\n");
        }
        return Ok(Output {
            stdout: String::new(),
            stderr,
            status: execution.shell_status(),
        });
    }
    let ready = outcome.ready.ok_or_else(|| {
        AppError::SudoExecution(
            "helper-handshake-missing: remote readiness was not observed".into(),
        )
    })?;
    let stdout = if json {
        serde_json::to_string(&serde_json::json!({
            "status": "ready", "transport": "ssh", "helper": ready,
            "sudo_authentication": "not-attempted"
        }))
        .expect("check output is serializable")
            + "\n"
    } else {
        format!(
            "SSH helper is ready as {} in {}; sudo authentication was not attempted.\n",
            plain_value(&ready.auth_user),
            plain_value(&ready.cwd)
        )
    };
    Ok(Output::success(stdout, String::new()))
}

fn signal_error() -> AppError {
    AppError::SudoExecution(
        "signal-unavailable: could not install sudo cancellation handlers".to_owned(),
    )
}

fn render_plan(
    profile: &Profile,
    entry: &str,
    target: &agentenv::config::SudoTarget,
    request: &ExecutionRequest,
    json: bool,
) -> Result<Output, AppError> {
    let (transport, endpoint, route_policy) = match &target.transport {
        SudoTransport::Local => ("local", "local".to_owned(), "local"),
        SudoTransport::Ssh(ssh) => {
            let (endpoint, route_policy) = match &ssh.connection {
                agentenv::config::SshConnection::Config { host_alias, .. } => (
                    format!("ssh-config:{host_alias} (unresolved)"),
                    "unverified",
                ),
                agentenv::config::SshConnection::Explicit {
                    hostname,
                    user,
                    port,
                } => (format!("{user}@{hostname}:{port}"), "explicit"),
            };
            ("ssh", endpoint, route_policy)
        }
    };
    let cwd = match (request.cwd(), &target.transport) {
        (Some(cwd), _) => Some(cwd.to_string_lossy().into_owned()),
        (None, SudoTransport::Local) => {
            let cwd = std::env::current_dir().map_err(|_| {
                AppError::Usage("could not determine the current working directory".to_owned())
            })?;
            Some(cwd.to_string_lossy().into_owned())
        }
        (None, SudoTransport::Ssh(_)) => None,
    };
    let cwd_text = cwd.as_deref().map_or_else(
        || "remote initial directory (unresolved)".to_owned(),
        plain_value,
    );
    let command: Vec<&str> = std::iter::once(
        request
            .executable()
            .to_str()
            .expect("validated request executable is UTF-8"),
    )
    .chain(request.arguments().iter().map(String::as_str))
    .collect();
    let command_text = format!(
        "[{}]",
        command
            .iter()
            .map(|argument| quoted(argument))
            .collect::<Vec<_>>()
            .join(", ")
    );
    let stdout = if json {
        serde_json::to_string(&serde_json::json!({
            "profile": profile.name,
            "target": entry,
            "transport": transport,
            "endpoint": endpoint,
            "route_policy": route_policy,
            "credential": target.credential.name,
            "auth_user": target.auth_user,
            "run_as": target.run_as,
            "cwd": cwd,
            "command": command,
        }))
        .expect("sudo plan JSON is serializable")
            + "\n"
    } else {
        format!(
            "Profile: {}\nTarget: {}\nTransport: {}\nEndpoint: {}\nRoute policy: {}\nCredential: {}\nAuthentication account: {}\nRun as: {}\nWorking directory: {}\nCommand: {}\n",
            profile.name,
            entry,
            transport,
            endpoint,
            route_policy,
            target.credential.name,
            target.auth_user,
            target.run_as,
            cwd_text,
            command_text
        )
    };
    Ok(Output::success(stdout, String::new()))
}

/// Renders a configuration or wire value on one plain-text line. Values with
/// control, bidirectional-formatting, or surrounding whitespace characters are
/// shown as an escaped quoted string so they cannot forge adjacent status lines
/// or reach the terminal as control sequences.
fn plain_value(value: &str) -> String {
    if value.is_empty() || value.trim() != value || value.chars().any(needs_escape) {
        quoted(value)
    } else {
        value.to_owned()
    }
}

/// A JSON-compatible quoted string that escapes every control and
/// bidirectional-formatting character, including DEL and C1 controls.
fn quoted(value: &str) -> String {
    let mut output = String::with_capacity(value.len() + 2);
    output.push('"');
    for character in value.chars() {
        match character {
            '"' => output.push_str("\\\""),
            '\\' => output.push_str("\\\\"),
            character if needs_escape(character) => {
                output.push_str(&format!("\\u{:04x}", u32::from(character)));
            }
            character => output.push(character),
        }
    }
    output.push('"');
    output
}

fn needs_escape(character: char) -> bool {
    character.is_control()
        || matches!(
            character,
            '\u{061c}' | '\u{200e}' | '\u{200f}' | '\u{202a}'..='\u{202e}' | '\u{2066}'..='\u{2069}'
        )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plain_plan_keeps_argument_boundaries_and_escapes_controls() {
        let directory = tempfile::tempdir().expect("config tempdir");
        let path = directory.path().join("agentenv.toml");
        std::fs::write(
            &path,
            r#"version = 1
default_profile = "work"
[profiles.work]
description = "Work."
[credentials.admin]
description = "Administrator password."
provider = "keychain"
service = "agentenv.test"
account = "admin"
usages = ["sudo"]
[profiles.work.admin]
description = "Local administrator."
kind = "sudo-target"
[profiles.work.admin.sudo]
transport = "local"
credential = "credential://admin"
auth_user = "operator"
run_as = "root"
sudo_path = "/usr/bin/sudo"
"#,
        )
        .expect("write config");
        let config = Config::load(Some(&path), &|_| None).expect("valid config");
        let profile = config.profile("work").expect("profile");
        let target = config.sudo_target(profile, "admin").expect("target");
        let arguments = [
            "",
            " spaced ",
            "line\nCredential: forged",
            "quote\"'",
            "\u{1b}[31m",
            "\u{9b}2J\u{7f}",
            "\u{202e}reversed",
        ];
        let request = ExecutionRequest::new(
            "/usr/bin/printf",
            arguments
                .iter()
                .map(|argument| (*argument).to_owned())
                .collect(),
            Some(PathBuf::from("/tmp/a\nRun as: nobody")),
        )
        .expect("valid request");

        let output = render_plan(profile, "admin", &target, &request, false).expect("plan");

        assert!(!output
            .stdout
            .chars()
            .any(|character| character != '\n' && needs_escape(character)));
        let lines: Vec<&str> = output.stdout.lines().collect();
        assert_eq!(lines.len(), 10, "{}", output.stdout);
        assert_eq!(lines[7], "Run as: root");
        assert_eq!(
            lines[8],
            r#"Working directory: "/tmp/a\u000aRun as: nobody""#
        );
        let command = lines[9].strip_prefix("Command: ").expect("command line");
        let parsed: Vec<String> = serde_json::from_str(command).expect("argv array");
        assert_eq!(parsed[0], "/usr/bin/printf");
        assert_eq!(parsed[1..], arguments);
    }
}
