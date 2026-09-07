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

pub struct Project {
    pub dir: TempDir,
    pub root: PathBuf,
    pub packets: TempDir,
}

/// Fixture documents are stored under placeholder names so that this
/// repository's own Memoria run (the self-demo) does not treat the sample
/// project as documentation boundaries; seeding restores the real names.
fn fixture_name(name: &str) -> String {
    match name {
        "README.fixture.md" => "README.md".to_string(),
        "README.memoria.fixture.yml" => "README.memoria.yml".to_string(),
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
        };
        project.git(&["init", "-q"]);
        project.git(&["config", "user.email", "fixture@example.com"]);
        project.git(&["config", "user.name", "Fixture"]);
        project.git(&["config", "commit.gpgsign", "false"]);
        project
    }

    pub fn git(&self, args: &[&str]) -> Output {
        let output = Command::new("git")
            .arg("-C")
            .arg(&self.root)
            .args(args)
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "git {args:?} failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        output
    }

    pub fn commit_all(&self, message: &str) {
        self.git(&["add", "-A"]);
        self.git(&["commit", "-q", "--allow-empty", "-m", message]);
    }

    pub fn command(&self, cwd: &Path, args: &[&str]) -> Command {
        let mut cmd = Command::new(memoria_bin());
        cmd.current_dir(cwd)
            .args(args)
            .env("GIT_CONFIG_NOSYSTEM", "1")
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

    pub fn remove(&self, relative: &str) {
        fs::remove_file(self.root.join(relative)).unwrap();
    }

    pub fn exists(&self, relative: &str) -> bool {
        self.root.join(relative).exists()
    }

    pub fn state(&self) -> Vec<u8> {
        self.read(".memoria/state.json")
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
        assert_eq!(self.run(&["init"]).status.code(), Some(0));
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
