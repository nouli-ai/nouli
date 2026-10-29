use std::io::{self, IsTerminal, Write};
use std::process::ExitCode;

use clap::{CommandFactory, Parser};

mod commands;
mod evaluate;
mod input;
mod inspect;
mod present;
mod render;
mod selection;
mod settings;
mod turns;
mod view;
mod workflows;

/// A decision harness for structured callers.
#[derive(Parser)]
#[command(name = "nouli", version, disable_help_subcommand = true, color=clap::ColorChoice::Never)]
struct Args {
    #[command(subcommand)]
    command: Option<commands::Command>,
    #[command(flatten)]
    output: render::OutputArgs,
    /// User configuration file (defaults to ~/.nouli/config.toml).
    #[arg(long, value_name = "FILE", global = true)]
    config: Option<std::path::PathBuf>,
    /// Credential env file (defaults to .env in the current directory).
    #[arg(long, value_name = "FILE", global = true)]
    env_file: Option<std::path::PathBuf>,
    /// Relocate the user ledger root.
    #[arg(long, value_name = "DIR", global = true)]
    store: Option<std::path::PathBuf>,
}

pub(crate) fn render_error(
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
            let Some(command) = &args.command else {
                // A bare invocation prints help. Settings flags are still
                // validated, and output flags without a command are a usage error.
                let outputs = args.output.json || args.output.jsonl || args.output.format.is_some();
                if args.config.is_some() || args.store.is_some() || outputs {
                    let settings = match settings::load(&args) {
                        Ok(settings) => settings,
                        Err(error) => {
                            return render_error(
                                &args,
                                nouli_core::config::OutputFormat::Auto,
                                settings::error(&args, error),
                            );
                        }
                    };
                    if outputs {
                        return render_error(
                            &args,
                            settings.config.output,
                            nouli_core::protocol::ErrorReport {
                                category: nouli_core::protocol::ErrorCategory::Input,
                                code: "command_required".into(),
                                message: "Choose a command; see --help".into(),
                                retryable: false,
                            },
                        );
                    }
                }
                return match Args::command().write_long_help(&mut io::stdout().lock()) {
                    Ok(()) => ExitCode::SUCCESS,
                    Err(_) => ExitCode::from(6),
                };
            };
            match command {
                commands::Command::New(verb) => {
                    return evaluate::command(&args, turns::Verb::New(verb));
                }
                commands::Command::Add(verb) => {
                    return evaluate::command(&args, turns::Verb::Add(verb));
                }
                commands::Command::Ask(verb) => {
                    return evaluate::command(&args, turns::Verb::Ask(verb));
                }
                commands::Command::Whatif(verb) => {
                    return evaluate::command(&args, turns::Verb::Whatif(verb));
                }
                commands::Command::Workflow { .. } | commands::Command::Inspect(_) => {}
            }
            let full = matches!(command,commands::Command::Workflow {command} if workflows::needs_config(command));
            let settings = match if full {
                settings::load(&args)
            } else {
                settings::load_inspection(&args)
            } {
                Ok(settings) => settings,
                Err(error) => {
                    return render_error(
                        &args,
                        nouli_core::config::OutputFormat::Auto,
                        settings::error(&args, error),
                    );
                }
            };
            let graph = match command {
                commands::Command::Inspect(inspect::Command::View(view)) => Some(view),
                _ => None,
            };
            let format = match if let Some(view) = graph {
                view::format(&args.output, view.mermaid)
            } else {
                render::resolve(
                    &args.output,
                    settings.config.output,
                    io::stdout().is_terminal(),
                )
            } {
                Ok(format) => format,
                Err(_) if graph.is_some_and(|view| view.mermaid) => {
                    return render_error(
                        &args,
                        settings.config.output,
                        inspect::input_error(
                            "--mermaid cannot be combined with output format options",
                        ),
                    );
                }
                Err(_) if graph.is_some() => {
                    return render_error(
                        &args,
                        settings.config.output,
                        inspect::input_error(
                            "Session graphs support text only. Use --mermaid for a Mermaid diagram, or nouli sessions export for JSON or TOON",
                        ),
                    );
                }
                Err(_) => return ExitCode::from(2),
            };
            if let commands::Command::Workflow { command } = command {
                return ExitCode::from(workflows::run(command, &settings, format));
            }
            let commands::Command::Inspect(command) = command else {
                return ExitCode::from(6);
            };
            if format == render::Format::Jsonl {
                return render_error(
                    &args,
                    settings.config.output,
                    inspect::input_error("Inspection supports text, JSON, or TOON"),
                );
            }
            match inspect::run(
                command,
                &settings.store,
                &mut io::stdout().lock(),
                format,
                render::color_enabled(io::stdout().is_terminal()),
            ) {
                Ok(()) => ExitCode::SUCCESS,
                Err(error) if error.output_failed() => ExitCode::from(6),
                Err(error) => render_error(&args, settings.config.output, error.report()),
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
