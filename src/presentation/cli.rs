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
    /// Print a shell completion script without project discovery or installation.
    Completions { shell: CompletionShell },
    /// Explain one README's whole-file freshness with verified local Git evidence.
    Explain { document: String },
    /// Validate root setup inputs, or create missing configuration and state with --apply.
    Init {
        /// Create the missing memoria.toml and memoria.lock files.
        #[arg(long)]
        apply: bool,
    },
    /// Show coverage, input size, and review state.
    Status {
        /// Explain why one project-relative path is selected or excluded.
        #[arg(long, value_name = "PATH")]
        explain: Option<String>,
        /// Emit bounded counts only. Cannot be combined with --explain.
        #[arg(long)]
        summary: bool,
    },
    /// Show the project documentation guidance that applies to a README.
    Guidance {
        /// Project-relative README path. Defaults to the root README.
        document: Option<String>,
    },
    /// Inspect or compare committed state without changing it.
    State {
        #[command(subcommand)]
        command: StateCommand,
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
    /// Install or remove the managed Memoria skill and hooks for an agent.
    Agent {
        #[command(subcommand)]
        command: AgentCommand,
    },
}

#[derive(Debug, Subcommand)]
pub enum StateCommand {
    /// Compare two explicit lock snapshots. Read-only; does not establish freshness.
    Diff { before: String, after: String },
    /// Decode and print committed state. Read-only.
    Inspect {
        /// Explicit state file, resolved from the current directory.
        /// Works outside a Git worktree and needs no configuration.
        #[arg(long, value_name = "PATH")]
        file: Option<String>,
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
    /// Who reviewed (1–128 characters). Overrides the opt-in MEMORIA_REVIEWER environment default.
    #[arg(long, value_name = "NAME")]
    pub reviewer: Option<String>,
    /// `updated` or `no-update`.
    #[arg(long, value_name = "RESULT")]
    pub result: String,
    /// Why this README is correct for this packet. After trim: 12–1000 Unicode characters,
    /// at least three words; CR/LF allowed, tabs and other controls forbidden. Generic notes are rejected.
    #[arg(long, value_name = "TEXT")]
    pub note: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum Target {
    Codex,
    Claude,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum Scope {
    /// Inside the selected Git worktree.
    Local,
    /// The user's home skills directory.
    Global,
}

#[derive(Debug, Args)]
pub struct AgentArgs {
    /// Agent to install for. Detected when exactly one of .agents or .claude exists.
    #[arg(long, value_enum)]
    pub target: Option<Target>,
    /// Installation scope. Global requires an explicit --target.
    #[arg(long, value_enum, default_value_t = Scope::Local)]
    pub scope: Scope,
    /// Custom skills parent directory (requires --target).
    #[arg(long, value_name = "DIRECTORY")]
    pub path: Option<String>,
    /// Show the plan without changing files.
    #[arg(long)]
    pub dry_run: bool,
    /// Replace an existing unmanaged package after a verified backup.
    #[arg(long)]
    pub replace_existing: bool,
}

#[derive(Debug, Subcommand)]
pub enum AgentCommand {
    /// Install the managed skill package.
    Install(AgentArgs),
    /// Report the installed package without changing it.
    Status(AgentArgs),
    /// Replace an older managed package with the embedded one.
    Upgrade(AgentArgs),
    /// Remove the managed skill package and restore replaced content when safe.
    Uninstall(AgentArgs),
    /// Install, inspect, or remove the project-level Stop hook.
    Hook {
        #[command(subcommand)]
        command: HookCommand,
    },
}

#[derive(Debug, Args)]
pub struct HookArgs {
    /// Agent whose native hook configuration to change.
    #[arg(long, value_enum)]
    pub target: Target,
    /// Show the plan without changing files.
    #[arg(long)]
    pub dry_run: bool,
}

#[derive(Debug, Subcommand)]
pub enum HookCommand {
    /// Add one owned Stop hook to the project client configuration.
    Install(HookArgs),
    /// Report the owned Stop hook and its activation state.
    Status {
        /// Agent whose native hook configuration to inspect.
        #[arg(long, value_enum)]
        target: Target,
    },
    /// Remove the owned Stop hook and any container Memoria created.
    Uninstall(HookArgs),
    /// The documented native hook endpoint. Reads event JSON on stdin.
    Run {
        /// Agent that invoked the hook.
        #[arg(long, value_enum)]
        target: Target,
        /// Native hook protocol version.
        #[arg(long, value_name = "VERSION")]
        protocol: u32,
        /// The canonical project root bound at installation.
        #[arg(long, value_name = "DIRECTORY")]
        configuration_root: String,
    },
}

impl Command {
    pub fn name(&self) -> &'static str {
        match self {
            Command::Completions { .. } => "completions",
            Command::Explain { .. } => "explain",
            Command::State {
                command: StateCommand::Diff { .. },
            } => "state diff",
            Command::Init { .. } => "init",
            Command::Status { .. } => "status",
            Command::Lint => "lint",
            Command::Review { .. } => "review",
            Command::Render { .. } => "render",
            Command::Ack(_) => "ack",
            Command::Invalidate { .. } => "invalidate",
            Command::Check => "check",
            Command::Graph => "graph",
            Command::Guidance { .. } => "guidance",
            Command::State {
                command: StateCommand::Inspect { .. },
            } => "state inspect",
            Command::Agent { command } => match command {
                AgentCommand::Install(_) => "agent install",
                AgentCommand::Status(_) => "agent status",
                AgentCommand::Upgrade(_) => "agent upgrade",
                AgentCommand::Uninstall(_) => "agent uninstall",
                AgentCommand::Hook { command } => match command {
                    HookCommand::Install(_) => "agent hook install",
                    HookCommand::Status { .. } => "agent hook status",
                    HookCommand::Uninstall(_) => "agent hook uninstall",
                    HookCommand::Run { .. } => "agent hook run",
                },
            },
        }
    }
}

#[derive(Debug, Clone, Copy, ValueEnum)]
pub enum CompletionShell {
    Bash,
    Zsh,
    Fish,
}

pub fn resolve_reviewer(
    explicit: Option<&str>,
    environment: Option<std::ffi::OsString>,
) -> Result<String, memoria_application::error::AppError> {
    use memoria_application::error::AppError;
    if let Some(label) = explicit {
        return Ok(label.to_string());
    }
    let label = environment
        .map(|value| {
            value.into_string().map_err(|_| {
                AppError::usage("reviewer_invalid", "MEMORIA_REVIEWER must be valid UTF-8")
            })
        })
        .transpose()?;
    label
        .filter(|value| !value.trim().is_empty())
        .ok_or_else(|| {
            AppError::usage(
                "reviewer_required",
                "Supply --reviewer NAME or set MEMORIA_REVIEWER to an explicit reviewer label.",
            )
        })
}
