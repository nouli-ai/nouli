use std::io::{self, IsTerminal, Write};
use std::process::ExitCode;

use clap::{CommandFactory, Parser};

mod auth;
mod commands;
mod connect;
mod evaluate;
mod input;
mod inspect;
mod palette;
mod present;
mod render;
mod repl;
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
    let Ok(format) = render::resolve(&args.output, configured, io::stdout().is_terminal()) else {
        return ExitCode::from(2);
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
    if let Some(code) = repl::tui::attachments::maybe_worker() {
        return code;
    }
    match Args::try_parse() {
        Ok(args) => run(&args),
        Err(error) => report_parse_error(&error),
    }
}

/// Dispatches a parsed invocation to the subsystem that owns its command.
fn run(args: &Args) -> ExitCode {
    let Some(command) = &args.command else {
        return run_without_command(args);
    };
    match command {
        commands::Command::Connect(options) => connect::run(args, options),
        commands::Command::New(verb) => evaluate::command(args, turns::Verb::New(verb)),
        commands::Command::Add(verb) => evaluate::command(args, turns::Verb::Add(verb)),
        commands::Command::Ask(verb) => evaluate::command(args, turns::Verb::Ask(verb)),
        commands::Command::Whatif(verb) => evaluate::command(args, turns::Verb::Whatif(verb)),
        commands::Command::Auth { command } => auth::run(args, command),
        commands::Command::Open(open) => {
            repl::run(args, repl::Target::Named(open.session.clone()), Some(open))
        }
        commands::Command::Workflow { command } => run_workflow(args, command),
        commands::Command::Inspect(command) => run_inspect(args, command),
    }
}

/// A bare invocation opens the session picker on a terminal and prints help
/// otherwise. Settings flags are still validated, and output flags without a
/// command are a usage error.
fn run_without_command(args: &Args) -> ExitCode {
    let outputs = args.output.json || args.output.jsonl || args.output.format.is_some();
    if args.config.is_some() || args.store.is_some() || outputs {
        let settings = match load_settings(args, settings::load) {
            Ok(settings) => settings,
            Err(code) => return code,
        };
        if outputs {
            return render_error(
                args,
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
    if repl::can_draw() {
        return repl::run(args, repl::Target::Pick, None);
    }
    match Args::command().write_long_help(&mut io::stdout().lock()) {
        Ok(()) => ExitCode::SUCCESS,
        Err(_) => ExitCode::from(6),
    }
}

/// Loads settings with `load`, rendering a load failure as an error report.
fn load_settings(
    args: &Args,
    load: fn(&Args) -> Result<settings::Settings, nouli_core::config::ConfigError>,
) -> Result<settings::Settings, ExitCode> {
    load(args).map_err(|error| {
        render_error(
            args,
            nouli_core::config::OutputFormat::Auto,
            settings::error(args, error),
        )
    })
}

fn run_workflow(args: &Args, command: &workflows::Command) -> ExitCode {
    let load = if workflows::needs_config(command) {
        settings::load
    } else {
        settings::load_inspection
    };
    let settings = match load_settings(args, load) {
        Ok(settings) => settings,
        Err(code) => return code,
    };
    match render::resolve(
        &args.output,
        settings.config.output,
        io::stdout().is_terminal(),
    ) {
        Ok(format) => ExitCode::from(workflows::run(command, &settings, format)),
        Err(_) => ExitCode::from(2),
    }
}

fn run_inspect(args: &Args, command: &inspect::Command) -> ExitCode {
    let settings = match load_settings(args, settings::load_inspection) {
        Ok(settings) => settings,
        Err(code) => return code,
    };
    let format = match inspection_format(args, &settings, command) {
        Ok(format) => format,
        Err(code) => return code,
    };
    if format == render::Format::Jsonl {
        return render_error(
            args,
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
        Err(error) => render_error(args, settings.config.output, error.report()),
    }
}

/// Resolves the output format for an inspection command. Session graphs have
/// their own format rules and report incompatible output options as input errors.
fn inspection_format(
    args: &Args,
    settings: &settings::Settings,
    command: &inspect::Command,
) -> Result<render::Format, ExitCode> {
    let inspect::Command::View(view) = command else {
        return render::resolve(
            &args.output,
            settings.config.output,
            io::stdout().is_terminal(),
        )
        .map_err(|_| ExitCode::from(2));
    };
    view::format(&args.output, view.mermaid).map_err(|_| {
        let message = if view.mermaid {
            "--mermaid cannot be combined with output format options"
        } else {
            "Session graphs support text only. Use --mermaid for a Mermaid diagram, or nouli sessions export for JSON or TOON"
        };
        render_error(args, settings.config.output, inspect::input_error(message))
    })
}

/// Writes clap's help, version, or usage error and maps it to an exit code.
fn report_parse_error(error: &clap::Error) -> ExitCode {
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
