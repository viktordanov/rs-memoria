//! Deterministic fault and boundary tests that run real use cases against
//! real adapters on a temporary Git repository.

use std::cell::Cell;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use memoria_application::error::AppError;
use memoria_application::ports::{AdapterError, GitRepository, Progress, Services, StateStore};
use memoria_application::usecases;
use memoria_infrastructure::config::YamlConfigurationReader;
use memoria_infrastructure::fs::{
    AtomicFileWriter, FsProjectFiles, LockFileCoordinator, SystemClock,
};
use memoria_infrastructure::{
    FsPacketInput, FsSkillStore, GitCli, JsonPacketCodec, JsonStateStore, PulldownMarkdownCodec,
    Xxh3Hasher,
};

struct Quiet;

impl Progress for Quiet {
    fn note(&self, _message: &str) {}
}

fn git(root: &Path, args: &[&str]) {
    let output = Command::new("git")
        .arg("-C")
        .arg(root)
        .args(args)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

/// A repository whose root README imports two child exports.
fn seed() -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    fs::write(root.join("memoria.yml"), "version: 1\n").unwrap();
    fs::write(
        root.join("README.md"),
        "# Root\n\n<!-- memoria:import src=\"a/README.md#s\" -->\n<!-- /memoria:import -->\n<!-- memoria:import src=\"b/README.md#s\" -->\n<!-- /memoria:import -->\n",
    )
    .unwrap();
    for name in ["a", "b"] {
        fs::create_dir_all(root.join(name)).unwrap();
        fs::write(root.join(name).join("README.md"), format!("# {name}\n\n<!-- memoria:export id=\"s\" -->\nSummary {name}.\n<!-- /memoria:export -->\n\n<!-- memoria:import src=\"../c/README.md#s\" -->\n<!-- /memoria:import -->\n")).unwrap();
        fs::write(root.join(name).join("src.rs"), format!("// {name}\n")).unwrap();
    }
    fs::create_dir_all(root.join("c")).unwrap();
    fs::write(
        root.join("c/README.md"),
        "# c\n\n<!-- memoria:export id=\"s\" -->\nSummary c.\n<!-- /memoria:export -->\n",
    )
    .unwrap();
    git(root, &["init", "-q"]);
    git(root, &["config", "user.email", "f@example.com"]);
    git(root, &["config", "user.name", "F"]);
    git(root, &["config", "commit.gpgsign", "false"]);
    git(root, &["add", "-A"]);
    git(root, &["commit", "-qm", "seed"]);
    dir
}

/// A Git port that runs a mutation on the Nth eligible-path enumeration.
struct MutatingGit<'a> {
    inner: GitCli,
    calls: Cell<u32>,
    mutate_on: &'a dyn Fn(u32),
}

impl GitRepository for MutatingGit<'_> {
    fn eligible_paths(&self) -> Result<Vec<Vec<u8>>, AdapterError> {
        let call = self.calls.get() + 1;
        self.calls.set(call);
        (self.mutate_on)(call);
        self.inner.eligible_paths()
    }
    fn unmerged_paths(&self) -> Result<Vec<String>, AdapterError> {
        self.inner.unmerged_paths()
    }
    fn sparse_checkout_enabled(&self) -> Result<bool, AdapterError> {
        self.inner.sparse_checkout_enabled()
    }
    fn global_excludes(&self) -> Result<Option<Vec<u8>>, AdapterError> {
        self.inner.global_excludes()
    }
    fn repository_excludes(&self) -> Result<Option<Vec<u8>>, AdapterError> {
        self.inner.repository_excludes()
    }
    fn head_commit(&self) -> Result<Option<String>, AdapterError> {
        self.inner.head_commit()
    }
    fn worktree_dirty(&self) -> Result<bool, AdapterError> {
        self.inner.worktree_dirty()
    }
    fn read_blob(&self, commit: &str, path: &str) -> Result<Option<Vec<u8>>, AdapterError> {
        self.inner.read_blob(commit, path)
    }
    fn explain_ignore(&self, path: &str) -> Result<Option<String>, AdapterError> {
        self.inner.explain_ignore(path)
    }
    fn ignored_directories(&self, directories: &[String]) -> Result<Vec<String>, AdapterError> {
        self.inner.ignored_directories(directories)
    }
}

struct Harness<'a> {
    root: PathBuf,
    files: FsProjectFiles,
    git: MutatingGit<'a>,
    config: YamlConfigurationReader,
    markdown: PulldownMarkdownCodec,
    hasher: Xxh3Hasher,
    state: JsonStateStore<'static>,
    clock: SystemClock,
    locks: LockFileCoordinator,
    writer: AtomicFileWriter<'a>,
    packet_input: FsPacketInput,
    skills: FsSkillStore<'static>,
    progress: Quiet,
}

fn harness<'a>(
    root: &Path,
    writer_faults: memoria_infrastructure::fs::FaultHook<'a>,
    mutate_on: &'a dyn Fn(u32),
) -> Harness<'a> {
    Harness {
        root: root.to_path_buf(),
        files: FsProjectFiles::new(root.to_path_buf()),
        git: MutatingGit {
            inner: GitCli::discover(root).unwrap(),
            calls: Cell::new(0),
            mutate_on,
        },
        config: YamlConfigurationReader,
        markdown: PulldownMarkdownCodec,
        hasher: Xxh3Hasher,
        state: JsonStateStore::new(root.to_path_buf()),
        clock: SystemClock,
        locks: LockFileCoordinator::new(root.to_path_buf()),
        writer: AtomicFileWriter::with_faults(root.to_path_buf(), writer_faults),
        packet_input: FsPacketInput,
        skills: FsSkillStore::new(root.to_path_buf(), "# skill\n", "0.1.0"),
        progress: Quiet,
    }
}

impl Harness<'_> {
    fn services<'s>(&'s self, packets: &'s JsonPacketCodec<'s>) -> Services<'s> {
        Services {
            files: &self.files,
            git: &self.git,
            config: &self.config,
            markdown: &self.markdown,
            hasher: &self.hasher,
            state: &self.state,
            clock: &self.clock,
            locks: &self.locks,
            writer: &self.writer,
            packets,
            packet_input: &self.packet_input,
            skills: &self.skills,
            progress: &self.progress,
        }
    }
}

#[test]
fn partial_render_failure_reports_applied_and_unapplied_documents_and_reruns_safely() {
    let dir = seed();
    let hook = |step: &str| {
        if step == "replace:b/README.md" {
            Some(std::io::Error::other("injected replace"))
        } else {
            None
        }
    };
    let quiet = |_: u32| {};
    let h = harness(dir.path(), &hook, &quiet);
    let codec = JsonPacketCodec::new(&h.hasher);
    let services = h.services(&codec);
    usecases::init::run(&services).unwrap();
    let err = usecases::render::run(&services, None, false).unwrap_err();
    let codes: Vec<&str> = err.diagnostics.iter().map(|d| d.code.as_str()).collect();
    assert!(
        codes.contains(&"io_error") && codes.contains(&"render_incomplete"),
        "{codes:?}"
    );
    let incomplete = err
        .diagnostics
        .iter()
        .find(|d| d.code == "render_incomplete")
        .unwrap();
    assert!(
        incomplete.message.contains("b/README.md"),
        "{}",
        incomplete.message
    );
    assert!(
        fs::read_to_string(h.root.join("a/README.md"))
            .unwrap()
            .contains("Summary c."),
        "the first document was applied"
    );
    assert!(
        !fs::read_to_string(h.root.join("b/README.md"))
            .unwrap()
            .contains("Summary c."),
        "the failed document is untouched"
    );
    // Data lists which documents were applied.
    let applied: Vec<(String, bool)> = match err.data.get("changes") {
        Some(memoria_application::error::Detail::List(items)) => items
            .iter()
            .map(|c| {
                (
                    match c.get("document") {
                        Some(memoria_application::error::Detail::Text(t)) => t.clone(),
                        _ => String::new(),
                    },
                    matches!(
                        c.get("applied"),
                        Some(memoria_application::error::Detail::Bool(true))
                    ),
                )
            })
            .collect(),
        _ => panic!("no changes in data"),
    };
    assert!(applied.contains(&("a/README.md".to_string(), true)));
    assert!(applied.contains(&("b/README.md".to_string(), false)));
    assert!(!fs::read_dir(h.root.join("b")).unwrap().any(|e| {
        e.unwrap()
            .file_name()
            .to_string_lossy()
            .contains("memoria-tmp")
    }));
    // No review state changed, and a plain rerun finishes the remaining documents.
    assert!(h.state.load().unwrap().unwrap().state.reviews.is_empty());
    let plain = harness(dir.path(), memoria_infrastructure::fs::NO_FAULTS, &quiet);
    let codec = JsonPacketCodec::new(&plain.hasher);
    let services = plain.services(&codec);
    let outcome = usecases::render::run(&services, None, false).unwrap();
    let changed: Vec<String> = outcome
        .data
        .changed()
        .map(|d| d.document.as_str().to_string())
        .collect();
    assert_eq!(
        changed,
        vec!["b/README.md"],
        "only the unapplied document remains"
    );
    assert!(
        fs::read_to_string(plain.root.join("README.md"))
            .unwrap()
            .contains("Summary a.")
    );
    assert!(
        fs::read_to_string(plain.root.join("b/README.md"))
            .unwrap()
            .contains("Summary c.")
    );
    let again = usecases::render::run(&services, None, false).unwrap();
    assert_eq!(again.data.changed().count(), 0);
}

#[test]
fn a_change_between_scan_passes_is_retried_once_and_repeated_change_is_rejected() {
    let dir = seed();
    let quiet = |_: u32| {};
    {
        let h = harness(dir.path(), memoria_infrastructure::fs::NO_FAULTS, &quiet);
        let codec = JsonPacketCodec::new(&h.hasher);
        let services = h.services(&codec);
        usecases::init::run(&services).unwrap();
        usecases::render::run(&services, None, false).unwrap();
    }
    // One mutation between the first and second pass: the retry produces a stable snapshot.
    let root = dir.path().to_path_buf();
    let once = move |call: u32| {
        if call == 2 {
            fs::write(root.join("c/extra.rs"), "// appeared between passes\n").unwrap();
        }
    };
    let h = harness(dir.path(), memoria_infrastructure::fs::NO_FAULTS, &once);
    let codec = JsonPacketCodec::new(&h.hasher);
    let services = h.services(&codec);
    let plan = usecases::plan::run(&services).unwrap();
    assert!(
        h.git.calls.get() >= 3,
        "a third pass ran after the mutation"
    );
    assert!(plan.data.tasks.len() == 4);
    // A mutation on every pass never stabilizes: conflict, no writes.
    let root = dir.path().to_path_buf();
    let counter = Cell::new(0u32);
    let always = move |call: u32| {
        counter.set(call);
        fs::write(root.join("c/extra.rs"), format!("// pass {call}\n")).unwrap();
    };
    let h = harness(dir.path(), memoria_infrastructure::fs::NO_FAULTS, &always);
    let codec = JsonPacketCodec::new(&h.hasher);
    let services = h.services(&codec);
    let err: AppError =
        usecases::invalidate::run(&services, "all", "Repeated instability must be rejected.")
            .unwrap_err();
    assert_eq!(err.diagnostics[0].code, "snapshot_changed");
    assert_eq!(err.class.code(), 3);
    assert!(
        h.state
            .load()
            .unwrap()
            .unwrap()
            .state
            .invalidations
            .is_empty(),
        "no state was written"
    );
}
