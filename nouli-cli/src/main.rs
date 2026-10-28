use std::io::{self, IsTerminal, Write};
use std::process::ExitCode;

use clap::{CommandFactory, Parser};

mod render;
mod settings;

/// A decision harness for structured callers.
#[derive(Parser)]
#[command(name = "nouli", version, disable_help_subcommand = true, color=clap::ColorChoice::Never)]
struct Args {
    #[command(flatten)]
    output: render::OutputArgs,
    /// User configuration file (defaults to ~/.nouli/config.toml).
    #[arg(long, value_name = "FILE")]
    config: Option<std::path::PathBuf>,
    /// Relocate the user ledger root.
    #[arg(long, value_name = "DIR")]
    store: Option<std::path::PathBuf>,
}

fn render_error(
    args: &Args,
    configured: nouli_core::config::OutputFormat,
    error: nouli_core::protocol::ErrorReport,
) -> ExitCode {
    let format = match render::resolve(&args.output, configured, io::stdout().is_terminal()) {
        Ok(format) => format,
        Err(_) => return ExitCode::from(2),
    };
    let report =
        nouli_core::protocol::ResultEnvelope::new(nouli_core::protocol::Report::Error(error));
    match render::result(
        &mut io::stdout().lock(),
        &mut io::stderr().lock(),
        format,
        &report,
    ) {
        Ok(code) => ExitCode::from(code),
        Err(_) => ExitCode::from(6),
    }
}

fn main() -> ExitCode {
    match Args::try_parse() {
        Ok(args) => {
            let configured = match settings::load(&args) {
                Ok((config, _)) => config.output,
                Err(error) => {
                    return render_error(
                        &args,
                        nouli_core::config::OutputFormat::Auto,
                        nouli_core::engine::EngineError::Config(error).report(),
                    );
                }
            };
            if args.output.json || args.output.jsonl || args.output.format.is_some() {
                return render_error(
                    &args,
                    configured,
                    nouli_core::protocol::ErrorReport {
                        category: nouli_core::protocol::ErrorCategory::Input,
                        code: "decision_input_required".into(),
                        message: "Supply decision input; see --help for supported operations"
                            .into(),
                        retryable: false,
                    },
                );
            }
            match Args::command().write_long_help(&mut io::stdout().lock()) {
                Ok(()) => ExitCode::SUCCESS,
                Err(_) => ExitCode::from(6),
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
            match written {
                Err(_) => ExitCode::from(6),
                Ok(()) if success => ExitCode::SUCCESS,
                Ok(()) => ExitCode::from(2),
            }
        }
    }
}
