//! Injected races against the managed GitHub workflow adapter.
//!
//! An advisory lock serializes Memoria writers only. It does not stop an
//! arbitrary editor. These tests inject an edit at the exact step where such
//! an editor could interfere, so the preservation behavior is deterministic
//! instead of timing-dependent.

use std::cell::RefCell;
use std::fs;
use std::io::Write as _;
use std::path::{Path, PathBuf};

use memoria_application::ports::{
    WorkflowFailure, WorkflowOperation, WorkflowRequest, WorkflowStore,
};
use memoria_infrastructure::github_workflow::{
    DEFAULT_PATH, FsWorkflowStore, NO_STEPS, StepHook, TEMPLATE_VERSION, record_path_for, render,
};
use tempfile::TempDir;

const VERSION: &str = "0.5.0";

struct Fixture {
    _dir: TempDir,
    root: PathBuf,
    git_dir: PathBuf,
}

fn fixture() -> Fixture {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().canonicalize().unwrap();
    let git_dir = root.join(".git");
    fs::create_dir_all(&git_dir).unwrap();
    Fixture {
        _dir: dir,
        root,
        git_dir,
    }
}

impl Fixture {
    fn store<'a>(&self, steps: StepHook<'a>) -> FsWorkflowStore<'a> {
        FsWorkflowStore::with_steps(self.root.clone(), self.git_dir.clone(), VERSION, steps)
    }

    fn request(&self, operation: WorkflowOperation, runner: &str) -> WorkflowRequest {
        WorkflowRequest {
            operation,
            path: DEFAULT_PATH.to_string(),
            version: None,
            action_ref: None,
            runner: Some(runner.to_string()),
            apply: true,
        }
    }

    fn install(&self) {
        let store = self.store(NO_STEPS);
        let request = self.request(WorkflowOperation::Install, "ubuntu-24.04");
        let plan = store.plan(&request).unwrap();
        store.apply(&request, &plan).unwrap();
    }

    fn read(&self, relative: &str) -> String {
        fs::read_to_string(self.root.join(relative)).unwrap()
    }

    fn private(&self) -> PathBuf {
        self.git_dir.join("memoria/github")
    }

    fn displaced(&self) -> Vec<PathBuf> {
        let Ok(entries) = fs::read_dir(self.private()) else {
            return vec![];
        };
        let mut found: Vec<PathBuf> = entries
            .flatten()
            .map(|entry| entry.path())
            .filter(|path| {
                path.file_name()
                    .and_then(|name| name.to_str())
                    .is_some_and(|name| name.contains(".displaced-"))
            })
            .collect();
        found.sort();
        found
    }
}

#[test]
fn an_edit_injected_before_displacement_is_preserved_and_refused() {
    let fixture = fixture();
    fixture.install();
    let original = fixture.read(DEFAULT_PATH);
    let edited = format!("{original}# an editor added this line\n");

    let target = fixture.root.join(DEFAULT_PATH);
    let injected = edited.clone();
    let hook = move |step: &str| {
        if step == format!("before-displace:{DEFAULT_PATH}") {
            fs::write(&target, injected.as_bytes()).unwrap();
        }
    };
    let store = fixture.store(&hook);
    let request = fixture.request(WorkflowOperation::Upgrade, "ubuntu-latest");
    // The plan is recomputed under the lock and still sees the recorded bytes.
    let plan = store.plan(&request).unwrap();
    let failure = store.apply(&request, &plan).unwrap_err();
    match failure {
        WorkflowFailure::Conflict { code, .. } => assert_eq!(code, "github_modified"),
        other => panic!("expected a preserved conflict, got {other:?}"),
    }
    // The injected edit survives exactly, and no replacement was written.
    assert_eq!(fixture.read(DEFAULT_PATH), edited);
    assert!(fixture.displaced().is_empty(), "the file was restored");
    // Nothing changed, so the next command does not ask for recovery.
    let store = fixture.store(NO_STEPS);
    let request = fixture.request(WorkflowOperation::Status, "ubuntu-24.04");
    let plan = store.plan(&request).unwrap();
    assert!(!plan.recovery_needed, "an aborted apply leaves no intent");
    assert_eq!(plan.state, "modified");
}

#[test]
fn a_file_that_appears_during_restoration_is_never_destroyed() {
    // The abort path has its own window: Memoria has already displaced the
    // file, decided to put it back, and another writer creates the
    // destination before the move completes. Both byte sequences must
    // survive, and the report must name both paths.
    let fixture = fixture();
    fixture.install();
    let original = fixture.read(DEFAULT_PATH);
    let edited = format!("{original}# an editor added this line\n");
    let squatter = "# another writer created this file during the abort\n";

    let target = fixture.root.join(DEFAULT_PATH);
    let injected = edited.clone();
    let first = target.clone();
    let second = target.clone();
    let hook = move |step: &str| {
        if step == format!("before-displace:{DEFAULT_PATH}") {
            fs::write(&first, injected.as_bytes()).unwrap();
        }
        if step == format!("before-restore:{DEFAULT_PATH}") {
            fs::write(&second, squatter).unwrap();
        }
    };
    let store = fixture.store(&hook);
    let request = fixture.request(WorkflowOperation::Upgrade, "ubuntu-latest");
    let plan = store.plan(&request).unwrap();
    let failure = store.apply(&request, &plan).unwrap_err();

    let (code, paths) = match &failure {
        WorkflowFailure::Conflict { code, paths, .. } => (*code, paths.clone()),
        other => panic!("expected a preserving conflict, got {other:?}"),
    };
    assert_eq!(code, "github_recovery_pending");

    // The file that appeared is intact.
    assert_eq!(fixture.read(DEFAULT_PATH), squatter);
    // The displaced edit is intact, in private recovery storage.
    let displaced = fixture.displaced();
    let preserved = displaced
        .iter()
        .find(|path| {
            path.file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| name.starts_with("memoria.yml.displaced-"))
        })
        .expect("the displaced edit is preserved");
    assert_eq!(fs::read_to_string(preserved).unwrap(), edited);

    // Both paths are reported, so a person can compare them.
    assert!(paths.iter().any(|path| path == DEFAULT_PATH), "{paths:?}");
    assert!(
        paths
            .iter()
            .any(|path| path == &preserved.display().to_string()),
        "{paths:?}"
    );

    // No replacement was installed, and the record still describes the old
    // installation, so the state stays inspectable.
    assert!(
        !fixture
            .read(DEFAULT_PATH)
            .contains("runs-on: ubuntu-latest")
    );
    let store = fixture.store(NO_STEPS);
    let request = fixture.request(WorkflowOperation::Status, "ubuntu-24.04");
    let plan = store.plan(&request).unwrap();
    assert_eq!(plan.state, "modified");
}

#[test]
fn a_restoration_without_interference_puts_the_edit_back() {
    // The ordinary abort keeps its behavior: the edit returns to its path and
    // no recovery copy is retained.
    let fixture = fixture();
    fixture.install();
    let original = fixture.read(DEFAULT_PATH);
    let edited = format!("{original}# an editor added this line\n");
    let target = fixture.root.join(DEFAULT_PATH);
    let injected = edited.clone();
    let hook = move |step: &str| {
        if step == format!("before-displace:{DEFAULT_PATH}") {
            fs::write(&target, injected.as_bytes()).unwrap();
        }
    };
    let store = fixture.store(&hook);
    let request = fixture.request(WorkflowOperation::Upgrade, "ubuntu-latest");
    let plan = store.plan(&request).unwrap();
    match store.apply(&request, &plan).unwrap_err() {
        WorkflowFailure::Conflict { code, .. } => assert_eq!(code, "github_modified"),
        other => panic!("expected a preserved conflict, got {other:?}"),
    }
    assert_eq!(fixture.read(DEFAULT_PATH), edited);
    assert!(fixture.displaced().is_empty(), "the file was restored");
}

#[test]
fn a_descriptor_held_across_the_replacement_keeps_writing_into_the_preserved_file() {
    let fixture = fixture();
    fixture.install();
    let original = fixture.read(DEFAULT_PATH);

    // An editor opens the destination before Memoria replaces it and keeps
    // the descriptor. Displacement moves that file rather than truncating it,
    // so the editor's later write lands in the preserved copy.
    let held: RefCell<Option<fs::File>> = RefCell::new(None);
    let target = fixture.root.join(DEFAULT_PATH);
    let hook = |step: &str| {
        if step == format!("before-displace:{DEFAULT_PATH}") {
            *held.borrow_mut() = Some(
                fs::OpenOptions::new()
                    .append(true)
                    .open(&target)
                    .expect("the editor opens the destination"),
            );
        }
    };
    let store = fixture.store(&hook);
    let request = fixture.request(WorkflowOperation::Upgrade, "ubuntu-24.04-arm");
    let plan = store.plan(&request).unwrap();
    let applied = store.apply(&request, &plan).unwrap();
    assert_eq!(applied.state, "current");

    let mut editor = held.borrow_mut().take().expect("the hook opened the file");
    editor.write_all(b"# a late editor write\n").unwrap();
    editor.flush().unwrap();
    drop(editor);

    // The installed workflow is the new one, untouched by the late write.
    let installed = fixture.read(DEFAULT_PATH);
    assert!(installed.contains("runs-on: ubuntu-24.04-arm"));
    assert!(!installed.contains("a late editor write"));

    // The displaced file holds the previous bytes plus the late write.
    let displaced = fixture.displaced();
    let workflow_backup = displaced
        .iter()
        .find(|path| {
            path.file_name()
                .and_then(|n| n.to_str())
                .is_some_and(|n| n.starts_with("memoria.yml.displaced-"))
        })
        .expect("the previous workflow is preserved");
    let preserved = fs::read_to_string(workflow_backup).unwrap();
    assert_eq!(preserved, format!("{original}# a late editor write\n"));
}

#[test]
fn a_destination_that_appears_during_the_apply_is_never_clobbered() {
    let fixture = fixture();
    let target = fixture.root.join(DEFAULT_PATH);
    let squatter = "# another tool created this file first\n";
    let hook = |step: &str| {
        if step == format!("before-create:{DEFAULT_PATH}") && !target.exists() {
            fs::create_dir_all(target.parent().unwrap()).unwrap();
            fs::write(&target, squatter).unwrap();
        }
    };
    let store = fixture.store(&hook);
    let request = fixture.request(WorkflowOperation::Install, "ubuntu-24.04");
    let plan = store.plan(&request).unwrap();
    let failure = store.apply(&request, &plan).unwrap_err();
    match failure {
        WorkflowFailure::Conflict { code, .. } => assert_eq!(code, "github_destination_appeared"),
        other => panic!("expected a no-clobber refusal, got {other:?}"),
    }
    assert_eq!(fixture.read(DEFAULT_PATH), squatter);
}

#[test]
fn an_edit_to_the_ownership_record_during_the_apply_is_preserved() {
    let fixture = fixture();
    fixture.install();
    let record = record_path_for(DEFAULT_PATH);
    let path = fixture.root.join(&record);
    let injected = "{ \"schema_version\": 1 }\n";
    let record_step = format!("before-displace:{record}");
    let hook = move |step: &str| {
        if step == record_step {
            fs::write(&path, injected).unwrap();
        }
    };
    let store = fixture.store(&hook);
    let request = fixture.request(WorkflowOperation::Upgrade, "ubuntu-latest");
    let plan = store.plan(&request).unwrap();
    let failure = store.apply(&request, &plan).unwrap_err();
    match failure {
        WorkflowFailure::Conflict { code, .. } => assert_eq!(code, "github_modified"),
        other => panic!("expected a preserved conflict, got {other:?}"),
    }
    assert_eq!(fixture.read(&record), injected);
}

#[test]
fn an_unrecognized_interrupted_state_preserves_both_files() {
    let fixture = fixture();
    fixture.install();
    let record = record_path_for(DEFAULT_PATH);
    let intent = fixture.private().join("memoria.yml.intent.json");
    // The intent names bytes that no current file holds, so the interruption
    // is not one of the two recognized states.
    let text = |value: &str| format!("{value:?}");
    fs::write(
        &intent,
        format!(
            "{{\n  \"schema_version\": 1,\n  \"operation\": \"upgrade\",\n  \"workflow_path\": {},\n  \"record_path\": {},\n  \"expected_workflow\": {},\n  \"intended_workflow\": {},\n  \"expected_record\": null,\n  \"intended_record\": null\n}}\n",
            text(DEFAULT_PATH),
            text(&record),
            text("neither"),
            text("nor this")
        ),
    )
    .unwrap();

    let store = fixture.store(NO_STEPS);
    let request = fixture.request(WorkflowOperation::Upgrade, "ubuntu-latest");
    let plan = store.plan(&request).unwrap();
    assert!(
        plan.recovery_needed,
        "the plan must report the interruption"
    );
    let failure = store.apply(&request, &plan).unwrap_err();
    match failure {
        WorkflowFailure::Conflict { code, .. } => assert_eq!(code, "github_recovery_needed"),
        other => panic!("expected a recovery refusal, got {other:?}"),
    }
    let expected = render(TEMPLATE_VERSION, "ubuntu-24.04", "v0.5.0", VERSION).unwrap();
    assert_eq!(fixture.read(DEFAULT_PATH), expected);
    assert!(
        Path::new(&intent).exists(),
        "the intent record is preserved"
    );
}

#[test]
fn a_completed_interrupted_state_is_recognized_and_cleared() {
    let fixture = fixture();
    fixture.install();
    let record = record_path_for(DEFAULT_PATH);
    let intent = fixture.private().join("memoria.yml.intent.json");
    let current_workflow = fixture.read(DEFAULT_PATH);
    let current_record = fixture.read(&record);
    let text = |value: &str| format!("{value:?}");
    // Both files already hold the intended bytes: the mutation finished and
    // only the intent record survived.
    fs::write(
        &intent,
        format!(
            "{{\n  \"schema_version\": 1,\n  \"operation\": \"install\",\n  \"workflow_path\": {},\n  \"record_path\": {},\n  \"expected_workflow\": null,\n  \"intended_workflow\": {},\n  \"expected_record\": null,\n  \"intended_record\": {}\n}}\n",
            text(DEFAULT_PATH),
            text(&record),
            text(&current_workflow),
            text(&current_record)
        ),
    )
    .unwrap();

    let store = fixture.store(NO_STEPS);
    let request = fixture.request(WorkflowOperation::Upgrade, "ubuntu-latest");
    let plan = store.plan(&request).unwrap();
    assert!(plan.recovery_needed);
    let applied = store.apply(&request, &plan).unwrap();
    assert_eq!(applied.state, "current");
    assert!(!applied.recovery_needed);
    assert!(
        !intent.exists(),
        "a recognized interruption clears its intent"
    );
    assert!(
        fixture
            .read(DEFAULT_PATH)
            .contains("runs-on: ubuntu-latest")
    );
}
