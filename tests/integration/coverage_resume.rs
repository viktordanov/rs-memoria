//! `memoria ack <DOCUMENT> --cover`: finishing the history check of a stored
//! review against HEAD, and the visible line when `ack` runs out of budget.

use crate::common;

use common::*;
use memoria_domain::DocumentId;

fn small() -> Project {
    let p = Project::empty_repo();
    p.write(
        "README.md",
        "# Limits\n\nThe request limit is ten. Read limits.rs for the contract.\n",
    );
    p.write("limits.rs", "pub const LIMIT: usize = 10;\n");
    p.commit_all("sources");
    p
}

fn record(p: &Project) -> memoria_domain::ReviewRecord {
    p.decoded_state().reviews[&DocumentId::parse("README.md").unwrap()].clone()
}

#[test]
fn cover_records_head_after_the_reviewed_bytes_are_committed() {
    let p = small();
    // Acknowledged on uncommitted bytes: no commit holds them yet.
    p.write("limits.rs", "pub const LIMIT: usize = 12;\n");
    p.baseline();
    let before = record(&p);
    assert_eq!(before.git.base_commit, None);

    // While the reviewed bytes are not in HEAD, nothing is written.
    let state = p.state();
    let (code, value) = p.json(&["ack", "README.md", "--cover"]);
    assert_eq!(code, 3, "{value:?}");
    assert!(diagnostic_codes(&value).contains(&"coverage_incomplete".to_string()));
    let text = stdout(&p.run(&["ack", "README.md", "--cover", "--format", "json"]));
    assert!(
        text.contains("limits.rs"),
        "the differing input is named: {text}"
    );
    assert_eq!(p.state(), state, "a failed cover writes nothing");

    p.commit_all("reviewed bytes");
    let head = stdout(&p.git(&["rev-parse", "HEAD"])).trim().to_string();
    let output = p.run(&["ack", "README.md", "--cover"]);
    assert_eq!(output.status.code(), Some(0), "{}", stderr(&output));
    assert!(
        stdout(&output).contains("Recorded base commit"),
        "{}",
        stdout(&output)
    );
    let after = record(&p);
    assert_eq!(after.git.base_commit.as_deref(), Some(head.as_str()));
    // Only the base commit changed.
    let mut expected = before.clone();
    expected.git.base_commit = Some(head.clone());
    assert_eq!(after, expected);
    assert_eq!(p.json(&["check"]).0, 0);

    // A second cover is a no-op.
    let state = p.state();
    let output = p.run(&["ack", "README.md", "--cover"]);
    assert_eq!(output.status.code(), Some(0));
    assert!(stdout(&output).contains("already has base commit"));
    assert_eq!(p.state(), state);
}

#[test]
fn cover_refuses_a_document_without_a_review_and_mixed_arguments() {
    let p = small();
    assert_eq!(p.run(&["init", "--apply"]).status.code(), Some(0));
    let (code, value) = p.json(&["ack", "README.md", "--cover"]);
    assert_eq!(code, 1, "{value:?}");
    assert!(diagnostic_codes(&value).contains(&"document_not_reviewed".to_string()));
    let output = p.run(&["ack", "README.md", "--cover", "--note", "a note of words"]);
    assert_eq!(output.status.code(), Some(2), "{}", stderr(&output));
    let output = p.run(&["ack", "README.md"]);
    assert_eq!(output.status.code(), Some(2), "{}", stderr(&output));
}

#[test]
fn an_ack_that_runs_out_of_history_budget_says_how_to_finish() {
    // 4,100 sources plus the document exceed the 4,096-read budget of one
    // history search at HEAD.
    let p = Project::empty_repo();
    p.write(
        "README.md",
        "# Many\n\nEvery file below is explained here.\n",
    );
    for i in 0..4100 {
        p.write(&format!("src/f{i:04}.rs"), format!("fn f{i}() {{}}\n"));
    }
    p.commit_all("sources");
    assert_eq!(p.run(&["init", "--apply"]).status.code(), Some(0));
    assert_eq!(p.run(&["render"]).status.code(), Some(0));
    p.commit_all("init");
    let (packet, token) = p.review_full("README.md");
    let output = p.ack("README.md", &packet, &token, "no-update", NOTE);
    assert_eq!(output.status.code(), Some(0), "{}", stderr(&output));
    let text = stdout(&output);
    // The read budget stops the search at 4,096 inputs. On a slow machine the
    // four-second budget can stop it earlier, with the same notice.
    let verified: usize = text
        .split_once("History check stopped at its limit: ")
        .and_then(|(_, rest)| rest.split_once(" of 4101 inputs verified, so no base commit"))
        .and_then(|(count, _)| count.parse().ok())
        .unwrap_or_else(|| panic!("the notice names the verified count: {text}"));
    assert!((1..=4096).contains(&verified), "{text}");
    assert!(
        text.contains("To finish it: memoria ack README.md --cover"),
        "{text}"
    );
    assert_eq!(record(&p).git.base_commit, None);

    // The cover runs in rounds, so it finishes where one search cannot.
    let output = p.run(&["ack", "README.md", "--cover"]);
    assert_eq!(output.status.code(), Some(0), "{}", stderr(&output));
    assert!(
        stdout(&output).contains("all 4101 reviewed inputs"),
        "{}",
        stdout(&output)
    );
    let head = stdout(&p.git(&["rev-parse", "HEAD"])).trim().to_string();
    assert_eq!(record(&p).git.base_commit.as_deref(), Some(head.as_str()));
}

#[test]
fn an_artifact_saved_before_cover_needs_a_fresh_review() {
    let p = small();
    p.write("limits.rs", "pub const LIMIT: usize = 12;\n");
    p.baseline();
    p.commit_all("reviewed bytes");
    // The document becomes pending, and a reviewer saves an artifact.
    p.write("limits.rs", "pub const LIMIT: usize = 14;\n");
    let (packet, token) = p.review_full("README.md");
    assert_eq!(
        p.run(&["ack", "README.md", "--cover"]).status.code(),
        Some(0)
    );
    // The base commit is part of the prior review the token binds.
    let state = p.state();
    let (code, value) = p.ack_json("README.md", &packet, &token, "no-update", NOTE);
    assert_eq!(code, 3, "{value:?}");
    assert!(diagnostic_codes(&value).contains(&"snapshot_changed".to_string()));
    assert_eq!(p.state(), state);
    // A fresh artifact acknowledges normally.
    let (packet, token) = p.review_full("README.md");
    let (code, value) = p.ack_json("README.md", &packet, &token, "no-update", NOTE);
    assert_eq!(code, 0, "{value:?}");
}
