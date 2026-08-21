use std::io::{self, Write};
use std::process;

use agent_context::error::AppError;
use clap::{error::ErrorKind, CommandFactory, Parser, Subcommand};

#[derive(Debug, Parser)]
#[command(
    name = "agent-context",
    version,
    about = "Inspect and run commands with agent context"
)]
struct Cli {
    #[arg(long, global = true, value_name = "NAME")]
    profile: Option<String>,

    #[arg(long, global = true)]
    json: bool,

    #[command(subcommand)]
    command: Option<Command>,
}

#[derive(Debug, Subcommand)]
enum Command {
    Version,
}

fn main() {
    let cli = parse_cli();

    match cli.command {
        Some(Command::Version) => println!("agent-context {}", env!("CARGO_PKG_VERSION")),
        None => print_help_and_exit(),
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

            let write_result = if is_display_request {
                io::stdout().write_all(output.as_bytes())
            } else {
                io::stderr().write_all(output.as_bytes())
            };

            if let Err(write_error) = write_result {
                let _ = writeln!(
                    io::stderr(),
                    "failed to write CLI diagnostic: {write_error}"
                );
                process::exit(1);
            }

            process::exit(if is_display_request { 0 } else { 1 });
        }
    }
}

fn print_help_and_exit() -> ! {
    let mut command = Cli::command();
    if let Err(error) = command.print_help() {
        let _ = writeln!(io::stderr(), "failed to write help: {error}");
        process::exit(AppError::Usage("a subcommand is required".to_owned()).exit_code());
    }

    if let Err(error) = writeln!(io::stdout()) {
        let _ = writeln!(io::stderr(), "failed to write help: {error}");
        process::exit(AppError::Usage("a subcommand is required".to_owned()).exit_code());
    }

    process::exit(AppError::Usage("a subcommand is required".to_owned()).exit_code());
}
