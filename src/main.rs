mod cli;

use std::io::{self, Write};
use std::process;

use agentenv::error::AppError;
use clap::{error::ErrorKind, CommandFactory, Parser};

use crate::cli::commands::{execute, Command, Invocation};

#[derive(Debug, Parser)]
#[command(
    name = "agentenv",
    version,
    about = "Inspect agent context without exposing credentials"
)]
struct Cli {
    #[arg(long, global = true, value_name = "NAME")]
    profile: Option<String>,

    #[arg(long, global = true)]
    json: bool,

    #[command(subcommand)]
    command: Option<Command>,
}

fn main() {
    let cli = parse_cli();
    let Some(command) = cli.command else {
        print_help_and_exit();
    };
    match execute(Invocation {
        profile: cli.profile,
        json: cli.json,
        command,
    }) {
        Ok(output) => {
            if let Err(error) = write_outputs(&output.stdout, &output.stderr) {
                let _ = writeln!(io::stderr(), "failed to write command output: {error}");
                process::exit(1);
            }
        }
        Err(error) => {
            let _ = write_error(&error);
            process::exit(error.exit_code());
        }
    }
}

fn parse_cli() -> Cli {
    match Cli::try_parse() {
        Ok(cli) => cli,
        Err(error) => {
            let is_display_request = matches!(
                error.kind(),
                ErrorKind::DisplayHelp | ErrorKind::DisplayVersion
            );
            let is_unknown_subcommand = matches!(error.kind(), ErrorKind::InvalidSubcommand);
            let mut output = error.to_string();
            if is_unknown_subcommand {
                output.push('\n');
                output.push_str(&Cli::command().render_help().to_string());
            }
            let result = if is_display_request {
                io::stdout().write_all(output.as_bytes())
            } else {
                io::stderr().write_all(output.as_bytes())
            };
            if let Err(write_error) = result {
                let _ = writeln!(
                    io::stderr(),
                    "failed to write CLI diagnostic: {write_error}"
                );
            }
            process::exit(if is_display_request { 0 } else { 1 });
        }
    }
}

fn write_outputs(stdout: &str, stderr: &str) -> io::Result<()> {
    io::stdout().write_all(stdout.as_bytes())?;
    io::stderr().write_all(stderr.as_bytes())
}

fn write_error(error: &AppError) -> io::Result<()> {
    let mut stderr = io::stderr();
    match error {
        AppError::Config(violations) => {
            let env = |name: &str| std::env::var(name).ok();
            if let Ok(path) = agentenv::config::locate::resolve_path(None, &env) {
                writeln!(stderr, "configuration file: {}", path.display())?;
            }
            for violation in violations {
                writeln!(stderr, "configuration error: {violation}")?;
            }
            Ok(())
        }
        _ => writeln!(stderr, "{error}"),
    }
}

fn print_help_and_exit() -> ! {
    let mut command = Cli::command();
    if let Err(error) = command.print_help() {
        let _ = writeln!(io::stderr(), "failed to write help: {error}");
    }
    let _ = writeln!(io::stdout());
    process::exit(AppError::Usage("a subcommand is required".to_owned()).exit_code());
}
