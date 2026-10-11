//! Batched Git object reads: call counts, framing of unusual file names,
//! replaced, missing, and filtered objects, and raw-byte dirtiness.
//!
//! A logging `git` shim on `PATH` records every Git invocation, so these
//! tests count processes directly instead of timing them.

use crate::common;

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

use common::*;
use memoria_infrastructure::json::Json;

fn simple(committed: bool) -> Project {
    let p = Project::empty_repo();
    p.write(
        "README.md",
        "# Limits\n\nThe request limit is ten. Read limits.rs for the contract.\n",
    );
    p.write("limits.rs", "pub const LIMIT: usize = 10;\n");
    p.write("request.rs", "// Requests use the documented limit.\n");
    if committed {
        p.commit_all("source baseline");
    }
    p.baseline();
    p
}

/// A `git` on `PATH` that logs its arguments, one invocation per line, and
/// then runs the real Git.
struct Shim {
    _dir: tempfile::TempDir,
    log: PathBuf,
    path: String,
}

impl Shim {
    fn new() -> Shim {
        Shim::with_batch("")
    }

    /// A shim whose `cat-file --batch` runs `batch` instead of Git, when
    /// `batch` is not empty. Every other command runs the real Git.
    fn with_batch(batch: &str) -> Shim {
        let real = std::process::Command::new("sh")
            .args(["-c", "command -v git"])
            .output()
            .unwrap();
        let real = String::from_utf8(real.stdout).unwrap().trim().to_string();
        assert!(!real.is_empty(), "git is on PATH");
        let dir = tempfile::tempdir().unwrap();
        let log = dir.path().join("calls.log");
        let script = dir.path().join("git");
        fs::write(
            &script,
            format!(
                "#!/bin/sh\nprintf '%s\\n' \"$*\" >> '{}'\n{}exec '{}' \"$@\"\n",
                log.display(),
                if batch.is_empty() {
                    String::new()
                } else {
                    format!("case \"$*\" in *'cat-file --batch'*) {batch} ;; esac\n")
                },
                real
            ),
        )
        .unwrap();
        fs::set_permissions(&script, fs::Permissions::from_mode(0o755)).unwrap();
        let path = format!(
            "{}:{}",
            dir.path().display(),
            std::env::var("PATH").unwrap_or_default()
        );
        Shim {
            _dir: dir,
            log,
            path,
        }
    }

    /// Run memoria with the shim and return its output and the Git calls.
    fn run(&self, project: &Project, args: &[&str]) -> (std::process::Output, Vec<String>) {
        let _ = fs::remove_file(&self.log);
        let output = project
            .command(&project.root, args)
            .env("PATH", &self.path)
            .output()
            .unwrap();
        let calls = fs::read_to_string(&self.log)
            .unwrap_or_default()
            .lines()
            .map(str::to_string)
            .collect();
        (output, calls)
    }
}

fn count(calls: &[String], needle: &str) -> usize {
    calls.iter().filter(|call| call.contains(needle)).count()
}

/// Historical reads run without an attribute source; raw dirtiness reads
/// run with one. The two readers are told apart by that argument.
fn history_batches(calls: &[String]) -> usize {
    calls
        .iter()
        .filter(|call| call.contains("cat-file --batch") && !call.contains("--attr-source"))
        .count()
}

fn dirtiness_batches(calls: &[String]) -> usize {
    calls
        .iter()
        .filter(|call| call.contains("cat-file --batch") && call.contains("--attr-source"))
        .count()
}

fn evidence(value: &Json) -> &Json {
    let Json::Array(items) = get(value, &["data", "evidence"]) else {
        panic!("no evidence list: {value:?}");
    };
    &items[0]
}

#[test]
fn historical_reads_share_one_batch_process_per_invocation() {
    let p = simple(false);
    // The reviewed bytes enter history, then twenty commits move away from
    // them, so the bounded search reads one candidate after another.
    p.commit_all("matching reviewed bytes");
    for i in 0..20 {
        p.write("request.rs", format!("// variant {i}\n"));
        p.commit_all(&format!("variant {i}"));
    }
    p.write("request.rs", "// current edit\n");
    let shim = Shim::new();

    let (output, calls) = shim.run(&p, &["explain", "README.md", "--format", "json"]);
    assert_eq!(output.status.code(), Some(0), "{}", stderr(&output));
    let value = parse_json(&output.stdout);
    let found = evidence(&value);
    assert!(get_bool(found, &["baseline_verified"]), "{found:?}");
    assert!(get_str(found, &["text"]).contains("+// current edit"));
    assert_eq!(history_batches(&calls), 1, "{calls:#?}");
    assert_eq!(count(&calls, "cat-file blob"), 0, "{calls:#?}");

    // The human review runs two historical searches; both share the one
    // process of this invocation.
    for args in [
        vec!["review", "README.md"],
        vec!["review", "README.md", "--format", "json"],
    ] {
        let (output, calls) = shim.run(&p, &args);
        assert_eq!(
            output.status.code(),
            Some(0),
            "{args:?}: {}",
            stderr(&output)
        );
        assert!(history_batches(&calls) <= 1, "{args:?}: {calls:#?}");
        assert_eq!(count(&calls, "cat-file blob"), 0, "{args:?}: {calls:#?}");
    }
}

#[test]
fn acknowledgement_coverage_reads_every_input_through_one_process() {
    let p = Project::empty_repo();
    p.write(
        "README.md",
        "# Many\n\nEvery file below is explained here.\n",
    );
    for i in 0..40 {
        p.write(
            &format!("src/file{i:02}.rs"),
            format!("pub fn f{i}() {{}}\n"),
        );
    }
    p.commit_all("sources");
    let shim = Shim::new();
    assert_eq!(p.run(&["init", "--apply"]).status.code(), Some(0));
    assert_eq!(p.run(&["render"]).status.code(), Some(0));
    p.commit_all("init");
    let (packet, token) = p.review_full("README.md");
    let _ = fs::remove_file(&shim.log);
    let output = p
        .command(
            &p.root,
            &[
                "ack",
                "README.md",
                "--packet",
                packet.to_str().unwrap(),
                "--token",
                &token,
                "--reviewer",
                "test",
                "--result",
                "no-update",
                "--note",
                NOTE,
                "--format",
                "json",
            ],
        )
        .env("PATH", &shim.path)
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(0), "{}", stdout(&output));
    let calls: Vec<String> = fs::read_to_string(&shim.log)
        .unwrap()
        .lines()
        .map(str::to_string)
        .collect();
    assert_eq!(history_batches(&calls), 1, "{calls:#?}");
    assert_eq!(count(&calls, "cat-file blob"), 0, "{calls:#?}");
    let doc = memoria_domain::DocumentId::parse("README.md").unwrap();
    assert!(
        p.decoded_state().reviews[&doc].git.base_commit.is_some(),
        "all 41 inputs were verified at HEAD"
    );
}

#[test]
fn unusual_supported_file_names_are_read_exactly() {
    let names = [
        "a b.rs",
        " leading space.rs",
        "trailing space .rs",
        "-starts-with-dash.rs",
        "colon:inside.rs",
        "hash#and%percent.rs",
        "quote'and\"double.rs",
        "ünïcødé ✓.rs",
        "dir with space/inner.rs",
        "missing",
        "x ambiguous",
    ];
    let p = Project::empty_repo();
    p.write(
        "README.md",
        "# Names\n\nEvery file below is explained here.\n",
    );
    for name in names {
        p.write(name, format!("content of {name}\n"));
    }
    p.commit_all("unusual names");
    p.baseline();
    let doc = memoria_domain::DocumentId::parse("README.md").unwrap();
    let state = p.decoded_state();
    let record = &state.reviews[&doc];
    assert!(
        record.git.base_commit.is_some(),
        "every unusual name was read from HEAD and matched"
    );
    // A later change is diffed against the bytes read for that name.
    p.write("colon:inside.rs", "changed content\n");
    p.commit_all("change one name");
    p.write("colon:inside.rs", "changed content\nand an edit\n");
    let (_, value) = p.json(&["explain", "README.md"]);
    let found = evidence(&value);
    assert!(get_bool(found, &["baseline_verified"]), "{found:?}");

    // Raw-byte dirtiness reads the same names by object ID.
    p.git(&["add", "-A"]);
    p.git(&["commit", "-qm", "clean"]);
    p.write(".git/info/attributes", "unused filter=anything\n");
    assert!(!dirty(&p), "clean tree");
    p.write("ünïcødé ✓.rs", "content of ünïcødé ✓.rX\n");
    assert!(dirty(&p), "same-size edit");
}

#[test]
fn historical_reads_ignore_replacement_objects() {
    let p = simple(true);
    let doc = memoria_domain::DocumentId::parse("README.md").unwrap();
    assert!(p.decoded_state().reviews[&doc].git.base_commit.is_some());
    let original = stdout(&p.git(&["rev-parse", "HEAD:request.rs"]))
        .trim()
        .to_string();
    let other = p.root.join("other-bytes");
    fs::write(&other, "// Replacement bytes that were never reviewed.\n").unwrap();
    let replacement = stdout(&p.git(&["hash-object", "-w", other.to_str().unwrap()]))
        .trim()
        .to_string();
    fs::remove_file(&other).unwrap();
    p.git(&["replace", &original, &replacement]);
    p.write("request.rs", "// A committed change.\n");
    p.commit_all("change");
    p.append("request.rs", "// And an edit.\n");
    let (_, value) = p.json(&["explain", "README.md"]);
    let found = evidence(&value);
    assert!(
        get_bool(found, &["baseline_verified"]),
        "the original object, not its replacement, matches the review: {found:?}"
    );
    assert!(get_str(found, &["text"]).contains("-// Requests use the documented limit."));
}

#[test]
fn historical_and_raw_reads_never_run_filters() {
    let p = simple(false);
    p.commit_all("matching reviewed bytes");
    p.write("request.rs", "// later bytes\n");
    p.commit_all("later");
    p.write("request.rs", "// current edit\n");
    let helpers = tempfile::tempdir().unwrap();
    let sentinel = helpers.path().join("filter-ran");
    let helper = helpers.path().join("probe.sh");
    fs::write(
        &helper,
        format!("#!/bin/sh\nprintf ran >> '{}'\ncat\n", sentinel.display()),
    )
    .unwrap();
    fs::set_permissions(&helper, fs::Permissions::from_mode(0o755)).unwrap();
    for key in [
        "filter.probe.clean",
        "filter.probe.smudge",
        "filter.probe.process",
        "diff.probe.textconv",
    ] {
        p.git(&["config", key, helper.to_str().unwrap()]);
    }
    // `info/attributes` is the one attribute file that `--attr-source`
    // cannot override, and it is outside the project inputs. A committed
    // `.gitattributes` is covered by `inspection_never_executes_configured_clean_filters`.
    p.write(".git/info/attributes", "*.rs filter=probe diff=probe\n");
    let shim = Shim::new();
    for args in [
        vec!["explain", "README.md", "--format", "json"],
        vec!["review", "README.md"],
        vec!["status", "--format", "json"],
    ] {
        let (output, calls) = shim.run(&p, &args);
        assert_eq!(
            output.status.code(),
            Some(0),
            "{args:?}: {}",
            stdout(&output)
        );
        assert!(!sentinel.exists(), "{args:?}: a filter ran");
        // The tracked edit is unstaged, so the raw comparison reads blobs.
        assert_eq!(dirtiness_batches(&calls), 1, "{args:?}: {calls:#?}");
    }
    let (_, value) = p.json(&["explain", "README.md"]);
    assert!(get_bool(evidence(&value), &["baseline_verified"]));
    assert!(!sentinel.exists());
}

/// An initialized, committed project whose `info/attributes` declares a
/// filter, so dirtiness takes the raw-byte path. `ignored` paths are
/// excluded from Memoria's inputs but stay tracked by Git.
fn raw_project(files: &[(&str, &[u8])], ignored: &[&str]) -> Project {
    let p = Project::empty_repo();
    p.write(
        "README.md",
        "# Raw\n\nThis README explains every file here.\n",
    );
    for (name, content) in files {
        p.write(name, content);
    }
    assert_eq!(p.run(&["init", "--apply"]).status.code(), Some(0));
    if !ignored.is_empty() {
        let rules: Vec<String> = ignored.iter().map(|path| format!("{path:?}")).collect();
        let text = p.read_string("memoria.toml");
        assert!(text.contains("ignore = []"), "{text}");
        p.write(
            "memoria.toml",
            text.replacen(
                "ignore = []",
                &format!("ignore = [{}]", rules.join(", ")),
                1,
            ),
        );
    }
    p.commit_all("files");
    p.write(".git/info/attributes", "unused filter=anything\n");
    p
}

fn raw_repository(files: &[(&str, &[u8])]) -> Project {
    raw_project(files, &[])
}

/// The dirty flag of a full export, from the built binary with the
/// harness's isolated Git environment. The document is never acknowledged,
/// so it stays pending and can always be exported.
fn dirty(p: &Project) -> bool {
    let output = p.run(&["review", "README.md", "--full", "--format", "json"]);
    assert_eq!(output.status.code(), Some(0), "{}", stdout(&output));
    get_bool(
        &parse_json(&output.stdout),
        &["data", "context", "git", "worktree_dirty"],
    )
}

#[test]
fn raw_dirtiness_uses_one_batch_process_and_no_per_object_reads() {
    let mut files = vec![("README.md".to_string(), b"# Raw\n".to_vec())];
    for i in 0..60 {
        files.push((
            format!("src/f{i:02}.rs"),
            format!("fn f{i}() {{}}\n").into_bytes(),
        ));
    }
    let borrowed: Vec<(&str, &[u8])> = files
        .iter()
        .map(|(n, c)| (n.as_str(), c.as_slice()))
        .collect();
    // A clean tree: the raw comparison reads every blob.
    let p = raw_repository(&borrowed);
    let shim = Shim::new();
    let (output, calls) = shim.run(&p, &["status", "--format", "json"]);
    assert_eq!(output.status.code(), Some(0), "{}", stdout(&output));
    assert_eq!(dirtiness_batches(&calls), 1, "{calls:#?}");
    assert_eq!(count(&calls, "cat-file blob"), 0, "{calls:#?}");
}

#[test]
fn raw_dirtiness_detects_every_kind_of_change() {
    let large: Vec<u8> = (0..300_000u32).map(|i| (i % 251) as u8).collect();
    let files: Vec<(String, Vec<u8>)> = (0..300)
        .map(|i| {
            (
                format!("many/f{i:03}.txt"),
                format!("file {i}\n").into_bytes(),
            )
        })
        .chain([
            ("large.bin".to_string(), large.clone()),
            ("tool.sh".to_string(), b"#!/bin/sh\n".to_vec()),
            ("plain.txt".to_string(), b"plain\n".to_vec()),
        ])
        .collect();
    let borrowed: Vec<(&str, &[u8])> = files
        .iter()
        .map(|(n, c)| (n.as_str(), c.as_slice()))
        .collect();
    let p = raw_project(&borrowed, &["link"]);
    fs::set_permissions(p.root.join("tool.sh"), fs::Permissions::from_mode(0o755)).unwrap();
    std::os::unix::fs::symlink("plain.txt", p.root.join("link")).unwrap();
    p.git(&["add", "-A"]);
    p.git(&["commit", "-qm", "mode and link"]);
    assert!(
        !dirty(&p),
        "a clean tree of 300 files, a large file, and a link"
    );

    let restore = |p: &Project| {
        p.git(&["checkout", "-q", "--", "."]);
        assert!(!dirty(p), "restored");
    };
    // Same size, different bytes, in the last of many files.
    p.write("many/f299.txt", "file X99\n");
    assert!(dirty(&p), "same-size edit");
    restore(&p);
    // A different size.
    p.append("many/f000.txt", "more\n");
    assert!(dirty(&p), "size change");
    restore(&p);
    // The last byte of a file larger than one pipe chunk.
    let mut edited = large.clone();
    *edited.last_mut().unwrap() ^= 1;
    p.write("large.bin", &edited);
    assert!(dirty(&p), "last byte of a large file");
    restore(&p);
    // The executable bit.
    fs::set_permissions(p.root.join("tool.sh"), fs::Permissions::from_mode(0o644)).unwrap();
    assert!(dirty(&p), "mode change");
    fs::set_permissions(p.root.join("tool.sh"), fs::Permissions::from_mode(0o755)).unwrap();
    assert!(!dirty(&p));
    // A retargeted link, compared by link text only.
    fs::remove_file(p.root.join("link")).unwrap();
    std::os::unix::fs::symlink("many/f001.txt", p.root.join("link")).unwrap();
    assert!(dirty(&p), "retargeted link");
    fs::remove_file(p.root.join("link")).unwrap();
    std::os::unix::fs::symlink("plain.txt", p.root.join("link")).unwrap();
    assert!(!dirty(&p));
    // A deleted tracked file.
    fs::remove_file(p.root.join("plain.txt")).unwrap();
    assert!(dirty(&p), "deleted file");
    restore(&p);
}

#[test]
fn raw_dirtiness_reports_a_missing_index_object_as_a_change() {
    let p = raw_repository(&[("README.md", b"# Raw\n"), ("kept.txt", b"kept bytes\n")]);
    assert!(!dirty(&p));
    let oid = stdout(&p.git(&["rev-parse", ":kept.txt"]))
        .trim()
        .to_string();
    let object = p.root.join(".git/objects").join(&oid[..2]).join(&oid[2..]);
    fs::remove_file(object).unwrap();
    assert!(dirty(&p), "an unreadable index blob is never clean");
}

#[test]
fn raw_dirtiness_never_fetches_a_missing_object_from_a_promisor() {
    let p = raw_repository(&[("README.md", b"# Raw\n"), ("kept.txt", b"kept bytes\n")]);
    let helpers = tempfile::tempdir().unwrap();
    let sentinel = helpers.path().join("remote-helper-called");
    let helper = helpers.path().join("git-remote-reviewtest");
    fs::write(
        &helper,
        format!(
            "#!/bin/sh\nprintf called > '{}'\nexit 1\n",
            sentinel.display()
        ),
    )
    .unwrap();
    fs::set_permissions(&helper, fs::Permissions::from_mode(0o755)).unwrap();
    p.git(&["config", "remote.origin.url", "reviewtest::unused"]);
    p.git(&["config", "remote.origin.promisor", "true"]);
    p.git(&["config", "remote.origin.partialclonefilter", "blob:none"]);
    p.git(&["config", "extensions.partialClone", "origin"]);
    let oid = stdout(&p.git(&["rev-parse", ":kept.txt"]))
        .trim()
        .to_string();
    fs::remove_file(p.root.join(".git/objects").join(&oid[..2]).join(&oid[2..])).unwrap();
    let shim = Shim::new();
    let path = format!("{}:{}", helpers.path().display(), shim.path);
    let output = p
        .command(&p.root, &["status", "--format", "json"])
        .env("PATH", &path)
        .env("GIT_ALLOW_PROTOCOL", "reviewtest")
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(0), "{}", stdout(&output));
    let calls: Vec<String> = fs::read_to_string(&shim.log)
        .unwrap()
        .lines()
        .map(str::to_string)
        .collect();
    assert_eq!(dirtiness_batches(&calls), 1, "{calls:#?}");
    assert!(!sentinel.exists(), "the remote helper must never run");
}

#[test]
fn raw_dirtiness_reads_replaced_objects_as_the_per_object_read_did() {
    let p = raw_repository(&[("README.md", b"# Raw\n"), ("kept.txt", b"kept bytes\n")]);
    assert!(!dirty(&p));
    let original = stdout(&p.git(&["rev-parse", ":kept.txt"]))
        .trim()
        .to_string();
    let other = p.root.join(".git/other-bytes");
    fs::write(&other, "other bytes\n").unwrap();
    let replacement = stdout(&p.git(&["hash-object", "-w", other.to_str().unwrap()]))
        .trim()
        .to_string();
    p.git(&["replace", &original, &replacement]);
    // This pins the earlier behavior, not a new policy: the raw comparison
    // honors replacement objects exactly as `git cat-file blob` does.
    let reference = raw_dirtiness_without_batch(&p);
    assert!(reference, "the reference reads the replacement");
    assert_eq!(dirty(&p), reference);
}

#[test]
fn raw_dirtiness_keeps_nested_repositories_opaque() {
    let p = raw_repository(&[("README.md", b"# Raw\n"), ("nested/inner.txt", b"inner\n")]);
    assert!(!dirty(&p));
    let nested = p.root.join("nested");
    p.git_in(&nested, &["init", "-q"]);
    // The entry below a nested repository is skipped, as before batching.
    p.write("nested/inner.txt", "INNER\n");
    assert_eq!(dirty(&p), raw_dirtiness_without_batch(&p));
}

/// The raw-byte answer computed by one `git cat-file blob` per entry, with
/// the same rules the adapter applies. It is the reference for cases whose
/// answer is defined by the earlier per-object implementation.
fn raw_dirtiness_without_batch(p: &Project) -> bool {
    let root = p.root.as_path();
    let git = |args: &[&str]| {
        p.isolated_command("git")
            .arg("-C")
            .arg(root)
            .args(args)
            .output()
            .unwrap()
    };
    if !git(&["ls-files", "--others", "--exclude-standard", "-z"])
        .stdout
        .is_empty()
    {
        return true;
    }
    let index = git(&["ls-files", "--stage", "-z"]).stdout;
    for entry in index.split(|b| *b == 0).filter(|e| !e.is_empty()) {
        let text = String::from_utf8_lossy(entry);
        let (meta, path) = text.split_once('\t').unwrap();
        let oid = meta.split_whitespace().nth(1).unwrap();
        let nested = Path::new(path)
            .ancestors()
            .skip(1)
            .filter(|a| !a.as_os_str().is_empty())
            .any(|a| root.join(a).join(".git").exists());
        if nested {
            continue;
        }
        let blob = git(&["cat-file", "blob", oid]).stdout;
        if fs::read(root.join(path)).ok() != Some(blob) {
            return true;
        }
    }
    false
}

/// Batch reader failures that each discard the reader: a malformed header,
/// a header without its content, and a process that exits at once.
const BROKEN_READERS: [&str; 3] = [
    "printf 'not a header\\n'; exit 0",
    "printf '0123456789abcdef0123456789abcdef01234567 blob 999\\nabc'; exit 0",
    "exit 1",
];

#[test]
fn historical_reads_fall_back_to_one_process_per_object() {
    let p = simple(false);
    p.commit_all("matching reviewed bytes");
    for i in 0..3 {
        p.write("request.rs", format!("// variant {i}\n"));
        p.commit_all(&format!("variant {i}"));
    }
    p.write("request.rs", "// current edit\n");
    let (_, expected) = p.json(&["explain", "README.md"]);
    for broken in BROKEN_READERS {
        let shim = Shim::with_batch(broken);
        let (output, calls) = shim.run(&p, &["explain", "README.md", "--format", "json"]);
        assert_eq!(
            output.status.code(),
            Some(0),
            "{broken}: {}",
            stderr(&output)
        );
        let value = parse_json(&output.stdout);
        assert_eq!(evidence(&value), evidence(&expected), "{broken}");
        assert!(
            get_bool(evidence(&value), &["baseline_verified"]),
            "{broken}"
        );
        // One failed reader, then the per-object reads.
        assert_eq!(history_batches(&calls), 1, "{broken}: {calls:#?}");
        assert!(count(&calls, "cat-file blob") >= 1, "{broken}: {calls:#?}");
    }
}

#[test]
fn raw_dirtiness_falls_back_to_one_process_per_object() {
    let p = raw_repository(&[
        ("README.md", b"# Raw\n"),
        ("a.txt", b"first\n"),
        ("b.txt", b"second\n"),
    ]);
    for (edit, expected) in [(None, false), (Some(("b.txt", "SECOND\n")), true)] {
        if let Some((name, content)) = edit {
            p.write(name, content);
        }
        // A well-formed header with a wrong size is a size mismatch, which
        // the raw comparison reports as a change without reading content,
        // so only the malformed and the exiting readers apply here.
        for broken in [BROKEN_READERS[0], BROKEN_READERS[2]] {
            let shim = Shim::with_batch(broken);
            let (output, calls) =
                shim.run(&p, &["review", "README.md", "--full", "--format", "json"]);
            assert_eq!(
                output.status.code(),
                Some(0),
                "{broken}: {}",
                stdout(&output)
            );
            let dirty = get_bool(
                &parse_json(&output.stdout),
                &["data", "context", "git", "worktree_dirty"],
            );
            assert_eq!(dirty, expected, "{broken}: {calls:#?}");
            assert!(dirtiness_batches(&calls) >= 1, "{broken}: {calls:#?}");
            assert!(count(&calls, "cat-file blob") >= 1, "{broken}: {calls:#?}");
        }
    }
}

#[test]
fn raw_dirtiness_peels_a_replacement_tag_as_the_per_object_read_did() {
    // `git replace` swaps the index blob for an annotated tag that points
    // to a blob with the worktree bytes. `git cat-file blob` peels the tag;
    // a batch reader sees a tag, so the comparison must fall back.
    let p = raw_repository(&[("kept.txt", b"kept bytes\n")]);
    assert!(!dirty(&p));
    let original = stdout(&p.git(&["rev-parse", ":kept.txt"]))
        .trim()
        .to_string();
    let other = p.root.join(".git/other-bytes");
    fs::write(&other, "kept bytes\nwith more\n").unwrap();
    let target = stdout(&p.git(&["hash-object", "-w", other.to_str().unwrap()]))
        .trim()
        .to_string();
    p.git(&["tag", "-a", "-m", "replacement", "peeled", &target]);
    let tag = stdout(&p.git(&["rev-parse", "peeled"])).trim().to_string();
    p.git(&["replace", "-f", &original, &tag]);
    // The worktree holds the bytes the replacement tag peels to.
    p.write("kept.txt", "kept bytes\nwith more\n");
    let reference = raw_dirtiness_without_batch(&p);
    assert_eq!(
        dirty(&p),
        reference,
        "batched and per-object answers differ"
    );
}

/// Whether `pid` still runs, checked for up to two seconds so that an
/// orphan that was just signalled has time to be reaped.
fn still_running(pid: &str) -> bool {
    let until = std::time::Instant::now() + std::time::Duration::from_secs(2);
    loop {
        let alive = std::process::Command::new("kill")
            .args(["-0", pid])
            .stderr(std::process::Stdio::null())
            .status()
            .unwrap()
            .success();
        if !alive || std::time::Instant::now() >= until {
            return alive;
        }
        std::thread::sleep(std::time::Duration::from_millis(25));
    }
}

#[test]
fn a_stalled_history_reader_ends_with_its_descendants_at_the_deadline() {
    // A Git wrapper starts a descendant and never answers. The historical
    // reader runs in its own group, as the supervised per-object read did,
    // so the history deadline ends the descendant too.
    let p = simple(false);
    p.commit_all("matching reviewed bytes");
    p.write("request.rs", "// later bytes\n");
    p.commit_all("later");
    p.write("request.rs", "// current edit\n");
    let marker = tempfile::tempdir().unwrap();
    let pid_file = marker.path().join("descendant.pid");
    let shim = Shim::with_batch(&format!(
        "/bin/sleep 30 & echo $! > '{}'; wait; exit 1",
        pid_file.display()
    ));
    let started = std::time::Instant::now();
    let (output, _) = shim.run(&p, &["explain", "README.md", "--format", "json"]);
    assert_eq!(output.status.code(), Some(0), "{}", stderr(&output));
    assert!(started.elapsed() < std::time::Duration::from_secs(20));
    let value = parse_json(&output.stdout);
    assert_eq!(
        get_str(evidence(&value), &["reason_code"]),
        "history_limit_exceeded"
    );
    let pid = fs::read_to_string(&pid_file).unwrap();
    assert!(
        !still_running(pid.trim()),
        "the wrapper's descendant {pid} outlived the command"
    );
}

#[test]
fn a_raw_reader_waits_for_what_git_started_as_command_output_did() {
    // A Git wrapper leaves a short-lived descendant that holds the output
    // pipes, or only stderr. The raw reader stays in the caller's group and
    // is not killed, so the command waits for that descendant, as one `git
    // cat-file blob` through `Command::output` did. The descendant marks its
    // end, so the mark exists as soon as the command returns.
    let p = raw_repository(&[("kept.txt", b"kept bytes\n")]);
    for (label, redirection) in [("both pipes", ""), ("stderr only", ">/dev/null")] {
        let marker = tempfile::tempdir().unwrap();
        let ended = marker.path().join("ended");
        let shim = Shim::with_batch(&format!(
            "( /bin/sleep 0.5; printf done > '{}' ) {redirection} &",
            ended.display()
        ));
        let (output, calls) = shim.run(&p, &["review", "README.md", "--full", "--format", "json"]);
        assert_eq!(
            output.status.code(),
            Some(0),
            "{label}: {}",
            stdout(&output)
        );
        assert_eq!(dirtiness_batches(&calls), 1, "{label}: {calls:#?}");
        assert!(!get_bool(
            &parse_json(&output.stdout),
            &["data", "context", "git", "worktree_dirty"]
        ));
        assert!(
            ended.exists(),
            "{label}: the command returned before the wrapper's descendant ended"
        );
    }
}

/// Bytes that the raw reader's Git wrote, through a shim that counts its
/// output on the way to Memoria.
fn raw_reader_bytes(p: &Project, args: &[&str]) -> u64 {
    let real = std::process::Command::new("sh")
        .args(["-c", "command -v git"])
        .output()
        .unwrap();
    let real = String::from_utf8(real.stdout).unwrap().trim().to_string();
    let marker = tempfile::tempdir().unwrap();
    let count = marker.path().join("bytes");
    let shim = Shim::with_batch(&format!(
        "{{ '{real}' \"$@\" | tee /dev/fd/3 | wc -c > '{}'; }} 3>&1; exit 0",
        count.display()
    ));
    let (output, calls) = shim.run(p, args);
    assert_eq!(output.status.code(), Some(0), "{}", stdout(&output));
    assert_eq!(dirtiness_batches(&calls), 1, "{calls:#?}");
    let until = std::time::Instant::now() + std::time::Duration::from_secs(5);
    loop {
        if let Ok(text) = fs::read_to_string(&count)
            && let Ok(bytes) = text.trim().parse()
        {
            return bytes;
        }
        assert!(std::time::Instant::now() < until, "no byte count");
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
}

#[test]
fn an_early_change_leaves_little_speculative_output() {
    let big = vec![b'x'; 1024 * 1024];
    let mut files: Vec<(String, Vec<u8>)> = vec![("a-first.bin".to_string(), big.clone())];
    // 24 MiB in all: well above the 4 MiB lookahead plus the 8 MiB drain
    // limit, so a missing lookahead bound still shows.
    for i in 0..23 {
        files.push((format!("big/{i:02}.bin"), big.clone()));
    }
    let borrowed: Vec<(&str, &[u8])> = files
        .iter()
        .map(|(n, c)| (n.as_str(), c.as_slice()))
        .collect();
    let p = raw_repository(&borrowed);
    // `status` runs the raw comparison; a full export would refuse 24 MiB.
    let args = ["status", "--format", "json"];
    // The first file changes with the same size: its object is compared, and
    // only the 4 MiB lookahead of later objects was requested ahead of it.
    // Without that bound, the drain limit alone lets about 9.5 MB through.
    let mut edited = big.clone();
    edited[0] = b'y';
    p.write("a-first.bin", &edited);
    let bytes = raw_reader_bytes(&p, &args);
    assert!(
        bytes < 6 * 1024 * 1024,
        "{bytes} bytes for one changed file"
    );
    // A deleted file is a change found without its object: only the small
    // `README.md`, which sorts before it, was requested.
    fs::remove_file(p.root.join("a-first.bin")).unwrap();
    let bytes = raw_reader_bytes(&p, &args);
    assert!(bytes < 1024, "{bytes} bytes before a deleted file");
}
