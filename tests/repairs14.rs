//! Regression tests for the round-14 triage finding (MEM-047).

mod common;

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Output;

use common::*;
use memoria_infrastructure::json::Json;

const REASON: &str = "Review the current documentation again.";

/// A project with one README owning one source, run with an isolated Git
/// global configuration and an explicit XDG configuration home.
struct Isolated {
    project: Project,
    xdg: PathBuf,
}

impl Isolated {
    fn new(xdg: &Path) -> Isolated {
        let project = Project::empty_repo();
        project.write("README.md", "# Root\n");
        project.write("a.rs", "source\n");
        project.commit_all("seed");
        let isolated = Isolated {
            project,
            xdg: xdg.to_path_buf(),
        };
        assert_eq!(isolated.run(&["init", "--apply"]).status.code(), Some(0));
        isolated
    }

    fn run(&self, args: &[&str]) -> Output {
        self.project
            .command(&self.project.root, args)
            .env("XDG_CONFIG_HOME", &self.xdg)
            .env("HOME", &self.xdg)
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .output()
            .unwrap()
    }

    fn json(&self, args: &[&str]) -> (i32, Json) {
        let mut full = args.to_vec();
        full.extend_from_slice(&["--format", "json"]);
        let output = self.run(&full);
        (output.status.code().unwrap(), parse_json(&output.stdout))
    }

    fn git(&self, args: &[&str]) -> Output {
        std::process::Command::new("git")
            .arg("-C")
            .arg(&self.project.root)
            .args(args)
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env("XDG_CONFIG_HOME", &self.xdg)
            .env("HOME", &self.xdg)
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .output()
            .unwrap()
    }

    fn set_excludes(&self, value: &str) {
        assert!(
            self.git(&["config", "core.excludesFile", value])
                .status
                .success()
        );
    }

    fn git_ignores(&self, path: &str) -> bool {
        self.git(&["check-ignore", "-q", path]).status.code() == Some(0)
    }

    fn outcome(&self, path: &str) -> String {
        let (code, explain) = self.json(&["status", "--explain", path]);
        assert_eq!(code, 0);
        get_str(&explain, &["data", "explanation", "outcome"]).to_string()
    }

    /// Capture a packet for the root README (ready: it has no providers).
    fn packet(&self) -> (PathBuf, String, String) {
        let output = self.run(&["review", "README.md", "--format", "json"]);
        assert_eq!(output.status.code(), Some(0), "{}", stdout(&output));
        let value = parse_json(&output.stdout);
        let path = self.project.packets.path().join(format!(
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

    fn ack(&self, packet: &Path, token: &str, format: &str) -> Output {
        self.run(&[
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
            format,
        ])
    }

    fn policy_hash(&self) -> String {
        let (packet, token, hash) = self.packet();
        assert_eq!(self.ack(&packet, &token, "json").status.code(), Some(0));
        hash
    }
}

// MEM-047
#[test]
fn empty_host_excludes_setting_disables_the_source_like_git() {
    let external = tempfile::tempdir().unwrap();
    let populated = external.path().join("xdg-populated");
    fs::create_dir_all(populated.join("git")).unwrap();
    fs::write(populated.join("git/ignore"), "blocked.txt\n").unwrap();
    let isolated = external.path().join("xdg-empty");
    fs::create_dir_all(&isolated).unwrap();

    // Absent setting: the XDG default applies, in Git and in Memoria.
    let absent = Isolated::new(&populated);
    absent.project.write("blocked.txt", "x\n");
    assert!(absent.git_ignores("blocked.txt"));
    assert_eq!(absent.outcome("blocked.txt"), "git-ignored");
    let hash_absent = absent.policy_hash();

    // Explicitly empty: no global source, no XDG fallback, full workflow.
    let empty = Isolated::new(&populated);
    empty.set_excludes("");
    empty.project.write("blocked.txt", "x\n");
    assert!(!empty.git_ignores("blocked.txt"), "Git disables the source");
    assert_eq!(empty.outcome("blocked.txt"), "selected");
    let (packet, token, hash_empty) = empty.packet();
    assert_eq!(empty.ack(&packet, &token, "json").status.code(), Some(0));
    let (code, _) = empty.json(&["invalidate", "all", "--reason", REASON]);
    assert_eq!(code, 0);
    for format in ["json", "human"] {
        for (args, expected) in [
            (vec!["status"], 0),
            (vec!["lint"], 0),
            (vec!["review"], 0),
            (vec!["review", "README.md"], 0),
            (vec!["check"], 1),
            (vec!["graph"], 0),
            (vec!["render", "--dry-run"], 0),
        ] {
            let mut full = args.clone();
            full.extend_from_slice(&["--format", format]);
            let output = empty.run(&full);
            assert_eq!(
                output.status.code(),
                Some(expected),
                "{args:?} {format}: {}{}",
                stdout(&output),
                stderr(&output)
            );
            assert!(
                !stderr(&output).contains("git_unavailable"),
                "{args:?} {format}"
            );
        }
    }
    let (packet, token, _) = empty.packet();
    // A host rule that matches no project input changes nothing: the packet
    // still acknowledges, because host settings never enter policy.
    let rules = external.path().join("real-rules");
    fs::write(&rules, "nothing-here/\n").unwrap();
    empty.set_excludes(rules.to_str().unwrap());
    let output = empty.ack(&packet, &token, "human");
    assert_eq!(output.status.code(), Some(0), "{}", stderr(&output));
    for format in ["json", "human"] {
        let output = empty.run(&["check", "--format", format]);
        assert_eq!(
            output.status.code(),
            Some(0),
            "{format}: {}",
            stderr(&output)
        );
    }

    // Equivalent no-rule controls hash the same policy as the empty setting.
    let empty_file = external.path().join("empty-global-ignore");
    fs::write(&empty_file, "").unwrap();
    let named_empty = Isolated::new(&populated);
    named_empty.set_excludes(empty_file.to_str().unwrap());
    assert!(!named_empty.git_ignores("blocked.txt"));
    let hash_named_empty = named_empty.policy_hash();
    let unset_isolated = Isolated::new(&isolated);
    let hash_unset_isolated = unset_isolated.policy_hash();
    // A whitespace-only value is a filename, never "empty".
    let space_name = Isolated::new(&populated);
    space_name.set_excludes(" ");
    assert!(!space_name.git_ignores("blocked.txt"));
    let hash_space_name = space_name.policy_hash();
    assert_eq!(hash_empty, hash_named_empty);
    assert_eq!(hash_empty, hash_unset_isolated);
    assert_eq!(hash_empty, hash_space_name);
    // Host rules decide eligibility, never policy: an applied XDG rule set
    // hashes exactly like no host source at all.
    assert_eq!(
        hash_empty, hash_absent,
        "host ignore rules must never enter the policy hash"
    );

    // A whitespace-bearing filename with rules keeps its policy (MEM-042).
    let spaced = external.path().join("ws ignore ");
    fs::write(&spaced, "blocked.txt\n").unwrap();
    let whitespace = Isolated::new(&isolated);
    whitespace.set_excludes(spaced.to_str().unwrap());
    whitespace.project.write("blocked.txt", "x\n");
    assert!(whitespace.git_ignores("blocked.txt"));
    assert_eq!(whitespace.outcome("blocked.txt"), "git-ignored");
    assert_eq!(
        whitespace.policy_hash(),
        hash_absent,
        "every host rule profile shares one repository policy"
    );
}
