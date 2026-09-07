//! Token, packet transport, integrity, limits, replay, and concurrency.

mod common;

use std::fs;
use std::path::Path;

use common::*;
use memoria_infrastructure::json::{self, Json, Limits, to_compact, to_pretty};

fn ack_with(project: &Project, document: &str, packet: &Path, token: &str) -> std::process::Output {
    project.run(&[
        "ack",
        document,
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
    ])
}

fn pending_execution(project: &Project) -> (std::path::PathBuf, String) {
    project.append("src/execution/runner.rs", "// pending\n");
    project.review_packet("src/execution/README.md")
}

#[test]
fn tokens_are_fixed_size_and_deterministic() {
    let project = Project::seed();
    project.baseline();
    let (packet_a, token_a) = pending_execution(&project);
    let (packet_b, token_b) = project.review_packet("src/execution/README.md");
    assert_eq!(
        token_a, token_b,
        "identical inputs produce identical tokens"
    );
    assert_eq!(token_a.len(), 21);
    assert!(token_a.starts_with("mrv1."));
    let a = parse_json(&fs::read(&packet_a).unwrap());
    let b = parse_json(&fs::read(&packet_b).unwrap());
    assert_eq!(
        get(&a, &["data", "manifest"]),
        get(&b, &["data", "manifest"])
    );
    // Any canonical snapshot change alters the digest.
    project.append("src/execution/runner.rs", "// again\n");
    let (_, token_c) = project.review_packet("src/execution/README.md");
    assert_ne!(token_a, token_c);
}

#[test]
fn oversized_and_malformed_tokens_are_rejected_before_reading_the_packet() {
    let project = Project::seed();
    project.baseline();
    let (packet, token) = pending_execution(&project);
    let cases: Vec<String> = vec![
        format!("{token}0"),
        "a".repeat(256),
        "a".repeat(257),
        "a".repeat(10_000),
        token.to_uppercase(),
        format!("{} ", &token[..20]),
        format!("{}é", &token[..20]),
        format!("mrv2.{}", &token[5..]),
        "mrv1.payload=eyJkb2MiOiJSRUFETUUifQ".to_string(),
    ];
    for bad in cases {
        let output = project.run(&[
            "ack",
            "src/execution/README.md",
            "--packet",
            "/nonexistent/packet.json",
            "--token",
            &bad,
            "--reviewer",
            "fixture",
            "--result",
            "no-update",
            "--note",
            NOTE,
            "--format",
            "json",
        ]);
        assert_eq!(output.status.code(), Some(2), "token {bad:?}");
        assert_eq!(
            diagnostic_codes(&parse_json(&output.stdout)),
            vec!["token_invalid"]
        );
    }
    // A syntactically valid but wrong token is a mismatch after packet validation.
    let wrong = format!("mrv1.{}", "0".repeat(16));
    let output = ack_with(&project, "src/execution/README.md", &packet, &wrong);
    assert_eq!(output.status.code(), Some(2));
    assert_eq!(
        diagnostic_codes(&parse_json(&output.stdout)),
        vec!["token_mismatch"]
    );
    assert!(!project.exists(".memoria/state.json.tmp"));
}

#[test]
fn file_and_stdin_transport_are_equivalent_and_replay_conflicts() {
    let project = Project::seed();
    project.baseline();
    let (packet, token) = pending_execution(&project);
    let bytes = fs::read(&packet).unwrap();
    let output = project.run_stdin(
        &[
            "ack",
            "src/execution/README.md",
            "--packet",
            "-",
            "--token",
            &token,
            "--reviewer",
            "fixture",
            "--result",
            "no-update",
            "--note",
            NOTE,
            "--format",
            "json",
        ],
        &bytes,
    );
    assert_eq!(output.status.code(), Some(0), "{}", stderr(&output));
    let via_stdin = parse_json(&project.state());
    // The same packet through the file transport is now a revision conflict.
    let output = ack_with(&project, "src/execution/README.md", &packet, &token);
    assert_eq!(output.status.code(), Some(3));
    assert_eq!(
        diagnostic_codes(&parse_json(&output.stdout)),
        vec!["revision_conflict"]
    );
    assert_eq!(parse_json(&project.state()), via_stdin);

    // An independent baseline through the file transport reaches an equivalent state.
    let other = Project::seed();
    other.baseline();
    let (packet, token) = pending_execution(&other);
    assert_eq!(
        ack_with(&other, "src/execution/README.md", &packet, &token)
            .status
            .code(),
        Some(0)
    );
    let via_file = parse_json(&other.state());
    let strip_time = |mut value: Json| {
        if let Json::Object(map) = &mut value
            && let Some(Json::Object(reviews)) = map.get_mut("reviews")
        {
            for record in reviews.values_mut() {
                if let Json::Object(fields) = record {
                    fields.remove("reviewed_at");
                    fields.remove("git");
                }
            }
        }
        value
    };
    assert_eq!(strip_time(via_stdin), strip_time(via_file));
}

#[test]
fn missing_packet_argument_never_reads_stdin() {
    let project = Project::seed();
    project.baseline();
    let (packet, token) = pending_execution(&project);
    let bytes = fs::read(&packet).unwrap();
    let output = project.run_stdin(
        &[
            "ack",
            "src/execution/README.md",
            "--token",
            &token,
            "--reviewer",
            "fixture",
            "--result",
            "no-update",
            "--note",
            NOTE,
        ],
        &bytes,
    );
    assert_eq!(output.status.code(), Some(2));
    assert!(stderr(&output).contains("--packet"));
    assert_eq!(project.status_label("src/execution/README.md"), "pending");
}

#[test]
fn file_transport_rejects_bad_sources() {
    let project = Project::seed();
    project.baseline();
    let (packet, token) = pending_execution(&project);
    let missing = project.packets.path().join("missing.json");
    let output = ack_with(&project, "src/execution/README.md", &missing, &token);
    assert_eq!(output.status.code(), Some(4));
    assert_eq!(
        diagnostic_codes(&parse_json(&output.stdout)),
        vec!["packet_unreadable"]
    );
    let output = ack_with(
        &project,
        "src/execution/README.md",
        project.packets.path(),
        &token,
    );
    assert_eq!(output.status.code(), Some(2));
    assert_eq!(
        diagnostic_codes(&parse_json(&output.stdout)),
        vec!["packet_source_invalid"]
    );
    let link = project.packets.path().join("link.json");
    std::os::unix::fs::symlink(&packet, &link).unwrap();
    let output = ack_with(&project, "src/execution/README.md", &link, &token);
    assert_eq!(output.status.code(), Some(2));
    assert_eq!(
        diagnostic_codes(&parse_json(&output.stdout)),
        vec!["packet_source_invalid"]
    );
    let malformed = project.packets.path().join("malformed.json");
    fs::write(&malformed, b"{\"schema_version\": 1").unwrap();
    let output = ack_with(&project, "src/execution/README.md", &malformed, &token);
    assert_eq!(output.status.code(), Some(2));
    assert_eq!(
        diagnostic_codes(&parse_json(&output.stdout)),
        vec!["packet_schema_invalid"]
    );
    let trailing = project.packets.path().join("trailing.json");
    let mut bytes = fs::read(&packet).unwrap();
    bytes.extend_from_slice(b"{}");
    fs::write(&trailing, &bytes).unwrap();
    let output = ack_with(&project, "src/execution/README.md", &trailing, &token);
    assert_eq!(output.status.code(), Some(2));
    assert_eq!(
        diagnostic_codes(&parse_json(&output.stdout)),
        vec!["packet_schema_invalid"]
    );
    // Truncated stdin.
    let truncated = &fs::read(&packet).unwrap()[..100];
    let output = project.run_stdin(
        &[
            "ack",
            "src/execution/README.md",
            "--packet",
            "-",
            "--token",
            &token,
            "--reviewer",
            "fixture",
            "--result",
            "no-update",
            "--note",
            NOTE,
            "--format",
            "json",
        ],
        truncated,
    );
    assert_eq!(output.status.code(), Some(2));
    assert!(!project.exists(".memoria/state.json.bak"));
    assert_eq!(project.status_label("src/execution/README.md"), "pending");
}

#[test]
fn tampered_packets_are_rejected_and_canonical_reformatting_is_accepted() {
    let project = Project::seed();
    project.baseline();
    let (packet, token) = pending_execution(&project);
    let original = parse_json(&fs::read(&packet).unwrap());
    let write = |name: &str, value: &Json| {
        let path = project.packets.path().join(name);
        fs::write(&path, to_pretty(value)).unwrap();
        path
    };
    let mutate = |f: &dyn Fn(&mut Json)| {
        let mut value = original.clone();
        f(&mut value);
        value
    };
    // Whitespace and key order do not matter: compact form is accepted.
    let compact = project.packets.path().join("compact.json");
    fs::write(&compact, to_compact(&original)).unwrap();
    let reordered = project.packets.path().join("reordered.json");
    fs::write(&reordered, reorder_keys(&to_pretty(&original))).unwrap();
    for (name, path) in [("compact", &compact), ("reordered", &reordered)] {
        let dry = Project::seed();
        let _ = dry;
        let value = json::parse(&fs::read(path).unwrap(), Limits::PACKET).unwrap();
        assert_eq!(value, original, "{name} must parse to the same value");
    }
    let output = ack_with(&project, "src/execution/README.md", &compact, &token);
    assert_eq!(
        output.status.code(),
        Some(0),
        "compact packet accepted: {}",
        stdout(&output)
    );

    // Every tampered variant fails before mutation.
    let project = Project::seed();
    project.baseline();
    let (packet, token) = pending_execution(&project);
    let original = parse_json(&fs::read(&packet).unwrap());
    let state_before = project.state();
    let cases: Vec<(&str, Json, &str)> = vec![
        (
            "token",
            mutate(&|v| {
                set_path(
                    v,
                    &["data", "token"],
                    Json::String(format!("mrv1.{}", "1".repeat(16))),
                )
            }),
            "packet_integrity_failed",
        ),
        (
            "manifest",
            mutate(&|v| set_path(v, &["data", "manifest", "document_bytes"], Json::Number(1))),
            "packet_integrity_failed",
        ),
        (
            "context",
            mutate(&|v| {
                set_path(
                    v,
                    &["data", "context", "git", "worktree_dirty"],
                    Json::Bool(false),
                )
            }),
            "packet_integrity_failed",
        ),
        (
            "content",
            mutate(&|v| {
                set_path(
                    v,
                    &["data", "content", "readme", "body"],
                    Json::String("x".into()),
                )
            }),
            "packet_integrity_failed",
        ),
        (
            "diagnostics",
            mutate(&|v| set_path(v, &["diagnostics"], Json::Array(vec![]))),
            "packet_integrity_failed",
        ),
        (
            "schema",
            mutate(&|v| set_path(v, &["schema_version"], Json::Number(2))),
            "packet_integrity_failed",
        ),
        (
            "unknown field",
            mutate(&|v| set_path(v, &["data", "extra"], Json::Null)),
            "packet_integrity_failed",
        ),
    ];
    for (name, value, expected) in cases {
        let path = write(&format!("{name}.json"), &value);
        let output = ack_with(&project, "src/execution/README.md", &path, &token);
        assert_eq!(output.status.code(), Some(2), "{name}");
        assert_eq!(
            diagnostic_codes(&parse_json(&output.stdout)),
            vec![expected],
            "{name}"
        );
        assert_eq!(project.state(), state_before);
    }
    // Recomputing the digest does not rescue inconsistent snapshot fields.
    let hasher = memoria_infrastructure::Xxh3Hasher;
    let redigest = |mut value: Json| {
        if let Json::Object(map) = &mut value
            && let Some(Json::Object(data)) = map.get_mut("data")
        {
            data.remove("packet_digest");
        }
        let digest = memoria_infrastructure::packet::packet_digest(&hasher, &value);
        set_path(&mut value, &["data", "packet_digest"], Json::String(digest));
        value
    };
    let cases: Vec<(&str, Json, &str)> = vec![
        (
            "token",
            redigest(mutate(&|v| {
                set_path(
                    v,
                    &["data", "token"],
                    Json::String(format!("mrv1.{}", "1".repeat(16))),
                )
            })),
            "packet_token_mismatch",
        ),
        (
            "revision",
            redigest(mutate(&|v| {
                set_path(v, &["data", "review_revision"], Json::Number(9))
            })),
            "packet_token_mismatch",
        ),
        (
            "body",
            redigest(mutate(&|v| {
                set_path(
                    v,
                    &["data", "content", "readme", "body"],
                    Json::String("x".into()),
                )
            })),
            "packet_content_mismatch",
        ),
        (
            "base64",
            redigest(mutate(&|v| {
                set_path(
                    v,
                    &["data", "content", "readme", "encoding"],
                    Json::String("base64".into()),
                );
                set_path(
                    v,
                    &["data", "content", "readme", "body"],
                    Json::String("@@@".into()),
                );
            })),
            "packet_schema_invalid",
        ),
        (
            "plan",
            redigest(mutate(&|v| {
                set_path(v, &["data", "kind"], Json::String("review_plan".into()))
            })),
            "packet_schema_invalid",
        ),
        (
            "version",
            redigest(mutate(&|v| {
                set_path(v, &["data", "packet_version"], Json::Number(2))
            })),
            "packet_schema_invalid",
        ),
        (
            "unknown",
            redigest(mutate(&|v| set_path(v, &["data", "extra"], Json::Null))),
            "packet_schema_invalid",
        ),
        (
            "wrong document",
            redigest(mutate(&|v| {
                set_path(
                    v,
                    &["data", "document"],
                    Json::String("src/corpus/README.md".into()),
                )
            })),
            "packet_token_mismatch",
        ),
    ];
    for (name, value, expected) in cases {
        let path = write(&format!("re-{name}.json"), &value);
        let output = ack_with(&project, "src/execution/README.md", &path, &token);
        assert_eq!(output.status.code(), Some(2), "{name}: {}", stdout(&output));
        assert_eq!(
            diagnostic_codes(&parse_json(&output.stdout)),
            vec![expected],
            "{name}"
        );
        assert_eq!(project.state(), state_before);
    }
    // Duplicate keys and a plan envelope.
    let dup = project.packets.path().join("dup.json");
    fs::write(
        &dup,
        to_pretty(&original).replacen(
            "\"schema_version\": 1",
            "\"schema_version\": 1, \"schema_version\": 1",
            1,
        ),
    )
    .unwrap();
    let output = ack_with(&project, "src/execution/README.md", &dup, &token);
    assert_eq!(output.status.code(), Some(2));
    assert_eq!(
        diagnostic_codes(&parse_json(&output.stdout)),
        vec!["packet_schema_invalid"]
    );
    let plan = project.packets.path().join("plan.json");
    fs::write(&plan, project.run(&["review", "--format", "json"]).stdout).unwrap();
    let output = ack_with(&project, "src/execution/README.md", &plan, &token);
    assert_eq!(output.status.code(), Some(2));
    // Wrong CLI document for a valid packet.
    let output = ack_with(&project, "src/corpus/README.md", &packet, &token);
    assert_eq!(output.status.code(), Some(2));
    assert_eq!(
        diagnostic_codes(&parse_json(&output.stdout)),
        vec!["packet_document_mismatch"]
    );
    assert_eq!(project.state(), state_before);
}

/// Rewrite the pretty JSON with object keys in reverse order (still valid JSON).
fn reorder_keys(text: &str) -> String {
    let value = json::parse(text.as_bytes(), Limits::PACKET).unwrap();
    fn write(value: &Json, out: &mut String) {
        match value {
            Json::Object(map) => {
                out.push('{');
                for (i, (k, v)) in map.iter().rev().enumerate() {
                    if i > 0 {
                        out.push_str(" , ");
                    }
                    out.push_str(&to_compact(&Json::String(k.clone())));
                    out.push_str(" :\n ");
                    write(v, out);
                }
                out.push('}');
            }
            Json::Array(items) => {
                out.push('[');
                for (i, item) in items.iter().enumerate() {
                    if i > 0 {
                        out.push_str(",\n");
                    }
                    write(item, out);
                }
                out.push(']');
            }
            other => out.push_str(&to_compact(other)),
        }
    }
    let mut out = String::new();
    write(&value, &mut out);
    out.push('\n');
    out
}

#[test]
fn serialized_and_structural_limits_are_enforced_at_the_boundary() {
    let project = Project::seed();
    project.baseline();
    let (packet, token) = pending_execution(&project);
    let cap: usize = 64 * 1024 * 1024;
    // Pad a valid envelope with trailing whitespace to exactly the cap, then one byte over.
    let mut bytes = fs::read(&packet).unwrap();
    bytes.resize(cap, b' ');
    let at_cap = project.packets.path().join("at-cap.json");
    fs::write(&at_cap, &bytes).unwrap();
    let output = project.run_stdin(
        &[
            "ack",
            "src/execution/README.md",
            "--packet",
            "-",
            "--token",
            &token,
            "--reviewer",
            "fixture",
            "--result",
            "no-update",
            "--note",
            NOTE,
            "--format",
            "json",
        ],
        &bytes,
    );
    assert_eq!(output.status.code(), Some(0), "{}", stdout(&output));
    let project = Project::seed();
    project.baseline();
    let (packet, token) = pending_execution(&project);
    let mut bytes = fs::read(&packet).unwrap();
    bytes.resize(cap + 1, b' ');
    let over = project.packets.path().join("over.json");
    fs::write(&over, &bytes).unwrap();
    let output = ack_with(&project, "src/execution/README.md", &over, &token);
    assert_eq!(output.status.code(), Some(2));
    assert_eq!(
        diagnostic_codes(&parse_json(&output.stdout)),
        vec!["packet_limit_exceeded"]
    );
    let output = project.run_stdin(
        &[
            "ack",
            "src/execution/README.md",
            "--packet",
            "-",
            "--token",
            &token,
            "--reviewer",
            "fixture",
            "--result",
            "no-update",
            "--note",
            NOTE,
            "--format",
            "json",
        ],
        &bytes,
    );
    assert_eq!(output.status.code(), Some(2));
    assert_eq!(
        diagnostic_codes(&parse_json(&output.stdout)),
        vec!["packet_limit_exceeded"]
    );
    assert!(!project.exists(".memoria/state.json.tmp"));

    // Record and depth limits at the codec boundary.
    let deep = |n: usize| {
        format!(
            "{{\"schema_version\":1,\"command\":\"review\",\"ok\":true,\"data\":{{\"packet_digest\":\"{}\"}},\"diagnostics\":[{}{}]}}",
            "0".repeat(16),
            "[".repeat(n),
            "]".repeat(n)
        )
    };
    let depth_ok = project.packets.path().join("depth-ok.json");
    fs::write(&depth_ok, deep(30)).unwrap(); // envelope=1, diagnostics=2, +30 = 32
    let output = ack_with(&project, "src/execution/README.md", &depth_ok, &token);
    assert_eq!(
        diagnostic_codes(&parse_json(&output.stdout)),
        vec!["packet_integrity_failed"],
        "depth 32 passes the limit check"
    );
    let depth_bad = project.packets.path().join("depth-bad.json");
    fs::write(&depth_bad, deep(31)).unwrap();
    let output = ack_with(&project, "src/execution/README.md", &depth_bad, &token);
    assert_eq!(
        diagnostic_codes(&parse_json(&output.stdout)),
        vec!["packet_limit_exceeded"],
        "depth 33 is rejected before descending"
    );
    let records = |n: usize| {
        format!(
            "{{\"schema_version\":1,\"command\":\"review\",\"ok\":true,\"data\":{{\"packet_digest\":\"{}\"}},\"diagnostics\":[{}]}}",
            "0".repeat(16),
            vec!["0"; n].join(",")
        )
    };
    let records_ok = project.packets.path().join("records-ok.json");
    fs::write(&records_ok, records(100_000)).unwrap();
    let output = ack_with(&project, "src/execution/README.md", &records_ok, &token);
    assert_eq!(
        diagnostic_codes(&parse_json(&output.stdout)),
        vec!["packet_integrity_failed"]
    );
    let records_bad = project.packets.path().join("records-bad.json");
    fs::write(&records_bad, records(100_001)).unwrap();
    let output = ack_with(&project, "src/execution/README.md", &records_bad, &token);
    assert_eq!(
        diagnostic_codes(&parse_json(&output.stdout)),
        vec!["packet_limit_exceeded"]
    );
    // Limits hidden inside context also count.
    let mut value = parse_json(&fs::read(&packet).unwrap());
    set_path(
        &mut value,
        &["data", "context", "consumers"],
        Json::Array(vec![Json::String("x".into()); 100_001]),
    );
    let hidden = project.packets.path().join("hidden.json");
    fs::write(&hidden, to_compact(&value)).unwrap();
    let output = ack_with(&project, "src/execution/README.md", &hidden, &token);
    assert_eq!(
        diagnostic_codes(&parse_json(&output.stdout)),
        vec!["packet_limit_exceeded"]
    );
}

#[test]
fn raw_input_budget_refuses_large_packets_without_a_token() {
    let project = Project::seed();
    project.baseline();
    let big = vec![b'x'; 9 * 1024 * 1024];
    project.write("src/execution/big.bin", &big);
    let (code, refused) = project.json(&["review", "src/execution/README.md"]);
    assert_eq!(code, 1);
    assert_eq!(diagnostic_codes(&refused), vec!["packet_too_large"]);
    assert_eq!(get_str(&refused, &["data", "kind"]), "packet_refused");
    assert!(matches!(get(&refused, &["data"]), Json::Object(map) if !map.contains_key("token")));
    assert!(get_u64(&refused, &["data", "size", "raw_input_bytes"]) > 9 * 1024 * 1024);
    let plan = project.plan();
    let Json::Array(tasks) = get(&plan, &["data", "tasks"]) else {
        panic!()
    };
    assert!(get_u64(&tasks[0], &["raw_input_bytes"]) > 9 * 1024 * 1024);
    let (code, _) = project.json(&[
        "review",
        "src/execution/README.md",
        "--max-bytes",
        "16777216",
    ]);
    assert_eq!(code, 0);
    let (code, _) = project.json(&[
        "review",
        "src/execution/README.md",
        "--max-bytes",
        "33554433",
    ]);
    assert_eq!(code, 2);
    let (code, _) = project.json(&["review", "src/execution/README.md", "--max-bytes", "0"]);
    assert_eq!(code, 2);
    let (code, _) = project.json(&["review", "--max-bytes", "10"]);
    assert_eq!(code, 2);
}

#[test]
fn packets_carry_binary_content_diffs_and_deleted_files() {
    let project = Project::seed();
    project.baseline();
    project.commit_all("baseline");
    project.write("src/execution/blob.bin", [0xff, 0xfe, 0x00, 0x41]);
    project.append("src/execution/runner.rs", "// modified\n");
    let (packet, _) = project.review_packet("src/execution/README.md");
    let value = parse_json(&fs::read(&packet).unwrap());
    let Json::Array(files) = get(&value, &["data", "content", "files"]) else {
        panic!()
    };
    let blob = files
        .iter()
        .find(|f| get_str(f, &["path"]) == "src/execution/blob.bin")
        .unwrap();
    assert_eq!(get_str(blob, &["encoding"]), "base64");
    assert_eq!(get_str(blob, &["body"]), "//4AQQ==");
    let Json::Array(diffs) = get(&value, &["data", "context", "diffs"]) else {
        panic!()
    };
    let runner = diffs
        .iter()
        .find(|d| get_str(d, &["identity"]) == "src/execution/runner.rs")
        .unwrap();
    assert_eq!(get_str(runner, &["status"]), "available");
    assert!(get_str(runner, &["text"]).contains("+// modified"));
    let added = diffs
        .iter()
        .find(|d| get_str(d, &["identity"]) == "src/execution/blob.bin")
        .unwrap();
    assert_eq!(get_str(added, &["status"]), "added");
    project.ack_ok("src/execution/README.md");

    // Uncommitted prior snapshot: the old content does not match the recorded hash.
    project.append("src/execution/runner.rs", "// later\n");
    let (packet, _) = project.review_packet("src/execution/README.md");
    let value = parse_json(&fs::read(&packet).unwrap());
    let Json::Array(diffs) = get(&value, &["data", "context", "diffs"]) else {
        panic!()
    };
    let runner = diffs
        .iter()
        .find(|d| get_str(d, &["identity"]) == "src/execution/runner.rs")
        .unwrap();
    assert_eq!(get_str(runner, &["status"]), "unavailable");
    assert!(get_str(runner, &["reason"]).contains("does not match"));
    project.commit_all("second");
    project.ack_ok("src/execution/README.md");

    // Deleted file remains listed with its previous identity and committed content.
    project.remove("src/execution/blob.bin");
    let (packet, _) = project.review_packet("src/execution/README.md");
    let value = parse_json(&fs::read(&packet).unwrap());
    let Json::Array(changes) = get(&value, &["data", "context", "changes"]) else {
        panic!()
    };
    let removed = changes
        .iter()
        .find(|c| get_str(c, &["identity"]) == "src/execution/blob.bin")
        .unwrap();
    assert_eq!(get_str(removed, &["change"]), "removed");
    assert!(matches!(get(removed, &["after_bytes"]), Json::Null));
    let Json::Array(diffs) = get(&value, &["data", "context", "diffs"]) else {
        panic!()
    };
    let removed = diffs
        .iter()
        .find(|d| get_str(d, &["identity"]) == "src/execution/blob.bin")
        .unwrap();
    assert_eq!(get_str(removed, &["status"]), "removed");
    assert_eq!(get_str(removed, &["old_body"]), "//4AQQ==");
}

#[test]
fn read_only_commands_write_nothing() {
    let project = Project::seed();
    project.baseline();
    project.append("src/execution/runner.rs", "// pending\n");
    let before = project.tree_snapshot();
    let commands: Vec<Vec<&str>> = vec![
        vec!["status"],
        vec!["status", "--explain", "app.rs"],
        vec!["lint"],
        vec!["review"],
        vec!["review", "src/execution/README.md"],
        vec!["review", "src/corpus/README.md"],
        vec!["check"],
        vec!["graph"],
        vec!["render", "--dry-run"],
        vec!["agent", "install", "--dry-run"],
        vec!["review", "missing/README.md"],
    ];
    for args in commands {
        let mut json_args = args.clone();
        json_args.extend(["--format", "json"]);
        let _ = project.run(&json_args);
        let _ = project.run(&args);
        assert_eq!(project.tree_snapshot(), before, "{args:?} must not write");
    }
    // Failed acknowledgements also write nothing.
    let (packet, token) = project.review_packet("src/execution/README.md");
    let bad = format!("mrv1.{}", "0".repeat(16));
    let _ = ack_with(&project, "src/execution/README.md", &packet, &bad);
    assert_eq!(project.tree_snapshot(), before);
    // An uninitialized project: no `.memoria` after malformed packets.
    let fresh = Project::seed();
    let malformed = fresh.packets.path().join("bad.json");
    fs::write(&malformed, "{").unwrap();
    let output = ack_with(&fresh, "README.md", &malformed, &token);
    assert_eq!(output.status.code(), Some(2));
    assert!(!fresh.exists(".memoria"));
}

#[test]
fn concurrent_acknowledgements_do_not_lose_reviews() {
    let project = Project::seed();
    project.baseline();
    project.append("src/execution/runner.rs", "// a\n");
    project.append("src/corpus/types.rs", "// b\n");
    let (packet_a, token_a) = project.review_packet("src/execution/README.md");
    let (packet_b, token_b) = project.review_packet("src/corpus/README.md");
    // Same document from two processes: exactly one succeeds.
    let (packet_a2, token_a2) = project.review_packet("src/execution/README.md");
    let barrier = std::sync::Arc::new(std::sync::Barrier::new(2));
    let spawn = |packet: std::path::PathBuf, token: String, doc: &'static str| {
        let barrier = barrier.clone();
        let root = project.root.clone();
        std::thread::spawn(move || {
            barrier.wait();
            std::process::Command::new(memoria_bin())
                .current_dir(&root)
                .args([
                    "ack",
                    doc,
                    "--packet",
                    packet.to_str().unwrap(),
                    "--token",
                    &token,
                    "--reviewer",
                    "fixture",
                    "--result",
                    "no-update",
                    "--note",
                    NOTE,
                    "--format",
                    "json",
                ])
                .output()
                .unwrap()
        })
    };
    let first = spawn(packet_a.clone(), token_a.clone(), "src/execution/README.md");
    let second = spawn(packet_a2, token_a2, "src/execution/README.md");
    let results = [first.join().unwrap(), second.join().unwrap()];
    let codes: Vec<i32> = results.iter().map(|o| o.status.code().unwrap()).collect();
    assert_eq!(codes.iter().filter(|c| **c == 0).count(), 1, "{codes:?}");
    assert!(codes.iter().all(|c| *c == 0 || *c == 3), "{codes:?}");
    assert_eq!(project.status_label("src/execution/README.md"), "current");
    let state = parse_json(&project.state());
    assert_eq!(
        get_u64(&state, &["reviews", "src/execution/README.md", "revision"]),
        2
    );
    // Different documents: retry after busy succeeds and both reviews remain.
    let output = ack_with(&project, "src/corpus/README.md", &packet_b, &token_b);
    assert_eq!(output.status.code(), Some(0));
    let state = parse_json(&project.state());
    assert_eq!(
        get_u64(&state, &["reviews", "src/corpus/README.md", "revision"]),
        2
    );
    assert_eq!(
        get_u64(&state, &["reviews", "src/execution/README.md", "revision"]),
        2
    );
    let _ = packet_a;
    let _ = token_a;
}

#[test]
fn held_lock_reports_busy_and_post_commit_edits_are_detected() {
    let project = Project::seed();
    project.baseline();
    project.append("src/execution/runner.rs", "// pending\n");
    let (packet, token) = project.review_packet("src/execution/README.md");
    let guard = memoria_infrastructure::fs::lock_file(&project.root.join(".memoria/write.lock"))
        .unwrap_or_else(|_| panic!("lock"));
    let output = ack_with(&project, "src/execution/README.md", &packet, &token);
    assert_eq!(output.status.code(), Some(3));
    assert_eq!(
        diagnostic_codes(&parse_json(&output.stdout)),
        vec!["state_busy"]
    );
    let (code, inv) = project.json(&[
        "invalidate",
        "all",
        "--reason",
        "Lock contention test reason.",
    ]);
    assert_eq!(code, 3);
    assert_eq!(diagnostic_codes(&inv), vec!["state_busy"]);
    drop(guard);
    let output = ack_with(&project, "src/execution/README.md", &packet, &token);
    assert_eq!(output.status.code(), Some(0));
    // An edit after commit is simply a later change that `check` reports.
    project.append("src/execution/runner.rs", "// after\n");
    let (code, check) = project.json(&["check"]);
    assert_eq!(code, 1);
    assert!(diagnostic_codes(&check).contains(&"review_pending".to_string()));
}
