//! Composition root for the `memoria` binary.

mod presentation;

use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use clap::Parser as _;
use memoria_application::error::{AppError, Detail, Diagnostic, Outcome};
use memoria_application::ports::{
    AgentScope, AgentTarget, PacketSource, Progress, Services, SkillOperation, WorkflowOperation,
};
use memoria_application::usecases;
use memoria_infrastructure::config::TomlConfigurationReader;
use memoria_infrastructure::{
    AtomicFileWriter, CommandClientProbe, EnvAgentLocations, FsHookStore, FsPacketInput,
    FsProjectFiles, FsSkillStore, FsWorkflowStore, GitCli, GixRepositoryIgnore, JsonPacketCodec,
    LockFileCoordinator, LockStateInspector, LockStateStore, PulldownMarkdownCodec,
    SelfStatusProcess, SystemClock, Xxh3Hasher,
};
use presentation::cli::{
    AgentCommand, Cli, Command, Format, GithubCommand, HookCommand, IntegrationsCommand, Scope,
    StateCommand, Target,
};
use presentation::{json, text};

/// The canonical agent skill, embedded at build time.
pub const SKILL: &str = include_str!("../skills/memoria/SKILL.md");

struct StderrProgress;

impl Progress for StderrProgress {
    fn note(&self, message: &str) {
        eprintln!("memoria: {message}");
    }
}

struct CommandOutput {
    data: Detail,
    human: String,
    diagnostics: Vec<Diagnostic>,
    /// Pre-encoded JSON for focused review packets.
    raw_json: Option<Vec<u8>>,
}

fn output<T>(outcome: Outcome<T>, data: Detail, human: String) -> CommandOutput {
    CommandOutput {
        data,
        human,
        diagnostics: outcome.diagnostics,
        raw_json: None,
    }
}

fn discover_root(requested: Option<&str>) -> Result<GitCli, AppError> {
    let start: PathBuf = match requested {
        Some(dir) => PathBuf::from(dir),
        None => std::env::current_dir().map_err(|e| {
            AppError::io(
                "io_error",
                format!("cannot read the current directory: {e}"),
            )
        })?,
    };
    let git =
        GitCli::discover(&start).map_err(|e| AppError::io("git_unavailable", e.to_string()))?;
    if let Some(dir) = requested {
        let canonical = Path::new(dir)
            .canonicalize()
            .map_err(|e| AppError::io("io_error", format!("--root {dir}: {e}")))?;
        let root = git
            .root()
            .canonicalize()
            .map_err(|e| AppError::io("io_error", e.to_string()))?;
        if canonical != root {
            return Err(AppError::usage(
                "root_mismatch",
                format!(
                    "--root {dir} is not the Git worktree root ({})",
                    root.display()
                ),
            ));
        }
    }
    Ok(git)
}

fn run(cli: &Cli) -> Result<CommandOutput, AppError> {
    if let Command::Completions { shell } = &cli.command {
        use clap::CommandFactory as _;
        use presentation::cli::CompletionShell;
        let shell = match shell {
            CompletionShell::Bash => clap_complete::Shell::Bash,
            CompletionShell::Zsh => clap_complete::Shell::Zsh,
            CompletionShell::Fish => clap_complete::Shell::Fish,
        };
        let mut script = Vec::new();
        clap_complete::generate(shell, &mut Cli::command(), "memoria", &mut script);
        let human = String::from_utf8(script).expect("completion generator emits UTF-8");
        return Ok(CommandOutput {
            data: Detail::map()
                .text("shell", shell.to_string())
                .text("script", human.clone())
                .build(),
            human,
            diagnostics: vec![],
            raw_json: None,
        });
    }
    if let Command::Packet {
        command:
            presentation::cli::PacketCommand::View {
                packet,
                section,
                file,
                trust_prior_review,
            },
    } = &cli.command
    {
        let hasher = Xxh3Hasher;
        let codec = JsonPacketCodec::new(&hasher);
        let source = if packet == "-" {
            PacketSource::Stdin
        } else {
            PacketSource::File(packet.clone())
        };
        let data = usecases::packet_view::run(
            &FsPacketInput,
            &codec,
            &hasher,
            &source,
            section,
            file.as_deref(),
            *trust_prior_review,
        )?;
        let mut human = "Saved packet reading view (not an acknowledgement packet)\n".to_string();
        presentation::human::detail(&mut human, &data, 0);
        return bounded_output(
            cli,
            vec![],
            data,
            human,
            memoria_application::packet::MAX_SERIALIZED_BYTES,
            "packet_view_limit_exceeded",
            false,
        );
    }
    if let Command::State {
        command: StateCommand::Diff { before, after },
    } = &cli.command
    {
        let inspector = LockStateInspector::new(None, invocation_dir());
        let outcome = usecases::state_diff::run_files(&inspector, before, after)?;
        let data = outcome.data.to_detail();
        let human = text::state_diff(&outcome.data);
        return bounded_output(
            cli,
            outcome.diagnostics,
            data,
            human,
            usecases::state_diff::MAX_RENDERED_BYTES,
            "state_comparison_limit_exceeded",
            true,
        );
    }
    let git = discover_root(cli.root.as_deref())?;
    let root = git.root().to_path_buf();
    let files = FsProjectFiles::new(root.clone());
    let config = TomlConfigurationReader;
    let markdown = PulldownMarkdownCodec;
    let hasher = Xxh3Hasher;
    let ignore = GixRepositoryIgnore;
    let state = LockStateStore::new(root.clone());
    let invocation = std::env::current_dir().unwrap_or_else(|_| root.clone());
    let inspector = LockStateInspector::new(Some(root.clone()), invocation);
    let clock = SystemClock;
    // The committed state never acts as the process lock: the write lock
    // lives on a worktree-private path under Git metadata. The metadata
    // directory is also the containment boundary for that path.
    let git_dir = PathBuf::from(
        memoria_application::ports::GitRepository::git_dir(&git)
            .map_err(|e| AppError::io("git_unavailable", e.to_string()))?,
    );
    let lock_path = PathBuf::from(
        memoria_application::ports::GitRepository::private_path(&git, "memoria/write.lock")
            .map_err(|e| AppError::io("git_unavailable", e.to_string()))?,
    );
    let locks = LockFileCoordinator::with_boundary(root.clone(), lock_path, git_dir.clone());
    let writer = AtomicFileWriter::new(root.clone());
    let packets = JsonPacketCodec::new(&hasher);
    let packet_input = FsPacketInput;
    let skills = FsSkillStore::new(root.clone(), SKILL, env!("CARGO_PKG_VERSION"));
    let locations = EnvAgentLocations::from_environment(Some(root.clone()), invocation_dir());
    let main_worktree = PathBuf::from(
        memoria_application::ports::GitRepository::main_worktree(&git)
            .unwrap_or_else(|_| root.display().to_string()),
    );
    // Client probing lives in the hook adapter and runs lazily, only for
    // the target an installation validates. Ordinary commands never execute
    // an optional client executable.
    let hooks = FsHookStore::new(
        root.clone(),
        main_worktree,
        current_executable(),
        git_dir.clone(),
        Box::new(CommandClientProbe),
    );
    // The managed consumer workflow lives in the selected worktree. Its
    // private coordination files stay under the same Git metadata directory
    // that bounds every other private Memoria path.
    let workflows = FsWorkflowStore::new(root.clone(), git_dir.clone(), env!("CARGO_PKG_VERSION"));
    let progress = StderrProgress;
    let services = Services {
        files: &files,
        git: &git,
        ignore: &ignore,
        config: &config,
        markdown: &markdown,
        hasher: &hasher,
        state: &state,
        inspector: &inspector,
        clock: &clock,
        locks: &locks,
        writer: &writer,
        packets: &packets,
        packet_input: &packet_input,
        skills: &skills,
        locations: &locations,
        hooks: &hooks,
        workflows: &workflows,
        progress: &progress,
    };

    match &cli.command {
        Command::Completions { .. }
        | Command::State {
            command: StateCommand::Diff { .. },
        } => unreachable!("early dispatch"),
        Command::Packet { .. } => unreachable!("early dispatch"),
        Command::Explain { document, full } => {
            let outcome = usecases::explain::run(&services, document)?;
            let data = outcome.data.to_detail();
            let human = if *full {
                text::explain(&outcome.data)
            } else {
                presentation::review::explain(&outcome.data)
            };
            bounded_output(
                cli,
                outcome.diagnostics,
                data,
                human,
                usecases::explain::MAX_RENDERED_BYTES,
                "explain_limit_exceeded",
                false,
            )
        }
        Command::Init { apply } => {
            let outcome = usecases::init::run(&services, *apply)?;
            let (data, human) = (outcome.data.to_detail(), text::init(&outcome.data));
            Ok(output(outcome, data, human))
        }
        Command::Status {
            explain: Some(_),
            summary: true,
        } => Err(AppError::usage(
            "summary_invalid",
            "--summary emits bounded counts only; it cannot be combined with --explain",
        )),
        Command::Status {
            summary: true,
            explain: None,
        } => {
            let outcome = usecases::status::run_summary(&services)?;
            let (data, human) = (outcome.data.to_detail(), text::summary(&outcome.data));
            Ok(output(outcome, data, human))
        }
        Command::Status {
            explain,
            summary: false,
        } => {
            let outcome = usecases::status::run(&services, explain.as_deref())?;
            let (data, human) = (outcome.data.to_detail(), text::status(&outcome.data));
            Ok(output(outcome, data, human))
        }
        Command::Guidance { document } => {
            let outcome = usecases::guidance::run(&services, document.as_deref())?;
            let (data, human) = (outcome.data.to_detail(), text::guidance(&outcome.data));
            Ok(output(outcome, data, human))
        }
        Command::State {
            command: StateCommand::Inspect { file },
        } => {
            let outcome = match file {
                Some(path) => usecases::state_inspect::run_file(&inspector, path)?,
                None => usecases::state_inspect::run(&services)?,
            };
            let (data, human) = (outcome.data.to_detail(), text::state_inspect(&outcome.data));
            Ok(output(outcome, data, human))
        }
        Command::Lint => {
            let outcome = usecases::lint::run(&services)?;
            let (data, human) = (outcome.data.to_detail(), text::lint(&outcome.data));
            Ok(output(outcome, data, human))
        }
        Command::Review {
            document: None,
            max_bytes,
            full,
        } => {
            if *full {
                return Err(AppError::usage(
                    "full_requires_document",
                    "--full requires a focused README",
                ));
            }
            if max_bytes.is_some() {
                return Err(AppError::usage(
                    "max_bytes_invalid",
                    "--max-bytes applies only to a focused packet; name a README",
                ));
            }
            let outcome = usecases::plan::run(&services)?;
            let (data, human) = (outcome.data.to_detail(), text::plan(&outcome.data));
            Ok(output(outcome, data, human))
        }
        Command::Review {
            document: Some(document),
            max_bytes,
            full,
        } => {
            let mut outcome = usecases::prepare_review::run(&services, document, *max_bytes)?;
            // Both presentations report the same complete-envelope record count.
            outcome.data.record_count = services
                .packets
                .complete_record_count(&outcome.data, &outcome.diagnostics);
            // The complete producer limits (records, depth, serialized size) are
            // enforced by encoding the envelope for every presentation; a refusal
            // never reaches the human view or emits a token either.
            let encoded = services
                .packets
                .encode(&outcome.data, &outcome.diagnostics)
                .map_err(|failure| match failure {
                    memoria_application::ports::PacketFailure::Invalid { code, message } => {
                        AppError::validation(code, message)
                    }
                    memoria_application::ports::PacketFailure::Io(err) => {
                        AppError::io("io_error", err.to_string())
                    }
                })?;
            let raw = if cli.format == Format::Json {
                Some(encoded)
            } else {
                None
            };
            let human = if *full {
                text::packet(&outcome.data)
            } else {
                presentation::review::packet(&outcome.data)
            };
            Ok(CommandOutput {
                data: outcome.data.to_detail(),
                human,
                diagnostics: outcome.diagnostics,
                raw_json: raw,
            })
        }
        Command::Render { document, dry_run } => {
            let outcome = usecases::render::run(&services, document.as_deref(), *dry_run)?;
            let (data, human) = (outcome.data.to_detail(), text::render(&outcome.data));
            Ok(output(outcome, data, human))
        }
        Command::Ack(args) => {
            let packet = if args.packet == "-" {
                PacketSource::Stdin
            } else {
                PacketSource::File(args.packet.clone())
            };
            let request = usecases::ack::AckArgs {
                document: args.document.clone(),
                packet,
                token: args.token.clone(),
                reviewer: presentation::cli::resolve_reviewer(
                    args.reviewer.as_deref(),
                    std::env::var_os("MEMORIA_REVIEWER"),
                )?,
                result: args.result.clone(),
                note: args.note.clone(),
            };
            let outcome = usecases::ack::run(&services, &request)?;
            let (data, human) = (outcome.data.to_detail(), text::ack(&outcome.data));
            Ok(output(outcome, data, human))
        }
        Command::Invalidate { scope, reason } => {
            let outcome = usecases::invalidate::run(&services, scope, reason)?;
            let (data, human) = (outcome.data.to_detail(), text::invalidate(&outcome.data));
            Ok(output(outcome, data, human))
        }
        Command::Check => {
            let outcome = usecases::check::run(&services)?;
            let (data, human) = (outcome.data.to_detail(), text::check(&outcome.data));
            Ok(output(outcome, data, human))
        }
        Command::Graph => {
            let outcome = usecases::graph::run(&services)?;
            let (data, human) = (outcome.data.to_detail(), text::graph(&outcome.data));
            Ok(output(outcome, data, human))
        }
        Command::Agent {
            command: AgentCommand::Hook { command },
        } => {
            let target = |t: &Target| match t {
                Target::Codex => AgentTarget::Codex,
                Target::Claude => AgentTarget::Claude,
            };
            let outcome = match command {
                HookCommand::Install(args) => {
                    usecases::agent_hooks::install(&services, target(&args.target), args.dry_run)?
                }
                HookCommand::Status { target: t } => {
                    usecases::agent_hooks::status(&services, target(t))?
                }
                HookCommand::Uninstall(args) => {
                    usecases::agent_hooks::uninstall(&services, target(&args.target), args.dry_run)?
                }
                HookCommand::Run { .. } => unreachable!("the runner is handled before dispatch"),
            };
            let (data, human) = (outcome.data.to_detail(), text::agent_hook(&outcome.data));
            Ok(output(outcome, data, human))
        }
        Command::Agent { command } => {
            let (operation, args) = match command {
                AgentCommand::Install(args) => (SkillOperation::Install, args),
                AgentCommand::Status(args) => (SkillOperation::Status, args),
                AgentCommand::Upgrade(args) => (SkillOperation::Upgrade, args),
                AgentCommand::Uninstall(args) => (SkillOperation::Uninstall, args),
                AgentCommand::Hook { .. } => unreachable!("handled above"),
            };
            let request = usecases::agent::AgentArgs {
                operation,
                target: args.target.map(|t| match t {
                    Target::Codex => AgentTarget::Codex,
                    Target::Claude => AgentTarget::Claude,
                }),
                scope: match args.scope {
                    Scope::Local => AgentScope::Local,
                    Scope::Global => AgentScope::Global,
                },
                path: args.path.clone(),
                dry_run: args.dry_run,
                replace_existing: args.replace_existing,
            };
            let outcome = usecases::agent::run(&services, &request)?;
            let (data, human) = (outcome.data.to_detail(), text::agent(&outcome.data));
            Ok(output(outcome, data, human))
        }
        Command::Integrations {
            command: IntegrationsCommand::Github { command },
        } => {
            let request = github_args(command);
            let outcome = usecases::github_workflow::run(&services, &request)?;
            let (data, human) = (
                outcome.data.to_detail(),
                text::github_workflow(&outcome.data),
            );
            Ok(output(outcome, data, human))
        }
        Command::Integrations { .. } => {
            unreachable!("skill and hook spellings normalize before dispatch")
        }
    }
}

/// Translate the GitHub workflow grammar into one application request.
fn github_args(command: &GithubCommand) -> usecases::github_workflow::WorkflowArgs {
    use usecases::github_workflow::WorkflowArgs;
    match command {
        GithubCommand::Install(args) => WorkflowArgs {
            operation: WorkflowOperation::Install,
            path: args.path.clone(),
            version: args.version.clone(),
            action_ref: args.action_ref.clone(),
            runner: args.runner.clone(),
            apply: args.apply,
            dry_run: args.dry_run,
        },
        GithubCommand::Upgrade(args) => WorkflowArgs {
            operation: WorkflowOperation::Upgrade,
            path: args.path.clone(),
            version: args.version.clone(),
            action_ref: args.action_ref.clone(),
            runner: args.runner.clone(),
            apply: args.apply,
            dry_run: args.dry_run,
        },
        GithubCommand::Status { path } => WorkflowArgs {
            operation: WorkflowOperation::Status,
            path: path.clone(),
            version: None,
            action_ref: None,
            runner: None,
            apply: false,
            dry_run: false,
        },
        GithubCommand::Uninstall(args) => WorkflowArgs {
            operation: WorkflowOperation::Uninstall,
            path: args.path.clone(),
            version: None,
            action_ref: None,
            runner: None,
            apply: args.apply,
            dry_run: args.dry_run,
        },
    }
}

fn bounded_output(
    cli: &Cli,
    diagnostics: Vec<Diagnostic>,
    data: Detail,
    human: String,
    limit: u64,
    code: &str,
    io: bool,
) -> Result<CommandOutput, AppError> {
    let encoded = json::envelope(cli.command.name(), true, &data, &diagnostics).into_bytes();
    if encoded.len() as u64 > limit || human.len() as u64 > limit {
        let message = format!(
            "The rendered output exceeds {limit} bytes (JSON: {}, human: {}).",
            encoded.len(),
            human.len()
        );
        return Err(if io {
            AppError::io(code, message)
        } else {
            AppError::validation(code, message)
        });
    }
    Ok(CommandOutput {
        data,
        human,
        diagnostics,
        raw_json: Some(encoded),
    })
}

/// The lifecycle arguments of an explicitly global agent operation.
fn global_agent_args(
    command: &AgentCommand,
) -> Option<(&presentation::cli::AgentArgs, SkillOperation)> {
    let (args, operation) = match command {
        AgentCommand::Install(args) => (args, SkillOperation::Install),
        AgentCommand::Status(args) => (args, SkillOperation::Status),
        AgentCommand::Upgrade(args) => (args, SkillOperation::Upgrade),
        AgentCommand::Uninstall(args) => (args, SkillOperation::Uninstall),
        AgentCommand::Hook { .. } => return None,
    };
    (args.scope == Scope::Global).then_some((args, operation))
}

fn run_global_agent(
    cli: &Cli,
    (args, operation): (&presentation::cli::AgentArgs, SkillOperation),
) -> ExitCode {
    let mut stdout = std::io::stdout().lock();
    let deliver_result = |stdout: &mut std::io::StdoutLock<'_>,
                          result: Result<
        memoria_application::error::Outcome<memoria_application::usecases::agent::AgentReport>,
        AppError,
    >|
     -> u8 {
        let (code, payload) = match result {
            Ok(outcome) => {
                let payload = match cli.format {
                    Format::Json => json::envelope(
                        "agent",
                        true,
                        &outcome.data.to_detail(),
                        &outcome.diagnostics,
                    )
                    .into_bytes(),
                    Format::Human => {
                        eprint!(
                            "{}",
                            presentation::human::render_diagnostics(
                                &outcome.diagnostics,
                                presentation::human::HumanOptions::stderr()
                            )
                        );
                        text::agent(&outcome.data).into_bytes()
                    }
                };
                (0u8, payload)
            }
            Err(error) => {
                let payload = match cli.format {
                    Format::Json => {
                        json::envelope("agent", false, &error.data, &error.diagnostics).into_bytes()
                    }
                    Format::Human => {
                        eprint!(
                            "{}",
                            presentation::human::render_diagnostics(
                                &error.diagnostics,
                                presentation::human::HumanOptions::stderr()
                            )
                        );
                        format!(
                            "memoria agent failed with exit status {}\n",
                            error.class.code()
                        )
                        .into_bytes()
                    }
                };
                (error.class.code() as u8, payload)
            }
        };
        if deliver(stdout, &payload).is_err() {
            eprintln!("memoria: io_error: cannot write the response to stdout");
            return 4;
        }
        code
    };
    if cli.root.is_some() {
        let error = AppError::usage(
            "root_invalid",
            "--root has no meaning for a global skill operation; remove it or use --scope local",
        );
        return ExitCode::from(deliver_result(&mut stdout, Err(error)));
    }
    let invocation = invocation_dir();
    // A worktree is not required. When one exists, it is used only to report
    // an overlapping local installation; it never becomes the destination.
    let worktree = GitCli::discover(&invocation)
        .ok()
        .map(|git| git.root().to_path_buf());
    let skills = FsSkillStore::new(
        worktree.clone().unwrap_or_else(|| invocation.clone()),
        SKILL,
        env!("CARGO_PKG_VERSION"),
    );
    let locations = EnvAgentLocations::from_environment(worktree, invocation);
    let progress = StderrProgress;
    let request = usecases::agent::AgentArgs {
        operation,
        target: args.target.map(|t| match t {
            Target::Codex => AgentTarget::Codex,
            Target::Claude => AgentTarget::Claude,
        }),
        scope: AgentScope::Global,
        path: args.path.clone(),
        dry_run: args.dry_run,
        replace_existing: args.replace_existing,
    };
    let result = usecases::agent::run_with(&skills, &locations, &progress, &request);
    ExitCode::from(deliver_result(&mut stdout, result))
}

/// The directory an explicit `--path` or `--file` resolves against.
fn invocation_dir() -> PathBuf {
    std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."))
}

/// The absolute path of this executable, bound into an installed hook.
fn current_executable() -> PathBuf {
    std::env::current_exe().unwrap_or_else(|_| PathBuf::from("memoria"))
}

/// Whether the raw arguments select an output format at all, in either
/// placement. The global option has a default value, so the parsed command
/// cannot distinguish an explicit selection from the default.
fn format_explicitly_selected(args: &[String]) -> bool {
    let mut previous: Option<&str> = None;
    for arg in args.iter().skip(1) {
        if arg.starts_with("--format=") || previous == Some("--format") {
            return true;
        }
        previous = Some(arg.as_str());
    }
    false
}

/// Whether the raw arguments explicitly request JSON output. Used before
/// argument parsing so usage errors can still honor the machine contract.
fn json_requested(args: &[String]) -> bool {
    let mut previous: Option<&str> = None;
    for arg in args.iter().skip(1) {
        if arg == "--format=json" || (previous == Some("--format") && arg == "json") {
            return true;
        }
        previous = Some(arg.as_str());
    }
    false
}

/// The command name for a usage-error envelope: the first known command word.
fn command_word(args: &[String]) -> &'static str {
    const KNOWN: [&str; 17] = [
        "completions",
        "packet",
        "explain",
        "init",
        "status",
        "guidance",
        "state",
        "lint",
        "review",
        "render",
        "ack",
        "invalidate",
        "check",
        "graph",
        "agent",
        "integrations",
        "unknown",
    ];
    let words = command_words(args);
    let Some(first) = words.first() else {
        return "unknown";
    };
    let found = KNOWN
        .iter()
        .find(|known| *known == first)
        .copied()
        .unwrap_or("unknown");
    if found != "integrations" {
        return found;
    }
    // An argument error reaches this path before normalization, so the alias
    // has to be recognized here too. `integrations skill` and
    // `integrations hook` are the established `agent` requests, and their
    // envelopes must keep the established command label.
    match words.get(1).map(String::as_str) {
        Some("skill") | Some("hook") => "agent",
        _ => "integrations",
    }
}

/// The command words of an invocation, in order, without options or their
/// values. Used before parsing, so it makes no grammar decision.
fn command_words(args: &[String]) -> Vec<String> {
    let mut words = Vec::new();
    let mut previous: Option<&str> = None;
    for arg in args.iter().skip(1) {
        let takes_value = matches!(previous, Some("--format") | Some("--root"));
        previous = Some(arg.as_str());
        if takes_value || arg.starts_with('-') {
            continue;
        }
        words.push(arg.clone());
        if words.len() == 2 {
            break;
        }
    }
    words
}

/// Write the final response. Any stdout failure, including a closed pipe, is
/// an I/O failure: the caller did not receive the response.
fn deliver(stdout: &mut std::io::StdoutLock<'_>, bytes: &[u8]) -> Result<(), std::io::Error> {
    stdout.write_all(bytes)?;
    stdout.flush()
}

/// Collect OS arguments without assuming UTF-8. An argument that is not
/// valid UTF-8 is a usage error with the index of the offending argument;
/// a JSON request elsewhere in the arguments is still honored.
fn collect_args() -> Result<Vec<String>, (usize, Vec<String>)> {
    let raw: Vec<std::ffi::OsString> = std::env::args_os().collect();
    let mut args = Vec::with_capacity(raw.len());
    let mut invalid: Option<usize> = None;
    for (index, arg) in raw.into_iter().enumerate() {
        match arg.into_string() {
            Ok(text) => args.push(text),
            Err(lossy) => {
                invalid.get_or_insert(index);
                args.push(lossy.to_string_lossy().into_owned());
            }
        }
    }
    match invalid {
        None => Ok(args),
        Some(index) => Err((index, args)),
    }
}

/// The documented native hook endpoint.
///
/// It reads native hook JSON on stdin and writes one buffered JSON object on
/// stdout. It always exits 0 for event handling, never emits a blocking
/// decision, and never accepts the ordinary output-format option.
fn run_hook(target: Target, protocol: u32, configuration_root: &str) -> ExitCode {
    use memoria_application::usecases::agent_hooks::{
        MAX_EVENT_BYTES, RUNNER_DEADLINE_MS, RunnerResult, run_stop,
    };
    use memoria_infrastructure::bounded_process::Supervisor;
    use std::io::Read as _;
    use std::sync::{Arc, mpsc};
    use std::time::{Duration, Instant};

    // One deadline covers the whole endpoint: the stdin read, Git
    // discovery, and the bounded status child. The client's own five-second
    // timeout is the final outer limit; this bound keeps the endpoint well
    // inside it.
    let started = Instant::now();
    let deadline = Duration::from_millis(RUNNER_DEADLINE_MS);
    let emit = |result: RunnerResult| -> ExitCode {
        let mut payload = match &result {
            RunnerResult::Silent => "{}".to_string(),
            other => other.to_json(),
        };
        payload.push('\n');
        let mut stdout = std::io::stdout().lock();
        let _ = deliver(&mut stdout, payload.as_bytes());
        ExitCode::from(0)
    };
    let unfinished = || {
        RunnerResult::Message(
            "Memoria did not finish its documentation inspection in time. Run `memoria status` when convenient."
                .to_string(),
        )
    };

    // The event arrives on a thread, so a producer that holds the pipe open
    // below the byte cap cannot block the endpoint.
    let (input_sender, input_receiver) = mpsc::channel();
    std::thread::spawn(move || {
        let mut buffer = Vec::new();
        let read = std::io::stdin()
            .lock()
            .take(MAX_EVENT_BYTES + 1)
            .read_to_end(&mut buffer);
        let _ = input_sender.send(read.map(|_| buffer));
    });
    let remaining = deadline.saturating_sub(started.elapsed());
    let buffer = match input_receiver.recv_timeout(remaining) {
        Ok(Ok(buffer)) if buffer.len() as u64 <= MAX_EVENT_BYTES => buffer,
        // A malformed, oversized, or unreadable event is not an error the
        // client should see: the endpoint stays silent and exits 0.
        Ok(_) => return emit(RunnerResult::Silent),
        Err(mpsc::RecvTimeoutError::Timeout) => return emit(unfinished()),
        Err(_) => return emit(RunnerResult::Silent),
    };
    let Some(event) = parse_event(&buffer) else {
        return emit(RunnerResult::Silent);
    };
    let target_name = match target {
        Target::Codex => "codex",
        Target::Claude => "claude",
    };
    let configuration_root = configuration_root.to_string();
    let executable = current_executable();

    // Discovery and the bounded status child run under one owner. Git
    // discovery is synchronous, so it runs on a worker thread, but every
    // process that worker starts belongs to this supervisor: each one gets
    // its own session, the deadline that remains, and termination when the
    // endpoint gives up.
    let supervisor = Supervisor::new(started + deadline);
    let worker_supervisor = Arc::clone(&supervisor);
    let (work_sender, work_receiver) = mpsc::channel();
    let worker_event = event.clone();
    std::thread::spawn(move || {
        let resolved = resolve_event_root(
            target_name,
            &worker_event,
            &configuration_root,
            &worker_supervisor,
        );
        let process = SelfStatusProcess::supervised(executable, Arc::clone(&worker_supervisor));
        let result = run_stop(&process, protocol, &worker_event, resolved.as_deref());
        let _ = work_sender.send(result);
    });
    let remaining = deadline.saturating_sub(started.elapsed());
    match work_receiver.recv_timeout(remaining) {
        Ok(result) => emit(result),
        Err(_) => {
            // The worker thread is detached, so it cannot be joined. Its
            // subprocesses are owned, so they can be stopped: terminate every
            // live group, and refuse the ones a late worker would start.
            supervisor.cancel();
            emit(unfinished())
        }
    }
}

/// Read the three fields the runner needs from a native event. It never
/// opens the transcript and never inspects the last assistant message.
fn parse_event(bytes: &[u8]) -> Option<memoria_application::usecases::agent_hooks::StopEvent> {
    use memoria_application::usecases::agent_hooks::StopEvent;
    use memoria_infrastructure::json::{self, Json, Limits};

    let value = json::parse(bytes, Limits::STATE).ok()?;
    let Json::Object(map) = value else {
        return None;
    };
    let text = |key: &str| match map.get(key) {
        Some(Json::String(value)) => Some(value.clone()),
        _ => None,
    };
    let event_name = text("hook_event_name")
        .or_else(|| text("event"))
        .or_else(|| text("eventName"))?;
    let working_directory = text("cwd")
        .or_else(|| text("working_directory"))
        .or_else(|| text("workingDirectory"));
    let stop_hook_active = matches!(map.get("stop_hook_active"), Some(Json::Bool(true)))
        || matches!(map.get("stopHookActive"), Some(Json::Bool(true)));
    Some(StopEvent {
        event_name,
        working_directory,
        stop_hook_active,
    })
}

/// Resolve the event's working directory to a Git worktree root and check it
/// against the registered configuration root.
///
/// The relationship is verified through Git metadata, never through an
/// event-supplied repository identifier. An event outside the registered
/// project returns `None` before any Memoria snapshot scan.
fn resolve_event_root(
    target: &str,
    event: &memoria_application::usecases::agent_hooks::StopEvent,
    configuration_root: &str,
    supervisor: &std::sync::Arc<memoria_infrastructure::bounded_process::Supervisor>,
) -> Option<String> {
    let start = event.working_directory.clone()?;
    let git =
        GitCli::discover_supervised(Path::new(&start), std::sync::Arc::clone(supervisor)).ok()?;
    let root = git.root().canonicalize().ok()?;
    let registered = Path::new(configuration_root).canonicalize().ok()?;
    // It inspects only a project with memoria.toml at that root.
    if !root.join("memoria.toml").is_file() {
        return None;
    }
    match target {
        // For Codex, the event root must equal the registered root.
        "codex" => (root == registered).then(|| root.display().to_string()),
        // For Claude, the event root must belong to the registered main
        // checkout's Git worktree family.
        _ => {
            if root == registered {
                return Some(root.display().to_string());
            }
            let family = memoria_application::ports::GitRepository::main_worktree(&git).ok()?;
            let family = Path::new(&family).canonicalize().ok()?;
            (family == registered).then(|| root.display().to_string())
        }
    }
}

fn main() -> ExitCode {
    let args = match collect_args() {
        Ok(args) => args,
        Err((index, args)) => {
            let message = format!(
                "argument {index} is not valid UTF-8; project paths and options must be UTF-8 text"
            );
            if json_requested(&args) {
                let diagnostic = Diagnostic::error("usage_error", message);
                let envelope =
                    json::envelope(command_word(&args), false, &Detail::Null, &[diagnostic]);
                let mut stdout = std::io::stdout().lock();
                if deliver(&mut stdout, envelope.as_bytes()).is_err() {
                    eprintln!("memoria: io_error: cannot write the response to stdout");
                    return ExitCode::from(4);
                }
                return ExitCode::from(2);
            }
            eprintln!("error: {message}");
            return ExitCode::from(2);
        }
    };
    let cli = match Cli::try_parse_from(&args) {
        Ok(cli) => cli,
        Err(err) => {
            // Help and version print to stdout with exit 0; usage errors exit 2.
            if !err.use_stderr() {
                // Help and version are responses too: a failed delivery is an I/O failure.
                let delivered = err.print().and_then(|_| std::io::stdout().flush());
                if let Err(io_err) = delivered {
                    eprintln!("memoria: io_error: cannot write the response to stdout: {io_err}");
                    return ExitCode::from(4);
                }
                return ExitCode::from(0);
            }
            if json_requested(&args) {
                let message = err.to_string();
                let diagnostic = Diagnostic::error("usage_error", message.trim_end().to_string())
                    .with_details(
                        memoria_application::error::DetailMap::default()
                            .text("kind", format!("{:?}", err.kind()))
                            .build(),
                    );
                let envelope =
                    json::envelope(command_word(&args), false, &Detail::Null, &[diagnostic]);
                let mut stdout = std::io::stdout().lock();
                if deliver(&mut stdout, envelope.as_bytes()).is_err() {
                    eprintln!("memoria: io_error: cannot write the response to stdout");
                    return ExitCode::from(4);
                }
                return ExitCode::from(2);
            }
            let _ = err.print();
            return ExitCode::from(2);
        }
    };
    // The new skill and hook spellings become the established agent requests
    // before any dispatch decision, so every early path and every output keeps
    // its previous behavior.
    let cli = cli.normalized();
    // A global agent operation resolves before project discovery: it works
    // outside Git repositories and without project configuration, so it
    // builds narrow agent services instead of a complete snapshot.
    if let Command::Agent { command } = &cli.command
        && let Some(args) = global_agent_args(command)
    {
        return run_global_agent(&cli, args);
    }
    // An explicit state file is inspected before project discovery: that
    // mode works outside Git and needs neither configuration nor a scan.
    if let Command::State {
        command: StateCommand::Inspect { file: Some(path) },
    } = &cli.command
    {
        let inspector = LockStateInspector::new(None, invocation_dir());
        let mut stdout = std::io::stdout().lock();
        let (code, payload) = match usecases::state_inspect::run_file(&inspector, path) {
            Ok(outcome) => {
                let payload = match cli.format {
                    Format::Json => {
                        json::envelope("state inspect", true, &outcome.data.to_detail(), &[])
                            .into_bytes()
                    }
                    Format::Human => text::state_inspect(&outcome.data).into_bytes(),
                };
                (0u8, payload)
            }
            Err(error) => {
                let payload = match cli.format {
                    Format::Json => {
                        json::envelope("state inspect", false, &error.data, &error.diagnostics)
                            .into_bytes()
                    }
                    Format::Human => {
                        eprint!(
                            "{}",
                            presentation::human::render_diagnostics(
                                &error.diagnostics,
                                presentation::human::HumanOptions::stderr()
                            )
                        );
                        format!(
                            "memoria state inspect failed with exit status {}\n",
                            error.class.code()
                        )
                        .into_bytes()
                    }
                };
                (error.class.code() as u8, payload)
            }
        };
        if deliver(&mut stdout, &payload).is_err() {
            eprintln!("memoria: io_error: cannot write the response to stdout");
            return ExitCode::from(4);
        }
        return ExitCode::from(code);
    }
    // The native hook endpoint is handled before ordinary dispatch: it uses
    // its own protocol on stdin and stdout, not the CLI output formats.
    if let Command::Agent {
        command:
            AgentCommand::Hook {
                command:
                    HookCommand::Run {
                        target,
                        protocol,
                        configuration_root,
                    },
            },
    } = &cli.command
    {
        // The public contract excludes `--format` from this endpoint. An
        // explicit selection is an argument error, not an event failure, so
        // the fail-open behavior for malformed events is unaffected.
        if format_explicitly_selected(&args) {
            let message =
                "the native hook endpoint has its own JSON protocol and does not accept --format";
            let diagnostic = Diagnostic::error("usage_error", message);
            let mut stdout = std::io::stdout().lock();
            if json_requested(&args) {
                let envelope =
                    json::envelope("agent hook run", false, &Detail::Null, &[diagnostic]);
                if deliver(&mut stdout, envelope.as_bytes()).is_err() {
                    eprintln!("memoria: io_error: cannot write the response to stdout");
                    return ExitCode::from(4);
                }
            } else {
                eprintln!("error: {message}");
            }
            return ExitCode::from(2);
        }
        return run_hook(*target, *protocol, configuration_root);
    }
    let command_name = cli.command.name();
    let mut stdout = std::io::stdout().lock();
    let (mut code, payload): (u8, Vec<u8>) = match run(&cli) {
        Ok(result) => {
            let payload = match cli.format {
                Format::Json => match result.raw_json {
                    Some(bytes) => bytes,
                    None => json::envelope(command_name, true, &result.data, &result.diagnostics)
                        .into_bytes(),
                },
                Format::Human => {
                    eprint!(
                        "{}",
                        presentation::human::render_diagnostics(
                            &result.diagnostics,
                            presentation::human::HumanOptions::stderr()
                        )
                    );
                    result.human.into_bytes()
                }
            };
            (0, payload)
        }
        Err(error) => {
            let payload = match cli.format {
                Format::Json => {
                    json::envelope(command_name, false, &error.data, &error.diagnostics)
                        .into_bytes()
                }
                Format::Human => {
                    eprint!(
                        "{}",
                        presentation::human::render_diagnostics(
                            &error.diagnostics,
                            presentation::human::HumanOptions::stderr()
                        )
                    );
                    format!(
                        "memoria {command_name} failed with exit status {}\n",
                        error.class.code()
                    )
                    .into_bytes()
                }
            };
            (error.class.code() as u8, payload)
        }
    };
    if let Err(err) = deliver(&mut stdout, &payload) {
        eprintln!("memoria: io_error: cannot write the response to stdout: {err}");
        // A completed mutation is not rolled back; only its report was lost.
        code = 4;
    }
    ExitCode::from(code)
}
