//! Shared harness for binary-level tests: seeded Git fixtures, JSON parsing,
//! packet capture outside the project, and filesystem snapshots.
#![allow(dead_code)]

use std::collections::BTreeMap;
use std::fs;
use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};

use memoria_infrastructure::json::{self, Json, Limits};
use tempfile::TempDir;

pub const NOTE: &str = "The current summary describes all reviewed inputs.";

pub fn memoria_bin() -> &'static str {
    env!("CARGO_BIN_EXE_memoria")
}

/// Apply the isolated Git environment to a command.
///
/// The system and global configuration files are replaced by `/dev/null`,
/// so no host setting reaches a fixture. A developer whose global
/// configuration enables commit signing, a custom `core.excludesFile`, or a
/// template directory still runs the same tests as CI.
pub fn isolate_git<'a>(command: &'a mut Command, home: &Path) -> &'a mut Command {
    command
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_CONFIG_SYSTEM", "/dev/null")
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("HOME", home)
        .env("XDG_CONFIG_HOME", home.join(".config"))
        // Both one-shot configuration mechanisms are cleared too, so an
        // outer environment cannot reintroduce a host setting that
        // `GIT_CONFIG_GLOBAL` alone would not override.
        .env_remove("GIT_CONFIG_COUNT")
        .env_remove("GIT_CONFIG_PARAMETERS")
        .env_remove("GIT_AUTHOR_NAME")
        .env_remove("GIT_AUTHOR_EMAIL")
        .env_remove("GIT_COMMITTER_NAME")
        .env_remove("GIT_COMMITTER_EMAIL")
        .env("GIT_AUTHOR_NAME", "Fixture")
        .env("GIT_AUTHOR_EMAIL", "fixture@example.com")
        .env("GIT_COMMITTER_NAME", "Fixture")
        .env("GIT_COMMITTER_EMAIL", "fixture@example.com")
}

/// Run one Git command in `directory` with the isolated environment.
pub fn git_isolated(directory: &Path, home: &Path, args: &[&str]) -> Output {
    let mut command = Command::new("git");
    command.arg("-C").arg(directory).args(args);
    isolate_git(&mut command, home);
    let output = command.output().unwrap();
    assert!(
        output.status.success(),
        "git {args:?} in {} failed: {}",
        directory.display(),
        String::from_utf8_lossy(&output.stderr)
    );
    output
}

/// Seed a bare fixture repository at `root` with the isolated environment.
pub fn seed_repository(root: &Path, home: &Path) {
    git_isolated(root, home, &["init", "-q"]);
    git_isolated(root, home, &["config", "user.email", "fixture@example.com"]);
    git_isolated(root, home, &["config", "user.name", "Fixture"]);
    git_isolated(root, home, &["config", "commit.gpgsign", "false"]);
    git_isolated(root, home, &["add", "-A"]);
    git_isolated(root, home, &["commit", "-q", "-m", "seed"]);
}

pub struct Project {
    pub dir: TempDir,
    pub root: PathBuf,
    pub packets: TempDir,
    /// An isolated home and XDG configuration directory. Tests that
    /// exercise host ignore rules override these locations explicitly.
    pub home: TempDir,
}

/// Fixture documents are stored under placeholder names so that this
/// repository's own Memoria run (the self-demo) does not treat the sample
/// project as documentation boundaries; seeding restores the real names.
fn fixture_name(name: &str) -> String {
    match name {
        "README.fixture.md" => "README.md".to_string(),
        "README.memoria.fixture.toml" => "README.memoria.toml".to_string(),
        other => other.to_string(),
    }
}

fn copy_tree(from: &Path, to: &Path) {
    for entry in fs::read_dir(from).unwrap() {
        let entry = entry.unwrap();
        let target = to.join(fixture_name(&entry.file_name().to_string_lossy()));
        if entry.file_type().unwrap().is_dir() {
            fs::create_dir_all(&target).unwrap();
            copy_tree(&entry.path(), &target);
        } else {
            fs::copy(entry.path(), &target).unwrap();
        }
    }
}

impl Project {
    /// Copy the three-level fixture into a fresh local Git repository.
    pub fn seed() -> Project {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().canonicalize().unwrap();
        let fixture = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/three-level");
        copy_tree(&fixture, &root);
        let project = Project {
            dir,
            root,
            packets: tempfile::tempdir().unwrap(),
            home: tempfile::tempdir().unwrap(),
        };
        project.git(&["init", "-q"]);
        project.git(&["config", "user.email", "fixture@example.com"]);
        project.git(&["config", "user.name", "Fixture"]);
        project.git(&["config", "commit.gpgsign", "false"]);
        project.git(&["add", "-A"]);
        project.git(&["commit", "-q", "-m", "seed"]);
        project
    }

    /// An empty Git repository with no commits.
    pub fn empty_repo() -> Project {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().canonicalize().unwrap();
        let project = Project {
            dir,
            root,
            packets: tempfile::tempdir().unwrap(),
            home: tempfile::tempdir().unwrap(),
        };
        project.git(&["init", "-q"]);
        project.git(&["config", "user.email", "fixture@example.com"]);
        project.git(&["config", "user.name", "Fixture"]);
        project.git(&["config", "commit.gpgsign", "false"]);
        project
    }

    /// Run one Git command in this project with the isolated environment.
    /// No host global or system configuration reaches a fixture.
    pub fn git(&self, args: &[&str]) -> Output {
        git_isolated(&self.root, self.home.path(), args)
    }

    /// Run one Git command in another directory, still isolated from the
    /// host. Tests that build a second repository use this.
    pub fn git_in(&self, directory: &Path, args: &[&str]) -> Output {
        git_isolated(directory, self.home.path(), args)
    }

    /// A command with this project's isolated Git and home environment.
    pub fn isolated_command(&self, program: &str) -> Command {
        let mut command = Command::new(program);
        isolate_git(&mut command, self.home.path());
        command
    }

    pub fn commit_all(&self, message: &str) {
        self.git(&["add", "-A"]);
        self.git(&["commit", "-q", "--allow-empty", "-m", message]);
    }

    pub fn command(&self, cwd: &Path, args: &[&str]) -> Command {
        let mut cmd = Command::new(memoria_bin());
        cmd.current_dir(cwd)
            .args(args)
            // Every test runs with an isolated home and XDG configuration,
            // so a developer's own Git or agent settings never reach a
            // fixture. Tests that exercise host rules override these.
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env("GIT_CONFIG_SYSTEM", "/dev/null")
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("HOME", self.home.path())
            .env("XDG_CONFIG_HOME", self.home.path().join(".config"))
            .env_remove("GIT_CONFIG_COUNT")
            .env_remove("GIT_CONFIG_PARAMETERS")
            .env_remove("CLAUDE_CONFIG_DIR")
            .env_remove("CODEX_HOME")
            .stdin(Stdio::null());
        cmd
    }

    pub fn run(&self, args: &[&str]) -> Output {
        self.command(&self.root, args).output().unwrap()
    }

    pub fn run_in(&self, cwd: &Path, args: &[&str]) -> Output {
        self.command(cwd, args).output().unwrap()
    }

    pub fn run_stdin(&self, args: &[&str], input: &[u8]) -> Output {
        let mut child = self
            .command(&self.root, args)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        let mut stdin = child.stdin.take().unwrap();
        let input = input.to_vec();
        let writer = std::thread::spawn(move || {
            let _ = stdin.write_all(&input);
        });
        let output = child.wait_with_output().unwrap();
        writer.join().unwrap();
        output
    }

    /// Run with `--format json`, returning the exit code and parsed envelope.
    pub fn json(&self, args: &[&str]) -> (i32, Json) {
        let mut full: Vec<&str> = args.to_vec();
        full.push("--format");
        full.push("json");
        let output = self.run(&full);
        let code = output.status.code().unwrap();
        let value = json::parse(&output.stdout, Limits::STATE).unwrap_or_else(|e| {
            panic!(
                "invalid JSON from {args:?}: {e}\n{}",
                String::from_utf8_lossy(&output.stdout)
            )
        });
        (code, value)
    }

    pub fn write(&self, relative: &str, content: impl AsRef<[u8]>) {
        let path = self.root.join(relative);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, content).unwrap();
    }

    pub fn append(&self, relative: &str, content: &str) {
        let path = self.root.join(relative);
        let mut existing = fs::read(&path).unwrap();
        existing.extend_from_slice(content.as_bytes());
        fs::write(path, existing).unwrap();
    }

    pub fn read(&self, relative: &str) -> Vec<u8> {
        fs::read(self.root.join(relative)).unwrap()
    }

    pub fn read_string(&self, relative: &str) -> String {
        String::from_utf8(self.read(relative)).unwrap()
    }

    /// The committed state rendered as inspection text, for substring checks
    /// that used to read the JSON state file directly.
    pub fn state_text(&self) -> String {
        json::to_pretty(&self.inspect_state())
    }

    pub fn remove(&self, relative: &str) {
        fs::remove_file(self.root.join(relative)).unwrap();
    }

    pub fn exists(&self, relative: &str) -> bool {
        self.root.join(relative).exists()
    }

    /// The committed state bytes.
    pub fn state(&self) -> Vec<u8> {
        self.read("memoria.lock")
    }

    /// The worktree-private write lock path.
    pub fn write_lock(&self) -> PathBuf {
        self.root.join(".git/memoria/write.lock")
    }

    /// The committed state, decoded through the production codec.
    pub fn decoded_state(&self) -> memoria_domain::ReviewState {
        memoria_infrastructure::lock_codec::decode(&self.state())
            .expect("the committed state decodes")
            .state
    }

    /// Write a state that a decoder must reject for a semantic reason, with
    /// framing and checksum that are themselves valid. This reaches the
    /// invariant checks instead of stopping at the frame.
    pub fn write_state(&self, state: &memoria_domain::ReviewState) -> Vec<u8> {
        // A deliberately impossible state must still reach disk, so the
        // decoder can reject it. Product writes use the verified encoder,
        // which refuses exactly these bytes.
        let bytes = memoria_infrastructure::lock_codec::encode_unverified(state)
            .expect("the fixture state encodes");
        self.write("memoria.lock", &bytes);
        bytes
    }

    /// The complete `state inspect` envelope.
    pub fn inspect_envelope(&self) -> Json {
        let (code, value) = self.json(&["state", "inspect"]);
        assert_eq!(code, 0, "state inspect failed: {}", json::to_pretty(&value));
        value
    }

    /// The decoded review state, as read-only inspection publishes it.
    pub fn inspect_state(&self) -> Json {
        get(&self.inspect_envelope(), &["data", "state"]).clone()
    }

    /// Capture a focused packet outside the project. Returns the packet path
    /// and its 21-byte token.
    pub fn review_packet(&self, document: &str) -> (PathBuf, String) {
        let output = self.run(&["review", document, "--format", "json"]);
        assert_eq!(
            output.status.code(),
            Some(0),
            "review {document} failed: {}{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        let name = document.replace('/', "_");
        let path = self.packets.path().join(format!(
            "{name}-{}.json",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::write(&path, &output.stdout).unwrap();
        let value = json::parse(&output.stdout, Limits::STATE).unwrap();
        let token = get_str(&value, &["data", "token"]).to_string();
        assert_eq!(token.len(), 21, "token must be 21 ASCII bytes");
        assert!(token.is_ascii());
        (path, token)
    }

    pub fn ack(
        &self,
        document: &str,
        packet: &Path,
        token: &str,
        result: &str,
        note: &str,
    ) -> Output {
        self.run(&[
            "ack",
            document,
            "--packet",
            packet.to_str().unwrap(),
            "--token",
            token,
            "--reviewer",
            "fixture",
            "--result",
            result,
            "--note",
            note,
        ])
    }

    /// Acknowledge with the JSON envelope, for diagnostic assertions.
    pub fn ack_json(
        &self,
        document: &str,
        packet: &Path,
        token: &str,
        result: &str,
        note: &str,
    ) -> (i32, Json) {
        let output = self.run(&[
            "ack",
            document,
            "--packet",
            packet.to_str().unwrap(),
            "--token",
            token,
            "--reviewer",
            "fixture",
            "--result",
            result,
            "--note",
            note,
            "--format",
            "json",
        ]);
        (
            output.status.code().unwrap(),
            json::parse(&output.stdout, Limits::STATE)
                .unwrap_or_else(|e| panic!("invalid JSON from ack: {e}\n{}", stdout(&output))),
        )
    }

    pub fn ack_ok(&self, document: &str) {
        let (packet, token) = self.review_packet(document);
        let output = self.ack(document, &packet, &token, "no-update", NOTE);
        assert_eq!(
            output.status.code(),
            Some(0),
            "ack {document} failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }

    pub fn plan(&self) -> Json {
        let (code, value) = self.json(&["review"]);
        assert_eq!(code, 0, "plan failed: {}", json::to_pretty(&value));
        value
    }

    pub fn next_ready(&self) -> Option<String> {
        let plan = self.plan();
        match get(&plan, &["data", "next_ready"]) {
            Json::String(s) => Some(s.clone()),
            _ => None,
        }
    }

    /// The plan's next executable step: `(kind, document)`.
    pub fn next_action(&self) -> Option<(String, String)> {
        let plan = self.plan();
        match get(&plan, &["data", "next_action"]) {
            Json::Object(_) => {
                let action = get(&plan, &["data", "next_action"]);
                Some((
                    get_str(action, &["kind"]).to_string(),
                    get_str(action, &["document"]).to_string(),
                ))
            }
            _ => None,
        }
    }

    /// Follow the canonical skill procedure: render when the plan says so,
    /// otherwise review and acknowledge, until no task remains.
    pub fn canonical_loop(&self) -> Vec<String> {
        let mut steps = Vec::new();
        for _ in 0..32 {
            match self.next_action() {
                Some((kind, document)) if kind == "render" => {
                    assert_eq!(self.run(&["render", &document]).status.code(), Some(0));
                    steps.push(format!("render {document}"));
                }
                Some((_, document)) => {
                    self.ack_ok(&document);
                    steps.push(format!("ack {document}"));
                }
                None => break,
            }
        }
        steps
    }

    /// init, render, acknowledge everything in order, and assert `check` passes.
    pub fn baseline(&self) {
        // A root README is authored before Memoria writes anything.
        if !self.exists("README.md") {
            self.write(
                "README.md",
                "# Fixture\n\nThis README owns every selected file that no nearer README explains.\n",
            );
        }
        assert_eq!(self.run(&["init", "--apply"]).status.code(), Some(0));
        assert_eq!(self.run(&["render"]).status.code(), Some(0));
        self.canonical_loop();
        let (code, value) = self.json(&["check"]);
        assert_eq!(code, 0, "check failed: {}", json::to_pretty(&value));
    }

    /// Document status entry from `status --format json`.
    pub fn doc_status(&self, document: &str) -> Json {
        let (_, value) = self.json(&["status"]);
        let docs = get(&value, &["data", "documents"]);
        let Json::Array(items) = docs else {
            panic!("documents is not an array")
        };
        items
            .iter()
            .find(|d| get_str(d, &["document"]) == document)
            .cloned()
            .unwrap_or_else(|| panic!("no status for {document}"))
    }

    pub fn status_label(&self, document: &str) -> String {
        get_str(&self.doc_status(document), &["status"]).to_string()
    }

    pub fn waiting_on(&self, document: &str) -> Vec<String> {
        strings(get(&self.doc_status(document), &["waiting_on"]))
    }

    pub fn cause_codes(&self, document: &str) -> Vec<String> {
        let status = self.doc_status(document);
        let Json::Array(causes) = get(&status, &["causes"]) else {
            return vec![];
        };
        causes
            .iter()
            .map(|c| get_str(c, &["code"]).to_string())
            .collect()
    }

    /// Snapshot of every file: relative path -> (bytes, modification time).
    pub fn tree_snapshot(&self) -> BTreeMap<String, (Vec<u8>, std::time::SystemTime)> {
        let mut out = BTreeMap::new();
        fn walk(
            base: &Path,
            dir: &Path,
            out: &mut BTreeMap<String, (Vec<u8>, std::time::SystemTime)>,
        ) {
            for entry in fs::read_dir(dir).unwrap() {
                let entry = entry.unwrap();
                let path = entry.path();
                let meta = fs::symlink_metadata(&path).unwrap();
                if meta.is_dir() {
                    walk(base, &path, out);
                } else if meta.is_file() {
                    let relative = path
                        .strip_prefix(base)
                        .unwrap()
                        .to_string_lossy()
                        .into_owned();
                    out.insert(
                        relative,
                        (fs::read(&path).unwrap(), meta.modified().unwrap()),
                    );
                }
            }
        }
        walk(&self.root, &self.root, &mut out);
        out
    }
}

pub fn get<'a>(value: &'a Json, path: &[&str]) -> &'a Json {
    let mut current = value;
    for key in path {
        current = match current {
            Json::Object(map) => map
                .get(*key)
                .unwrap_or_else(|| panic!("missing key {key} in {}", json::to_compact(current))),
            _ => panic!("expected object at {key}"),
        };
    }
    current
}

pub fn get_str<'a>(value: &'a Json, path: &[&str]) -> &'a str {
    match get(value, path) {
        Json::String(s) => s,
        other => panic!(
            "expected string at {path:?}, got {}",
            json::to_compact(other)
        ),
    }
}

pub fn get_u64(value: &Json, path: &[&str]) -> u64 {
    match get(value, path) {
        Json::Number(n) => *n,
        other => panic!(
            "expected number at {path:?}, got {}",
            json::to_compact(other)
        ),
    }
}

pub fn get_bool(value: &Json, path: &[&str]) -> bool {
    match get(value, path) {
        Json::Bool(b) => *b,
        other => panic!("expected bool at {path:?}, got {}", json::to_compact(other)),
    }
}

pub fn strings(value: &Json) -> Vec<String> {
    match value {
        Json::Array(items) => items
            .iter()
            .map(|i| {
                if let Json::String(s) = i {
                    s.clone()
                } else {
                    panic!("not a string")
                }
            })
            .collect(),
        _ => panic!("not an array"),
    }
}

pub fn numbers(value: &Json) -> Vec<u64> {
    match value {
        Json::Array(items) => items
            .iter()
            .map(|i| {
                if let Json::Number(n) = i {
                    *n
                } else {
                    panic!("not a number")
                }
            })
            .collect(),
        _ => panic!("not an array"),
    }
}

pub fn diagnostic_codes(value: &Json) -> Vec<String> {
    let Json::Array(items) = get(value, &["diagnostics"]) else {
        return vec![];
    };
    items
        .iter()
        .map(|d| get_str(d, &["code"]).to_string())
        .collect()
}

pub fn stdout(output: &Output) -> String {
    String::from_utf8_lossy(&output.stdout).into_owned()
}

pub fn stderr(output: &Output) -> String {
    String::from_utf8_lossy(&output.stderr).into_owned()
}

pub fn parse_json(bytes: &[u8]) -> Json {
    json::parse(bytes, Limits::STATE).unwrap()
}

pub fn set_path(value: &mut Json, path: &[&str], new: Json) {
    let mut current = value;
    for (index, key) in path.iter().enumerate() {
        let Json::Object(map) = current else {
            panic!("expected object")
        };
        if index == path.len() - 1 {
            map.insert((*key).to_string(), new);
            return;
        }
        current = map.get_mut(*key).unwrap();
    }
}
