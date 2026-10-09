//! The README tree cookbook: every block on its page matches a real run of
//! its fixture project.

mod common;

use std::fs;

use common::*;
use memoria_infrastructure::json;

const PAGE: &str = "docs/cookbooks/readme-tree/README.md";

/// The README tree fixture: three folder READMEs, a root that imports two
/// summaries and links the third README, central guidance with one guidance
/// file, and one sidecar. Every document is acknowledged and committed.
fn readme_tree() -> Project {
    let project = Project::seed_from("readme-tree");
    project.canonical_loop();
    let (code, value) = project.json(&["check"]);
    assert_eq!(code, 0, "check failed: {}", json::to_pretty(&value));
    project.commit_all("baseline");
    project
}

/// Save a fresh artifact of `document` into the masked directory, then
/// acknowledge it as `docs-agent`. Returns the human output of the ack.
fn save_and_ack(project: &Project, document: &str, result: &str, note: &str) -> String {
    let save = project.packets.path().to_str().unwrap().to_string();
    let output = project.run(&["review", document, "--save", &save]);
    assert_eq!(output.status.code(), Some(0), "save {document}");
    let prefix = format!("memoria-manifest-{}-", document.replace('/', "_"));
    let mut names: Vec<_> = fs::read_dir(project.packets.path())
        .unwrap()
        .map(|e| e.unwrap().path())
        .filter(|p| {
            p.file_name()
                .unwrap()
                .to_string_lossy()
                .starts_with(&prefix)
        })
        .collect();
    assert_eq!(names.len(), 1, "{names:?}");
    let packet = names.pop().unwrap();
    let text = cookbook_run(
        project,
        &[
            "ack",
            document,
            "--packet",
            packet.to_str().unwrap(),
            "--reviewer",
            "docs-agent",
            "--result",
            result,
            "--note",
            note,
        ],
    );
    fs::remove_file(packet).unwrap();
    text
}

fn replace(project: &Project, path: &str, from: &str, to: &str) {
    let text = project.read_string(path);
    assert!(text.contains(from), "{path} contains {from:?}");
    project.write(path, text.replace(from, to));
}

#[test]
fn the_readme_tree_cookbook_matches_a_real_run() {
    let page = fs::read_to_string(std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join(PAGE))
        .unwrap_or_default();
    let print = std::env::var_os("MEMORIA_PRINT_COOKBOOK").is_some();
    let mut captured: Vec<(&str, String)> = Vec::new();
    let project = readme_tree();

    // The files the cookbook shows are the fixture files.
    for path in [
        "memoria.toml",
        "docs/writing-guide.md",
        "README.md",
        "services/api/README.md",
        "services/api/README.memoria.toml",
    ] {
        if !print {
            assert_eq!(
                format!("{}\n", cookbook_block(&page, "file", path)),
                project.read_string(path),
                "{path}"
            );
        }
    }

    // Scenario 1: the pattern.
    captured.push(("graph", cookbook_run(&project, &["graph"])));
    captured.push((
        "guidance-api",
        cookbook_run(&project, &["guidance", "services/api/README.md"]),
    ));

    // Scenario 2: a refactor keeps the summary true.
    project.write(
        "services/api/src/orders.rs",
        project
            .read_string("services/api/src/orders.rs")
            .replace("items", "by_id"),
    );
    captured.push(("plan-refactor", cookbook_run(&project, &["review"])));
    assert_eq!(project.pending(), vec!["services/api/README.md"]);
    assert_eq!(project.status_label("README.md"), "current");
    captured.push((
        "review-refactor",
        cookbook_run(&project, &["review", "services/api/README.md"]),
    ));
    captured.push((
        "ack-refactor",
        save_and_ack(
            &project,
            "services/api/README.md",
            "no-update",
            "The rename is internal. The endpoints and the summary stay true.",
        ),
    ));
    captured.push(("plan-after-refactor", cookbook_run(&project, &["review"])));
    assert!(project.pending().is_empty());
    project.commit_all("refactor");

    // Scenario 3: a behavior change changes the exported summary.
    replace(
        &project,
        "services/api/src/orders.rs",
        "use std::collections::HashMap;\n",
        "use std::collections::HashMap;\nuse std::fs::File;\nuse std::io::Write;\n",
    );
    replace(
        &project,
        "services/api/src/orders.rs",
        "    by_id: HashMap<u64, String>,\n",
        "    by_id: HashMap<u64, String>,\n    log: File,\n",
    );
    replace(
        &project,
        "services/api/src/orders.rs",
        "        self.by_id.insert(id, book);\n",
        "        writeln!(self.log, \"{id}\\t{book}\").unwrap();\n        self.by_id.insert(id, book);\n",
    );
    captured.push(("plan-behavior", cookbook_run(&project, &["review"])));
    replace(
        &project,
        "services/api/README.md",
        "accepts orders over HTTP and keeps them in memory.",
        "accepts orders over HTTP, keeps them in memory, and appends each one to `orders.log`.",
    );
    captured.push((
        "ack-behavior",
        save_and_ack(
            &project,
            "services/api/README.md",
            "updated",
            "The API now appends each order to orders.log, and the summary says so.",
        ),
    ));
    captured.push(("plan-render", cookbook_run(&project, &["review"])));
    captured.push(("render", cookbook_run(&project, &["render"])));
    captured.push((
        "review-root",
        cookbook_run(&project, &["review", "README.md"]),
    ));
    captured.push((
        "ack-root",
        save_and_ack(
            &project,
            "README.md",
            "no-update",
            "The new API summary fits the root page. The root text around it stays true.",
        ),
    ));
    captured.push(("check", cookbook_run(&project, &["check"])));
    assert!(project.pending().is_empty());
    project.commit_all("orders log");

    // Scenario 4, contrast: an edit to the sidecar reaches only its subtree.
    let sidecar = project.read_string("services/api/README.memoria.toml");
    replace(
        &project,
        "services/api/README.memoria.toml",
        "and say what it returns.",
        "say what it returns, and name its error codes.",
    );
    captured.push((
        "changed-sidecar",
        cookbook_run(&project, &["guidance", "--changed"]),
    ));
    project.write("services/api/README.memoria.toml", &sidecar);

    // Scenario 4: a central guideline changes.
    project.append(
        "docs/writing-guide.md",
        "4. In a service README, list each environment variable that the service reads.\n",
    );
    captured.push(("plan-guide", cookbook_run(&project, &["review"])));
    assert!(project.pending().is_empty());
    captured.push((
        "changed-guide",
        cookbook_run(&project, &["guidance", "--changed"]),
    ));
    captured.push((
        "invalidate",
        cookbook_run(
            &project,
            &[
                "invalidate",
                "subtree:services",
                "--reason",
                "The writing guide now asks each service README to list its environment variables.",
            ],
        ),
    ));
    assert_eq!(
        project.pending(),
        vec!["services/api/README.md", "services/web/README.md"]
    );
    captured.push((
        "review-web",
        cookbook_run(&project, &["review", "services/web/README.md"]),
    ));
    captured.push((
        "ack-api-guide",
        save_and_ack(
            &project,
            "services/api/README.md",
            "no-update",
            "The API reads no environment variable, so the README needs no list.",
        ),
    ));
    project.append(
        "services/web/README.md",
        "\n## Environment\n\n- `API_URL` sets the address of the orders API. The default is `http://localhost:8080`.\n",
    );
    captured.push((
        "ack-web-guide",
        save_and_ack(
            &project,
            "services/web/README.md",
            "updated",
            "The README lists API_URL, the one variable that cart.ts reads.",
        ),
    ));
    captured.push(("plan-after-guide", cookbook_run(&project, &["review"])));
    assert!(project.pending().is_empty());
    assert_eq!(project.run(&["check"]).status.code(), Some(0));
    project.commit_all("environment");

    // Scenario 5: a README that its parent does not link.
    project.write(
        "services/mailer/README.md",
        "# Mailer\n\nThis folder owns the mailer. It sends an email for each new order.\nRun `cargo run` in this folder.\n",
    );
    project.write(
        "services/mailer/src/mail.rs",
        "pub fn send(to: &str, body: &str) {\n    println!(\"mail to {to}: {body}\");\n}\n",
    );
    captured.push(("lint-mailer", cookbook_run(&project, &["lint"])));
    captured.push((
        "explain-mailer",
        cookbook_run(
            &project,
            &["status", "--explain", "services/mailer/src/mail.rs"],
        ),
    ));
    replace(
        &project,
        "README.md",
        "## Libraries\n",
        "- [services/mailer](services/mailer/README.md) sends an email for each new order.\n\n## Libraries\n",
    );
    captured.push(("lint-fixed", cookbook_run(&project, &["lint"])));
    let explained = cookbook_run(
        &project,
        &["status", "--explain", "services/mailer/src/mail.rs"],
    );
    assert!(
        explained.contains("  covered by services/mailer/README.md\n"),
        "{explained}"
    );
    assert_eq!(
        project.pending(),
        vec!["README.md", "services/mailer/README.md"]
    );

    for (key, text) in &captured {
        if print {
            println!("<<<{key}\n{text}\n>>>");
        } else {
            assert_eq!(&cookbook_block(&page, "output", key), text, "{key}");
        }
    }
}
