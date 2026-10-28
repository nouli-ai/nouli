use std::io::{self, IsTerminal, Write};
use std::process::ExitCode;

use clap::{CommandFactory, Parser};

mod commands;
mod evaluate;
mod input;
mod inspect;
mod render;
mod selection;
mod settings;
mod workflows;

/// A decision harness for structured callers.
#[derive(Parser)]
#[command(name = "nouli", version, disable_help_subcommand = true, color=clap::ColorChoice::Never)]
struct Args {
    #[command(subcommand)]
    command: Option<commands::Command>,
    #[command(flatten)]
    output: render::OutputArgs,
    #[command(flatten)]
    decision: input::DecisionArgs,
    /// User configuration file (defaults to ~/.nouli/config.toml).
    #[arg(long, value_name = "FILE", global = true)]
    config: Option<std::path::PathBuf>,
    /// Relocate the user ledger root.
    #[arg(long, value_name = "DIR", global = true)]
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
            if let Some(command) = &args.command {
                if args.decision.has_options() {
                    return render_error(
                        &args,
                        nouli_core::config::OutputFormat::Auto,
                        inspect::input_error(
                            "Use command options instead of root decision inputs or model overrides",
                        ),
                    );
                }
                let full = matches!(command,commands::Command::Workflows {command} if workflows::needs_config(command));
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
                            nouli_core::engine::EngineError::Config(error).report(),
                        );
                    }
                };
                let format = match render::resolve(
                    &args.output,
                    settings.config.output,
                    io::stdout().is_terminal(),
                ) {
                    Ok(format) => format,
                    Err(_) => return ExitCode::from(2),
                };
                match command {
                    commands::Command::Workflows { command } => {
                        return ExitCode::from(workflows::run(command, &settings, format));
                    }
                    commands::Command::Inspect(command) => {
                        if format == render::Format::Jsonl {
                            return render_error(
                                &args,
                                settings.config.output,
                                inspect::input_error("Inspection supports text, JSON, or TOON"),
                            );
                        }
                        return match inspect::run(
                            command,
                            &settings.store,
                            &mut io::stdout().lock(),
                            format,
                        ) {
                            Ok(()) => ExitCode::SUCCESS,
                            Err(error) if error.output_failed() => ExitCode::from(6),
                            Err(error) => {
                                render_error(&args, settings.config.output, error.report())
                            }
                        };
                    }
                }
            }
            if !args.decision.has_options()
                && args.config.is_none()
                && args.store.is_none()
                && !args.output.json
                && !args.output.jsonl
                && args.output.format.is_none()
            {
                return match Args::command().write_long_help(&mut io::stdout().lock()) {
                    Ok(()) => ExitCode::SUCCESS,
                    Err(_) => ExitCode::from(6),
                };
            }
            let settings = match settings::load(&args) {
                Ok(settings) => settings,
                Err(error) => {
                    return render_error(
                        &args,
                        nouli_core::config::OutputFormat::Auto,
                        nouli_core::engine::EngineError::Config(error).report(),
                    );
                }
            };
            if !args.decision.has_input() {
                if args.decision.has_options()
                    || args.output.json
                    || args.output.jsonl
                    || args.output.format.is_some()
                {
                    return render_error(
                        &args,
                        settings.config.output,
                        evaluate::input_error(&input::InputError::Missing),
                    );
                }
                return match Args::command().write_long_help(&mut io::stdout().lock()) {
                    Ok(()) => ExitCode::SUCCESS,
                    Err(_) => ExitCode::from(6),
                };
            }
            let request = match args.decision.request(&mut io::stdin().lock()) {
                Ok(request) => request,
                Err(error) => {
                    return render_error(
                        &args,
                        settings.config.output,
                        evaluate::input_error(&error),
                    );
                }
            };
            let format = match render::resolve(
                &args.output,
                settings.config.output,
                io::stdout().is_terminal(),
            ) {
                Ok(format) => format,
                Err(_) => return ExitCode::from(2),
            };
            let runtime = match tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
            {
                Ok(runtime) => runtime,
                Err(_) => return ExitCode::from(6),
            };
            let result = runtime.block_on(evaluate::run(&args.decision, &request, &settings));
            match render::result(
                &mut io::stdout().lock(),
                &mut io::stderr().lock(),
                format,
                &result,
            ) {
                Ok(code) => ExitCode::from(code),
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
