//! The central docs/ site cookbook: a page in `docs/` sees code changes only
//! through the summaries that it imports from READMEs beside the code. Every
//! file block and output block on the page comes from this real run.

mod common;

use std::fs;

use common::*;

const PAGE: &str = "docs/cookbooks/central-docs/README.md";

/// The guide before the pattern: it links the code and repeats its limits.
const PITFALL_GUIDE: &str = "# User guide

<!-- memoria:export id=\"intro\" -->
Keyhole signs you in to the team tools.
<!-- /memoria:export -->

## Sign in

Open the sign-in page and enter your email and password.
A session ends after 30 minutes without activity. After 5 failed sign-in attempts, the account locks.

If your account locks, ask an administrator to unlock it.
The [login code](../src/auth/login.rs) holds these limits.
";

/// Seed the fixture, acknowledge every document, and commit the state.
fn central_docs(guide: Option<&str>) -> Project {
    let project = Project::seed_from("central-docs");
    if let Some(guide) = guide {
        project.write("docs/guide.md", guide);
    }
    project.canonical_loop();
    let (code, value) = project.json(&["check"]);
    assert_eq!(code, 0, "check failed: {value:?}");
    project.commit_all("baseline");
    project
}

/// The one artifact saved for `document` in the project's packet directory.
fn saved(project: &Project, document: &str) -> String {
    let prefix = format!("memoria-manifest-{}-", document.replace('/', "_"));
    let names: Vec<String> = fs::read_dir(project.packets.path())
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .filter(|name| name.starts_with(&prefix))
        .collect();
    assert_eq!(names.len(), 1, "{prefix}: {names:?}");
    project
        .packets
        .path()
        .join(&names[0])
        .to_str()
        .unwrap()
        .to_string()
}

fn ack(project: &Project, document: &str, result: &str, note: &str) -> String {
    let packet = saved(project, document);
    let text = cookbook_run(
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
    fs::remove_file(packet).unwrap();
    text
}

#[test]
fn the_central_docs_cookbook_matches_a_real_run() {
    let page = fs::read_to_string(std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join(PAGE))
        .unwrap_or_default();
    let print = std::env::var_os("MEMORIA_PRINT_COOKBOOK").is_some();
    let mut captured: Vec<(&str, String)> = Vec::new();

    // Scenario 1: the guide only links the code.
    let pitfall = central_docs(Some(PITFALL_GUIDE));
    if !print {
        assert_eq!(
            format!(
                "{}\n",
                cookbook_block(&page, "file", "docs/guide.md (link only)")
            ),
            PITFALL_GUIDE
        );
    }
    pitfall.write(
        "src/auth/login.rs",
        pitfall
            .read_string("src/auth/login.rs")
            .replace("SESSION_MINUTES: u32 = 30;", "SESSION_MINUTES: u32 = 15;"),
    );
    captured.push(("pitfall-plan", cookbook_run(&pitfall, &["review"])));
    assert_eq!(pitfall.pending(), vec!["src/auth/README.md"]);
    captured.push((
        "pitfall-explain",
        cookbook_run(&pitfall, &["status", "--explain", "src/auth/login.rs"]),
    ));
    captured.push(("pitfall-graph", cookbook_run(&pitfall, &["graph"])));
    pitfall.write(
        "docs/guide.md",
        PITFALL_GUIDE
            .replace(
                "## Sign in\n",
                "<!-- memoria:section id=\"sign-in\" files=\"../src/auth/login.rs\" -->\n## Sign in\n",
            )
            .replace(
                "holds these limits.\n",
                "holds these limits.\n<!-- /memoria:section -->\n",
            ),
    );
    captured.push(("pitfall-section", cookbook_run(&pitfall, &["lint"])));

    // Scenario 2: the guide imports the summary of the auth README.
    let project = central_docs(None);
    let save = project.packets.path().to_str().unwrap().to_string();
    for path in [
        "memoria.toml",
        "README.md",
        "docs/guide.md",
        "src/auth/README.md",
        "src/auth/login.rs",
    ] {
        if !print {
            assert_eq!(
                format!("{}\n", cookbook_block(&page, "file", path)),
                project.read_string(path),
                "{path}"
            );
        }
    }
    captured.push(("graph", cookbook_run(&project, &["graph"])));
    // The import gives the guide no coverage of the provider's sources.
    assert_eq!(
        project.covered_by("src/auth/login.rs"),
        "src/auth/README.md"
    );

    // Scenario 3: a code change that leaves the summary true.
    project.append(
        "src/auth/login.rs",
        "\n/// Whether `attempts` failed sign-ins lock the account.\npub fn is_locked(attempts: u32) -> bool {\n    attempts >= MAX_ATTEMPTS\n}\n",
    );
    captured.push(("refactor-plan", cookbook_run(&project, &["review"])));
    assert_eq!(project.pending(), vec!["src/auth/README.md"]);
    cookbook_run(&project, &["review", "src/auth/README.md", "--save", &save]);
    captured.push((
        "refactor-ack",
        ack(
            &project,
            "src/auth/README.md",
            "no-update",
            "The new is_locked function uses the same limit of 5 attempts, so the summary stays true.",
        ),
    ));
    captured.push(("refactor-done", cookbook_run(&project, &["review"])));
    assert!(project.pending().is_empty());
    project.commit_all("is_locked");

    // Scenario 4: a code change that makes the summary wrong.
    project.write(
        "src/auth/login.rs",
        project
            .read_string("src/auth/login.rs")
            .replace("SESSION_MINUTES: u32 = 30;", "SESSION_MINUTES: u32 = 15;"),
    );
    captured.push(("change-plan", cookbook_run(&project, &["review"])));
    assert_eq!(project.pending(), vec!["src/auth/README.md"]);
    project.write(
        "src/auth/README.md",
        project
            .read_string("src/auth/README.md")
            .replace("after 30 minutes", "after 15 minutes"),
    );
    captured.push(("summary-plan", cookbook_run(&project, &["review"])));
    assert_eq!(
        project.waiting(),
        vec![(
            "docs/guide.md".to_string(),
            vec!["src/auth/README.md".to_string()]
        )]
    );
    captured.push((
        "guide-waits",
        cookbook_run(&project, &["review", "docs/guide.md"]),
    ));
    captured.push((
        "save-auth",
        cookbook_run(&project, &["review", "src/auth/README.md", "--save", &save]),
    ));
    captured.push((
        "ack-auth",
        ack(
            &project,
            "src/auth/README.md",
            "updated",
            "Sessions now end after 15 minutes, and the summary states the new limit.",
        ),
    ));
    captured.push(("render-plan", cookbook_run(&project, &["review"])));
    captured.push(("render", cookbook_run(&project, &["render"])));
    assert!(
        project
            .read_string("docs/guide.md")
            .contains("A session ends after 15 minutes without activity.")
    );
    captured.push((
        "save-guide",
        cookbook_run(&project, &["review", "docs/guide.md", "--save", &save]),
    ));
    captured.push((
        "ack-guide",
        ack(
            &project,
            "docs/guide.md",
            "no-update",
            "The imported summary states 15 minutes, and the prose around it names no time limit.",
        ),
    ));
    captured.push(("check", cookbook_run(&project, &["check"])));
    project.commit_all("fifteen minutes");

    // Scenario 5: an export body with a relative link.
    let readme = project.read_string("src/auth/README.md");
    project.write(
        "src/auth/README.md",
        readme.replace(
            "the account locks.\n<!-- /memoria:export -->",
            "the account locks. See [login.rs](login.rs).\n<!-- /memoria:export -->",
        ),
    );
    captured.push(("export-link", cookbook_run(&project, &["lint"])));
    project.write("src/auth/README.md", &readme);

    for (key, text) in &captured {
        if print {
            println!("<<<{key}\n{text}\n>>>");
        } else {
            assert_eq!(&cookbook_block(&page, "output", key), text, "{key}");
        }
    }
}
