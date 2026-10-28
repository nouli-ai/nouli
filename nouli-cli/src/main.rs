use std::io::{self, Write};
use std::process::ExitCode;

use clap::Parser;

mod settings;

/// A decision harness for structured callers.
#[derive(Parser)]
#[command(name = "nouli", version, disable_help_subcommand = true)]
struct Args {
    /// User configuration file (defaults to ~/.nouli/config.toml).
    #[arg(long, value_name = "FILE")]
    config: Option<std::path::PathBuf>,
    /// Relocate the user ledger root.
    #[arg(long, value_name = "DIR")]
    store: Option<std::path::PathBuf>,
}

fn main() -> ExitCode {
    match Args::try_parse() {
        Ok(args) => {
            if let Err(error) = settings::load(&args) {
                if writeln!(io::stderr().lock(), "{error}").is_err() {
                    return ExitCode::FAILURE;
                }
                return ExitCode::from(2);
            }
            match io::stdout()
                .lock()
                .write_all(b"Nouli workspace foundation. Use --help for available options.\n")
            {
                Ok(()) => ExitCode::SUCCESS,
                Err(_) => ExitCode::FAILURE,
            }
        }
        Err(error) => {
            let success = matches!(
                error.kind(),
                clap::error::ErrorKind::DisplayHelp | clap::error::ErrorKind::DisplayVersion
            );
            let written = if success {
                write!(io::stdout().lock(), "{error}")
            } else {
                write!(io::stderr().lock(), "{error}")
            };
            if success && written.is_ok() {
                ExitCode::SUCCESS
            } else {
                ExitCode::from(2)
            }
        }
    }
}
