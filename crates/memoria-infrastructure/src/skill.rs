//! Managed skill package transactions for Codex and Claude.
//!
//! Every mutation runs under a parent-directory lock with a durable
//! transaction record. Temporary directories are used only when a validated
//! transaction owns them; unknown content is preserved and reported.

use std::collections::BTreeMap;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use memoria_application::ports::{
    AdapterError, AgentScope, AgentTarget, OverlappingPackage, RetainedArtifact, SkillFailure,
    SkillOperation, SkillPackageStore, SkillPlan, SkillRequest,
};

use crate::fs::{FaultHook, NO_FAULTS, durable_replace, fault, kind_of, lock_file, sync_dir};
use crate::json::{self, Json, Limits, ObjectReader};

pub const PACKAGE_NAME: &str = "memoria";
pub const SKILL_FILE: &str = "SKILL.md";
pub const RECORD_FILE: &str = ".memoria-install.json";
pub const BACKUP_DIR: &str = "memoria.backup";
pub const STAGING_DIR: &str = "memoria.staging";
pub const REMOVING_DIR: &str = "memoria.removing";
pub const TXN_FILE: &str = "memoria.install-txn.json";
pub const LOCK_FILE: &str = "memoria.install.lock";

pub struct FsSkillStore<'a> {
    root: PathBuf,
    skill: &'static str,
    version: &'static str,
    faults: FaultHook<'a>,
}

struct Layout {
    parent: PathBuf,
    destination: PathBuf,
    backup: PathBuf,
    staging: PathBuf,
    removing: PathBuf,
    txn: PathBuf,
    lock: PathBuf,
}

fn io_failure(operation: &str, path: &Path, err: io::Error) -> SkillFailure {
    SkillFailure::Io(AdapterError::new(
        operation,
        Some(path.display().to_string()),
        err.to_string(),
    ))
}

fn conflict(message: impl Into<String>, paths: Vec<PathBuf>) -> SkillFailure {
    SkillFailure::Conflict {
        message: message.into(),
        paths: paths.iter().map(|p| p.display().to_string()).collect(),
    }
}

fn hash_hex(bytes: &[u8]) -> String {
    use memoria_application::ports::FingerprintHasher;

    crate::hash::Xxh3Hasher.hash(bytes).to_hex()
}

#[derive(Debug, Clone)]
struct Record {
    /// 1 for a package this release found, 2 for one it writes.
    schema_version: u64,
    target: String,
    /// `local` or `global`. Schema 1 records carry no scope.
    scope: Option<String>,
    package_version: String,
    hashes: BTreeMap<String, String>,
    backup: Option<String>,
}

fn record_to_json(record: &Record) -> Json {
    let mut map = BTreeMap::new();
    map.insert("schema_version".into(), Json::Number(2));
    map.insert("target".into(), Json::String(record.target.clone()));
    map.insert(
        "scope".into(),
        record.scope.clone().map(Json::String).unwrap_or(Json::Null),
    );
    map.insert(
        "package_version".into(),
        Json::String(record.package_version.clone()),
    );
    map.insert(
        "managed_paths".into(),
        Json::Array(
            record
                .hashes
                .keys()
                .map(|k| Json::String(k.clone()))
                .collect(),
        ),
    );
    map.insert(
        "hashes".into(),
        Json::Object(
            record
                .hashes
                .iter()
                .map(|(k, v)| (k.clone(), Json::String(v.clone())))
                .collect(),
        ),
    );
    map.insert(
        "backup".into(),
        record
            .backup
            .clone()
            .map(Json::String)
            .unwrap_or(Json::Null),
    );
    Json::Object(map)
}

fn record_from_bytes(bytes: &[u8]) -> Result<Record, String> {
    let value = json::parse(bytes, Limits::STATE).map_err(|e| e.message)?;
    let mut reader = ObjectReader::new(value, "install record").map_err(|e| e.message)?;
    let convert = |e: json::JsonError| e.message;
    // Schema 1 records are read for an explicit upgrade or uninstall. This
    // limited installer compatibility does not imply compatibility with
    // version 1 review state.
    let schema_version = reader.take_u64("schema_version").map_err(convert)?;
    if schema_version != 1 && schema_version != 2 {
        return Err(format!(
            "unsupported install record schema {schema_version}; this release reads schema 1 and 2"
        ));
    }
    let target = reader.take_string("target").map_err(convert)?;
    let scope = if schema_version >= 2 {
        let scope = reader.take_optional_string("scope").map_err(convert)?;
        if let Some(scope) = &scope
            && scope != "local"
            && scope != "global"
        {
            return Err(format!(
                "install record scope {scope:?} is not local or global"
            ));
        }
        scope
    } else {
        None
    };
    let package_version = reader.take_string("package_version").map_err(convert)?;
    let _ = reader.take_array("managed_paths").map_err(convert)?;
    let hashes_reader = reader.take_object("hashes").map_err(convert)?;
    let mut hashes = BTreeMap::new();
    for (key, value) in hashes_reader.into_fields() {
        if key.contains('/') || key.contains("..") || key == RECORD_FILE {
            return Err(format!("managed path {key:?} is outside the package"));
        }
        hashes.insert(key, json::expect_string(value, "hash").map_err(convert)?);
    }
    let backup = reader.take_optional_string("backup").map_err(convert)?;
    reader.finish().map_err(convert)?;
    Ok(Record {
        schema_version,
        target,
        scope,
        package_version,
        hashes,
        backup,
    })
}

/// Resolve a recorded backup identity against the active installation.
/// Backups are sibling directories named `memoria.backup*`; a record names
/// one by file name. A legacy absolute path is accepted only when it is such
/// a sibling of this installation. Anything else is rejected without writes,
/// so a copied or edited record can never move another installation's backup.
fn resolve_backup(layout: &Layout, recorded: &str) -> Result<PathBuf, SkillFailure> {
    let path = Path::new(recorded);
    let name = if path.is_absolute() {
        if path.parent() != Some(layout.parent.as_path()) {
            return Err(conflict(
                format!(
                    "the install record names backup {recorded} outside this installation's parent {}; nothing was changed",
                    layout.parent.display()
                ),
                vec![path.to_path_buf()],
            ));
        }
        path.file_name()
            .and_then(|n| n.to_str())
            .unwrap_or_default()
            .to_string()
    } else {
        recorded.to_string()
    };
    let valid = !name.contains('/')
        && (name == BACKUP_DIR
            || name
                .strip_prefix(BACKUP_DIR)
                .is_some_and(|rest| rest.starts_with('-')));
    if !valid {
        return Err(conflict(
            format!(
                "the install record names an invalid backup identity {recorded:?}; nothing was changed"
            ),
            vec![layout.destination.join(RECORD_FILE)],
        ));
    }
    Ok(layout.parent.join(name))
}

/// The durable transaction record kept beside the package while a
/// mutation is in flight.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Transaction {
    phase: String,
    backup: Option<PathBuf>,
}

fn transaction_to_json(layout: &Layout, txn: &Transaction) -> Json {
    let mut map = BTreeMap::new();
    map.insert("schema_version".into(), Json::Number(1));
    map.insert("phase".into(), Json::String(txn.phase.clone()));
    map.insert(
        "destination".into(),
        Json::String(layout.destination.display().to_string()),
    );
    map.insert(
        "staging".into(),
        Json::String(layout.staging.display().to_string()),
    );
    map.insert(
        "removing".into(),
        Json::String(layout.removing.display().to_string()),
    );
    map.insert(
        "backup".into(),
        txn.backup
            .as_ref()
            .and_then(|b| b.file_name())
            .map(|b| Json::String(b.to_string_lossy().into_owned()))
            .unwrap_or(Json::Null),
    );
    Json::Object(map)
}

fn transaction_from_bytes(layout: &Layout, bytes: &[u8]) -> Result<Transaction, String> {
    let value = json::parse(bytes, Limits::STATE).map_err(|e| e.message)?;
    let mut reader = ObjectReader::new(value, "transaction").map_err(|e| e.message)?;
    let convert = |e: json::JsonError| e.message;
    if reader.take_u64("schema_version").map_err(convert)? != 1 {
        return Err("unsupported transaction schema".into());
    }
    let phase = reader.take_string("phase").map_err(convert)?;
    if phase != "staged" && phase != "removing" {
        return Err(format!("unknown transaction phase {phase:?}"));
    }
    let expect_path = |actual: String, expected: &Path, name: &str| -> Result<(), String> {
        if Path::new(&actual) == expected {
            Ok(())
        } else {
            Err(format!(
                "transaction {name} {actual:?} does not match {}",
                expected.display()
            ))
        }
    };
    expect_path(
        reader.take_string("destination").map_err(convert)?,
        &layout.destination,
        "destination",
    )?;
    expect_path(
        reader.take_string("staging").map_err(convert)?,
        &layout.staging,
        "staging",
    )?;
    expect_path(
        reader.take_string("removing").map_err(convert)?,
        &layout.removing,
        "removing",
    )?;
    let backup = match reader.take_optional_string("backup").map_err(convert)? {
        None => None,
        Some(recorded) => Some(resolve_backup(layout, &recorded).map_err(|e| failure_text(&e))?),
    };
    reader.finish().map_err(convert)?;
    Ok(Transaction { phase, backup })
}

enum Existing {
    Absent,
    Unmanaged(Vec<String>),
    Managed {
        record: Record,
        edited: Vec<String>,
        unknown: Vec<String>,
    },
}

fn entries_of(dir: &Path) -> Result<Vec<String>, SkillFailure> {
    let mut entries = Vec::new();
    for entry in fs::read_dir(dir).map_err(|e| io_failure("read_dir", dir, e))? {
        let entry = entry.map_err(|e| io_failure("read_dir", dir, e))?;
        entries.push(entry.file_name().to_string_lossy().into_owned());
    }
    entries.sort();
    Ok(entries)
}

/// Inspect a package directory: absent, unmanaged, or managed with its
/// edited and unknown entries.
fn inspect(destination: &Path) -> Result<Existing, SkillFailure> {
    match kind_of(destination).map_err(|e| io_failure("stat", destination, e))? {
        memoria_application::ports::FileKind::Missing => return Ok(Existing::Absent),
        memoria_application::ports::FileKind::Directory => {}
        other => {
            return Err(conflict(
                format!(
                    "{} exists but is a {other:?}, not a directory",
                    destination.display()
                ),
                vec![destination.to_path_buf()],
            ));
        }
    }
    let entries = entries_of(destination)?;
    let record_path = destination.join(RECORD_FILE);
    // Every managed entry is inspected by kind before any read: a symlink,
    // FIFO, or directory where a regular file was installed is a local
    // change that is preserved and reported, never followed or opened.
    match kind_of(&record_path).map_err(|e| io_failure("stat", &record_path, e))? {
        memoria_application::ports::FileKind::Regular => {}
        memoria_application::ports::FileKind::Missing => return Ok(Existing::Unmanaged(entries)),
        other => {
            return Err(conflict(
                format!(
                    "{} is a {other:?}, not the regular installation record Memoria wrote; the package is preserved",
                    record_path.display()
                ),
                vec![record_path.clone()],
            ));
        }
    }
    let record = record_from_bytes(
        &fs::read(&record_path).map_err(|e| io_failure("read", &record_path, e))?,
    )
    .map_err(|message| {
        conflict(
            format!("install record is unreadable: {message}"),
            vec![record_path.clone()],
        )
    })?;
    let mut edited = Vec::new();
    for (name, expected) in &record.hashes {
        let path = destination.join(name);
        let regular = matches!(
            kind_of(&path),
            Ok(memoria_application::ports::FileKind::Regular)
        );
        if !regular {
            edited.push(path.display().to_string());
            continue;
        }
        match fs::read(&path) {
            Ok(bytes) if hash_hex(&bytes) == *expected => {}
            _ => edited.push(path.display().to_string()),
        }
    }
    let unknown: Vec<String> = entries
        .iter()
        .filter(|e| e.as_str() != RECORD_FILE && !record.hashes.contains_key(e.as_str()))
        .map(|e| destination.join(e).display().to_string())
        .collect();
    Ok(Existing::Managed {
        record,
        edited,
        unknown,
    })
}

/// A directory that Memoria created itself: exactly the managed files with
/// matching hashes. Anything else is not ours to delete.
fn validate_owned(dir: &Path) -> Result<(), SkillFailure> {
    match inspect(dir)? {
        Existing::Managed {
            edited, unknown, ..
        } if edited.is_empty() && unknown.is_empty() => Ok(()),
        Existing::Managed {
            edited, unknown, ..
        } => {
            let mut paths: Vec<PathBuf> = edited.iter().map(PathBuf::from).collect();
            paths.extend(unknown.iter().map(PathBuf::from));
            Err(conflict(
                format!(
                    "{} contains content Memoria did not write; it is preserved",
                    dir.display()
                ),
                paths,
            ))
        }
        Existing::Unmanaged(_) | Existing::Absent => Err(conflict(
            format!(
                "{} is not a Memoria-owned package directory; it is preserved",
                dir.display()
            ),
            vec![dir.to_path_buf()],
        )),
    }
}

impl FsSkillStore<'static> {
    pub fn new(root: PathBuf, skill: &'static str, version: &'static str) -> FsSkillStore<'static> {
        FsSkillStore {
            root,
            skill,
            version,
            faults: NO_FAULTS,
        }
    }
}

impl<'a> FsSkillStore<'a> {
    /// A store that consults `faults` at the named transaction steps:
    /// `stage-write`, `txn-write`, `rename-backup`, `rename-staging`,
    /// `sync-parent`, `txn-remove`, `rename-removing`, `remove-removing`,
    /// and `restore-backup`.
    pub fn with_faults(
        root: PathBuf,
        skill: &'static str,
        version: &'static str,
        faults: FaultHook<'a>,
    ) -> FsSkillStore<'a> {
        FsSkillStore {
            root,
            skill,
            version,
            faults,
        }
    }

    fn layout(&self, parent: &str) -> Layout {
        let parent_path = PathBuf::from(parent);
        let parent = if parent_path.is_absolute() {
            parent_path
        } else {
            std::env::current_dir()
                .map(|cwd| cwd.join(&parent_path))
                .unwrap_or(parent_path)
        };
        Layout {
            destination: parent.join(PACKAGE_NAME),
            backup: parent.join(BACKUP_DIR),
            staging: parent.join(STAGING_DIR),
            removing: parent.join(REMOVING_DIR),
            txn: parent.join(TXN_FILE),
            lock: parent.join(LOCK_FILE),
            parent,
        }
    }

    fn managed_files(
        &self,
        target: AgentTarget,
        scope: AgentScope,
        backup: Option<String>,
    ) -> Vec<(String, Vec<u8>)> {
        let skill_bytes = self.skill.as_bytes().to_vec();
        let mut hashes = BTreeMap::new();
        hashes.insert(SKILL_FILE.to_string(), hash_hex(&skill_bytes));
        let record = Record {
            schema_version: 2,
            target: target.as_str().to_string(),
            scope: Some(scope.as_str().to_string()),
            package_version: self.version.to_string(),
            hashes,
            backup,
        };
        let record_bytes = json::to_pretty(&record_to_json(&record)).into_bytes();
        vec![
            (SKILL_FILE.to_string(), skill_bytes),
            (RECORD_FILE.to_string(), record_bytes),
        ]
    }

    fn step(&self, name: &str, path: &Path) -> Result<(), SkillFailure> {
        fault(self.faults, name).map_err(|e| io_failure(name, path, e))
    }

    fn sync_parent(&self, layout: &Layout) -> Result<(), SkillFailure> {
        self.step("sync-parent", &layout.parent)?;
        sync_dir(&layout.parent).map_err(|e| io_failure("sync_dir", &layout.parent, e))
    }

    fn write_txn(&self, layout: &Layout, txn: &Transaction) -> Result<(), SkillFailure> {
        self.step("txn-write", &layout.txn)?;
        durable_replace(
            &layout.txn,
            json::to_pretty(&transaction_to_json(layout, txn)).as_bytes(),
            Some(0o600),
        )
        .map_err(SkillFailure::Io)
    }

    fn remove_txn(&self, layout: &Layout) -> Result<(), SkillFailure> {
        self.step("txn-remove", &layout.txn)?;
        fs::remove_file(&layout.txn).map_err(|e| io_failure("remove", &layout.txn, e))?;
        sync_dir(&layout.parent).map_err(|e| io_failure("sync_dir", &layout.parent, e))
    }

    /// Read-only: what recovery would have to do, or the conflict it would hit.
    fn recovery_state(&self, layout: &Layout) -> Result<Option<Transaction>, SkillFailure> {
        let txn = match kind_of(&layout.txn).map_err(|e| io_failure("stat", &layout.txn, e))? {
            memoria_application::ports::FileKind::Missing => None,
            memoria_application::ports::FileKind::Regular => {
                let bytes =
                    fs::read(&layout.txn).map_err(|e| io_failure("read", &layout.txn, e))?;
                Some(transaction_from_bytes(layout, &bytes).map_err(|message| {
                    conflict(
                        format!("the transaction record is invalid ({message}); nothing was changed; inspect {}", layout.parent.display()),
                        vec![layout.txn.clone()],
                    )
                })?)
            }
            other => {
                return Err(conflict(
                    format!(
                        "{} is a {other:?}; expected a transaction record file",
                        layout.txn.display()
                    ),
                    vec![layout.txn.clone()],
                ));
            }
        };
        for stray in [&layout.staging, &layout.removing] {
            if stray.exists() {
                if txn.is_none() {
                    return Err(conflict(
                        format!(
                            "{} exists but no recorded transaction created it; move it away before continuing",
                            stray.display()
                        ),
                        vec![stray.clone()],
                    ));
                }
                validate_owned(stray)?;
            }
        }
        Ok(txn)
    }

    /// Finish or roll back an interrupted transaction. Every deletion touches
    /// only directories that [`validate_owned`] accepted.
    fn recover(&self, layout: &Layout) -> Result<Option<String>, SkillFailure> {
        let Some(txn) = self.recovery_state(layout)? else {
            return Ok(None);
        };
        match txn.phase.as_str() {
            "staged" => {
                if layout.staging.exists() {
                    if layout.destination.exists() {
                        match &txn.backup {
                            Some(backup) if !backup.exists() => {
                                self.rename(&layout.destination, backup, "rename-backup")?;
                            }
                            Some(backup) => {
                                return Err(conflict(
                                    format!(
                                        "cannot finish the interrupted installation: both {} and its backup {} exist",
                                        layout.destination.display(),
                                        backup.display()
                                    ),
                                    vec![layout.destination.clone(), backup.clone()],
                                ));
                            }
                            None => {
                                return Err(conflict(
                                    format!(
                                        "cannot finish the interrupted installation: {} appeared after staging",
                                        layout.destination.display()
                                    ),
                                    vec![layout.destination.clone()],
                                ));
                            }
                        }
                    }
                    self.rename(&layout.staging, &layout.destination, "rename-staging")?;
                } else if !layout.destination.exists()
                    && let Some(backup) = &txn.backup
                    && backup.exists()
                {
                    self.rename(backup, &layout.destination, "restore-backup")?;
                }
            }
            _ => {
                if layout.removing.exists() {
                    self.step("remove-removing", &layout.removing)?;
                    fs::remove_dir_all(&layout.removing)
                        .map_err(|e| io_failure("remove", &layout.removing, e))?;
                }
                if !layout.destination.exists()
                    && let Some(backup) = &txn.backup
                    && backup.exists()
                {
                    self.rename(backup, &layout.destination, "restore-backup")?;
                }
            }
        }
        self.remove_txn(layout)?;
        Ok(Some(txn.phase))
    }

    /// Rename and sync the parent. A failure reports whether the rename
    /// itself completed (`true`) so callers can distinguish a completed
    /// transition with uncertain durability from one that never happened.
    fn rename_moved(&self, from: &Path, to: &Path, step: &str) -> Result<(), (bool, SkillFailure)> {
        self.step(step, from).map_err(|e| (false, e))?;
        fs::rename(from, to).map_err(|e| (false, io_failure("rename", from, e)))?;
        if let Some(parent) = to.parent() {
            self.step("sync-after-rename", parent)
                .map_err(|e| (true, e))?;
            sync_dir(parent).map_err(|e| (true, io_failure("sync_dir", parent, e)))?;
        }
        Ok(())
    }

    fn rename(&self, from: &Path, to: &Path, step: &str) -> Result<(), SkillFailure> {
        self.rename_moved(from, to, step).map_err(|(_, e)| e)
    }

    fn lock(&self, layout: &Layout) -> Result<crate::fs::FileGuard, SkillFailure> {
        fs::create_dir_all(&layout.parent)
            .map_err(|e| io_failure("create_dir", &layout.parent, e))?;
        lock_file(&layout.lock).map_err(|f| match f {
            memoria_application::ports::LockFailure::Busy => conflict(
                "another installation is in progress",
                vec![layout.lock.clone()],
            ),
            memoria_application::ports::LockFailure::Io(err) => SkillFailure::Io(err),
        })
    }
}

fn next_backup(base: &Path) -> PathBuf {
    if !base.exists() {
        return base.to_path_buf();
    }
    let mut n = 2;
    loop {
        let candidate = base.with_file_name(format!("{BACKUP_DIR}-{n}"));
        if !candidate.exists() {
            return candidate;
        }
        n += 1;
    }
}

impl FsSkillStore<'_> {
    /// Inspect a destination and build the plan for one request. This never
    /// writes and never creates a directory or a lock, so `status` and every
    /// dry run preserve each byte.
    fn classify(&self, request: &SkillRequest) -> Result<SkillPlan, SkillFailure> {
        let layout = self.layout(&request.parent);
        // An absent parent needs no recovery probe and must not be created.
        let recovery = if layout.parent.exists() {
            self.recovery_state(&layout)?
        } else {
            None
        };
        let recovery_needed = recovery.is_some();
        let mut retained = Vec::new();
        if layout.lock.exists() {
            // The parent lock is a deliberate synchronization artifact.
            // Deleting a lock pathname can let another process lock a
            // different inode during a concurrent operation.
            retained.push(RetainedArtifact {
                path: layout.lock.display().to_string(),
                reason: "synchronization_lock",
                removable_by_uninstall: false,
            });
        }
        let base = SkillPlan {
            operation: request.operation,
            target: request.target,
            scope: request.scope,
            destination: layout.destination.display().to_string(),
            state: "absent".into(),
            package_version: None,
            embedded_version: self.version.to_string(),
            backup: None,
            writes: vec![],
            removals: vec![],
            replaced: vec![],
            modified_paths: vec![],
            unknown_paths: vec![],
            retained_artifacts: retained,
            overlapping: self.overlapping(request),
            no_change: false,
            recovery_needed,
            existing: "absent".into(),
        };
        let existing = inspect(&layout.destination)?;
        let plan = match existing {
            Existing::Absent => SkillPlan {
                existing: match &recovery {
                    Some(txn) => format!("interrupted {} transaction awaiting recovery", txn.phase),
                    None => "absent".into(),
                },
                backup: recovery
                    .as_ref()
                    .and_then(|t| t.backup.as_ref())
                    .map(|b| b.display().to_string()),
                ..base
            },
            Existing::Unmanaged(entries) => SkillPlan {
                state: "unmanaged".into(),
                unknown_paths: entries
                    .iter()
                    .map(|e| layout.destination.join(e).display().to_string())
                    .collect(),
                existing: "unmanaged directory with no Memoria install record".into(),
                ..base
            },
            Existing::Managed {
                record,
                edited,
                unknown,
            } => {
                let state = if !edited.is_empty() || !unknown.is_empty() {
                    "modified"
                } else if record.target != request.target.as_str() {
                    // Another target's managed package is never ours to replace.
                    "conflict"
                } else if record.package_version == self.version
                    && fs::read(layout.destination.join(SKILL_FILE))
                        .map(|bytes| bytes == self.skill.as_bytes())
                        .unwrap_or(false)
                {
                    "current"
                } else {
                    "outdated"
                };
                let backup = match &record.backup {
                    Some(recorded) => {
                        Some(resolve_backup(&layout, recorded)?).filter(|b| b.exists())
                    }
                    None => None,
                };
                let mut retained = base.retained_artifacts.clone();
                if let Some(backup) = &backup {
                    retained.push(RetainedArtifact {
                        path: backup.display().to_string(),
                        reason: "user_backup",
                        removable_by_uninstall: true,
                    });
                }
                SkillPlan {
                    state: state.into(),
                    package_version: Some(record.package_version.clone()),
                    backup: backup.map(|b| b.display().to_string()),
                    modified_paths: edited,
                    unknown_paths: unknown,
                    retained_artifacts: retained,
                    existing: format!(
                        "managed package version {} for {} (record schema {})",
                        record.package_version, record.target, record.schema_version
                    ),
                    ..base
                }
            }
        };
        Ok(plan)
    }

    /// Same-name packages in other scopes, reported without any mutation.
    fn overlapping(&self, request: &SkillRequest) -> Vec<OverlappingPackage> {
        let mut out = Vec::new();
        for (scope, parent) in &request.other_parents {
            let destination = PathBuf::from(parent).join(PACKAGE_NAME);
            if !destination.exists() {
                continue;
            }
            let state = match inspect(&destination) {
                Ok(Existing::Managed { record, .. }) => {
                    format!("managed version {}", record.package_version)
                }
                Ok(Existing::Unmanaged(_)) => "unmanaged".to_string(),
                Ok(Existing::Absent) => continue,
                Err(_) => "unreadable".to_string(),
            };
            let note = match scope.as_str() {
                "global" => {
                    "a personal skill can take precedence over a project skill with the same name"
                        .to_string()
                }
                "local" => "a project skill with the same name exists".to_string(),
                _ => "a package exists at a recognized legacy location".to_string(),
            };
            out.push(OverlappingPackage {
                scope: scope.clone(),
                destination: destination.display().to_string(),
                state,
                note,
            });
        }
        out
    }

    /// The install plan: absent installs, current is a no-op, an older
    /// managed package requires an explicit upgrade, and unmanaged content
    /// requires explicit replacement.
    fn plan_install(&self, request: &SkillRequest) -> Result<SkillPlan, SkillFailure> {
        let layout = self.layout(&request.parent);
        let mut plan = self.classify(request)?;
        let writes: Vec<String> = self
            .managed_files(request.target, request.scope, None)
            .iter()
            .map(|(name, _)| name.clone())
            .collect();
        match plan.state.as_str() {
            "absent" => {
                plan.writes = writes;
                Ok(plan)
            }
            "current" if !plan.recovery_needed => {
                plan.no_change = true;
                Ok(plan)
            }
            "current" => {
                plan.writes = writes;
                Ok(plan)
            }
            "outdated" => Err(SkillFailure::UpgradeRequired(format!(
                "{} holds managed package version {}; this executable installs {}. Run `memoria agent upgrade` to replace it.",
                layout.destination.display(),
                plan.package_version.as_deref().unwrap_or("unknown"),
                self.version
            ))),
            "unmanaged" if request.replace_existing => {
                plan.writes = writes;
                // `replaced` names are relative to the destination, like
                // `writes` and `removals`; `unknown_paths` stay absolute.
                plan.replaced = plan
                    .unknown_paths
                    .iter()
                    .filter_map(|path| {
                        Path::new(path)
                            .file_name()
                            .map(|name| name.to_string_lossy().into_owned())
                    })
                    .collect();
                plan.backup = Some(next_backup(&layout.backup).display().to_string());
                Ok(plan)
            }
            "unmanaged" => Err(SkillFailure::ReplacementRequired(format!(
                "{} already exists and Memoria did not write it. Pass --replace-existing to back it up and replace it, or choose another --path.",
                layout.destination.display()
            ))),
            _ => Err(SkillFailure::Conflict {
                message: format!(
                    "{} was modified locally or belongs to another target; it is preserved",
                    layout.destination.display()
                ),
                paths: plan
                    .modified_paths
                    .iter()
                    .chain(&plan.unknown_paths)
                    .cloned()
                    .collect(),
            }),
        }
    }

    /// The upgrade plan: only a verified managed package of this target is
    /// replaced, and only the original user backup is retained.
    fn plan_upgrade(&self, request: &SkillRequest) -> Result<SkillPlan, SkillFailure> {
        let layout = self.layout(&request.parent);
        let mut plan = self.classify(request)?;
        match plan.state.as_str() {
            "absent" | "unmanaged" => Err(SkillFailure::NotInstalled(
                layout.destination.display().to_string(),
            )),
            "current" if !plan.recovery_needed => {
                plan.no_change = true;
                Ok(plan)
            }
            "current" | "outdated" => {
                plan.writes = self
                    .managed_files(request.target, request.scope, None)
                    .iter()
                    .map(|(name, _)| name.clone())
                    .collect();
                plan.replaced = vec![SKILL_FILE.into(), RECORD_FILE.into()];
                // An upgrade keeps only the original unmanaged backup. The
                // previous managed version is obsolete, not user content.
                plan.backup = self.original_backup(&layout, request)?;
                Ok(plan)
            }
            _ => Err(SkillFailure::Conflict {
                message: format!(
                    "{} was modified locally or belongs to another target; it is preserved",
                    layout.destination.display()
                ),
                paths: plan
                    .modified_paths
                    .iter()
                    .chain(&plan.unknown_paths)
                    .cloned()
                    .collect(),
            }),
        }
    }

    /// Follow the recorded backup chain to the original unmanaged directory.
    ///
    /// A backup that is itself a managed package of the same target is an
    /// obsolete Memoria version, so the chain continues through it. Cycles,
    /// escaped paths, changed hashes, and foreign targets are rejected
    /// before any destructive cleanup.
    fn original_backup(
        &self,
        layout: &Layout,
        request: &SkillRequest,
    ) -> Result<Option<String>, SkillFailure> {
        Ok(self.backup_chain(layout, request)?.1)
    }

    /// The verified backup chain: every obsolete managed version, then the
    /// original unmanaged directory when one survives.
    fn backup_chain(
        &self,
        layout: &Layout,
        request: &SkillRequest,
    ) -> Result<(Vec<PathBuf>, Option<String>), SkillFailure> {
        let mut obsolete: Vec<PathBuf> = Vec::new();
        let mut seen: Vec<PathBuf> = Vec::new();
        let mut current = match inspect(&layout.destination)? {
            Existing::Managed { record, .. } => record.backup.clone(),
            _ => None,
        };
        while let Some(recorded) = current {
            let path = resolve_backup(layout, &recorded)?;
            if seen.contains(&path) {
                return Err(conflict(
                    format!(
                        "the backup chain under {} contains a cycle at {}; nothing was changed",
                        layout.parent.display(),
                        path.display()
                    ),
                    vec![path],
                ));
            }
            seen.push(path.clone());
            if !path.exists() {
                return Ok((obsolete, None));
            }
            match inspect(&path)? {
                // A previous Memoria package: obsolete, keep following.
                Existing::Managed {
                    record,
                    edited,
                    unknown,
                } if record.target == request.target.as_str()
                    && edited.is_empty()
                    && unknown.is_empty() =>
                {
                    obsolete.push(path.clone());
                    current = record.backup.clone();
                }
                Existing::Managed {
                    edited, unknown, ..
                } => {
                    let mut paths: Vec<PathBuf> = edited.iter().map(PathBuf::from).collect();
                    paths.extend(unknown.iter().map(PathBuf::from));
                    if paths.is_empty() {
                        paths.push(path.clone());
                    }
                    return Err(conflict(
                        format!(
                            "the backup at {} was edited or belongs to another target; it is preserved",
                            path.display()
                        ),
                        paths,
                    ));
                }
                // The original user content: this is the backup to retain.
                Existing::Unmanaged(_) => {
                    return Ok((obsolete, Some(path.display().to_string())));
                }
                Existing::Absent => return Ok((obsolete, None)),
            }
        }
        Ok((obsolete, None))
    }

    /// The uninstall plan. An absent package is a successful no-op that
    /// creates no directory and no lock.
    fn plan_uninstall(&self, request: &SkillRequest) -> Result<SkillPlan, SkillFailure> {
        let layout = self.layout(&request.parent);
        let mut plan = self.classify(request)?;
        match plan.state.as_str() {
            "absent" if plan.recovery_needed => Ok(plan),
            "absent" => {
                plan.no_change = true;
                Ok(plan)
            }
            "unmanaged" => Err(conflict(
                format!(
                    "{} is not a Memoria-managed package; it has no install record and stays untouched",
                    layout.destination.display()
                ),
                vec![layout.destination.clone()],
            )),
            "current" | "outdated" => {
                let mut removals: Vec<String> = match inspect(&layout.destination)? {
                    Existing::Managed { record, .. } => record.hashes.keys().cloned().collect(),
                    _ => vec![SKILL_FILE.into()],
                };
                removals.push(RECORD_FILE.into());
                plan.removals = removals;
                // Uninstall restores the original unmanaged directory once.
                // It never restores an obsolete Memoria package.
                plan.backup = self.original_backup(&layout, request)?;
                Ok(plan)
            }
            _ => Err(SkillFailure::Conflict {
                message: format!(
                    "the installed package at {} was modified locally or belongs to another target; the package and any backup are preserved",
                    layout.destination.display()
                ),
                paths: plan
                    .modified_paths
                    .iter()
                    .chain(&plan.unknown_paths)
                    .cloned()
                    .collect(),
            }),
        }
    }

    fn apply_install(&self, request: &SkillRequest, plan: &SkillPlan) -> Result<(), SkillFailure> {
        let destination = PathBuf::from(&plan.destination);
        let parent = destination
            .parent()
            .expect("destination has a parent")
            .to_path_buf();
        let layout = self.layout(&parent.display().to_string());
        // The plan names the destination; a plan built for one parent is
        // applied to that parent, never to the request's default.
        let request = &SkillRequest {
            parent: layout.parent.display().to_string(),
            ..request.clone()
        };
        let _guard = self.lock(&layout)?;
        self.recover(&layout)?;
        // Re-run the operation's own eligibility check under the lock, not
        // just a classification. An install that became an upgrade, or a
        // package that gained unknown content, must fail here rather than
        // proceed on a stale plan.
        let fresh = match request.operation {
            SkillOperation::Upgrade => self.plan_upgrade(request)?,
            _ => self.plan_install(request)?,
        };
        if fresh.no_change {
            return Ok(());
        }
        // Recovery deliberately changes the destination, so a plan that
        // asked for recovery is compared against nothing: the fresh plan is
        // authoritative from here. Every other plan must still describe the
        // destination it is about to change.
        if !plan.recovery_needed {
            agrees_with_plan(plan, &fresh)?;
        }
        let backup_path = if layout.destination.exists() {
            Some(next_backup(&layout.backup))
        } else {
            None
        };
        // An upgrade retains only the original user backup reference. The
        // previous managed version is moved aside for the swap and removed
        // once the new package is durable, so the chain never grows.
        let original = if request.operation == SkillOperation::Upgrade {
            self.backup_chain(&layout, request)?.1.and_then(|path| {
                PathBuf::from(path)
                    .file_name()
                    .map(|n| n.to_string_lossy().into_owned())
            })
        } else {
            backup_path
                .as_ref()
                .and_then(|p| p.file_name())
                .map(|n| n.to_string_lossy().into_owned())
        };
        let files = self.managed_files(plan.target, plan.scope, original);

        // Stage a complete package, then record the transaction.
        fs::create_dir(&layout.staging)
            .map_err(|e| io_failure("create_dir", &layout.staging, e))?;
        let staged = (|| -> Result<(), SkillFailure> {
            for (name, bytes) in &files {
                self.step("stage-write", &layout.staging.join(name))?;
                durable_replace(&layout.staging.join(name), bytes, None)
                    .map_err(SkillFailure::Io)?;
            }
            sync_dir(&layout.staging).map_err(|e| io_failure("sync_dir", &layout.staging, e))?;
            self.write_txn(
                &layout,
                &Transaction {
                    phase: "staged".into(),
                    backup: backup_path.clone(),
                },
            )
        })();
        if let Err(err) = staged {
            let _ = fs::remove_dir_all(&layout.staging);
            let _ = fs::remove_file(&layout.txn);
            return Err(err);
        }

        // Swap the package into place.
        let mut moved_backup: Option<PathBuf> = None;
        let mut package_in_place = false;
        let swapped = (|| -> Result<(), SkillFailure> {
            if let Some(backup) = &backup_path {
                match self.rename_moved(&layout.destination, backup, "rename-backup") {
                    Ok(()) => moved_backup = Some(backup.clone()),
                    Err((true, err)) => {
                        moved_backup = Some(backup.clone());
                        return Err(err);
                    }
                    Err((false, err)) => return Err(err),
                }
            }
            match self.rename_moved(&layout.staging, &layout.destination, "rename-staging") {
                Ok(()) => Ok(()),
                Err((true, err)) => {
                    package_in_place = true;
                    Err(err)
                }
                Err((false, err)) => Err(err),
            }
        })();
        if package_in_place && let Err(err) = swapped {
            // The new package is in place but its directory sync failed. Keep
            // the transaction so recovery can finish durably on the next run.
            return Err(SkillFailure::Io(AdapterError::new(
                "sync_dir",
                Some(layout.parent.display().to_string()),
                format!(
                    "the package was renamed into place but the directory sync failed ({}); the transaction record {} is retained and the next `agent install` or `agent uninstall` completes recovery{}",
                    failure_text(&err),
                    layout.txn.display(),
                    moved_backup
                        .as_ref()
                        .map(|b| format!("; the previous package is kept at {}", b.display()))
                        .unwrap_or_default()
                ),
            )));
        }
        if let Err(err) = swapped {
            // Ordinary error: restore the old package and report every failure.
            let mut messages = vec![format!(
                "installation failed: {err_text}",
                err_text = failure_text(&err)
            )];
            if let Some(backup) = &moved_backup
                && !layout.destination.exists()
                && let Err(restore) = fs::rename(backup, &layout.destination)
            {
                messages.push(format!(
                    "restoring the previous package from {} failed: {restore}",
                    backup.display()
                ));
            }
            if layout.staging.exists()
                && let Err(cleanup) = fs::remove_dir_all(&layout.staging)
            {
                messages.push(format!(
                    "removing the staged package {} failed: {cleanup}",
                    layout.staging.display()
                ));
            }
            if let Err(cleanup) = fs::remove_file(&layout.txn) {
                messages.push(format!("removing the transaction record failed: {cleanup}"));
            }
            let _ = sync_dir(&layout.parent);
            if messages.len() > 1 {
                return Err(conflict(messages.join("; "), vec![layout.parent.clone()]));
            }
            return Err(err);
        }
        if let Err(err) = self.sync_parent(&layout) {
            // The package is complete; only durability of the rename is uncertain.
            let _ = self.remove_txn(&layout);
            return Err(SkillFailure::Io(AdapterError::new(
                "sync_dir",
                Some(layout.parent.display().to_string()),
                format!(
                    "the package is installed but the directory sync failed; durability is not guaranteed: {}",
                    failure_text(&err)
                ),
            )));
        }
        self.remove_txn(&layout)?;
        // An upgrade retains only the original user backup. The previous
        // managed version is obsolete once the new package is durable, and
        // it is removed only after verified ownership.
        if request.operation == SkillOperation::Upgrade
            && let Some(previous) = &backup_path
            && previous.exists()
            && validate_owned(previous).is_ok()
        {
            let _ = fs::remove_dir_all(previous);
            let _ = sync_dir(&layout.parent);
        }
        Ok(())
    }

    fn apply_uninstall(
        &self,
        request: &SkillRequest,
        plan: &SkillPlan,
    ) -> Result<(), SkillFailure> {
        let destination = PathBuf::from(&plan.destination);
        let parent = destination
            .parent()
            .expect("destination has a parent")
            .to_path_buf();
        let layout = self.layout(&parent.display().to_string());
        let request = &SkillRequest {
            parent: layout.parent.display().to_string(),
            ..request.clone()
        };
        let _guard = self.lock(&layout)?;
        if self.recover(&layout)?.as_deref() == Some("removing") {
            // The interrupted removal is now complete; nothing else to remove.
            return Ok(());
        }
        // The eligibility check runs again under the retained lock, and its
        // result is honored. A file added between planning and application
        // makes the package unknown content, which must survive.
        let fresh = match self.plan_uninstall(request) {
            Ok(fresh) if fresh.state == "absent" && !fresh.recovery_needed => return Ok(()),
            Ok(fresh) => fresh,
            Err(SkillFailure::NotInstalled(_)) if plan.recovery_needed => return Ok(()),
            Err(err) => return Err(err),
        };
        if !plan.recovery_needed {
            agrees_with_plan(plan, &fresh)?;
        }
        let (obsolete, original) = self.backup_chain(&layout, request)?;
        let backup = original.as_ref().map(PathBuf::from);
        self.write_txn(
            &layout,
            &Transaction {
                phase: "removing".into(),
                backup: backup.clone(),
            },
        )?;
        match self.rename_moved(&layout.destination, &layout.removing, "rename-removing") {
            Ok(()) => {}
            Err((false, err)) => {
                let _ = fs::remove_file(&layout.txn);
                return Err(err);
            }
            Err((true, err)) => {
                // The package moved but the directory sync failed: keep the
                // transaction so the next run finishes the removal.
                return Err(SkillFailure::Io(AdapterError::new(
                    "sync_dir",
                    Some(layout.parent.display().to_string()),
                    format!(
                        "the package was moved to {} but the directory sync failed ({}); the transaction record {} is retained and the next `agent uninstall` completes the removal{}",
                        layout.removing.display(),
                        failure_text(&err),
                        layout.txn.display(),
                        backup
                            .as_ref()
                            .map(|b| format!("; the backup at {} will be restored", b.display()))
                            .unwrap_or_default()
                    ),
                )));
            }
        }
        self.step("remove-removing", &layout.removing)?;
        fs::remove_dir_all(&layout.removing)
            .map_err(|e| io_failure("remove", &layout.removing, e))?;
        if let Some(backup) = &backup
            && backup.exists()
            && !layout.destination.exists()
            && let Err((moved, err)) =
                self.rename_moved(backup, &layout.destination, "restore-backup")
        {
            // Either way the transaction stays: recovery restores the backup
            // when it is still pending, or clears the record when done.
            return Err(SkillFailure::Io(AdapterError::new(
                "restore",
                Some(layout.parent.display().to_string()),
                format!(
                    "{} ({}); the transaction record {} is retained for recovery",
                    if moved {
                        "the backup was restored but the directory sync failed"
                    } else {
                        "restoring the backup failed"
                    },
                    failure_text(&err),
                    layout.txn.display()
                ),
            )));
        }
        // Obsolete managed versions no longer serve a recovery purpose. The
        // original user backup, if any, has already been restored.
        for path in obsolete {
            if path.exists() && validate_owned(&path).is_ok() {
                let _ = fs::remove_dir_all(&path);
            }
        }
        let _ = sync_dir(&layout.parent);
        self.remove_txn(&layout)
    }
}

impl SkillPackageStore for FsSkillStore<'_> {
    fn directory_exists(&self, relative: &str) -> bool {
        self.root.join(relative).is_dir()
    }

    fn is_managed_package(&self, relative: &str) -> bool {
        matches!(
            inspect(&self.root.join(relative)),
            Ok(Existing::Managed { .. })
        )
    }

    fn plan(&self, request: &SkillRequest) -> Result<SkillPlan, SkillFailure> {
        match request.operation {
            SkillOperation::Status => self.classify(request),
            SkillOperation::Install => self.plan_install(request),
            SkillOperation::Upgrade => self.plan_upgrade(request),
            SkillOperation::Uninstall => self.plan_uninstall(request),
        }
    }

    fn apply(&self, request: &SkillRequest, plan: &SkillPlan) -> Result<(), SkillFailure> {
        match request.operation {
            // Status never writes.
            SkillOperation::Status => Ok(()),
            SkillOperation::Install | SkillOperation::Upgrade => self.apply_install(request, plan),
            SkillOperation::Uninstall => self.apply_uninstall(request, plan),
        }
    }
}

/// Refuse to apply a plan that no longer describes the destination.
///
/// Planning and application are separate commands' worth of work, and the
/// lock is taken between them. If the package identity, its state, or its
/// unknown content changed in that window, the plan is stale and applying
/// it could destroy content nobody reviewed.
fn agrees_with_plan(plan: &SkillPlan, fresh: &SkillPlan) -> Result<(), SkillFailure> {
    let mut differences = Vec::new();
    if plan.destination != fresh.destination {
        differences.push(format!(
            "destination {} became {}",
            plan.destination, fresh.destination
        ));
    }
    if plan.state != fresh.state {
        differences.push(format!("state {} became {}", plan.state, fresh.state));
    }
    if plan.package_version != fresh.package_version {
        differences.push(format!(
            "installed version {} became {}",
            plan.package_version.as_deref().unwrap_or("none"),
            fresh.package_version.as_deref().unwrap_or("none")
        ));
    }
    if plan.unknown_paths != fresh.unknown_paths {
        differences.push("the package gained or lost unknown content".to_string());
    }
    if plan.modified_paths != fresh.modified_paths {
        differences.push("the package gained or lost local edits".to_string());
    }
    if differences.is_empty() {
        return Ok(());
    }
    let mut paths = fresh.unknown_paths.clone();
    paths.extend(fresh.modified_paths.clone());
    if paths.is_empty() {
        paths.push(fresh.destination.clone());
    }
    Err(SkillFailure::Conflict {
        message: format!(
            "{} changed after the plan was made ({}); every file is preserved. Run the command again.",
            fresh.destination,
            differences.join("; ")
        ),
        paths,
    })
}

fn failure_text(failure: &SkillFailure) -> String {
    match failure {
        SkillFailure::Conflict { message, .. } => message.clone(),
        SkillFailure::Io(err) => err.to_string(),
        SkillFailure::NotInstalled(path) => format!("no package at {path}"),
        SkillFailure::UpgradeRequired(message) | SkillFailure::ReplacementRequired(message) => {
            message.clone()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn store(root: &Path) -> FsSkillStore<'static> {
        FsSkillStore::new(root.to_path_buf(), "# Skill\n", "0.1.0")
    }

    fn parent_of(dir: &tempfile::TempDir) -> (PathBuf, String) {
        let parent = dir.path().join("skills");
        let text = parent.display().to_string();
        (parent, text)
    }

    fn request(operation: SkillOperation, parent: &str) -> SkillRequest {
        SkillRequest {
            operation,
            target: AgentTarget::Codex,
            scope: AgentScope::Local,
            parent: parent.to_string(),
            replace_existing: true,
            other_parents: vec![],
        }
    }

    fn install(store: &FsSkillStore<'_>, parent: &str) -> Result<(), SkillFailure> {
        let request = request(SkillOperation::Install, parent);
        let plan = store.plan(&request)?;
        store.apply(&request, &plan)
    }

    #[allow(dead_code)]
    fn uninstall(store: &FsSkillStore<'_>, parent: &str) -> Result<(), SkillFailure> {
        let request = request(SkillOperation::Uninstall, parent);
        let plan = store.plan(&request)?;
        store.apply(&request, &plan)
    }

    fn assert_installed(parent: &Path) {
        assert_eq!(
            fs::read_to_string(parent.join("memoria/SKILL.md")).unwrap(),
            "# Skill\n"
        );
        assert!(parent.join("memoria").join(RECORD_FILE).exists());
        assert!(
            !parent.join(TXN_FILE).exists(),
            "no transaction record remains"
        );
        assert!(!parent.join(STAGING_DIR).exists());
        assert!(!parent.join(REMOVING_DIR).exists());
    }

    #[test]
    fn install_reinstall_uninstall_cycle() {
        let dir = tempfile::tempdir().unwrap();
        let (parent, text) = parent_of(&dir);
        let store = store(dir.path());
        install(&store, &text).unwrap();
        assert_installed(&parent);
        assert!(
            store
                .plan(&request(SkillOperation::Install, &text))
                .unwrap()
                .no_change
        );
        assert!(store.is_managed_package("skills/memoria"));
        assert!(!store.is_managed_package("skills"));
        let plan = store
            .plan(&request(SkillOperation::Uninstall, &text))
            .unwrap();
        store
            .apply(&request(SkillOperation::Uninstall, &text), &plan)
            .unwrap();
        assert!(!parent.join("memoria").exists());
        assert!(!parent.join(TXN_FILE).exists());
    }

    #[test]
    fn substituted_managed_entries_are_detected_by_kind_without_being_opened() {
        let dir = tempfile::tempdir().unwrap();
        let (parent, text) = parent_of(&dir);
        let store = store(dir.path());
        install(&store, &text).unwrap();
        // A FIFO in place of SKILL.md: classification terminates and reports a conflict.
        fs::remove_file(parent.join("memoria/SKILL.md")).unwrap();
        // Create the FIFO in-process: forking a helper while other tests hold
        // advisory locks would briefly duplicate their lock descriptors.
        rustix::fs::mknodat(
            rustix::fs::CWD,
            parent.join("memoria/SKILL.md"),
            rustix::fs::FileType::Fifo,
            rustix::fs::Mode::RUSR | rustix::fs::Mode::WUSR,
            0,
        )
        .unwrap();
        assert!(
            store.is_managed_package("skills/memoria"),
            "still a recognized (edited) package"
        );
        assert!(matches!(
            store.plan(&request(SkillOperation::Uninstall, &text)),
            Err(SkillFailure::Conflict { .. })
        ));
        assert!(matches!(
            store.plan(&request(SkillOperation::Install, &text)),
            Err(SkillFailure::Conflict { .. })
        ));
        fs::remove_file(parent.join("memoria/SKILL.md")).unwrap();
        // A symlink to identical external bytes is a local change, not the installed file.
        let outside = tempfile::tempdir().unwrap();
        fs::write(outside.path().join("SKILL.md"), "# Skill\n").unwrap();
        std::os::unix::fs::symlink(
            outside.path().join("SKILL.md"),
            parent.join("memoria/SKILL.md"),
        )
        .unwrap();
        let err = store
            .plan(&request(SkillOperation::Uninstall, &text))
            .unwrap_err();
        assert!(
            matches!(&err, SkillFailure::Conflict { paths, .. } if paths.iter().any(|p| p.ends_with("SKILL.md"))),
            "{err:?}"
        );
        assert!(
            fs::symlink_metadata(parent.join("memoria/SKILL.md"))
                .unwrap()
                .file_type()
                .is_symlink(),
            "preserved"
        );
        assert!(outside.path().join("SKILL.md").exists());
        // A symlinked record is never followed.
        fs::remove_file(parent.join("memoria/SKILL.md")).unwrap();
        fs::write(parent.join("memoria/SKILL.md"), "# Skill\n").unwrap();
        let record = parent.join("memoria").join(RECORD_FILE);
        let moved = outside.path().join("record.json");
        fs::rename(&record, &moved).unwrap();
        std::os::unix::fs::symlink(&moved, &record).unwrap();
        assert!(matches!(
            store.plan(&request(SkillOperation::Uninstall, &text)),
            Err(SkillFailure::Conflict { .. })
        ));
        assert!(!store.is_managed_package("skills/memoria"));
        assert!(moved.exists());
        // Recovery inspection applies the same rule to staged content.
        fs::remove_file(&record).unwrap();
        fs::rename(&moved, &record).unwrap();
        fs::create_dir_all(parent.join(STAGING_DIR)).unwrap();
        for (name, bytes) in store.managed_files(AgentTarget::Codex, AgentScope::Local, None) {
            fs::write(parent.join(STAGING_DIR).join(&name), bytes).unwrap();
        }
        fs::remove_file(parent.join(STAGING_DIR).join("SKILL.md")).unwrap();
        std::os::unix::fs::symlink(
            outside.path().join("SKILL.md"),
            parent.join(STAGING_DIR).join("SKILL.md"),
        )
        .unwrap();
        write_txn(&store, &text, "staged", None);
        assert!(matches!(
            store.plan(&request(SkillOperation::Install, &text)),
            Err(SkillFailure::Conflict { .. })
        ));
        assert!(parent.join(STAGING_DIR).join("SKILL.md").exists());
    }

    #[test]
    fn edited_skill_conflicts_and_is_preserved() {
        let dir = tempfile::tempdir().unwrap();
        let (parent, text) = parent_of(&dir);
        let store = store(dir.path());
        install(&store, &text).unwrap();
        fs::write(parent.join("memoria/SKILL.md"), "edited\n").unwrap();
        assert!(matches!(
            store.plan(&request(SkillOperation::Uninstall, &text)),
            Err(SkillFailure::Conflict { .. })
        ));
        assert_eq!(
            fs::read_to_string(parent.join("memoria/SKILL.md")).unwrap(),
            "edited\n"
        );
        assert!(matches!(
            store.plan(&request(SkillOperation::Install, &text)),
            Err(SkillFailure::Conflict { .. })
        ));
    }

    #[test]
    fn unmanaged_directory_is_backed_up_and_restored() {
        let dir = tempfile::tempdir().unwrap();
        let (parent, text) = parent_of(&dir);
        fs::create_dir_all(parent.join("memoria")).unwrap();
        fs::write(parent.join("memoria/notes.md"), "mine\n").unwrap();
        let store = store(dir.path());
        let plan = store
            .plan(&request(SkillOperation::Install, &text))
            .unwrap();
        assert_eq!(plan.replaced, vec!["notes.md"]);
        store
            .apply(&request(SkillOperation::Install, &text), &plan)
            .unwrap();
        assert_eq!(
            fs::read_to_string(parent.join("memoria.backup/notes.md")).unwrap(),
            "mine\n"
        );
        let plan = store
            .plan(&request(SkillOperation::Uninstall, &text))
            .unwrap();
        assert!(plan.backup.is_some());
        store
            .apply(&request(SkillOperation::Uninstall, &text), &plan)
            .unwrap();
        assert_eq!(
            fs::read_to_string(parent.join("memoria/notes.md")).unwrap(),
            "mine\n"
        );
        assert!(!parent.join("memoria.backup").exists());
    }

    #[test]
    fn unknown_staging_and_removing_content_is_preserved() {
        let dir = tempfile::tempdir().unwrap();
        let (parent, text) = parent_of(&dir);
        fs::create_dir_all(parent.join(STAGING_DIR)).unwrap();
        fs::write(parent.join(STAGING_DIR).join("user.txt"), "keep this").unwrap();
        let store = store(dir.path());
        assert!(matches!(
            store.plan(&request(SkillOperation::Install, &text)),
            Err(SkillFailure::Conflict { .. })
        ));
        let plan = SkillPlan {
            operation: SkillOperation::Install,
            target: AgentTarget::Codex,
            scope: AgentScope::Local,
            destination: parent.join("memoria").display().to_string(),
            state: "absent".into(),
            package_version: None,
            embedded_version: "0.1.0".into(),
            backup: None,
            writes: vec![],
            removals: vec![],
            replaced: vec![],
            modified_paths: vec![],
            unknown_paths: vec![],
            retained_artifacts: vec![],
            overlapping: vec![],
            no_change: false,
            recovery_needed: false,
            existing: String::new(),
        };
        assert!(matches!(
            store.apply(&request(SkillOperation::Install, &text), &plan),
            Err(SkillFailure::Conflict { .. })
        ));
        assert_eq!(
            fs::read_to_string(parent.join(STAGING_DIR).join("user.txt")).unwrap(),
            "keep this"
        );
        assert!(!parent.join("memoria").exists());
        fs::rename(parent.join(STAGING_DIR), parent.join(REMOVING_DIR)).unwrap();
        assert!(matches!(
            store.plan(&request(SkillOperation::Install, &text)),
            Err(SkillFailure::Conflict { .. })
        ));
        assert_eq!(
            fs::read_to_string(parent.join(REMOVING_DIR).join("user.txt")).unwrap(),
            "keep this"
        );
    }

    #[test]
    fn invalid_transaction_records_block_recovery_without_deleting() {
        let dir = tempfile::tempdir().unwrap();
        let (parent, text) = parent_of(&dir);
        fs::create_dir_all(parent.join(STAGING_DIR)).unwrap();
        fs::write(parent.join(STAGING_DIR).join("SKILL.md"), "# Skill\n").unwrap();
        fs::write(parent.join(TXN_FILE), "{}").unwrap();
        let store = store(dir.path());
        let err = store
            .plan(&request(SkillOperation::Install, &text))
            .unwrap_err();
        assert!(
            matches!(&err, SkillFailure::Conflict { message, .. } if message.contains("transaction record is invalid")),
            "{err:?}"
        );
        // A record for a different destination is rejected too.
        fs::write(parent.join(TXN_FILE), "{\"schema_version\":1,\"phase\":\"staged\",\"destination\":\"/elsewhere/memoria\",\"staging\":\"/x\",\"removing\":\"/y\",\"backup\":null}").unwrap();
        assert!(matches!(
            store.plan(&request(SkillOperation::Install, &text)),
            Err(SkillFailure::Conflict { .. })
        ));
        assert!(parent.join(STAGING_DIR).join("SKILL.md").exists());
    }

    fn write_txn(store: &FsSkillStore<'_>, parent: &str, phase: &str, backup: Option<PathBuf>) {
        let layout = store.layout(parent);
        store
            .write_txn(
                &layout,
                &Transaction {
                    phase: phase.into(),
                    backup,
                },
            )
            .unwrap();
    }

    #[test]
    fn interrupted_staged_transaction_is_finished_when_owned() {
        let dir = tempfile::tempdir().unwrap();
        let (parent, text) = parent_of(&dir);
        let store = store(dir.path());
        // Stage a real package the way apply_install does, then "crash".
        fs::create_dir_all(parent.join(STAGING_DIR)).unwrap();
        for (name, bytes) in store.managed_files(AgentTarget::Codex, AgentScope::Local, None) {
            fs::write(parent.join(STAGING_DIR).join(name), bytes).unwrap();
        }
        write_txn(&store, &text, "staged", None);
        let plan = store
            .plan(&request(SkillOperation::Install, &text))
            .unwrap();
        assert!(plan.recovery_needed);
        store
            .apply(&request(SkillOperation::Install, &text), &plan)
            .unwrap();
        assert_installed(&parent);
        // A staged directory with an extra file is not finished; it is preserved.
        fs::create_dir_all(parent.join(STAGING_DIR)).unwrap();
        for (name, bytes) in store.managed_files(AgentTarget::Codex, AgentScope::Local, None) {
            fs::write(parent.join(STAGING_DIR).join(name), bytes).unwrap();
        }
        fs::write(parent.join(STAGING_DIR).join("extra.txt"), "x").unwrap();
        write_txn(&store, &text, "staged", None);
        assert!(matches!(
            store.plan(&request(SkillOperation::Install, &text)),
            Err(SkillFailure::Conflict { .. })
        ));
        assert!(parent.join(STAGING_DIR).join("extra.txt").exists());
    }

    #[test]
    fn interrupted_removal_restores_the_backup() {
        let dir = tempfile::tempdir().unwrap();
        let (parent, text) = parent_of(&dir);
        let store = store(dir.path());
        install(&store, &text).unwrap();
        // Simulate a crash after the package moved to `memoria.removing`.
        fs::create_dir_all(parent.join("memoria.backup")).unwrap();
        fs::write(parent.join("memoria.backup/notes.md"), "mine\n").unwrap();
        fs::rename(parent.join("memoria"), parent.join(REMOVING_DIR)).unwrap();
        write_txn(
            &store,
            &text,
            "removing",
            Some(parent.join("memoria.backup")),
        );
        let plan = store.plan(&request(SkillOperation::Install, &text));
        assert!(plan.is_ok(), "{plan:?}");
        assert!(plan.unwrap().recovery_needed);
        install(&store, &text).unwrap();
        // Recovery restored the backup, then the install replaced it again with a new backup.
        assert_installed(&parent);
        assert!(parent.join("memoria.backup/notes.md").exists());
        assert!(!parent.join(REMOVING_DIR).exists());
    }

    #[test]
    fn copied_installations_restore_only_their_own_sibling_backup() {
        let dir = tempfile::tempdir().unwrap();
        let (parent, text) = parent_of(&dir);
        fs::create_dir_all(parent.join("memoria")).unwrap();
        fs::write(parent.join("memoria/user.txt"), "original\n").unwrap();
        let store = store(dir.path());
        install(&store, &text).unwrap();
        let record = fs::read_to_string(parent.join("memoria").join(RECORD_FILE)).unwrap();
        assert!(
            record.contains("\"backup\": \"memoria.backup\""),
            "{record}"
        );
        let elsewhere = tempfile::tempdir().unwrap();
        let copy = elsewhere.path().join("skills");
        copy_dir(&parent, &copy);
        let plan = store
            .plan(&request(SkillOperation::Uninstall, copy.to_str().unwrap()))
            .unwrap();
        assert_eq!(
            plan.backup.as_deref(),
            Some(copy.join("memoria.backup").to_str().unwrap())
        );
        store
            .apply(&request(SkillOperation::Uninstall, &text), &plan)
            .unwrap();
        assert_eq!(
            fs::read_to_string(copy.join("memoria/user.txt")).unwrap(),
            "original\n"
        );
        assert!(!copy.join("memoria.backup").exists());
        assert!(
            parent.join("memoria.backup/user.txt").exists(),
            "the original backup is untouched"
        );
        assert!(parent.join("memoria/SKILL.md").exists());
        let outside = tempfile::tempdir().unwrap();
        fs::write(outside.path().join("keep.txt"), "keep").unwrap();
        let record_path = parent.join("memoria").join(RECORD_FILE);
        let edited = fs::read_to_string(&record_path).unwrap().replace(
            "\"backup\": \"memoria.backup\"",
            &format!("\"backup\": \"{}\"", outside.path().display()),
        );
        fs::write(&record_path, edited).unwrap();
        assert!(matches!(
            store.plan(&request(SkillOperation::Uninstall, &text)),
            Err(SkillFailure::Conflict { .. })
        ));
        assert!(outside.path().join("keep.txt").exists());
        assert!(parent.join("memoria/SKILL.md").exists());
        let edited = fs::read_to_string(&record_path).unwrap().replace(
            &format!("\"backup\": \"{}\"", outside.path().display()),
            &format!(
                "\"backup\": \"{}\"",
                parent.join("memoria.backup").display()
            ),
        );
        fs::write(&record_path, edited).unwrap();
        let plan = store
            .plan(&request(SkillOperation::Uninstall, &text))
            .unwrap();
        store
            .apply(&request(SkillOperation::Uninstall, &text), &plan)
            .unwrap();
        assert_eq!(
            fs::read_to_string(parent.join("memoria/user.txt")).unwrap(),
            "original\n"
        );
    }

    fn copy_dir(from: &Path, to: &Path) {
        fs::create_dir_all(to).unwrap();
        for entry in fs::read_dir(from).unwrap() {
            let entry = entry.unwrap();
            let target = to.join(entry.file_name());
            if entry.file_type().unwrap().is_dir() {
                copy_dir(&entry.path(), &target);
            } else {
                fs::copy(entry.path(), &target).unwrap();
            }
        }
    }

    #[test]
    fn interrupted_removal_is_actionable_through_planning() {
        let dir = tempfile::tempdir().unwrap();
        let (parent, text) = parent_of(&dir);
        let store = store(dir.path());
        install(&store, &text).unwrap();
        fs::rename(parent.join("memoria"), parent.join(REMOVING_DIR)).unwrap();
        write_txn(&store, &text, "removing", None);
        let plan = store
            .plan(&request(SkillOperation::Uninstall, &text))
            .unwrap();
        assert!(plan.recovery_needed);
        assert!(plan.existing.contains("interrupted removing"));
        assert!(
            parent.join(REMOVING_DIR).exists(),
            "planning writes nothing"
        );
        store
            .apply(&request(SkillOperation::Uninstall, &text), &plan)
            .unwrap();
        assert!(!parent.join(REMOVING_DIR).exists());
        assert!(!parent.join(TXN_FILE).exists());
        assert!(!parent.join("memoria").exists());
        // An absent package is a successful, idempotent no-op.
        let repeated = store
            .plan(&request(SkillOperation::Uninstall, &text))
            .unwrap();
        assert!(repeated.no_change);
        assert_eq!(repeated.state, "absent");
    }

    /// Fail the `nth` occurrence of `step` (1-based).
    fn failing_nth(step: &'static str, nth: u32) -> impl Fn(&str) -> Option<io::Error> {
        let seen = std::cell::Cell::new(0u32);
        move |name| {
            if name != step {
                return None;
            }
            seen.set(seen.get() + 1);
            if seen.get() == nth {
                Some(io::Error::other(format!("injected {step} #{nth}")))
            } else {
                None
            }
        }
    }

    #[test]
    fn sync_failure_after_a_completed_rename_keeps_recovery_state() {
        // Install over an unmanaged package: first rename moves it to the backup.
        let dir = tempfile::tempdir().unwrap();
        let (parent, text) = parent_of(&dir);
        fs::create_dir_all(parent.join("memoria")).unwrap();
        fs::write(parent.join("memoria/user.txt"), "original user content\n").unwrap();
        let hook = failing_nth("sync-after-rename", 1);
        let faulty =
            FsSkillStore::with_faults(dir.path().to_path_buf(), "# Skill\n", "0.1.0", &hook);
        let err = install(&faulty, &text).unwrap_err();
        assert!(failure_text(&err).contains("injected"), "{err:?}");
        assert_eq!(
            fs::read_to_string(parent.join("memoria/user.txt")).unwrap(),
            "original user content\n",
            "the previous package was restored"
        );
        assert!(!parent.join(STAGING_DIR).exists());
        assert!(!parent.join(TXN_FILE).exists());
        install(&store(dir.path()), &text).unwrap();
        assert_installed(&parent);
        assert_eq!(
            fs::read_to_string(parent.join("memoria.backup/user.txt")).unwrap(),
            "original user content\n"
        );

        // Second rename (staging into place) synced badly: the package is complete and the transaction is retained.
        let dir = tempfile::tempdir().unwrap();
        let (parent, text) = parent_of(&dir);
        fs::create_dir_all(parent.join("memoria")).unwrap();
        fs::write(parent.join("memoria/user.txt"), "original user content\n").unwrap();
        let hook = failing_nth("sync-after-rename", 2);
        let faulty =
            FsSkillStore::with_faults(dir.path().to_path_buf(), "# Skill\n", "0.1.0", &hook);
        let err = install(&faulty, &text).unwrap_err();
        let message = failure_text(&err);
        assert!(
            message.contains("retained")
                && message.contains(TXN_FILE)
                && message.contains("memoria.backup"),
            "{message}"
        );
        assert!(parent.join("memoria/SKILL.md").exists());
        assert!(
            parent.join(TXN_FILE).exists(),
            "the transaction survives for recovery"
        );
        assert!(parent.join("memoria.backup/user.txt").exists());
        let plain = store(dir.path());
        let plan = plain
            .plan(&request(SkillOperation::Install, &text))
            .unwrap();
        assert!(plan.recovery_needed);
        plain
            .apply(&request(SkillOperation::Install, &text), &plan)
            .unwrap();
        assert_installed(&parent);
        let plan = plain
            .plan(&request(SkillOperation::Uninstall, &text))
            .unwrap();
        plain
            .apply(&request(SkillOperation::Uninstall, &text), &plan)
            .unwrap();
        assert_eq!(
            fs::read_to_string(parent.join("memoria/user.txt")).unwrap(),
            "original user content\n"
        );

        // Uninstall: the removal rename completes but its sync fails; the retry finishes and restores the backup.
        let dir = tempfile::tempdir().unwrap();
        let (parent, text) = parent_of(&dir);
        fs::create_dir_all(parent.join("memoria")).unwrap();
        fs::write(parent.join("memoria/user.txt"), "original user content\n").unwrap();
        install(&store(dir.path()), &text).unwrap();
        let hook = failing_nth("sync-after-rename", 1);
        let faulty =
            FsSkillStore::with_faults(dir.path().to_path_buf(), "# Skill\n", "0.1.0", &hook);
        let plan = faulty
            .plan(&request(SkillOperation::Uninstall, &text))
            .unwrap();
        let err = faulty
            .apply(&request(SkillOperation::Uninstall, &text), &plan)
            .unwrap_err();
        let message = failure_text(&err);
        assert!(
            message.contains("retained") && message.contains("memoria.backup"),
            "{message}"
        );
        assert!(parent.join(REMOVING_DIR).exists());
        assert!(parent.join(TXN_FILE).exists());
        assert!(parent.join("memoria.backup/user.txt").exists());
        let plain = store(dir.path());
        let plan = plain
            .plan(&request(SkillOperation::Uninstall, &text))
            .unwrap();
        assert!(plan.recovery_needed);
        plain
            .apply(&request(SkillOperation::Uninstall, &text), &plan)
            .unwrap();
        assert!(!parent.join(REMOVING_DIR).exists());
        assert!(!parent.join(TXN_FILE).exists());
        assert_eq!(
            fs::read_to_string(parent.join("memoria/user.txt")).unwrap(),
            "original user content\n"
        );
    }

    fn failing(step: &'static str) -> impl Fn(&str) -> Option<io::Error> {
        move |name| {
            if name == step {
                Some(io::Error::other(format!("injected {step}")))
            } else {
                None
            }
        }
    }

    #[test]
    fn injected_install_failures_restore_the_previous_package_and_recover() {
        for step in [
            "stage-write",
            "txn-write",
            "rename-backup",
            "rename-staging",
        ] {
            let dir = tempfile::tempdir().unwrap();
            let (parent, text) = parent_of(&dir);
            // Existing unmanaged package with user content.
            fs::create_dir_all(parent.join("memoria")).unwrap();
            fs::write(parent.join("memoria/notes.md"), "mine\n").unwrap();
            let hook = failing(step);
            let faulty =
                FsSkillStore::with_faults(dir.path().to_path_buf(), "# Skill\n", "0.1.0", &hook);
            let err = install(&faulty, &text).unwrap_err();
            assert!(failure_text(&err).contains("injected"), "{step}: {err:?}");
            assert_eq!(
                fs::read_to_string(parent.join("memoria/notes.md")).unwrap(),
                "mine\n",
                "{step}: previous package restored"
            );
            assert!(
                !parent.join(STAGING_DIR).exists(),
                "{step}: staging removed"
            );
            assert!(
                !parent.join(TXN_FILE).exists(),
                "{step}: transaction cleared"
            );
            assert!(
                !parent.join("memoria/SKILL.md").exists(),
                "{step}: no partial package"
            );
            // A plain retry succeeds.
            let plain = store(dir.path());
            install(&plain, &text).unwrap();
            assert_installed(&parent);
            assert_eq!(
                fs::read_to_string(parent.join("memoria.backup/notes.md")).unwrap(),
                "mine\n"
            );
        }
    }

    #[test]
    fn sync_and_transaction_removal_failures_leave_a_complete_recoverable_package() {
        for step in ["sync-parent", "txn-remove"] {
            let dir = tempfile::tempdir().unwrap();
            let (parent, text) = parent_of(&dir);
            let hook = failing(step);
            let faulty =
                FsSkillStore::with_faults(dir.path().to_path_buf(), "# Skill\n", "0.1.0", &hook);
            let err = install(&faulty, &text).unwrap_err();
            assert!(failure_text(&err).contains("injected"), "{step}: {err:?}");
            assert_eq!(
                fs::read_to_string(parent.join("memoria/SKILL.md")).unwrap(),
                "# Skill\n",
                "{step}: package complete"
            );
            let plain = store(dir.path());
            let plan = plain
                .plan(&request(SkillOperation::Install, &text))
                .unwrap();
            assert_eq!(plan.recovery_needed, step == "txn-remove", "{step}");
            plain
                .apply(&request(SkillOperation::Install, &text), &plan)
                .unwrap();
            assert_installed(&parent);
        }
    }

    #[test]
    fn injected_uninstall_failures_keep_state_consistent() {
        // rename-removing: nothing moved, transaction cleared, package intact.
        let dir = tempfile::tempdir().unwrap();
        let (parent, text) = parent_of(&dir);
        install(&store(dir.path()), &text).unwrap();
        let hook = failing("rename-removing");
        let faulty =
            FsSkillStore::with_faults(dir.path().to_path_buf(), "# Skill\n", "0.1.0", &hook);
        let plan = faulty
            .plan(&request(SkillOperation::Uninstall, &text))
            .unwrap();
        assert!(
            faulty
                .apply(&request(SkillOperation::Uninstall, &text), &plan)
                .is_err()
        );
        assert_installed(&parent);
        // remove-removing: the package sits in memoria.removing with its transaction; recovery finishes.
        let hook = failing("remove-removing");
        let faulty =
            FsSkillStore::with_faults(dir.path().to_path_buf(), "# Skill\n", "0.1.0", &hook);
        let plan = faulty
            .plan(&request(SkillOperation::Uninstall, &text))
            .unwrap();
        assert!(
            faulty
                .apply(&request(SkillOperation::Uninstall, &text), &plan)
                .is_err()
        );
        assert!(parent.join(REMOVING_DIR).exists());
        assert!(parent.join(TXN_FILE).exists());
        let plain = store(dir.path());
        let recovery = plain
            .plan(&request(SkillOperation::Uninstall, &text))
            .unwrap();
        assert!(
            recovery.recovery_needed,
            "the interrupted removal is actionable"
        );
        install(&plain, &text).unwrap();
        assert_installed(&parent);
        // restore-backup failure after removal leaves the transaction for recovery, backup intact.
        fs::create_dir_all(parent.join("memoria.backup")).unwrap();
        fs::write(parent.join("memoria.backup/notes.md"), "mine\n").unwrap();
        let record_path = parent.join("memoria").join(RECORD_FILE);
        let record = fs::read_to_string(&record_path).unwrap().replace(
            "\"backup\": null",
            &format!(
                "\"backup\": \"{}\"",
                parent.join("memoria.backup").display()
            ),
        );
        fs::write(&record_path, record).unwrap();
        let hook = failing("restore-backup");
        let faulty =
            FsSkillStore::with_faults(dir.path().to_path_buf(), "# Skill\n", "0.1.0", &hook);
        let plan = faulty
            .plan(&request(SkillOperation::Uninstall, &text))
            .unwrap();
        assert!(plan.backup.is_some());
        assert!(
            faulty
                .apply(&request(SkillOperation::Uninstall, &text), &plan)
                .is_err()
        );
        assert!(parent.join(TXN_FILE).exists());
        assert_eq!(
            fs::read_to_string(parent.join("memoria.backup/notes.md")).unwrap(),
            "mine\n"
        );
        let plan = plain
            .plan(&request(SkillOperation::Install, &text))
            .unwrap();
        assert!(plan.recovery_needed);
        plain
            .apply(&request(SkillOperation::Install, &text), &plan)
            .unwrap();
        assert_installed(&parent);
    }
}
