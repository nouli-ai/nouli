//! Top-level command names; each subsystem owns its arguments and execution.
#[derive(Debug, clap::Subcommand)]
pub(crate) enum Command {
    /// Configure and authenticate a decision endpoint.
    Connect(crate::connect::Connect),
    #[command(flatten)]
    Inspect(crate::inspect::Command),
    /// Create a session, and run its first turn when facts or a question are given.
    New(crate::turns::New),
    /// Run a turn on an existing session: new facts and a question.
    Add(crate::turns::Add),
    /// Ask a question without new facts.
    Ask(crate::turns::Ask),
    /// Run one turn from a session's head and save nothing.
    Whatif(crate::turns::Whatif),
    /// Store, remove, and list provider credentials.
    Auth {
        #[command(subcommand)]
        command: crate::auth::Command,
    },
    /// Open the interactive session loop on a session.
    Open(crate::repl::Open),
    /// Execute, inspect, or explicitly resume a frozen decision workflow.
    #[command(name = "workflow")]
    Workflow {
        #[command(subcommand)]
        command: crate::workflows::Command,
    },
}
