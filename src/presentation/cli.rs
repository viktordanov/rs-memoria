//! Command-line grammar.

use clap::{Args, Parser, Subcommand, ValueEnum};

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum Format {
    Human,
    Json,
}

#[derive(Debug, Parser)]
#[command(name = "memoria", version, about = "Keep a project's documented mental model connected to its code.", long_about = None)]
pub struct Cli {
    /// Project root. Must be the Git worktree root. Defaults to discovery from the current directory.
    #[arg(long, global = true, value_name = "DIRECTORY")]
    pub root: Option<String>,
    /// Output format.
    #[arg(long, global = true, value_enum, default_value_t = Format::Human)]
    pub format: Format,
    #[command(subcommand)]
    pub command: Command,
}

#[derive(Debug, Subcommand)]
pub enum Command {
    /// Create root configuration, a minimal root README, and empty state.
    Init,
    /// Show coverage, input size, and review state.
    Status {
        /// Explain why one project-relative path is selected or excluded.
        #[arg(long, value_name = "PATH")]
        explain: Option<String>,
    },
    /// Check structure, configuration, markers, and link hints.
    Lint,
    /// Show the ordered review plan, or a focused packet for one README.
    Review {
        /// Project-relative README path for a focused packet.
        document: Option<String>,
        /// Raw input budget in bytes for the focused packet (default 8 MiB, at most 32 MiB).
        #[arg(long, value_name = "BYTES")]
        max_bytes: Option<u64>,
    },
    /// Refresh declared import blocks only.
    Render {
        /// Project-relative README path. Defaults to every README.
        document: Option<String>,
        /// Show planned changes without writing.
        #[arg(long)]
        dry_run: bool,
    },
    /// Record a review result against the exact packet snapshot.
    Ack(AckArgs),
    /// Mark one README, a subtree, or the whole project for semantic review.
    Invalidate {
        /// `all`, `doc:<README.md>`, or `subtree:<directory>`.
        scope: String,
        /// Why the documentation needs review (at least three words).
        #[arg(long, value_name = "TEXT")]
        reason: String,
    },
    /// Run read-only validation for CI.
    Check,
    /// Show documentation ownership, imports, navigation, and status.
    Graph,
    /// Install or remove the managed Memoria skill for an agent.
    Agent {
        #[command(subcommand)]
        command: AgentCommand,
    },
}

#[derive(Debug, Args)]
pub struct AckArgs {
    /// Project-relative README path.
    pub document: String,
    /// Packet file from `memoria review <README.md> --format json`, or `-` for stdin.
    #[arg(long, value_name = "FILE|-")]
    pub packet: String,
    /// The 21-byte token from the packet (`data.token`).
    #[arg(long, value_name = "TOKEN")]
    pub token: String,
    /// Who reviewed (1–128 characters).
    #[arg(long, value_name = "NAME")]
    pub reviewer: String,
    /// `updated` or `no-update`.
    #[arg(long, value_name = "RESULT")]
    pub result: String,
    /// Why the documentation is correct now (12–1000 characters, at least three words).
    #[arg(long, value_name = "TEXT")]
    pub note: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum Target {
    Codex,
    Claude,
}

#[derive(Debug, Args)]
pub struct AgentArgs {
    /// Agent to install for. Detected when exactly one of .agents or .claude exists.
    #[arg(long, value_enum)]
    pub target: Option<Target>,
    /// Custom skills parent directory (requires --target). Resolved from the current directory.
    #[arg(long, value_name = "DIRECTORY")]
    pub path: Option<String>,
    /// Show the plan without changing files.
    #[arg(long)]
    pub dry_run: bool,
}

#[derive(Debug, Subcommand)]
pub enum AgentCommand {
    /// Install the managed skill package.
    Install(AgentArgs),
    /// Remove the managed skill package and restore replaced content when safe.
    Uninstall(AgentArgs),
}

impl Command {
    pub fn name(&self) -> &'static str {
        match self {
            Command::Init => "init",
            Command::Status { .. } => "status",
            Command::Lint => "lint",
            Command::Review { .. } => "review",
            Command::Render { .. } => "render",
            Command::Ack(_) => "ack",
            Command::Invalidate { .. } => "invalidate",
            Command::Check => "check",
            Command::Graph => "graph",
            Command::Agent {
                command: AgentCommand::Install(_),
            } => "agent install",
            Command::Agent {
                command: AgentCommand::Uninstall(_),
            } => "agent uninstall",
        }
    }
}
