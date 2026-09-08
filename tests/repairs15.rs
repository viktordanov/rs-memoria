//! Regression tests for the round-15 triage finding (MEM-048).

mod common;

use std::collections::BTreeSet;
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use common::*;
use memoria_infrastructure::json::Json;

/// Byte-range rules that Git evaluates differently against valid UTF-8
/// names; under lossy decoding both read as `[�-�]*`.
const FIRST: &[u8] = b"[\xc2-\xc3]*\n";
const SECOND: &[u8] = b"[\xc3-\xc4]*\n";

/// Names Git matches against the rules: `é.txt` (0xc3 0xa9) and `Ā.txt`
/// (0xc4 0x80). Hypothetical only; no such file exists in the project.
fn git_oracle(project: &Project) -> BTreeSet<String> {
    let mut command = Command::new("git");
    command
        .arg("-C")
        .arg(&project.root)
        .args(["check-ignore", "--no-index", "-z", "--stdin"])
        .env("LC_ALL", "C")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    isolate_git(&mut command, project.home.path());
    let mut child = command.spawn().unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all("é.txt\0Ā.txt\0".as_bytes())
        .unwrap();
    let output = child.wait_with_output().unwrap();
    String::from_utf8(output.stdout)
        .unwrap()
        .split('\0')
        .filter(|s| !s.is_empty())
        .map(str::to_string)
        .collect()
}

fn capture(project: &Project) -> (PathBuf, String, String) {
    let output = project.run(&["review", "README.md", "--format", "json"]);
    assert_eq!(output.status.code(), Some(0), "{}", stdout(&output));
    let value = parse_json(&output.stdout);
    let path = project.packets.path().join(format!(
        "packet-{}.json",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    fs::write(&path, &output.stdout).unwrap();
    (
        path,
        get_str(&value, &["data", "token"]).to_string(),
        get_str(&value, &["data", "manifest", "policy_hash"]).to_string(),
    )
}

fn ack_json(project: &Project, packet: &Path, token: &str) -> (i32, Json) {
    let output = project.run(&[
        "ack",
        "README.md",
        "--packet",
        packet.to_str().unwrap(),
        "--token",
        token,
        "--reviewer",
        "fixture",
        "--result",
        "no-update",
        "--note",
        NOTE,
        "--format",
        "json",
    ]);
    (output.status.code().unwrap(), parse_json(&output.stdout))
}

// MEM-048
#[test]
fn ignore_rule_bytes_outside_utf8_change_the_repository_policy() {
    // Only committed `.gitignore` files are repository policy. `info/exclude`
    // and `core.excludesFile` are host sources: they still decide actual Git
    // eligibility, but their bytes never enter the policy hash.
    {
        let source = "local";
        let project = Project::empty_repo();
        project.write("README.md", "# Root\n");
        project.write("a.rs", "source\n");
        project.commit_all("seed");
        assert_eq!(
            project.run(&["init", "--apply"]).status.code(),
            Some(0),
            "{source}"
        );
        let rule = project.root.join(".gitignore");
        fs::write(&rule, FIRST).unwrap();
        let before_oracle = git_oracle(&project);
        let (packet, token, hash_first) = capture(&project);
        let state = project.state();
        // Only the rule bytes change; the selected sources and README do not.
        fs::write(&rule, SECOND).unwrap();
        let after_oracle = git_oracle(&project);
        assert_ne!(
            before_oracle, after_oracle,
            "{source}: Git evaluates the two byte ranges differently"
        );
        let (_, fresh_token, hash_second) = capture(&project);
        assert_ne!(
            hash_first, hash_second,
            "{source}: policy fingerprint changed"
        );
        assert_ne!(token, fresh_token, "{source}: token changed");
        let (code, value) = ack_json(&project, &packet, &token);
        assert_eq!(
            code,
            3,
            "{source}: {}",
            memoria_infrastructure::json::to_compact(&value)
        );
        assert_eq!(
            diagnostic_codes(&value),
            vec!["snapshot_changed"],
            "{source}"
        );
        let Json::Array(diagnostics) = get(&value, &["diagnostics"]) else {
            panic!()
        };
        let Json::Array(changes) = get(&diagnostics[0], &["details", "changes"]) else {
            panic!()
        };
        assert_eq!(get_str(&changes[0], &["kind"]), "policy", "{source}");
        assert_eq!(project.state(), state, "{source}: state preserved");
        assert_eq!(
            project.json(&["check"]).0,
            1,
            "{source}: review still required"
        );
        // The current packet acknowledges and the project is clean.
        let (packet, token, _) = capture(&project);
        assert_eq!(ack_json(&project, &packet, &token).0, 0, "{source}");
        assert_eq!(project.json(&["check"]).0, 0, "{source}");
        // ASCII control in the same source: unchanged selection, changed policy.
        fs::write(&rule, [SECOND, b"*.log\n"].concat()).unwrap();
        let (packet, token, _) = capture(&project);
        let state = project.state();
        fs::write(&rule, [SECOND, b"*.tmp\n"].concat()).unwrap();
        let (code, value) = ack_json(&project, &packet, &token);
        assert_eq!(code, 3, "{source}: ASCII control");
        assert_eq!(diagnostic_codes(&value), vec!["snapshot_changed"]);
        assert_eq!(
            project.state(),
            state,
            "{source}: ASCII control state preserved"
        );
        // Restoring the exact bytes restores the packet's policy.
        fs::write(&rule, [SECOND, b"*.log\n"].concat()).unwrap();
        assert_eq!(ack_json(&project, &packet, &token).0, 0, "{source}");
        assert_eq!(project.json(&["check"]).0, 0, "{source}");
    }
}

// MEM-048: host rule bytes are eligibility, never policy.
#[test]
fn host_ignore_rule_bytes_change_eligibility_without_changing_policy() {
    let external = tempfile::tempdir().unwrap();
    for source in ["repository", "global"] {
        let project = Project::empty_repo();
        project.write("README.md", "# Root\n");
        project.write("a.rs", "source\n");
        project.commit_all("seed");
        assert_eq!(
            project.run(&["init", "--apply"]).status.code(),
            Some(0),
            "{source}"
        );
        let rule = match source {
            "repository" => project.root.join(".git/info/exclude"),
            _ => {
                let path = external.path().join(format!("host-byte-ignore-{source}"));
                project.git(&["config", "core.excludesFile", path.to_str().unwrap()]);
                path
            }
        };
        fs::create_dir_all(rule.parent().unwrap()).unwrap();
        fs::write(&rule, FIRST).unwrap();
        let before_oracle = git_oracle(&project);
        let (packet, token, hash_first) = capture(&project);
        assert_eq!(ack_json(&project, &packet, &token).0, 0, "{source}");
        let state = project.state();
        // Only the host rule bytes change; the selected sources do not.
        fs::write(&rule, SECOND).unwrap();
        let after_oracle = git_oracle(&project);
        assert_ne!(
            before_oracle, after_oracle,
            "{source}: Git evaluates the two byte ranges differently"
        );
        // The document stays current, so no packet exists to compare; the
        // policy is compared through the review record instead.
        assert_eq!(project.json(&["check"]).0, 0, "{source}: still current");
        assert_eq!(project.state(), state, "{source}: state preserved");
        let recorded = project.inspect_state();
        let hash_second = get_str(
            &recorded,
            &["reviews", "README.md", "input_manifest", "policy_hash"],
        )
        .to_string();
        assert_eq!(
            hash_first, hash_second,
            "{source}: host rule bytes must not change the policy fingerprint"
        );
        let _ = token;
    }
}

// MEM-048: valid UTF-8 rules hash exactly as before, so existing state stays valid.
#[test]
fn utf8_rules_keep_their_fingerprints_and_prior_state_stays_current() {
    let project = Project::seed();
    project.baseline();
    let state = project.state();
    // A rule containing multibyte UTF-8 is a policy change like any other,
    // and the same bytes restore the reviewed policy exactly.
    let rules = project.read(".gitignore");
    project.write(
        ".gitignore",
        [rules.as_slice(), "caf\u{e9}/\n".as_bytes()].concat(),
    );
    assert_eq!(
        project.cause_codes("src/execution/README.md"),
        vec!["input_changed"]
    );
    project.write(".gitignore", &rules);
    assert_eq!(project.json(&["check"]).0, 0);
    assert_eq!(project.state(), state, "inspection never rewrote state");
}
