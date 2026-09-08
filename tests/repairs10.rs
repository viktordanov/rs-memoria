//! Regression tests for the round-10 triage findings (MEM-042, MEM-043).

mod common;

use std::collections::BTreeSet;
use std::fs;
use std::path::Path;

use common::*;
use memoria_infrastructure::json::Json;

fn policy_hash(packet: &Path) -> String {
    get_str(
        &parse_json(&fs::read(packet).unwrap()),
        &["data", "manifest", "policy_hash"],
    )
    .to_string()
}

// MEM-042
#[test]
fn whitespace_bearing_host_exclude_paths_change_selection_only() {
    let external = tempfile::tempdir().unwrap();
    // (label, configured core.excludesFile value, relative to the worktree root?)
    let trailing = external.path().join("global ignore ");
    let ordinary = external.path().join("global-ignore");
    let cases = [
        ("trailing-space absolute", trailing.to_str().unwrap(), false),
        ("leading-space relative", " spaced-excludes", true),
        ("ordinary absolute", ordinary.to_str().unwrap(), false),
    ];
    for (label, configured, relative) in cases {
        let project = Project::seed();
        let file = if relative {
            // The rule file itself stays out of the selection through the
            // constant repository excludes.
            fs::create_dir_all(project.root.join(".git/info")).unwrap();
            fs::write(project.root.join(".git/info/exclude"), "*spaced-excludes\n").unwrap();
            project.root.join(configured)
        } else {
            Path::new(configured).to_path_buf()
        };
        fs::write(&file, "*.before\n").unwrap();
        project.git(&["config", "core.excludesFile", configured]);
        // Git itself reads the exact configured path.
        let ignored = project.git(&["check-ignore", "x.before"]);
        assert!(
            stdout(&ignored).contains("x.before"),
            "{label}: Git does not use the configured file"
        );
        project.baseline();
        project.append("src/execution/runner.rs", "// pending\n");
        let (packet, token) = project.review_packet("src/execution/README.md");
        let before = project.state();
        // The host rules change; the selected sources and their bytes do not.
        // Host settings decide Git eligibility, never repository policy, so
        // no document becomes stale and the packet still acknowledges.
        fs::write(&file, "*.after\n").unwrap();
        assert!(stdout(&project.git(&["check-ignore", "x.after"])).contains("x.after"));
        assert_eq!(
            project.cause_codes("src/corpus/README.md"),
            Vec::<String>::new(),
            "{label}: an irrelevant host rule must not make a document stale"
        );
        assert_eq!(
            project.cause_codes("src/execution/README.md"),
            vec!["input_changed"],
            "{label}: the real byte change is still visible"
        );
        let output = project.ack(
            "src/execution/README.md",
            &packet,
            &token,
            "no-update",
            NOTE,
        );
        assert_eq!(
            output.status.code(),
            Some(0),
            "{label}: {}",
            stdout(&output)
        );
        assert_ne!(project.state(), before, "{label}: the review was recorded");
        let (code, check) = project.json(&["check"]);
        assert_eq!(
            code,
            0,
            "{label}: {}",
            memoria_infrastructure::json::to_compact(&check)
        );
        // Identities stay logical and host-independent: a twin project with
        // different host rules hashes the same policy.
        project.append("src/execution/runner.rs", "// again\n");
        let (fresh, _) = project.review_packet("src/execution/README.md");
        assert_eq!(policy_hash(&packet), policy_hash(&fresh), "{label}");
        let twin = Project::seed();
        let twin_file = external
            .path()
            .join(format!("twin-{}", label.replace(' ', "-")));
        fs::write(&twin_file, "*.unrelated\n").unwrap();
        twin.git(&["config", "core.excludesFile", twin_file.to_str().unwrap()]);
        if relative {
            fs::write(twin.root.join(".git/info/exclude"), "*spaced-excludes\n").unwrap();
        }
        assert_eq!(twin.run(&["init", "--apply"]).status.code(), Some(0));
        assert_eq!(twin.run(&["render"]).status.code(), Some(0));
        twin.append("src/execution/runner.rs", "// pending\n");
        let (twin_packet, _) = twin.review_packet("src/execution/README.md");
        assert_eq!(policy_hash(&fresh), policy_hash(&twin_packet), "{label}");
    }
}

// MEM-043
#[test]
fn quoted_policy_values_keep_comments_out_of_rules() {
    // Root ignore rules: every spelling excludes the generated file and
    // yields one effective policy.
    let variants = [
        ("uncommented", "version = 2\nignore = [\"build's/**\"]\n"),
        (
            "commented",
            "version = 2\nignore = [\"build's/**\"] # generated output\n",
        ),
        (
            "literal",
            "version = 2\nignore = ['''build's/**'''] # generated output\n",
        ),
        (
            "multiline",
            "version = 2\nignore = [\n \"build's/**\", # generated output\n]\n",
        ),
    ];
    let mut hashes = BTreeSet::new();
    let mut selected = BTreeSet::new();
    for (label, config) in variants {
        let project = Project::seed();
        project.write("build's/output.rs", "generated\n");
        project.write("memoria.toml", config);
        assert_eq!(
            project.run(&["init", "--apply"]).status.code(),
            Some(0),
            "{label}"
        );
        assert_eq!(project.run(&["render"]).status.code(), Some(0), "{label}");
        let (code, explain) = project.json(&["status", "--explain", "build's/output.rs"]);
        assert_eq!(code, 0, "{label}");
        assert_eq!(
            get_str(&explain, &["data", "explanation", "outcome"]),
            "excluded",
            "{label}: {}",
            memoria_infrastructure::json::to_compact(&explain)
        );
        let (_, status) = project.json(&["status"]);
        selected.insert(get_u64(&status, &["data", "selected_files"]));
        let (packet, _) = project.review_packet("src/execution/README.md");
        hashes.insert(policy_hash(&packet));
    }
    assert_eq!(hashes.len(), 1, "one effective policy: {hashes:?}");
    assert_eq!(selected.len(), 1, "one selection: {selected:?}");

    // Sidecar include and ignore rules with apostrophes and double quotes.
    let mut hashes = BTreeSet::new();
    for comment in ["", " # reviewer's note"] {
        let project = Project::seed();
        project.write("src/retrieval/fixture's/sample.txt", "sample\n");
        project.write("src/retrieval/say \"hi\"/noise.rs", "noise\n");
        project.write(
            "memoria.toml",
            format!("version = 2\nignore = [\n \"**/generated/**\",{comment}\n \"**/fixture's/**\",{comment}\n]\n"),
        );
        project.write(
            "src/retrieval/README.memoria.toml",
            format!("include = [\"fixture's/**\"]{comment}\nignore = ['say \"hi\"/**']{comment}\n"),
        );
        assert_eq!(
            project.run(&["init", "--apply"]).status.code(),
            Some(0),
            "{comment:?}"
        );
        assert_eq!(
            project.run(&["render"]).status.code(),
            Some(0),
            "{comment:?}"
        );
        let outcome = |path: &str| {
            let (code, explain) = project.json(&["status", "--explain", path]);
            assert_eq!(code, 0, "{comment:?}: {path}");
            get_str(&explain, &["data", "explanation", "outcome"]).to_string()
        };
        assert_eq!(
            outcome("src/retrieval/fixture's/sample.txt"),
            "selected",
            "{comment:?}"
        );
        assert_eq!(
            outcome("src/retrieval/say \"hi\"/noise.rs"),
            "excluded",
            "{comment:?}"
        );
        assert_eq!(
            outcome("src/retrieval/generated/table.rs"),
            "excluded",
            "{comment:?}"
        );
        // The retrieval scope's leaf document imports from execution.
        project.ack_ok("src/execution/README.md");
        let (packet, _) = project.review_packet("src/retrieval/naive/README.md");
        hashes.insert(policy_hash(&packet));
        let (_, status) = project.json(&["status"]);
        let Json::Array(documents) = get(&status, &["data", "documents"]) else {
            panic!()
        };
        assert!(!documents.is_empty());
    }
    assert_eq!(
        hashes.len(),
        1,
        "comments have no policy meaning: {hashes:?}"
    );
}
