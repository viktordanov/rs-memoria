//! Regression tests for the round-12 triage finding (MEM-045).

mod common;

use std::collections::BTreeMap;

use common::*;
use memoria_infrastructure::json::{self, Json};

type Corruption<'a> = Box<dyn Fn(&mut Json) + 'a>;

const REASON: &str = "Review every explanation once again.";

fn state_json(project: &Project) -> Json {
    project.inspect_state()
}

fn write_state(project: &Project, value: &Json) -> Vec<u8> {
    let bytes = json::to_pretty(value).into_bytes();
    project.write("memoria.lock", &bytes);
    bytes
}

fn invalidation_mut(value: &mut Json, index: usize) -> &mut BTreeMap<String, Json> {
    let Json::Object(root) = value else { panic!() };
    let Json::Array(items) = root.get_mut("invalidations").unwrap() else {
        panic!()
    };
    let Json::Object(inv) = &mut items[index] else {
        panic!()
    };
    inv
}

fn record_mut<'a>(value: &'a mut Json, document: &str) -> &'a mut BTreeMap<String, Json> {
    let Json::Object(root) = value else { panic!() };
    let Json::Object(reviews) = root.get_mut("reviews").unwrap() else {
        panic!()
    };
    let Json::Object(record) = reviews.get_mut(document).unwrap() else {
        panic!()
    };
    record
}

fn docs(items: &[&str]) -> Json {
    Json::Array(
        items
            .iter()
            .map(|d| Json::String((*d).to_string()))
            .collect(),
    )
}

fn ids(items: &[u64]) -> Json {
    Json::Array(items.iter().map(|id| Json::Number(*id)).collect())
}

const ALL: &[&str] = &[
    "README.md",
    "src/corpus/README.md",
    "src/disconnected/README.md",
    "src/execution/README.md",
    "src/retrieval/README.md",
    "src/retrieval/naive/README.md",
];

/// A genuine baseline written only by the tool: every document reviewed,
/// one completed invalidation (its id acknowledged in a record, no active
/// record left), then active `all`, subtree, and document invalidations.
fn genuine_baseline() -> Project {
    let project = Project::seed();
    project.baseline();
    let invalidate = |scope: &str| {
        assert_eq!(
            project.json(&["invalidate", scope, "--reason", REASON]).0,
            0,
            "{scope}"
        );
    };
    invalidate("doc:src/execution/README.md");
    project.ack_ok("src/execution/README.md");
    assert_eq!(project.json(&["check"]).0, 0);
    invalidate("all");
    invalidate("subtree:src/retrieval");
    invalidate("doc:src/corpus/README.md");
    assert_eq!(project.json(&["status"]).0, 0);
    let state = state_json(&project);
    assert_eq!(
        get(
            &state,
            &[
                "reviews",
                "src/execution/README.md",
                "acknowledged_invalidations"
            ]
        ),
        &ids(&[1]),
        "completed invalidation 1 is acknowledged but no longer active"
    );
    assert_eq!(get_u64(&state, &["next_invalidation_id"]), 5);
    let Json::Array(invalidations) = get(&state, &["invalidations"]) else {
        panic!()
    };
    assert_eq!(
        invalidations
            .iter()
            .map(|i| get_u64(i, &["id"]))
            .collect::<Vec<_>>(),
        vec![2, 3, 4]
    );
    assert_eq!(get(&invalidations[0], &["targets"]), &docs(ALL));
    project
}

// MEM-045
#[test]
fn corrupt_invalidation_references_and_sets_are_rejected_before_any_write() {
    let project = genuine_baseline();
    let good = state_json(&project);
    let good_bytes = project.state();
    let reversed: Vec<&str> = ALL.iter().rev().copied().collect();
    let cases: Vec<(&str, Corruption)> = vec![
        (
            "acknowledged id zero",
            Box::new(|s| {
                record_mut(s, "src/execution/README.md")
                    .insert("acknowledged_invalidations".into(), ids(&[0]));
            }),
        ),
        (
            "acknowledged ids duplicated",
            Box::new(|s| {
                record_mut(s, "src/execution/README.md")
                    .insert("acknowledged_invalidations".into(), ids(&[1, 1]));
            }),
        ),
        (
            "acknowledged ids unsorted",
            Box::new(|s| {
                record_mut(s, "src/execution/README.md")
                    .insert("acknowledged_invalidations".into(), ids(&[2, 1]));
            }),
        ),
        (
            "acknowledged id beyond counter",
            Box::new(|s| {
                record_mut(s, "src/execution/README.md")
                    .insert("acknowledged_invalidations".into(), ids(&[1, 5]));
            }),
        ),
        (
            "targets duplicated nonadjacently",
            Box::new(|s| {
                invalidation_mut(s, 0).insert(
                    "targets".into(),
                    docs(&["README.md", "src/corpus/README.md", "README.md"]),
                );
                invalidation_mut(s, 0).insert("pending_documents".into(), docs(&["README.md"]));
            }),
        ),
        (
            "targets unsorted",
            Box::new(|s| {
                invalidation_mut(s, 0).insert("targets".into(), docs(&reversed));
                invalidation_mut(s, 0).insert("pending_documents".into(), docs(&["README.md"]));
            }),
        ),
        (
            "pending duplicated",
            Box::new(|s| {
                invalidation_mut(s, 0).insert(
                    "pending_documents".into(),
                    docs(&["README.md", "README.md", "src/corpus/README.md"]),
                );
            }),
        ),
        (
            "pending unsorted",
            Box::new(|s| {
                invalidation_mut(s, 0).insert("pending_documents".into(), docs(&reversed));
            }),
        ),
        (
            "target outside document scope",
            Box::new(|s| {
                invalidation_mut(s, 2).insert(
                    "targets".into(),
                    docs(&["README.md", "src/corpus/README.md"]),
                );
                invalidation_mut(s, 2).insert(
                    "pending_documents".into(),
                    docs(&["README.md", "src/corpus/README.md"]),
                );
            }),
        ),
        (
            "target outside subtree scope",
            Box::new(|s| {
                invalidation_mut(s, 1).insert(
                    "targets".into(),
                    docs(&["src/execution/README.md", "src/retrieval/README.md"]),
                );
                invalidation_mut(s, 1).insert(
                    "pending_documents".into(),
                    docs(&["src/retrieval/README.md"]),
                );
            }),
        ),
        (
            "counter zero control",
            Box::new(|s| {
                let Json::Object(root) = s else { panic!() };
                root.insert("next_invalidation_id".into(), Json::Number(0));
            }),
        ),
    ];
    for (name, corrupt) in cases {
        let mut state = good.clone();
        corrupt(&mut state);
        assert_ne!(state, good, "{name}: fixture must change the state");
        let bytes = write_state(&project, &state);
        for command in [
            vec!["status"],
            vec!["check"],
            vec![
                "invalidate",
                "doc:src/retrieval/naive/README.md",
                "--reason",
                REASON,
            ],
            vec!["invalidate", "all", "--reason", REASON],
        ] {
            let (code, value) = project.json(&command);
            assert_eq!(code, 4, "{name}: {command:?}: {}", json::to_compact(&value));
            assert_eq!(
                diagnostic_codes(&value),
                vec!["state_corrupt"],
                "{name}: {command:?}"
            );
            assert_eq!(
                project.state(),
                bytes,
                "{name}: {command:?}: bytes untouched"
            );
        }
        let output = project.run(&["status"]);
        assert_eq!(output.status.code(), Some(4), "{name}: human output");
        assert!(
            format!("{}{}", stdout(&output), stderr(&output)).contains("state_corrupt"),
            "{name}: human output"
        );
    }
    // The genuine state is accepted again, and mutations keep it canonical.
    project.write("memoria.lock", &good_bytes);
    assert_eq!(project.json(&["status"]).0, 0);
    assert_eq!(
        project
            .json(&[
                "invalidate",
                "doc:src/retrieval/naive/README.md",
                "--reason",
                REASON
            ])
            .0,
        0
    );
    assert_eq!(project.json(&["status"]).0, 0);
    let after = state_json(&project);
    assert_eq!(get_u64(&after, &["next_invalidation_id"]), 6);
    assert_eq!(
        get(
            &after,
            &[
                "reviews",
                "src/execution/README.md",
                "acknowledged_invalidations"
            ]
        ),
        &ids(&[1])
    );
}

// MEM-045: scope consistency uses stored identities, not today's documents.
#[test]
fn dormant_targets_and_later_documents_keep_genuine_state_valid() {
    let project = genuine_baseline();
    let before = project.state();
    // A target that no longer exists stays a valid historical identity.
    project.remove("src/disconnected/README.md");
    let (code, status) = project.json(&["status"]);
    assert_eq!(code, 0, "{}", json::to_compact(&status));
    assert_eq!(
        project.json(&["check"]).0,
        1,
        "pending invalidations remain"
    );
    // A document created after the invalidations never joined them.
    project.write("extra/README.md", "# Extra\n");
    project.write("extra/thing.rs", "extra\n");
    let (code, status) = project.json(&["status"]);
    assert_eq!(code, 0, "{}", json::to_compact(&status));
    let Json::Array(documents) = get(&status, &["data", "documents"]) else {
        panic!()
    };
    let extra = documents
        .iter()
        .find(|d| get_str(d, &["document"]) == "extra/README.md")
        .unwrap();
    assert_eq!(
        get_str(extra, &["status"]),
        "never_reviewed",
        "{}",
        json::to_compact(extra)
    );
    assert_eq!(project.state(), before, "inspection never rewrites state");
    // A mutation with the dormant target still stored succeeds and keeps it.
    assert_eq!(
        project
            .json(&["invalidate", "doc:extra/README.md", "--reason", REASON])
            .0,
        0
    );
    let after = state_json(&project);
    let Json::Array(invalidations) = get(&after, &["invalidations"]) else {
        panic!()
    };
    assert_eq!(get(&invalidations[0], &["targets"]), &docs(ALL));
    assert_eq!(project.json(&["status"]).0, 0);
}
