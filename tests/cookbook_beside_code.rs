//! The "documents beside the code" cookbook: every file block and every
//! output block on its page comes from this run of its fixture.

mod common;

use std::fs;

use common::{Project, cookbook_block, cookbook_run};
use memoria_infrastructure::json;

const PAGE: &str = "docs/cookbooks/beside-code/README.md";

/// The beside-code fixture: three READMEs and two opted-in pages beside the
/// code they describe. Every document is acknowledged and committed.
fn beside_code() -> Project {
    let project = Project::seed_from("beside-code");
    project.canonical_loop();
    let (code, value) = project.json(&["check"]);
    assert_eq!(code, 0, "check failed: {}", json::to_pretty(&value));
    project.commit_all("baseline");
    project
}

/// The lines of `text` from the line that starts with `from` up to, and not
/// including, the next line that starts with `to`.
fn excerpt(text: &str, from: &str, to: &str) -> String {
    let lines: Vec<&str> = text.lines().collect();
    let start = lines
        .iter()
        .position(|line| line.starts_with(from))
        .unwrap_or_else(|| panic!("no line starts with {from:?} in:\n{text}"));
    // `END` takes every line to the end of the text.
    let end = if to == "END" {
        lines.len()
    } else {
        start
            + 1
            + lines[start + 1..]
                .iter()
                .position(|line| line.starts_with(to))
                .unwrap_or_else(|| panic!("no line starts with {to:?} in:\n{text}"))
    };
    lines[start..end].join("\n")
}

/// The one saved artifact of `document` in the project's save directory.
fn saved(project: &Project, document: &str) -> String {
    let prefix = format!("memoria-manifest-{}-", document.replace('/', "_"));
    let mut names: Vec<String> = fs::read_dir(project.packets.path())
        .unwrap()
        .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
        .filter(|name| name.starts_with(&prefix))
        .collect();
    assert_eq!(names.len(), 1, "{names:?}");
    let path = project.packets.path().join(names.pop().unwrap());
    path.to_str().unwrap().to_string()
}

/// Save a fresh artifact of `document`, acknowledge it as docs-agent, and
/// return both outputs. The artifact is removed after the acknowledgement.
fn save_and_ack(project: &Project, document: &str, result: &str, note: &str) -> (String, String) {
    let save = project.packets.path().to_str().unwrap().to_string();
    let review = cookbook_run(project, &["review", document, "--save", &save]);
    let packet = saved(project, document);
    let ack = cookbook_run(
        project,
        &[
            "ack",
            document,
            "--packet",
            &packet,
            "--reviewer",
            "docs-agent",
            "--result",
            result,
            "--note",
            note,
        ],
    );
    assert!(ack.starts_with("Recorded "), "{ack}");
    fs::remove_file(&packet).unwrap();
    (review, ack)
}

fn edit(project: &Project, path: &str, from: &str, to: &str) {
    let text = project.read_string(path);
    assert!(text.contains(from), "{path} has no {from:?}");
    project.write(path, text.replace(from, to));
}

#[test]
fn the_beside_code_cookbook_matches_a_real_run() {
    let page = fs::read_to_string(std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join(PAGE))
        .unwrap_or_default();
    let print = std::env::var_os("MEMORIA_PRINT_COOKBOOK").is_some();
    let mut captured: Vec<(&str, String)> = Vec::new();
    let project = beside_code();

    // The files the page shows are the fixture files.
    for path in [
        "memoria.toml",
        "README.md",
        "billing/README.md",
        "billing/design.md",
        "billing/tax.rs",
        "deploy/runbook.md",
    ] {
        if !print {
            assert_eq!(
                format!("{}\n", cookbook_block(&page, "file", path)),
                project.read_string(path),
                "{path}"
            );
        }
    }

    // Scenario 1: the pattern. One source, two documents.
    captured.push((
        "status-explain",
        cookbook_run(&project, &["status", "--explain", "billing/tax.rs"]),
    ));

    // Scenario 2: an unmarked page is a source, even when a README links it.
    project.append(
        "billing/notes.md",
        "\n## Currency\n\nEvery amount is in euro cents. We have no plan for a second currency.\n",
    );
    captured.push(("plan-notes", cookbook_run(&project, &["review"])));
    assert_eq!(
        project.pending(),
        vec!["billing/README.md", "billing/design.md"]
    );
    captured.push((
        "explain-notes",
        cookbook_run(&project, &["explain", "billing/notes.md"]),
    ));
    edit(
        &project,
        "billing/notes.md",
        "## Credit notes\n\nA refund needs a credit note. We have no design for credit notes yet.\n",
        "<!-- memoria:section id=\"credit-notes\" files=\"invoice.rs\" -->\n## Credit notes\n\nA refund needs a credit note. We have no design for credit notes yet.\n<!-- /memoria:section -->\n",
    );
    if !print {
        assert_eq!(
            format!("{}\n", cookbook_block(&page, "file", "billing/notes.md")),
            project.read_string("billing/notes.md")
        );
    }
    captured.push(("plan-opt-in", cookbook_run(&project, &["review"])));
    assert_eq!(
        project.pending(),
        vec!["billing/README.md", "billing/design.md", "billing/notes.md"]
    );
    captured.push((
        "review-design-opt-in",
        cookbook_run(&project, &["review", "billing/design.md"]),
    ));
    captured.push((
        "review-notes-first",
        excerpt(
            &cookbook_run(&project, &["review", "billing/notes.md"]),
            "How to read:",
            "Next:",
        ),
    ));
    project.canonical_loop();
    assert!(project.pending().is_empty());
    project.commit_all("notes");

    // Scenario 3: a code change reaches every document of its folder.
    edit(
        &project,
        "billing/tax.rs",
        "/// The VAT on a subtotal, rounded down to a whole cent.\npub fn vat(subtotal_cents: u64) -> u64 {\n    subtotal_cents * VAT_BASIS_POINTS / 10_000\n",
        "/// The VAT on a subtotal, rounded to the nearest cent. A half cent rounds up.\npub fn vat(subtotal_cents: u64) -> u64 {\n    (subtotal_cents * VAT_BASIS_POINTS + 5_000) / 10_000\n",
    );
    captured.push(("plan-tax", cookbook_run(&project, &["review"])));
    assert_eq!(
        project.pending(),
        vec!["billing/README.md", "billing/design.md", "billing/notes.md"]
    );
    captured.push((
        "review-design-tax",
        cookbook_run(&project, &["review", "billing/design.md"]),
    ));
    captured.push((
        "review-readme-tax",
        excerpt(
            &cookbook_run(&project, &["review", "billing/README.md"]),
            "How to read:",
            "Next:",
        ),
    ));
    edit(
        &project,
        "billing/design.md",
        "The tax rounds down to a whole cent, so a customer never pays more than the rate.",
        "The tax rounds to the nearest cent. A half cent rounds up.",
    );
    let (save, ack) = save_and_ack(
        &project,
        "billing/design.md",
        "updated",
        "The tax section now says that VAT rounds to the nearest cent, half up.",
    );
    captured.push(("save-design", save));
    captured.push(("ack-design", ack));
    // README.md and notes.md say nothing about rounding.
    project.ack_ok("billing/README.md");
    project.ack_ok("billing/notes.md");
    assert!(project.pending().is_empty());
    project.commit_all("rounding");

    // Scenario 4: a mapped file is renamed.
    project.git(&["mv", "deploy/deploy.sh", "deploy/release.sh"]);
    captured.push(("lint-rename", cookbook_run(&project, &["lint"])));
    captured.push((
        "review-runbook-rename",
        excerpt(
            &cookbook_run(&project, &["review", "deploy/runbook.md"]),
            "How to read:",
            "Next:",
        ),
    ));
    // The fix maps every shell script, so the next rename needs no edit.
    edit(
        &project,
        "deploy/runbook.md",
        "files=\"deploy.sh\"",
        "files=\"*.sh\"",
    );
    for path in ["deploy/runbook.md", "deploy/README.md"] {
        let text = project.read_string(path);
        project.write(path, text.replace("deploy.sh", "release.sh"));
    }
    captured.push(("lint-fixed", cookbook_run(&project, &["lint"])));
    let (save, _) = save_and_ack(
        &project,
        "deploy/runbook.md",
        "updated",
        "The release step names release.sh, and the release section maps every shell script.",
    );
    captured.push((
        "save-runbook-fixed",
        excerpt(&save, "How to read:", "Next:"),
    ));
    save_and_ack(
        &project,
        "deploy/README.md",
        "updated",
        "The file list names release.sh instead of deploy.sh.",
    );
    assert!(project.pending().is_empty());
    project.commit_all("rename");

    // Scenario 5: a pattern follows a new file.
    project.write(
        "deploy/rollback.sh",
        "#!/bin/sh\n# Return the server to the previous release.\nset -eu\nscp ledger.prev ledger.example.com:/srv/ledger/ledger\n",
    );
    captured.push(("plan-rollback", cookbook_run(&project, &["review"])));
    assert_eq!(
        project.pending(),
        vec!["deploy/README.md", "deploy/runbook.md"]
    );
    captured.push((
        "review-runbook-added",
        cookbook_run(&project, &["review", "deploy/runbook.md"]),
    ));
    edit(
        &project,
        "deploy/runbook.md",
        "The release succeeded when the page shows `ok`.\n",
        "The release succeeded when the page shows `ok`.\n3. If the page does not show `ok`, run `./rollback.sh`.\n",
    );
    save_and_ack(
        &project,
        "deploy/runbook.md",
        "updated",
        "The release section now says to run rollback.sh when the health page fails.",
    );
    edit(
        &project,
        "deploy/README.md",
        "- `release.sh` builds the release binary and copies it to the server.\n",
        "- `release.sh` builds the release binary and copies it to the server.\n- `rollback.sh` returns the server to the previous release.\n",
    );
    save_and_ack(
        &project,
        "deploy/README.md",
        "updated",
        "The file list now names rollback.sh and what it does.",
    );
    assert!(project.pending().is_empty());
    project.commit_all("rollback");
    captured.push((
        "explain-runbook",
        excerpt(
            &cookbook_run(&project, &["status", "--explain", "deploy/runbook.md"]),
            "Explain",
            "END",
        ),
    ));
    edit(
        &project,
        "deploy/rollback.sh",
        "scp ledger.prev",
        "scp -p ledger.prev",
    );
    captured.push((
        "review-runbook-pattern",
        excerpt(
            &cookbook_run(&project, &["review", "deploy/runbook.md"]),
            "How to read:",
            "Next:",
        ),
    ));
    project.ack_ok("deploy/runbook.md");
    project.ack_ok("deploy/README.md");
    assert!(project.pending().is_empty());
    project.commit_all("rollback flag");

    // Scenario 6: a decision changes, and no file does.
    captured.push((
        "invalidate",
        cookbook_run(
            &project,
            &[
                "invalidate",
                "doc:deploy/runbook.md",
                "--reason",
                "The team no longer releases on Fridays.",
            ],
        ),
    ));
    captured.push(("plan-invalidate", cookbook_run(&project, &["review"])));
    assert_eq!(project.pending(), vec!["deploy/runbook.md"]);
    captured.push((
        "review-runbook-invalidate",
        cookbook_run(&project, &["review", "deploy/runbook.md"]),
    ));
    edit(
        &project,
        "deploy/runbook.md",
        "Release on a weekday between 09:00 and 16:00 UTC.",
        "Release from Monday to Thursday, between 09:00 and 16:00 UTC. Do not release on a Friday.",
    );
    let (_, ack) = save_and_ack(
        &project,
        "deploy/runbook.md",
        "updated",
        "The release window now ends on Thursday, as the team decided.",
    );
    captured.push(("ack-runbook", ack));
    captured.push(("check", cookbook_run(&project, &["check"])));

    for (key, text) in &captured {
        if print {
            println!("<<<{key}\n{text}\n>>>");
        } else {
            assert_eq!(&cookbook_block(&page, "output", key), text, "{key}");
        }
    }

    // The root README shows the same run in "The CLI at a glance".
    if !print {
        let readme =
            fs::read_to_string(std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("README.md"))
                .unwrap();
        for key in ["plan-tax", "review-design-tax", "ack-design"] {
            let (_, text) = captured.iter().find(|(k, _)| *k == key).unwrap();
            assert!(
                readme.contains(text.as_str()),
                "README.md does not show the current {key} output"
            );
        }
    }
}
