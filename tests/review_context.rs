//! Snapshot views, opt-in review preparation, and verified historical references.
mod common;
use common::*;
use memoria_domain::DocumentId;
use memoria_infrastructure::json::{self, Json};
use std::fs;

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

fn selection(p: &Project, packet: &std::path::Path, trust: bool) -> Json {
    let mut args = vec![
        "packet",
        "view",
        packet.to_str().unwrap(),
        "--section",
        "incremental",
    ];
    if trust {
        args.push("--trust-prior-review");
    }
    let (code, view) = p.json(&args);
    assert_eq!(code, 0, "{}", json::to_pretty(&view));
    get(&view, &["data", "selection"]).clone()
}

fn items(value: &Json) -> &[Json] {
    let Json::Array(items) = value else {
        panic!("expected array")
    };
    items
}

#[test]
fn saved_views_are_offline_exact_and_never_canonical() {
    let p = simple(true);
    p.append("request.rs", "// A clarification.\n");
    let saved = p.read_string("request.rs");
    let (packet, token) = p.review_packet("README.md");
    p.write("request.rs", "different live bytes\n");
    let before = p.tree_snapshot();
    let out = p.run_in(
        p.packets.path(),
        &[
            "packet",
            "view",
            packet.to_str().unwrap(),
            "--file",
            "request.rs",
            "--format",
            "json",
        ],
    );
    assert_eq!(out.status.code(), Some(0));
    let view = parse_json(&out.stdout);
    assert_eq!(get_str(&view, &["data", "selection", "body"]), saved);
    assert_eq!(get_str(&view, &["data", "kind"]), "packet_view");
    assert!(!get_bool(&view, &["data", "canonical"]));
    let projection = p.packets.path().join("projection.json");
    fs::write(&projection, &out.stdout).unwrap();
    assert_ne!(
        p.ack("README.md", &projection, &token, "no-update", NOTE)
            .status
            .code(),
        Some(0)
    );
    assert_ne!(
        p.ack("README.md", &packet, &token, "no-update", NOTE)
            .status
            .code(),
        Some(0)
    );
    assert_eq!(p.tree_snapshot(), before);
    let corrupt = fs::read_to_string(&packet)
        .unwrap()
        .replace("A clarification.", "A falsification.");
    fs::write(&packet, corrupt).unwrap();
    let (code, result) = p.json(&["packet", "view", packet.to_str().unwrap()]);
    assert_ne!(code, 0);
    assert!(diagnostic_codes(&result).contains(&"packet_integrity_failed".to_string()));
}

#[test]
fn incremental_is_opt_in_and_keeps_semantic_context_retrievable() {
    let p = simple(true);
    // This is a semantic control: the README still says ten, but requests bypass the limit.
    p.write("request.rs", "// limits.rs defines LIMIT; this path incorrectly bypasses it.\npub const UNLIMITED: bool = true;\n");
    let (packet, _) = p.review_packet("README.md");
    assert!(get_bool(
        &selection(&p, &packet, false),
        &["full_review_required"]
    ));
    let view = selection(&p, &packet, true);
    assert!(!get_bool(&view, &["model_quality_gate_passed"]));
    assert!(!get_bool(&view, &["full_review_required"]));
    assert_eq!(items(get(&view, &["files"])).len(), 1);
    assert!(get_str(&view, &["readme", "body"]).contains("limit is ten"));
    let coverage = items(get(&view, &["coverage"]));
    assert_eq!(coverage.len(), 3);
    let contract = coverage
        .iter()
        .find(|x| get_str(x, &["path"]) == "limits.rs")
        .unwrap();
    assert_eq!(
        get_str(contract, &["required_disposition"]),
        "justify_reuse_or_examine"
    );
    assert!(!get_bool(contract, &["reviewed_by_this_view"]));
    let (_, retrieved) = p.json(&[
        "packet",
        "view",
        packet.to_str().unwrap(),
        "--file",
        "limits.rs",
    ]);
    assert!(get_str(&retrieved, &["data", "selection", "body"]).contains("LIMIT: usize = 10"));
    // No model judged this case. The test establishes evidence access, not review quality.
}

#[test]
fn new_guidance_invalidation_and_path_changes_require_full_review() {
    for variant in [
        "new",
        "guidance",
        "policy",
        "invalidation",
        "add",
        "delete",
        "rename",
        "move",
        "missing-history",
    ] {
        let p = if variant == "new" {
            let p = Project::empty_repo();
            p.write("README.md", "# New boundary\n");
            p.run(&["init", "--apply"]);
            p
        } else {
            simple(variant != "missing-history")
        };
        match variant {
            "new" => {}
            "guidance" => p.write(
                "README.memoria.toml",
                "[documentation]\nguidance = [\"Explain the request limit and all exceptions.\"]\n",
            ),
            "policy" => p.write(".gitignore", "irrelevant-future-path\n"),
            "invalidation" => {
                p.run(&[
                    "invalidate",
                    "doc:README.md",
                    "--reason",
                    "Reconsider all request assumptions",
                ]);
            }
            "add" => p.write("added.rs", "new input\n"),
            "delete" => p.remove("limits.rs"),
            "rename" => {
                fs::rename(p.root.join("limits.rs"), p.root.join("renamed.rs")).unwrap();
            }
            "move" => {
                p.write("nested/README.md", "# New child\n");
                fs::rename(p.root.join("limits.rs"), p.root.join("nested/limits.rs")).unwrap();
            }
            _ => {}
        }
        if ["guidance", "missing-history"].contains(&variant) {
            p.append("request.rs", "// Additional context.\n");
        }
        let (packet, _) = p.review_packet("README.md");
        let view = selection(&p, &packet, true);
        assert!(get_bool(&view, &["full_review_required"]), "{variant}");
        assert!(!items(get(&view, &["fallback_reasons"])).is_empty());
    }
}

#[test]
fn ack_records_only_full_verified_content_and_later_commit_recovers_hunks() {
    let p = simple(true);
    let doc = DocumentId::parse("README.md").unwrap();
    assert!(p.decoded_state().reviews[&doc].git.base_commit.is_some());
    p.append("request.rs", "// Dirty but reviewed bytes.\n");
    let (packet, token) = p.review_packet("README.md");
    let (code, result) = p.ack_json("README.md", &packet, &token, "no-update", NOTE);
    assert_eq!(code, 0);
    assert!(diagnostic_codes(&result).contains(&"historical_coverage".to_string()));
    assert!(p.decoded_state().reviews[&doc].git.base_commit.is_none());
    assert_eq!(p.json(&["check"]).0, 0);
    let state = p.state();
    p.commit_all("later commit of exactly reviewed bytes");
    p.append("request.rs", "// Next edit.\n");
    let (_, explain) = p.json(&["explain", "README.md"]);
    let evidence = &items(get(&explain, &["data", "evidence"]))[0];
    assert!(get_bool(evidence, &["baseline_verified"]));
    assert!(get_str(evidence, &["text"]).contains("+// Next edit."));
    assert_ne!(get(evidence, &["base_commit"]), &Json::Null);
    assert_eq!(
        p.state(),
        state,
        "read-only recovery must not rewrite attribution"
    );
}

#[test]
fn bounded_candidate_search_and_shallow_history_do_not_guess() {
    let p = simple(false);
    p.commit_all("matching reviewed bytes");
    p.write("request.rs", "replacement content\n");
    p.commit_all("different bytes");
    for _ in 0..64 {
        p.git(&["commit", "--allow-empty", "-qm", "history distance"]);
    }
    let (_, result) = p.json(&["explain", "README.md"]);
    let e = &items(get(&result, &["data", "evidence"]))[0];
    assert!(!get_bool(e, &["baseline_verified"]));
    assert_eq!(get_str(e, &["reason_code"]), "history_limit_exceeded");
    // A shallow boundary makes older objects unreachable to the bounded ancestry search.
    let head = p.git(&["rev-parse", "HEAD"]);
    p.write(".git/shallow", stdout(&head));
    let (_, result) = p.json(&["explain", "README.md"]);
    assert!(!get_bool(
        &items(get(&result, &["data", "evidence"]))[0],
        &["baseline_verified"]
    ));
}

#[test]
fn missing_historical_blob_and_byte_exhaustion_are_explicit() {
    let p = simple(true);
    let old_blob = stdout(&p.git(&["rev-parse", "HEAD:request.rs"]))
        .trim()
        .to_string();
    p.write("request.rs", "changed and committed bytes\n");
    p.commit_all("new current content");
    fs::remove_file(p.root.join(format!(
        ".git/objects/{}/{}",
        &old_blob[..2],
        &old_blob[2..]
    )))
    .unwrap();
    let (_, result) = p.json(&["explain", "README.md"]);
    let e = &items(get(&result, &["data", "evidence"]))[0];
    assert!(!get_bool(e, &["baseline_verified"]));
    assert_eq!(get_str(e, &["reason_code"]), "blob_unavailable");

    let p = simple(false);
    p.write("request.rs", vec![b'x'; 33 * 1024 * 1024]);
    p.commit_all("oversized unrelated historical object");
    p.write("request.rs", "small current content\n");
    let (_, result) = p.json(&["explain", "README.md"]);
    let e = &items(get(&result, &["data", "evidence"]))[0];
    assert!(!get_bool(e, &["baseline_verified"]));
    assert_eq!(get_str(e, &["reason_code"]), "history_limit_exceeded");
}

#[test]
fn large_nested_markdown_and_fanout_views_keep_boundaries_and_import_evidence() {
    for shape in ["large", "nested", "markdown", "fanout"] {
        let p = Project::empty_repo();
        p.write("README.md", "# Root\n");
        let doc = if shape == "nested" {
            "a/b/c/d/e/f/g/h/README.md"
        } else if shape == "fanout" {
            "provider/README.md"
        } else {
            "README.md"
        };
        p.write(doc, "# Owner\n\n<!-- memoria:export id=\"summary\" -->\nThe constant is ten.\n<!-- /memoria:export -->\n");
        let prefix = doc.strip_suffix("README.md").unwrap();
        let count = if shape == "large" {
            256
        } else if shape == "markdown" {
            64
        } else {
            3
        };
        let extension = if shape == "markdown" { "md" } else { "rs" };
        for i in 0..count {
            p.write(
                &format!("{prefix}input{i:03}.{extension}"),
                "// The constant is ten.\n".repeat(64),
            );
        }
        if shape == "fanout" {
            for i in 0..8 {
                p.write(&format!("consumer{i}/README.md"), "# Consumer\n\n<!-- memoria:import src=\"../provider/README.md#summary\" -->\n<!-- /memoria:import -->\n");
            }
        }
        p.run(&["init", "--apply"]);
        p.run(&["render"]);
        p.commit_all("source before review");
        p.baseline();
        p.append(&format!("{prefix}input000.{extension}"), "\n");
        let human = p.run(&["review", doc]);
        assert_eq!(human.status.code(), Some(0));
        assert!(
            human.stdout.len() < 4096,
            "{shape}: {} bytes",
            human.stdout.len()
        );
        assert!(stdout(&human).contains("@@"));
        let (packet, _) = p.review_packet(doc);
        let view = selection(&p, &packet, true);
        assert_eq!(
            get_bool(&view, &["full_review_required"]),
            shape == "fanout"
        );
        if shape == "fanout" {
            p.write("provider/README.md", "# Owner\n\n<!-- memoria:export id=\"summary\" -->\nThe constant is eleven.\n<!-- /memoria:export -->\n");
            p.ack_ok("provider/README.md");
            p.run(&["render"]);
            let (_, result) = p.json(&["explain", "consumer0/README.md"]);
            let imported = items(get(&result, &["data", "evidence"]))
                .iter()
                .find(|e| get_str(e, &["identity"]).contains('#'))
                .unwrap();
            assert!(get_bool(imported, &["baseline_verified"]));
            let (packet, _) = p.review_packet("consumer0/README.md");
            assert!(get_bool(
                &selection(&p, &packet, true),
                &["full_review_required"]
            ));
        }
    }
}

#[test]
fn source_only_commit_is_the_review_baseline_and_content_hashes_remain_required() {
    let p = simple(true);
    p.append("request.rs", "// Source-only change.\n");
    p.commit_all("source-only review snapshot");
    let head = stdout(&p.git(&["rev-parse", "HEAD"])).trim().to_string();
    p.ack_ok("README.md");
    let doc = DocumentId::parse("README.md").unwrap();
    assert_eq!(
        p.decoded_state().reviews[&doc].git.base_commit.as_deref(),
        Some(head.as_str())
    );
    p.append("request.rs", "// Next edit.\n");
    let (packet, _) = p.review_packet("README.md");
    let mut value = parse_json(&fs::read(&packet).unwrap());
    // Recompute the outer digest after tampering: the independent content hash still refuses it.
    if let Json::Object(envelope) = &mut value {
        let Json::Object(data) = envelope.get_mut("data").unwrap() else {
            panic!()
        };
        let Json::Object(content) = data.get_mut("content").unwrap() else {
            panic!()
        };
        let Json::Object(readme) = content.get_mut("readme").unwrap() else {
            panic!()
        };
        let Json::String(body) = readme.get_mut("body").unwrap() else {
            panic!()
        };
        *body = body.replace("ten", "six");
        data.remove("packet_digest");
    }
    let digest =
        memoria_infrastructure::packet::packet_digest(&memoria_infrastructure::Xxh3Hasher, &value);
    if let Json::Object(envelope) = &mut value {
        let Json::Object(data) = envelope.get_mut("data").unwrap() else {
            panic!()
        };
        data.insert("packet_digest".into(), Json::String(digest));
    }
    fs::write(&packet, json::to_pretty(&value)).unwrap();
    let (code, result) = p.json(&["packet", "view", packet.to_str().unwrap()]);
    assert_ne!(code, 0);
    assert!(diagnostic_codes(&result).contains(&"packet_content_mismatch".into()));
}

#[test]
fn expired_historical_deadline_is_a_bounded_refusal() {
    use memoria_application::ports::GitRepository;
    let p = simple(true);
    let git = memoria_infrastructure::git::GitCli::discover(&p.root).unwrap();
    let head = stdout(&p.git(&["rev-parse", "HEAD"])).trim().to_string();
    let deadline = std::time::Instant::now() - std::time::Duration::from_millis(1);
    let error = git
        .historical_blob(&head, "request.rs", 1024, deadline)
        .unwrap_err();
    assert_eq!(error.operation, "history_limit");
    let error = git.recent_commits(64, deadline).unwrap_err();
    assert_eq!(error.operation, "history_limit");
}
