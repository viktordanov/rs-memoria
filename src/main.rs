//! Composition root for the `memoria` binary.

mod presentation;

use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use clap::Parser as _;
use memoria_application::error::{AppError, Detail, Diagnostic, Outcome};
use memoria_application::ports::{AgentTarget, PacketSource, Progress, Services, SkillOperation};
use memoria_application::usecases;
use memoria_infrastructure::config::YamlConfigurationReader;
use memoria_infrastructure::{
    AtomicFileWriter, FsPacketInput, FsProjectFiles, FsSkillStore, GitCli, JsonPacketCodec,
    JsonStateStore, LockFileCoordinator, PulldownMarkdownCodec, SystemClock, Xxh3Hasher,
};
use presentation::cli::{AgentCommand, Cli, Command, Format, Target};
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
    let git = discover_root(cli.root.as_deref())?;
    let root = git.root().to_path_buf();
    let files = FsProjectFiles::new(root.clone());
    let config = YamlConfigurationReader;
    let markdown = PulldownMarkdownCodec;
    let hasher = Xxh3Hasher;
    let state = JsonStateStore::new(root.clone());
    let clock = SystemClock;
    let locks = LockFileCoordinator::new(root.clone());
    let writer = AtomicFileWriter::new(root.clone());
    let packets = JsonPacketCodec::new(&hasher);
    let packet_input = FsPacketInput;
    let skills = FsSkillStore::new(root.clone(), SKILL, env!("CARGO_PKG_VERSION"));
    let progress = StderrProgress;
    let services = Services {
        files: &files,
        git: &git,
        config: &config,
        markdown: &markdown,
        hasher: &hasher,
        state: &state,
        clock: &clock,
        locks: &locks,
        writer: &writer,
        packets: &packets,
        packet_input: &packet_input,
        skills: &skills,
        progress: &progress,
    };

    match &cli.command {
        Command::Init => {
            let outcome = usecases::init::run(&services)?;
            let (data, human) = (outcome.data.to_detail(), text::init(&outcome.data));
            Ok(output(outcome, data, human))
        }
        Command::Status { explain } => {
            let outcome = usecases::status::run(&services, explain.as_deref())?;
            let (data, human) = (outcome.data.to_detail(), text::status(&outcome.data));
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
        } => {
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
            let human = text::packet(&outcome.data);
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
                reviewer: args.reviewer.clone(),
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
        Command::Agent { command } => {
            let (operation, args) = match command {
                AgentCommand::Install(args) => (SkillOperation::Install, args),
                AgentCommand::Uninstall(args) => (SkillOperation::Uninstall, args),
            };
            let target = args.target.map(|t| match t {
                Target::Codex => AgentTarget::Codex,
                Target::Claude => AgentTarget::Claude,
            });
            let outcome = usecases::agent::run(
                &services,
                operation,
                target,
                args.path.as_deref(),
                args.dry_run,
            )?;
            let (data, human) = (outcome.data.to_detail(), text::agent(&outcome.data));
            Ok(output(outcome, data, human))
        }
    }
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
    const KNOWN: [&str; 11] = [
        "init",
        "status",
        "lint",
        "review",
        "render",
        "ack",
        "invalidate",
        "check",
        "graph",
        "agent",
        "unknown",
    ];
    let mut previous: Option<&str> = None;
    for arg in args.iter().skip(1) {
        let takes_value = matches!(previous, Some("--format") | Some("--root"));
        previous = Some(arg.as_str());
        if takes_value || arg.starts_with('-') {
            continue;
        }
        return KNOWN
            .iter()
            .find(|k| *k == arg)
            .copied()
            .unwrap_or("unknown");
    }
    "unknown"
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
                    for diagnostic in &result.diagnostics {
                        eprintln!("{}", text::diagnostic_line(diagnostic));
                    }
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
                    for diagnostic in &error.diagnostics {
                        eprintln!("{}", text::diagnostic_line(diagnostic));
                    }
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
