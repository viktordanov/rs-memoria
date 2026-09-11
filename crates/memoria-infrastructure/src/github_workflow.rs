//! The managed consumer GitHub Actions workflow and its ownership record.
//!
//! This adapter owns one deterministic workflow template. It renders the
//! template from recorded parameters, compares exact bytes, and refuses every
//! file it does not own.
//!
//! The adapter never parses YAML. It never merges, adopts, or repairs a
//! workflow. A file without a valid ownership record is unmanaged, even when
//! its bytes match the current template exactly.
//!
//! Durability comes from three parts: an advisory lock on a worktree-private
//! path, a durable intent record that names the expected and intended bytes of
//! both files, and displaced-file recovery. Displacement moves the previous
//! file to a private recovery path before the replacement is created, so an
//! editor that still holds the displaced file keeps writing into a file this
//! adapter preserves.
//!
//! The honest limit: an advisory lock serializes Memoria writers only. It does
//! not stop an arbitrary editor. The adapter therefore compares bytes again
//! immediately before every write and removal, and preserves what it displaced.

use std::fs::{self, File, OpenOptions};
use std::io::Write as _;
use std::os::unix::fs::{MetadataExt as _, OpenOptionsExt as _};
use std::path::{Path, PathBuf};

use memoria_application::ports::{
    AdapterError, FileKind, RetainedArtifact, WorkflowFailure, WorkflowOperation, WorkflowPlan,
    WorkflowRequest, WorkflowStore,
};
use memoria_application::usecases::github_workflow::{
    ACTION_REPOSITORY, DEFAULT_RUNNER, RUNNERS, compare_versions, is_commit_ref, parse_action_ref,
    parse_runner, parse_version,
};

use crate::fs::{check_ancestors, kind_of, lock_file_within, sync_dir};
use crate::json::{self, Json, Limits, ObjectReader};

/// The project-relative default destination.
pub const DEFAULT_PATH: &str = ".github/workflows/memoria.yml";

/// The directory that holds the portable ownership records.
pub const RECORD_DIR: &str = ".github/memoria-workflows";

/// The ownership record schema this release writes and accepts.
pub const SCHEMA_VERSION: u64 = 1;

/// The workflow template this release renders.
pub const TEMPLATE_VERSION: u64 = 1;

/// Bound on the recorded expected workflow bytes.
pub const MAX_EXPECTED_BYTES: u64 = 64 * 1024;

/// Bound on the whole ownership record.
pub const MAX_RECORD_BYTES: u64 = 128 * 1024;

/// The reviewed full commit SHA of `actions/checkout` v7.0.1.
const CHECKOUT_PIN: &str = "3d3c42e5aac5ba805825da76410c181273ba90b1";
const CHECKOUT_TAG: &str = "v7.0.1";

/// The private coordination directory, relative to the Git metadata directory.
const PRIVATE_DIR: &str = "memoria/github";

/// An observation point inside one mutation.
///
/// Production code passes [`NO_STEPS`]. A test hook runs at a named step and
/// can change the filesystem there, so an injected race is deterministic
/// instead of timing-dependent. This follows the `FaultHook` pattern that
/// `fs.rs` already uses for durability tests.
pub type StepHook<'a> = &'a dyn Fn(&str);

/// A hook that observes nothing.
pub const NO_STEPS: StepHook<'static> = &|_| {};

fn io(operation: &str, path: &Path, err: std::io::Error) -> WorkflowFailure {
    WorkflowFailure::Io(AdapterError::new(
        operation,
        Some(path.display().to_string()),
        err.to_string(),
    ))
}

fn conflict(code: &'static str, message: impl Into<String>, paths: Vec<String>) -> WorkflowFailure {
    WorkflowFailure::Conflict {
        code,
        message: message.into(),
        paths,
    }
}

// ---------------------------------------------------------------------------
// Template
// ---------------------------------------------------------------------------

/// Render the consumer workflow for one set of recorded parameters.
///
/// The result is deterministic UTF-8 with line feeds only.
pub fn render(template: u64, runner: &str, action_ref: &str, version: &str) -> Option<String> {
    if template != TEMPLATE_VERSION {
        return None;
    }
    Some(format!(
        "# This workflow is managed by Memoria.\n\
         # Change it with `memoria integrations github upgrade --apply`.\n\
         # A manual edit makes the file modified, and Memoria then preserves it.\n\
         name: Memoria documentation\n\
         on: [push, pull_request]\n\
         permissions:\n\
         \x20 contents: read\n\
         jobs:\n\
         \x20 memoria:\n\
         \x20   runs-on: {runner}\n\
         \x20   timeout-minutes: 10\n\
         \x20   steps:\n\
         \x20     - uses: actions/checkout@{CHECKOUT_PIN} # {CHECKOUT_TAG}\n\
         \x20       with:\n\
         \x20         persist-credentials: false\n\
         \x20     - name: Set up Memoria\n\
         \x20       id: memoria\n\
         \x20       uses: {ACTION_REPOSITORY}@{action_ref}\n\
         \x20       with:\n\
         \x20         version: '{version}'\n\
         \x20     - run: memoria --version\n\
         \x20     - run: memoria check\n"
    ))
}

// ---------------------------------------------------------------------------
// Ownership record
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Record {
    pub template_version: u64,
    pub workflow_path: String,
    pub installer_version: String,
    pub binary_version: String,
    pub action_ref: String,
    pub runner: String,
    pub expected_workflow: String,
}

impl Record {
    fn encode(&self) -> String {
        let mut map = std::collections::BTreeMap::new();
        map.insert("schema_version".to_string(), Json::Number(SCHEMA_VERSION));
        map.insert(
            "template_version".to_string(),
            Json::Number(self.template_version),
        );
        map.insert(
            "workflow_path".to_string(),
            Json::String(self.workflow_path.clone()),
        );
        map.insert(
            "installer_version".to_string(),
            Json::String(self.installer_version.clone()),
        );
        map.insert(
            "binary_version".to_string(),
            Json::String(self.binary_version.clone()),
        );
        map.insert(
            "action_ref".to_string(),
            Json::String(self.action_ref.clone()),
        );
        map.insert("runner".to_string(), Json::String(self.runner.clone()));
        map.insert(
            "expected_workflow".to_string(),
            Json::String(self.expected_workflow.clone()),
        );
        json::to_pretty(&Json::Object(map))
    }

    /// Decode and validate one record for the selected workflow path.
    ///
    /// The record must name this workflow, hold supported schema and template
    /// versions, hold valid parameters, and reproduce its own expected bytes
    /// from those parameters.
    fn decode(bytes: &[u8], expected_path: &str) -> Result<Record, String> {
        if bytes.len() as u64 > MAX_RECORD_BYTES {
            return Err(format!(
                "the ownership record is larger than {MAX_RECORD_BYTES} bytes"
            ));
        }
        let value = json::parse(bytes, Limits::PACKET).map_err(|e| e.message)?;
        let mut reader = ObjectReader::new(value, "ownership record").map_err(|e| e.message)?;
        let schema = reader.take_u64("schema_version").map_err(|e| e.message)?;
        if schema != SCHEMA_VERSION {
            return Err(format!(
                "ownership schema version {schema} is outside this release's support"
            ));
        }
        let template_version = reader.take_u64("template_version").map_err(|e| e.message)?;
        let workflow_path = reader.take_string("workflow_path").map_err(|e| e.message)?;
        let installer_version = reader
            .take_string("installer_version")
            .map_err(|e| e.message)?;
        let binary_version = reader
            .take_string("binary_version")
            .map_err(|e| e.message)?;
        let action_ref = reader.take_string("action_ref").map_err(|e| e.message)?;
        let runner = reader.take_string("runner").map_err(|e| e.message)?;
        let expected_workflow = reader
            .take_string("expected_workflow")
            .map_err(|e| e.message)?;
        reader.finish().map_err(|e| e.message)?;

        if workflow_path != expected_path {
            return Err(format!(
                "the ownership record names {workflow_path}, not the selected {expected_path}"
            ));
        }
        if expected_workflow.len() as u64 > MAX_EXPECTED_BYTES {
            return Err(format!(
                "the recorded workflow is larger than {MAX_EXPECTED_BYTES} bytes"
            ));
        }
        if parse_version(&binary_version).is_err() {
            return Err(format!(
                "the recorded binary version {binary_version} is invalid"
            ));
        }
        if parse_action_ref(&action_ref).is_err() {
            return Err(format!(
                "the recorded Action reference {action_ref} is invalid"
            ));
        }
        if parse_runner(&runner).is_err() {
            return Err(format!("the recorded runner {runner} is invalid"));
        }
        let rebuilt =
            render(template_version, &runner, &action_ref, &binary_version).ok_or_else(|| {
                format!(
                    "template version {template_version} is outside this release's support; \
                     Memoria preserves the file it cannot rebuild"
                )
            })?;
        if rebuilt != expected_workflow {
            return Err(
                "the recorded workflow does not match its own recorded parameters".to_string(),
            );
        }
        Ok(Record {
            template_version,
            workflow_path,
            installer_version,
            binary_version,
            action_ref,
            runner,
            expected_workflow,
        })
    }
}

/// The ownership record path for one workflow path.
pub fn record_path_for(workflow_path: &str) -> String {
    let name = workflow_path.rsplit('/').next().unwrap_or(workflow_path);
    format!("{RECORD_DIR}/{name}.json")
}

// ---------------------------------------------------------------------------
// Adapter
// ---------------------------------------------------------------------------

/// What the destination currently holds.
#[derive(Debug, Clone)]
struct Observed {
    workflow_path: String,
    workflow: Option<Vec<u8>>,
    record_bytes: Option<Vec<u8>>,
    record: Option<Record>,
    record_problem: Option<String>,
    siblings: Vec<String>,
    intent: Option<IntentState>,
}

/// What the private intent path holds.
#[derive(Debug, Clone)]
enum IntentState {
    /// Transaction data this release understands, for this destination.
    Valid(Intent),
    /// Present but unusable. It is preserved, never cleared automatically.
    Invalid(String),
}

/// The durable intent of one interrupted or running mutation.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Intent {
    operation: String,
    workflow_path: String,
    record_path: String,
    expected_workflow: Option<String>,
    intended_workflow: Option<String>,
    expected_record: Option<String>,
    intended_record: Option<String>,
}

impl Intent {
    fn encode(&self) -> String {
        let mut map = std::collections::BTreeMap::new();
        let text = |value: &Option<String>| match value {
            Some(v) => Json::String(v.clone()),
            None => Json::Null,
        };
        map.insert("schema_version".to_string(), Json::Number(SCHEMA_VERSION));
        map.insert(
            "operation".to_string(),
            Json::String(self.operation.clone()),
        );
        map.insert(
            "workflow_path".to_string(),
            Json::String(self.workflow_path.clone()),
        );
        map.insert(
            "record_path".to_string(),
            Json::String(self.record_path.clone()),
        );
        map.insert(
            "expected_workflow".to_string(),
            text(&self.expected_workflow),
        );
        map.insert(
            "intended_workflow".to_string(),
            text(&self.intended_workflow),
        );
        map.insert("expected_record".to_string(), text(&self.expected_record));
        map.insert("intended_record".to_string(), text(&self.intended_record));
        json::to_pretty(&Json::Object(map))
    }

    /// Whether this record describes the selected destination pair.
    fn describes(&self, workflow_path: &str, record_path: &str) -> bool {
        self.workflow_path == workflow_path
            && self.record_path == record_path
            && matches!(
                self.operation.as_str(),
                "install" | "upgrade" | "uninstall" | "status"
            )
    }

    fn decode(bytes: &[u8]) -> Option<Intent> {
        let value = json::parse(bytes, Limits::PACKET).ok()?;
        let mut reader = ObjectReader::new(value, "intent").ok()?;
        if reader.take_u64("schema_version").ok()? != SCHEMA_VERSION {
            return None;
        }
        let intent = Intent {
            operation: reader.take_string("operation").ok()?,
            workflow_path: reader.take_string("workflow_path").ok()?,
            record_path: reader.take_string("record_path").ok()?,
            expected_workflow: reader.take_optional_string("expected_workflow").ok()?,
            intended_workflow: reader.take_optional_string("intended_workflow").ok()?,
            expected_record: reader.take_optional_string("expected_record").ok()?,
            intended_record: reader.take_optional_string("intended_record").ok()?,
        };
        reader.finish().ok()?;
        Some(intent)
    }
}

/// The managed consumer workflow, stored in the project's Git worktree.
pub struct FsWorkflowStore<'a> {
    root: PathBuf,
    git_dir: PathBuf,
    installer_version: String,
    steps: StepHook<'a>,
}

impl FsWorkflowStore<'static> {
    pub fn new(
        root: PathBuf,
        git_dir: PathBuf,
        installer_version: &str,
    ) -> FsWorkflowStore<'static> {
        FsWorkflowStore {
            root,
            git_dir,
            installer_version: installer_version.to_string(),
            steps: NO_STEPS,
        }
    }
}

impl<'a> FsWorkflowStore<'a> {
    /// [`FsWorkflowStore::new`] with an observation hook for durability tests.
    pub fn with_steps(
        root: PathBuf,
        git_dir: PathBuf,
        installer_version: &str,
        steps: StepHook<'a>,
    ) -> FsWorkflowStore<'a> {
        FsWorkflowStore {
            root,
            git_dir,
            installer_version: installer_version.to_string(),
            steps,
        }
    }

    fn full(&self, relative: &str) -> PathBuf {
        self.root.join(relative)
    }

    fn private_dir(&self) -> PathBuf {
        self.git_dir.join(PRIVATE_DIR)
    }

    fn lock_path(&self, workflow_path: &str) -> PathBuf {
        let name = workflow_path.rsplit('/').next().unwrap_or("memoria.yml");
        self.private_dir().join(format!("{name}.lock"))
    }

    fn intent_path(&self, workflow_path: &str) -> PathBuf {
        let name = workflow_path.rsplit('/').next().unwrap_or("memoria.yml");
        self.private_dir().join(format!("{name}.intent.json"))
    }

    /// Read one project file that must be a regular file or missing.
    fn read_owned(&self, relative: &str) -> Result<Option<Vec<u8>>, WorkflowFailure> {
        check_ancestors(&self.root, relative).map_err(WorkflowFailure::Io)?;
        let full = self.full(relative);
        match kind_of(&full).map_err(|e| io("stat", &full, e))? {
            FileKind::Missing => Ok(None),
            FileKind::Regular => {
                let bytes = fs::read(&full).map_err(|e| io("read", &full, e))?;
                Ok(Some(bytes))
            }
            FileKind::Symlink => Err(conflict(
                "github_path_unsafe",
                format!("{relative} is a symlink; Memoria never follows one inside the project"),
                vec![relative.to_string()],
            )),
            other => Err(conflict(
                "github_path_unsafe",
                format!("{relative} is a {other:?}, not a regular file"),
                vec![relative.to_string()],
            )),
        }
    }

    fn siblings(&self, workflow_path: &str) -> Vec<String> {
        let directory = self.full(".github/workflows");
        let own = workflow_path.rsplit('/').next().unwrap_or("");
        let Ok(entries) = fs::read_dir(&directory) else {
            return vec![];
        };
        let mut found: Vec<String> = entries
            .flatten()
            .filter_map(|entry| {
                let name = entry.file_name().to_string_lossy().into_owned();
                if name == own {
                    return None;
                }
                (name.ends_with(".yml") || name.ends_with(".yaml")).then_some(name)
            })
            .collect();
        found.sort();
        found
    }

    fn observe(&self, workflow_path: &str) -> Result<Observed, WorkflowFailure> {
        let record_path = record_path_for(workflow_path);
        let workflow = self.read_owned(workflow_path)?;
        let record_bytes = self.read_owned(&record_path)?;
        let (record, record_problem) = match &record_bytes {
            None => (None, None),
            Some(bytes) => match Record::decode(bytes, workflow_path) {
                Ok(record) => (Some(record), None),
                Err(problem) => (None, Some(problem)),
            },
        };
        // An unreadable or foreign intent is recovery evidence, not an empty
        // transaction. Inventing an all-absent intent here would let recovery
        // treat a corrupt record as a recognized state and erase it.
        let intent = match fs::read(self.intent_path(workflow_path)) {
            Ok(bytes) => match Intent::decode(&bytes) {
                Some(intent) if intent.describes(workflow_path, &record_path) => {
                    Some(IntentState::Valid(intent))
                }
                Some(_) => Some(IntentState::Invalid(
                    "the intent record names another workflow or record path".to_string(),
                )),
                None => Some(IntentState::Invalid(
                    "the intent record is not readable Memoria transaction data".to_string(),
                )),
            },
            Err(_) => None,
        };
        Ok(Observed {
            workflow_path: workflow_path.to_string(),
            workflow,
            record_bytes,
            record,
            record_problem,
            siblings: self.siblings(workflow_path),
            intent,
        })
    }

    /// The state label for one observation.
    fn state(&self, observed: &Observed) -> &'static str {
        match (&observed.workflow, &observed.record_bytes, &observed.record) {
            (None, None, _) => "absent",
            (Some(_), None, _) => "unmanaged",
            (None, Some(_), _) => "conflict",
            (Some(_), Some(_), None) => "conflict",
            (Some(bytes), Some(_), Some(record)) => {
                if bytes.as_slice() == record.expected_workflow.as_bytes() {
                    "managed"
                } else {
                    "modified"
                }
            }
        }
    }

    /// The values one install or upgrade would record.
    fn desired(
        &self,
        request: &WorkflowRequest,
        installed: Option<&Record>,
    ) -> Result<(String, String, String, Vec<String>), WorkflowFailure> {
        let mut notes = Vec::new();
        let version = match (&request.version, installed) {
            (Some(explicit), _) => explicit.clone(),
            // The default target is this executable's own package version.
            // It passes the same validation an explicit value passes, so a
            // build below the Action's floor cannot produce a workflow that
            // the Action would refuse.
            (None, _) => parse_version(&self.installer_version).map_err(|_| {
                WorkflowFailure::Usage {
                    code: "version_unsupported",
                    message: format!(
                        "this executable reports version {}, which the setup Action cannot install;                          give --version with a supported version",
                        self.installer_version
                    ),
                }
            })?,
        };
        if let Some(record) = installed
            && request.operation == WorkflowOperation::Upgrade
            && compare_versions(&version, &record.binary_version) == std::cmp::Ordering::Less
        {
            return Err(WorkflowFailure::Usage {
                code: "github_downgrade_refused",
                message: format!(
                    "the recorded workflow installs Memoria {}, and {} is older; \
                     Memoria does not downgrade a managed workflow",
                    record.binary_version, version
                ),
            });
        }
        let action_ref = match (&request.action_ref, installed) {
            (Some(explicit), _) => explicit.clone(),
            (None, Some(record)) if is_commit_ref(&record.action_ref) => {
                notes.push(format!(
                    "the recorded Action reference {} is a commit pin; it is preserved. Give --action-ref to change it.",
                    record.action_ref
                ));
                record.action_ref.clone()
            }
            (None, _) => format!("v{version}"),
        };
        let runner = match (&request.runner, installed) {
            (Some(explicit), _) => explicit.clone(),
            (None, Some(record)) => record.runner.clone(),
            (None, None) => DEFAULT_RUNNER.to_string(),
        };
        if !RUNNERS.contains(&runner.as_str()) {
            return Err(WorkflowFailure::Usage {
                code: "runner_invalid",
                message: format!("runner {runner} is not supported"),
            });
        }
        Ok((version, action_ref, runner, notes))
    }

    fn build_plan(
        &self,
        request: &WorkflowRequest,
        observed: &Observed,
    ) -> Result<WorkflowPlan, WorkflowFailure> {
        let record_path = record_path_for(&request.path);
        let state = self.state(observed);
        let (version, action_ref, runner, mut notes) =
            self.desired(request, observed.record.as_ref())?;
        let rendered =
            render(TEMPLATE_VERSION, &runner, &action_ref, &version).ok_or_else(|| {
                WorkflowFailure::Usage {
                    code: "github_template_unsupported",
                    message: "this release renders only template version 1".to_string(),
                }
            })?;

        if let Some(problem) = &observed.record_problem {
            notes.push(format!("the ownership record is not usable: {problem}"));
        }
        let matches_desired = observed.record.as_ref().is_some_and(|record| {
            record.template_version == TEMPLATE_VERSION
                && record.binary_version == version
                && record.action_ref == action_ref
                && record.runner == runner
        });
        let reported_state = match state {
            "managed" if matches_desired => "current",
            "managed" => "outdated",
            other => other,
        };
        if observed.record.as_ref().is_some_and(|record| {
            compare_versions(&record.binary_version, &self.installer_version)
                == std::cmp::Ordering::Greater
        }) {
            notes.push(format!(
                "the recorded workflow installs a Memoria version newer than this executable ({})",
                self.installer_version
            ));
        }

        let mut plan = WorkflowPlan {
            path: request.path.clone(),
            record_path: record_path.clone(),
            state: reported_state.to_string(),
            installed_version: observed.record.as_ref().map(|r| r.binary_version.clone()),
            installed_action_ref: observed.record.as_ref().map(|r| r.action_ref.clone()),
            installed_runner: observed.record.as_ref().map(|r| r.runner.clone()),
            installed_template: observed.record.as_ref().map(|r| r.template_version),
            desired_version: version,
            desired_action_ref: action_ref,
            desired_runner: runner,
            desired_template: TEMPLATE_VERSION,
            rendered: None,
            writes: vec![],
            removals: vec![],
            no_change: false,
            recovery_needed: observed.intent.is_some(),
            retained_artifacts: vec![],
            siblings: observed.siblings.clone(),
            notes,
        };

        match request.operation {
            WorkflowOperation::Status => {
                plan.no_change = true;
            }
            WorkflowOperation::Install => match reported_state {
                "absent" => {
                    plan.rendered = Some(rendered);
                    plan.writes = vec![request.path.clone(), record_path];
                }
                "current" => {
                    plan.no_change = true;
                    plan.rendered = Some(rendered);
                }
                "outdated" => {
                    return Err(WorkflowFailure::UpgradeRequired(format!(
                        "{} is managed with different parameters; run `memoria integrations github upgrade` to review the change",
                        request.path
                    )));
                }
                _ => return Err(self.preserve(reported_state, request, observed)),
            },
            WorkflowOperation::Upgrade => match reported_state {
                "absent" => {
                    return Err(WorkflowFailure::NotInstalled(format!(
                        "no managed Memoria workflow at {}; run `memoria integrations github install` first",
                        request.path
                    )));
                }
                "current" => {
                    plan.no_change = true;
                    plan.rendered = Some(rendered);
                }
                "outdated" => {
                    plan.rendered = Some(rendered);
                    plan.writes = vec![request.path.clone(), record_path];
                }
                _ => return Err(self.preserve(reported_state, request, observed)),
            },
            WorkflowOperation::Uninstall => match reported_state {
                "absent" => {
                    plan.no_change = true;
                }
                "current" | "outdated" => {
                    plan.removals = vec![request.path.clone(), record_path];
                }
                _ => return Err(self.preserve(reported_state, request, observed)),
            },
        }
        Ok(plan)
    }

    fn preserve(
        &self,
        state: &str,
        request: &WorkflowRequest,
        observed: &Observed,
    ) -> WorkflowFailure {
        let record_path = record_path_for(&request.path);
        let paths = vec![request.path.clone(), record_path.clone()];
        match state {
            "unmanaged" => conflict(
                "github_unmanaged",
                format!(
                    "{} already exists and Memoria does not own it. Memoria never adopts a workflow without a valid ownership record, even when the bytes match. Keep that file and manage it yourself, or choose another --path.",
                    request.path
                ),
                paths,
            ),
            "modified" => conflict(
                "github_modified",
                format!(
                    "{} was changed after Memoria wrote it. Memoria preserves your edit. Restore the recorded content or manage that workflow yourself.",
                    request.path
                ),
                paths,
            ),
            _ => conflict(
                "github_ownership_conflict",
                match (&observed.workflow, &observed.record_bytes) {
                    (None, Some(_)) => format!(
                        "{record_path} exists without {}. Memoria preserves both sides of an inconsistent pair.",
                        request.path
                    ),
                    _ => format!(
                        "the ownership record for {} is not usable: {}",
                        request.path,
                        observed
                            .record_problem
                            .clone()
                            .unwrap_or_else(|| "unknown".to_string())
                    ),
                },
                paths,
            ),
        }
    }

    // -- durable mutation ---------------------------------------------------

    fn ensure_private_dir(&self) -> Result<PathBuf, WorkflowFailure> {
        let directory = self.private_dir();
        fs::create_dir_all(&directory).map_err(|e| io("create_dir", &directory, e))?;
        Ok(directory)
    }

    /// Refuse a recovery destination that a rename cannot reach atomically.
    fn check_recovery_device(&self, private: &Path) -> Result<(), WorkflowFailure> {
        let workflows = self.full(".github/workflows");
        let reference = if workflows.is_dir() {
            workflows
        } else {
            self.root.clone()
        };
        let left = fs::metadata(&reference).map_err(|e| io("stat", &reference, e))?;
        let right = fs::metadata(private).map_err(|e| io("stat", private, e))?;
        if left.dev() != right.dev() {
            return Err(conflict(
                "github_recovery_unavailable",
                format!(
                    "the private recovery directory {} is on another filesystem than {}; \
                     Memoria refuses to mutate without an atomic displacement",
                    private.display(),
                    reference.display()
                ),
                vec![private.display().to_string()],
            ));
        }
        Ok(())
    }

    fn create_directories(&self, relative: &str) -> Result<(), WorkflowFailure> {
        check_ancestors(&self.root, relative).map_err(WorkflowFailure::Io)?;
        if let Some(parent) = Path::new(relative).parent()
            && !parent.as_os_str().is_empty()
        {
            let full = self.full(&parent.to_string_lossy());
            fs::create_dir_all(&full).map_err(|e| io("create_dir", &full, e))?;
        }
        check_ancestors(&self.root, relative).map_err(WorkflowFailure::Io)?;
        Ok(())
    }

    /// Create a file that must not exist, durably and without following a
    /// symlink. A file that appears between the plan and this call wins: the
    /// creation fails instead of clobbering it.
    fn create_new(&self, relative: &str, bytes: &str) -> Result<(), WorkflowFailure> {
        (self.steps)(&format!("before-create:{relative}"));
        let full = self.full(relative);
        let mut options = OpenOptions::new();
        options
            .write(true)
            .create_new(true)
            .custom_flags(rustix::fs::OFlags::NOFOLLOW.bits() as i32);
        let mut file = match options.open(&full) {
            Ok(file) => file,
            Err(err) if err.kind() == std::io::ErrorKind::AlreadyExists => {
                return Err(conflict(
                    "github_destination_appeared",
                    format!(
                        "{relative} appeared while Memoria was writing; Memoria never clobbers a file it did not observe"
                    ),
                    vec![relative.to_string()],
                ));
            }
            Err(err) => return Err(io("create", &full, err)),
        };
        file.write_all(bytes.as_bytes())
            .map_err(|e| io("write", &full, e))?;
        file.flush().map_err(|e| io("write", &full, e))?;
        file.sync_all().map_err(|e| io("sync_file", &full, e))?;
        drop(file);
        if let Some(parent) = full.parent() {
            sync_dir(parent).map_err(|e| io("sync_dir", parent, e))?;
        }
        Ok(())
    }

    /// Move one file out of the worktree into private recovery storage.
    ///
    /// The move is an atomic same-filesystem rename, so an editor that holds
    /// the displaced file keeps writing into the file this adapter preserves.
    fn displace(
        &self,
        relative: &str,
        expected: &str,
        private: &Path,
        stamp: &str,
    ) -> Result<PathBuf, WorkflowFailure> {
        let full = self.full(relative);
        let name = relative.rsplit('/').next().unwrap_or(relative);
        let recovery = private.join(format!("{name}.displaced-{stamp}"));
        (self.steps)(&format!("before-displace:{relative}"));
        fs::rename(&full, &recovery).map_err(|e| io("rename", &full, e))?;
        sync_dir(private).map_err(|e| io("sync_dir", private, e))?;
        if let Some(parent) = full.parent() {
            sync_dir(parent).map_err(|e| io("sync_dir", parent, e))?;
        }
        let bytes = fs::read(&recovery).map_err(|e| io("read", &recovery, e))?;
        if bytes.as_slice() != expected.as_bytes() {
            // The bytes changed between the plan and the displacement. Put
            // them back, and never destroy a file that appeared in the
            // meantime: the restoration itself is no-clobber.
            return Err(self.restore(relative, &recovery));
        }
        Ok(recovery)
    }

    /// Put a displaced file back without clobbering an appearing destination.
    ///
    /// A test for absence followed by a rename has a window: another writer
    /// can create the destination between the two calls, and the rename would
    /// then destroy it. A hard link fails when the destination exists, so the
    /// decision and the move are one operation. The link keeps the same inode,
    /// so an editor that still holds the displaced file keeps writing into the
    /// restored file.
    ///
    /// Both byte sequences always survive. The return value is the conflict to
    /// report; this function never returns success, because a restoration only
    /// happens on a path that refuses the mutation.
    fn restore(&self, relative: &str, recovery: &Path) -> WorkflowFailure {
        let full = self.full(relative);
        (self.steps)(&format!("before-restore:{relative}"));
        match fs::hard_link(recovery, &full) {
            Ok(()) => {
                // The destination is back. Drop the extra name for the same
                // inode; a failure there costs a retained copy, not content.
                let _ = fs::remove_file(recovery);
                if let Some(parent) = full.parent() {
                    let _ = sync_dir(parent);
                }
                conflict(
                    "github_modified",
                    format!(
                        "{relative} changed while Memoria was applying its plan; the change is preserved and nothing was written"
                    ),
                    vec![relative.to_string()],
                )
            }
            Err(err) if err.kind() == std::io::ErrorKind::AlreadyExists => conflict(
                "github_recovery_pending",
                format!(
                    "{relative} changed during the apply, and another file appeared at that path before Memoria could put it back. \
                     Both files are kept: the appearing file stays at {relative}, and the displaced bytes stay at {}. \
                     Compare them and keep the one you want.",
                    recovery.display()
                ),
                vec![relative.to_string(), recovery.display().to_string()],
            ),
            Err(err) => conflict(
                "github_recovery_pending",
                format!(
                    "{relative} changed during the apply and could not be put back ({err}); \
                     its bytes are preserved at {}",
                    recovery.display()
                ),
                vec![relative.to_string(), recovery.display().to_string()],
            ),
        }
    }

    fn write_intent(&self, path: &Path, intent: &Intent) -> Result<(), WorkflowFailure> {
        let encoded = intent.encode();
        let temporary = path.with_extension("json.new");
        let mut file = File::create(&temporary).map_err(|e| io("create", &temporary, e))?;
        file.write_all(encoded.as_bytes())
            .map_err(|e| io("write", &temporary, e))?;
        file.sync_all()
            .map_err(|e| io("sync_file", &temporary, e))?;
        drop(file);
        fs::rename(&temporary, path).map_err(|e| io("rename", &temporary, e))?;
        if let Some(parent) = path.parent() {
            sync_dir(parent).map_err(|e| io("sync_dir", parent, e))?;
        }
        Ok(())
    }

    fn clear_intent(&self, path: &Path) -> Result<(), WorkflowFailure> {
        match fs::remove_file(path) {
            Ok(()) => {}
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => {}
            Err(err) => return Err(io("remove", path, err)),
        }
        if let Some(parent) = path.parent() {
            sync_dir(parent).map_err(|e| io("sync_dir", parent, e))?;
        }
        Ok(())
    }

    /// Resolve an interrupted transaction conservatively.
    ///
    /// Only two states are recognized: the expected bytes, which means the
    /// mutation never started, and the intended bytes, which means it
    /// finished. Any third state preserves every file and reports the path.
    fn recover(&self, state: &IntentState, observed: &Observed) -> Result<(), WorkflowFailure> {
        let intent = match state {
            IntentState::Valid(intent) => intent,
            IntentState::Invalid(problem) => {
                // Unusable transaction data is evidence. It is preserved and
                // reported; it never authorizes a change.
                let path = self.intent_path(&observed.workflow_path);
                return Err(conflict(
                    "github_recovery_needed",
                    format!(
                        "the interrupted-transaction record at {} is not usable: {problem}. \
                         Memoria preserves it and changes nothing. Inspect both managed files and remove that record when you have resolved them.",
                        path.display()
                    ),
                    vec![
                        observed.workflow_path.clone(),
                        record_path_for(&observed.workflow_path),
                        path.display().to_string(),
                    ],
                ));
            }
        };
        let matches = |current: &Option<Vec<u8>>, wanted: &Option<String>| match (current, wanted) {
            (None, None) => true,
            (Some(bytes), Some(text)) => bytes.as_slice() == text.as_bytes(),
            _ => false,
        };
        let workflow_known = matches(&observed.workflow, &intent.expected_workflow)
            || matches(&observed.workflow, &intent.intended_workflow);
        let record_known = matches(&observed.record_bytes, &intent.expected_record)
            || matches(&observed.record_bytes, &intent.intended_record);
        if workflow_known && record_known {
            return self.clear_intent(&self.intent_path(&intent.workflow_path));
        }
        Err(conflict(
            "github_recovery_needed",
            format!(
                "an interrupted `{}` left {} or {} in a state Memoria does not recognize; \
                 inspect both files and the intent record at {}",
                intent.operation,
                intent.workflow_path,
                intent.record_path,
                self.intent_path(&intent.workflow_path).display()
            ),
            vec![
                intent.workflow_path.clone(),
                intent.record_path.clone(),
                self.intent_path(&intent.workflow_path)
                    .display()
                    .to_string(),
            ],
        ))
    }
}

fn stamp() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    format!("{nanos}-{}", std::process::id())
}

impl WorkflowStore for FsWorkflowStore<'_> {
    fn default_path(&self) -> &'static str {
        DEFAULT_PATH
    }

    fn plan(&self, request: &WorkflowRequest) -> Result<WorkflowPlan, WorkflowFailure> {
        let observed = self.observe(&request.path)?;
        self.build_plan(request, &observed)
    }

    fn apply(
        &self,
        request: &WorkflowRequest,
        _plan: &WorkflowPlan,
    ) -> Result<WorkflowPlan, WorkflowFailure> {
        let private = self.ensure_private_dir()?;
        let _guard =
            lock_file_within(&self.lock_path(&request.path), &self.git_dir).map_err(|failure| {
                match failure {
                    memoria_application::ports::LockFailure::Busy => conflict(
                        "github_busy",
                        "another Memoria workflow mutation holds the lock; retry shortly",
                        vec![request.path.clone()],
                    ),
                    memoria_application::ports::LockFailure::Io(err) => WorkflowFailure::Io(err),
                }
            })?;

        // An interrupted transaction resolves before anything else.
        let observed = self.observe(&request.path)?;
        if let Some(intent) = &observed.intent {
            self.recover(intent, &observed)?;
        }

        // The plan is recomputed from the bytes on disk right now. An earlier
        // preview never authorizes an overwrite of a later edit.
        let observed = self.observe(&request.path)?;
        let mut plan = self.build_plan(request, &observed)?;
        if plan.no_change {
            return Ok(plan);
        }
        self.check_recovery_device(&private)?;

        let record_path = record_path_for(&request.path);
        let intent_path = self.intent_path(&request.path);
        let current_workflow = observed
            .workflow
            .as_ref()
            .map(|bytes| String::from_utf8_lossy(bytes).into_owned());
        let current_record = observed
            .record_bytes
            .as_ref()
            .map(|bytes| String::from_utf8_lossy(bytes).into_owned());

        let (intended_workflow, intended_record) = match request.operation {
            WorkflowOperation::Uninstall => (None, None),
            _ => {
                let rendered = plan
                    .rendered
                    .clone()
                    .expect("install and upgrade plans render a workflow");
                let record = Record {
                    template_version: TEMPLATE_VERSION,
                    workflow_path: request.path.clone(),
                    installer_version: self.installer_version.clone(),
                    binary_version: plan.desired_version.clone(),
                    action_ref: plan.desired_action_ref.clone(),
                    runner: plan.desired_runner.clone(),
                    expected_workflow: rendered.clone(),
                };
                (Some(rendered), Some(record.encode()))
            }
        };
        let intent = Intent {
            operation: request.operation.as_str().to_string(),
            workflow_path: request.path.clone(),
            record_path: record_path.clone(),
            expected_workflow: current_workflow.clone(),
            intended_workflow: intended_workflow.clone(),
            expected_record: current_record.clone(),
            intended_record: intended_record.clone(),
        };
        self.write_intent(&intent_path, &intent)?;

        let stamp = stamp();
        let mut retained: Vec<RetainedArtifact> = Vec::new();
        // Whether anything has moved or appeared yet. A refusal before the
        // first change leaves nothing to recover, so its intent record is
        // stale and must not make the next command ask for recovery.
        let mut mutated = false;

        let mutation = (|| -> Result<(), WorkflowFailure> {
            // Displace what exists before anything replaces it.
            for (relative, expected) in [
                (request.path.clone(), current_workflow.clone()),
                (record_path.clone(), current_record.clone()),
            ] {
                if let Some(expected) = expected {
                    let recovery = self.displace(&relative, &expected, &private, &stamp)?;
                    mutated = true;
                    retained.push(RetainedArtifact {
                        path: recovery.display().to_string(),
                        reason: "user_backup",
                        removable_by_uninstall: false,
                    });
                }
            }

            if request.operation != WorkflowOperation::Uninstall {
                self.create_directories(&request.path)?;
                self.create_directories(&record_path)?;
                self.create_new(
                    &request.path,
                    intended_workflow
                        .as_deref()
                        .expect("install and upgrade render a workflow"),
                )?;
                mutated = true;
                self.create_new(
                    &record_path,
                    intended_record
                        .as_deref()
                        .expect("install and upgrade write a record"),
                )?;
            }
            Ok(())
        })();
        if let Err(failure) = mutation {
            if !mutated {
                self.clear_intent(&intent_path)?;
            }
            return Err(failure);
        }

        self.clear_intent(&intent_path)?;

        retained.push(RetainedArtifact {
            path: self.lock_path(&request.path).display().to_string(),
            reason: "synchronization_lock",
            removable_by_uninstall: false,
        });
        // Report the values that now exist on disk, not the values the plan
        // observed before the change.
        let after = self.observe(&request.path)?;
        plan.state = match request.operation {
            WorkflowOperation::Uninstall => "absent".to_string(),
            _ => self.state(&after).replace("managed", "current"),
        };
        plan.installed_version = after.record.as_ref().map(|r| r.binary_version.clone());
        plan.installed_action_ref = after.record.as_ref().map(|r| r.action_ref.clone());
        plan.installed_runner = after.record.as_ref().map(|r| r.runner.clone());
        plan.installed_template = after.record.as_ref().map(|r| r.template_version);
        plan.retained_artifacts = retained;
        plan.recovery_needed = false;
        Ok(plan)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> Record {
        let rendered = render(TEMPLATE_VERSION, DEFAULT_RUNNER, "v0.5.0", "0.5.0").unwrap();
        Record {
            template_version: TEMPLATE_VERSION,
            workflow_path: DEFAULT_PATH.to_string(),
            installer_version: "0.5.0".to_string(),
            binary_version: "0.5.0".to_string(),
            action_ref: "v0.5.0".to_string(),
            runner: DEFAULT_RUNNER.to_string(),
            expected_workflow: rendered,
        }
    }

    #[test]
    fn renders_deterministic_line_feed_bytes() {
        let first = render(TEMPLATE_VERSION, DEFAULT_RUNNER, "v0.5.0", "0.5.0").unwrap();
        let second = render(TEMPLATE_VERSION, DEFAULT_RUNNER, "v0.5.0", "0.5.0").unwrap();
        assert_eq!(first, second);
        assert!(!first.contains('\r'));
        assert!(first.ends_with('\n'));
        assert!(first.contains("on: [push, pull_request]"));
        assert!(first.contains("permissions:\n  contents: read"));
        assert!(first.contains(&format!("actions/checkout@{CHECKOUT_PIN}")));
        assert!(first.contains("persist-credentials: false"));
        assert!(first.contains("runs-on: ubuntu-24.04"));
        assert!(first.contains("- run: memoria check"));
        assert!(!first.contains("pull_request_target"));
    }

    #[test]
    fn renders_every_supported_runner_and_reference() {
        for runner in RUNNERS {
            let text = render(TEMPLATE_VERSION, runner, &"a".repeat(40), "1.2.3").unwrap();
            assert!(text.contains(&format!("runs-on: {runner}")));
            assert!(text.contains(&format!("{ACTION_REPOSITORY}@{}", "a".repeat(40))));
            assert!(text.contains("version: '1.2.3'"));
        }
        assert!(render(TEMPLATE_VERSION + 1, DEFAULT_RUNNER, "v0.5.0", "0.5.0").is_none());
    }

    #[test]
    fn round_trips_an_ownership_record() {
        let record = sample();
        let decoded = Record::decode(record.encode().as_bytes(), DEFAULT_PATH).unwrap();
        assert_eq!(decoded, record);
    }

    #[test]
    fn refuses_a_record_that_names_another_path() {
        let record = sample();
        let problem =
            Record::decode(record.encode().as_bytes(), ".github/workflows/other.yml").unwrap_err();
        assert!(problem.contains("names"), "{problem}");
    }

    #[test]
    fn refuses_inconsistent_and_unsupported_records() {
        let mut record = sample();
        record.runner = "ubuntu-latest".to_string();
        let problem = Record::decode(record.encode().as_bytes(), DEFAULT_PATH).unwrap_err();
        assert!(problem.contains("does not match"), "{problem}");

        let encoded = sample()
            .encode()
            .replace("\"schema_version\": 1", "\"schema_version\": 9");
        let problem = Record::decode(encoded.as_bytes(), DEFAULT_PATH).unwrap_err();
        assert!(problem.contains("schema version 9"), "{problem}");

        let encoded = sample()
            .encode()
            .replace("\"template_version\": 1", "\"template_version\": 2");
        let problem = Record::decode(encoded.as_bytes(), DEFAULT_PATH).unwrap_err();
        assert!(problem.contains("template version 2"), "{problem}");
    }

    #[test]
    fn refuses_a_duplicate_field() {
        let encoded = sample().encode();
        let injected = encoded.replacen('{', "{\n  \"runner\": \"ubuntu-latest\",", 1);
        assert!(Record::decode(injected.as_bytes(), DEFAULT_PATH).is_err());
    }

    #[test]
    fn record_path_follows_the_workflow_name() {
        assert_eq!(
            record_path_for(".github/workflows/memoria.yml"),
            ".github/memoria-workflows/memoria.yml.json"
        );
        assert_eq!(
            record_path_for(".github/workflows/docs.yaml"),
            ".github/memoria-workflows/docs.yaml.json"
        );
    }

    #[test]
    fn intent_round_trips() {
        let intent = Intent {
            operation: "upgrade".to_string(),
            workflow_path: DEFAULT_PATH.to_string(),
            record_path: record_path_for(DEFAULT_PATH),
            expected_workflow: Some("old".to_string()),
            intended_workflow: Some("new".to_string()),
            expected_record: None,
            intended_record: Some("{}".to_string()),
        };
        assert_eq!(Intent::decode(intent.encode().as_bytes()), Some(intent));
    }
}
