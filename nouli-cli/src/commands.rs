//! Top-level command names; each subsystem owns its arguments and execution.
#[derive(Debug, clap::Subcommand)]
pub(crate) enum Command {
    #[command(flatten)]
    Inspect(crate::inspect::Command),
    /// Execute, inspect, or explicitly resume a frozen decision workflow.
    Workflows {
        #[command(subcommand)]
        command: crate::workflows::Command,
    },
}
