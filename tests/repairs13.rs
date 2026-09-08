//! Regression tests for the round-13 triage finding (MEM-046).

mod common;

use std::fs;

use common::*;
use memoria_infrastructure::json::{self, Json};

const FILES: usize = 100_001;

/// A root README owning `FILES` untracked 100-byte sources: raw inputs above
/// the default 8 MiB budget and, with the budget raised, above the record cap.
fn oversized_project() -> Project {
    let project = Project::empty_repo();
    project.write("README.md", "# Root\n");
    project.commit_all("seed");
    assert_eq!(project.run(&["init", "--apply"]).status.code(), Some(0));
    let body = [b'x'; 100];
    for i in 0..FILES {
        fs::write(project.root.join(format!("{i:06}.txt")), body).unwrap();
    }
    project
}

fn review_json(project: &Project, args: &[&str]) -> (i32, Vec<u8>) {
    let mut full = vec!["review", "README.md"];
    full.extend_from_slice(args);
    full.extend_from_slice(&["--format", "json"]);
    let output = project.run(&full);
    (output.status.code().unwrap(), output.stdout)
}

fn assert_bounded_refusal(label: &str, code: i32, stdout: &[u8], files: u64) -> Json {
    assert_eq!(code, 1, "{label}");
    let value = parse_json(stdout);
    assert_eq!(
        diagnostic_codes(&value),
        vec!["packet_too_large"],
        "{label}"
    );
    assert!(
        !stdout.windows(7).any(|w| w == b"\"token\""),
        "{label}: no token"
    );
    assert_eq!(
        get_str(&value, &["data", "kind"]),
        "packet_refused",
        "{label}"
    );
    assert!(get_bool(&value, &["data", "manifest_omitted"]), "{label}");
    assert!(
        matches!(get(&value, &["data"]), Json::Object(map) if !map.contains_key("manifest")),
        "{label}: the oversized manifest is omitted"
    );
    assert_eq!(
        get_u64(&value, &["data", "manifest_summary", "files"]),
        files,
        "{label}"
    );
    assert_eq!(
        get_u64(&value, &["data", "manifest_summary", "imports"]),
        0,
        "{label}"
    );
    assert_eq!(
        get_u64(&value, &["data", "manifest_summary", "record_count"]),
        files,
        "{label}"
    );
    assert!(
        get_u64(&value, &["data", "manifest_summary", "envelope_records"]) > 100_000,
        "{label}: the measured full envelope was over the cap"
    );
    assert_eq!(
        get_u64(&value, &["data", "size", "record_count"]),
        files,
        "{label}"
    );
    let records = json::count_records(&value);
    assert!(
        records <= 100_000,
        "{label}: {records} records in the refusal"
    );
    assert!(stdout.len() < 64 * 1024, "{label}: {} bytes", stdout.len());
    let Json::Array(diagnostics) = get(&value, &["diagnostics"]) else {
        panic!()
    };
    assert_eq!(
        get_u64(&diagnostics[0], &["details", "files"]),
        files,
        "{label}"
    );
    value
}

// MEM-046
#[test]
fn oversized_refusals_are_bounded_in_both_presentations() {
    let project = oversized_project();
    let state = project.state();
    // Default raw budget: the refusal would carry 100,001 manifest files.
    let (code, out) = review_json(&project, &[]);
    let value = assert_bounded_refusal("default budget", code, &out, FILES as u64);
    let Json::Array(diagnostics) = get(&value, &["diagnostics"]) else {
        panic!()
    };
    assert!(
        get_str(&diagnostics[0], &["message"]).contains("raw review inputs are"),
        "{}",
        get_str(&diagnostics[0], &["message"])
    );
    assert_eq!(project.state(), state, "read-only refusal");
    // Explicit 32 MiB budget: the packet itself would exceed the record cap.
    let (code, out) = review_json(&project, &["--max-bytes", "33554432"]);
    let value = assert_bounded_refusal("explicit budget", code, &out, FILES as u64);
    let Json::Array(diagnostics) = get(&value, &["diagnostics"]) else {
        panic!()
    };
    assert!(
        get_str(&diagnostics[0], &["message"]).contains("above the hard cap of 100000"),
        "{}",
        get_str(&diagnostics[0], &["message"])
    );
    assert_eq!(project.state(), state);
    // Human output: exit 1, no token, the same bounded counts on stderr.
    let output = project.run(&["review", "README.md"]);
    assert_eq!(output.status.code(), Some(1));
    assert!(!stdout(&output).contains("mrv2."));
    let err = stderr(&output);
    assert!(err.contains("packet_too_large"), "{err}");
    assert!(err.contains("100001"), "{err}");
    assert!(err.len() < 64 * 1024, "{}", err.len());
    assert_eq!(project.state(), state);
    // Exact-cap control: 99,999 files plus one diagnostic is exactly 100,000
    // records, so the default refusal keeps its full manifest.
    fs::remove_file(project.root.join(format!("{:06}.txt", FILES - 1))).unwrap();
    fs::remove_file(project.root.join(format!("{:06}.txt", FILES - 2))).unwrap();
    let (code, out) = review_json(&project, &[]);
    assert_eq!(code, 1);
    let value = parse_json(&out);
    assert_eq!(diagnostic_codes(&value), vec!["packet_too_large"]);
    assert!(!out.windows(7).any(|w| w == b"\"token\""));
    let Json::Array(files) = get(&value, &["data", "manifest", "files"]) else {
        panic!("full manifest expected at the cap")
    };
    assert_eq!(files.len(), FILES - 2);
    assert_eq!(json::count_records(&value), 100_000);
    assert!(
        matches!(get(&value, &["data"]), Json::Object(map) if !map.contains_key("manifest_omitted"))
    );
    assert_eq!(project.state(), state);
}

// MEM-046: a small refusal keeps its full manifest on the shared decoded-content path too.
#[test]
fn small_refusals_keep_the_full_manifest() {
    let project = Project::empty_repo();
    project.write("README.md", "# Root\n");
    project.commit_all("seed");
    assert_eq!(project.run(&["init", "--apply"]).status.code(), Some(0));
    project.write("large.bin", vec![b'x'; 8 * 1024 * 1024 + 1]);
    let (code, out) = review_json(&project, &[]);
    assert_eq!(code, 1);
    let value = parse_json(&out);
    assert_eq!(diagnostic_codes(&value), vec!["packet_too_large"]);
    let Json::Array(files) = get(&value, &["data", "manifest", "files"]) else {
        panic!("full manifest expected")
    };
    assert_eq!(files.len(), 1);
    assert_eq!(get_str(&files[0], &["path"]), "large.bin");
    assert_eq!(
        get_u64(&value, &["data", "size", "raw_input_bytes"]),
        8 * 1024 * 1024 + 1 + 7
    );
    assert!(
        matches!(get(&value, &["data"]), Json::Object(map) if !map.contains_key("manifest_omitted"))
    );
    let Json::Array(diagnostics) = get(&value, &["diagnostics"]) else {
        panic!()
    };
    assert_eq!(get_u64(&diagnostics[0], &["details", "files"]), 1);
}
