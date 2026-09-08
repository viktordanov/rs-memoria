//! Reversible, project-level native `Stop` hooks for Codex and Claude.
//!
//! Memoria owns exactly one group in one configuration file, plus one
//! ownership record beside it. It never adopts a user entry, never migrates
//! hooks between representations, never sets trust flags, and never disables
//! another hook. Uninstall removes only the unchanged owned group and the
//! containers Memoria created.

use std::fs;
use std::path::{Path, PathBuf};

use memoria_application::ports::{
    AdapterError, AgentTarget, FileKind, HookFailure, HookPlan, HookStore,
};

use crate::client_probe::{self, ClientProbe};
use serde_json::{Map, Value};

use crate::fs::{durable_replace, kind_of};
use crate::hook_config::{
    self, EVENT, MAX_CONFIGURATION_BYTES, MAX_RECORD_BYTES, PROTOCOL, entry_hash, hook_command,
    owned_entry, parse_json, resembles_owned, write_json,
};
use crate::json::{self, Json, Limits, ObjectReader};

/// Codex hook configuration inside the project.
pub const CODEX_JSON: &str = ".codex/hooks.json";
/// Codex inline hook configuration.
pub const CODEX_TOML: &str = ".codex/config.toml";
/// Codex ownership record.
pub const CODEX_RECORD: &str = ".codex/memoria-hook.json";
/// Claude local settings at the main checkout.
pub const CLAUDE_JSON: &str = ".claude/settings.local.json";
/// Claude ownership record.
pub const CLAUDE_RECORD: &str = ".claude/memoria-hook.json";

fn io(operation: &str, path: &Path, err: std::io::Error) -> HookFailure {
    HookFailure::Io(AdapterError::new(
        operation,
        Some(path.display().to_string()),
        err.to_string(),
    ))
}

fn conflict(code: &'static str, message: impl Into<String>) -> HookFailure {
    HookFailure::Conflict {
        code,
        message: message.into(),
    }
}

fn unsupported(code: &'static str, message: impl Into<String>) -> HookFailure {
    HookFailure::Unsupported {
        code,
        message: message.into(),
    }
}

/// Which containers Memoria created, so uninstall removes only those.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
struct Created {
    file: bool,
    hooks: bool,
    stop: bool,
}

/// The ownership record beside the client configuration.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Ownership {
    target: String,
    /// The configuration path, relative to the configuration root.
    configuration: String,
    entry: Value,
    entry_hash: String,
    created: Created,
}

fn ownership_to_bytes(record: &Ownership) -> Result<Vec<u8>, HookFailure> {
    let mut created = Map::new();
    created.insert("file".into(), Value::Bool(record.created.file));
    created.insert("hooks".into(), Value::Bool(record.created.hooks));
    created.insert("stop".into(), Value::Bool(record.created.stop));
    let mut map = Map::new();
    map.insert("version".into(), Value::from(1));
    map.insert("target".into(), Value::String(record.target.clone()));
    map.insert("event".into(), Value::String(EVENT.to_string()));
    map.insert("protocol".into(), Value::from(PROTOCOL));
    map.insert(
        "configuration".into(),
        Value::String(record.configuration.clone()),
    );
    map.insert("entry".into(), record.entry.clone());
    map.insert(
        "entry_hash".into(),
        Value::String(record.entry_hash.clone()),
    );
    map.insert("created_containers".into(), Value::Object(created));
    write_json(&Value::Object(map)).map_err(|e| conflict("hook_configuration_invalid", e.message))
}

fn ownership_from_bytes(bytes: &[u8]) -> Result<Ownership, HookFailure> {
    if bytes.len() as u64 > MAX_RECORD_BYTES {
        return Err(conflict(
            "hook_conflict",
            format!(
                "the ownership record is {} bytes, above the limit of {MAX_RECORD_BYTES}",
                bytes.len()
            ),
        ));
    }
    let value = parse_json(bytes)
        .map_err(|e| conflict("hook_conflict", format!("ownership record: {e}")))?;
    let bad = |what: &str| conflict("hook_conflict", format!("ownership record: {what}"));
    let object = value.as_object().ok_or_else(|| bad("not an object"))?;
    if object.get("version").and_then(Value::as_u64) != Some(1) {
        return Err(bad("unsupported record version"));
    }
    if object.get("event").and_then(Value::as_str) != Some(EVENT) {
        return Err(bad("unexpected event"));
    }
    if object.get("protocol").and_then(Value::as_u64) != Some(u64::from(PROTOCOL)) {
        return Err(bad("unsupported protocol"));
    }
    let target = object
        .get("target")
        .and_then(Value::as_str)
        .ok_or_else(|| bad("missing target"))?
        .to_string();
    let configuration = object
        .get("configuration")
        .and_then(Value::as_str)
        .ok_or_else(|| bad("missing configuration"))?
        .to_string();
    if configuration.starts_with('/') || configuration.contains("..") {
        return Err(bad("the configuration path escapes the configuration root"));
    }
    let entry = object
        .get("entry")
        .cloned()
        .ok_or_else(|| bad("missing entry"))?;
    let recorded_hash = object
        .get("entry_hash")
        .and_then(Value::as_str)
        .ok_or_else(|| bad("missing entry_hash"))?
        .to_string();
    if entry_hash(&entry) != recorded_hash {
        return Err(bad("entry_hash does not match the recorded entry"));
    }
    let containers = object
        .get("created_containers")
        .and_then(Value::as_object)
        .ok_or_else(|| bad("missing created_containers"))?;
    let flag = |key: &str| {
        containers
            .get(key)
            .and_then(Value::as_bool)
            .unwrap_or(false)
    };
    Ok(Ownership {
        target,
        configuration,
        entry,
        entry_hash: recorded_hash,
        created: Created {
            file: flag("file"),
            hooks: flag("hooks"),
            stop: flag("stop"),
        },
    })
}

/// Which representation holds the project's Codex hooks.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Representation {
    Json,
    InlineToml,
}

/// The resolved destination for one target.
struct Destination {
    /// The configuration root the record's relative paths resolve against.
    root: PathBuf,
    /// The Git metadata directory that owns this destination's private
    /// paths. A linked worktree has its own.
    git_dir: PathBuf,
    target: AgentTarget,
    configuration: PathBuf,
    relative: String,
    record: PathBuf,
    representation: Representation,
}

pub struct FsHookStore {
    worktree: PathBuf,
    /// The main checkout of this repository family, for Claude.
    main_worktree: PathBuf,
    /// Absolute path of the installing executable.
    executable: PathBuf,
    /// The per-worktree Git metadata directory. Private paths live under it
    /// and may not be substituted.
    git_dir: PathBuf,
    /// Lazy, target-specific version probing. Only an install probes, and
    /// only the target it installs for.
    clients: Box<dyn ClientProbe>,
}

impl FsHookStore {
    pub fn new(
        worktree: PathBuf,
        main_worktree: PathBuf,
        executable: PathBuf,
        git_dir: PathBuf,
        clients: Box<dyn ClientProbe>,
    ) -> FsHookStore {
        FsHookStore {
            worktree,
            main_worktree,
            executable,
            git_dir,
            clients,
        }
    }

    /// Refuse a path whose ancestor is a symlink or whose destination is a
    /// special file, before any mutation.
    fn require_plain(&self, path: &Path) -> Result<bool, HookFailure> {
        let mut current = path.parent();
        while let Some(dir) = current {
            if let Ok(meta) = fs::symlink_metadata(dir)
                && meta.file_type().is_symlink()
            {
                return Err(conflict(
                    "hook_configuration_invalid",
                    format!(
                        "{} is a symlink; Memoria refuses to write through it",
                        dir.display()
                    ),
                ));
            }
            if dir == self.worktree || dir == self.main_worktree || dir.parent().is_none() {
                break;
            }
            current = dir.parent();
        }
        match kind_of(path).map_err(|e| io("stat", path, e))? {
            FileKind::Missing => Ok(false),
            FileKind::Regular => Ok(true),
            other => Err(conflict(
                "hook_configuration_invalid",
                format!("{} is a {other:?}, not a regular file", path.display()),
            )),
        }
    }

    fn read_bounded(&self, path: &Path) -> Result<Vec<u8>, HookFailure> {
        let metadata = fs::metadata(path).map_err(|e| io("stat", path, e))?;
        if metadata.len() > MAX_CONFIGURATION_BYTES {
            return Err(conflict(
                "hook_configuration_too_large",
                format!(
                    "{} is {} bytes, above the limit of {MAX_CONFIGURATION_BYTES}",
                    path.display(),
                    metadata.len()
                ),
            ));
        }
        fs::read(path).map_err(|e| io("read", path, e))
    }

    /// Whether the inline Codex hook table already holds groups.
    fn inline_hooks_present(&self, path: &Path) -> Result<bool, HookFailure> {
        if !self.require_plain(path)? {
            return Ok(false);
        }
        let bytes = self.read_bounded(path)?;
        let text = std::str::from_utf8(&bytes).map_err(|_| {
            conflict(
                "hook_configuration_invalid",
                format!("{} is not valid UTF-8", path.display()),
            )
        })?;
        let document: toml_edit::DocumentMut = text.parse().map_err(|err| {
            conflict(
                "hook_configuration_invalid",
                format!("{} is not valid TOML: {err}", path.display()),
            )
        })?;
        Ok(document
            .get("hooks")
            .and_then(|hooks| hooks.as_table_like())
            .is_some_and(|table| table.iter().any(|(_, item)| !item.is_none())))
    }

    fn destination(&self, target: AgentTarget) -> Result<Destination, HookFailure> {
        match target {
            AgentTarget::Codex => {
                let json = self.worktree.join(CODEX_JSON);
                let toml = self.worktree.join(CODEX_TOML);
                let inline = self.inline_hooks_present(&toml)?;
                let json_has_hooks = if self.require_plain(&json)? {
                    let value = parse_json(&self.read_bounded(&json)?).map_err(|e| {
                        conflict(
                            "hook_configuration_invalid",
                            format!("{}: {e}", json.display()),
                        )
                    })?;
                    hook_config::stop_groups(&value).is_some_and(|groups| !groups.is_empty())
                        || value
                            .get("hooks")
                            .and_then(Value::as_object)
                            .is_some_and(|hooks| !hooks.is_empty())
                } else {
                    false
                };
                if inline && json_has_hooks {
                    return Err(conflict(
                        "hook_configuration_ambiguous",
                        format!(
                            "both {} and the inline [hooks] table in {} contain hooks. Memoria does not migrate hooks between representations or create a second active one. Keep one representation, then reinstall.",
                            CODEX_JSON, CODEX_TOML
                        ),
                    ));
                }
                let (configuration, relative, representation) = if inline {
                    (toml, CODEX_TOML.to_string(), Representation::InlineToml)
                } else {
                    (json, CODEX_JSON.to_string(), Representation::Json)
                };
                Ok(Destination {
                    root: self.worktree.clone(),
                    // Codex hooks are per worktree, so the private paths
                    // belong to this worktree's own metadata directory.
                    git_dir: self.git_dir.clone(),
                    target,
                    configuration,
                    relative,
                    record: self.worktree.join(CODEX_RECORD),
                    representation: Representation::Json.min_with(representation),
                })
            }
            AgentTarget::Claude => {
                // Current Claude documentation places local settings at the
                // main checkout for linked worktrees.
                let root = self.main_worktree.clone();
                if root.parent().is_none() {
                    return Err(unsupported(
                        "hook_location_unsupported",
                        "the resolved Claude settings location has no parent directory",
                    ));
                }
                Ok(Destination {
                    configuration: root.join(CLAUDE_JSON),
                    relative: CLAUDE_JSON.to_string(),
                    record: root.join(CLAUDE_RECORD),
                    representation: Representation::Json,
                    // Claude settings live at the main checkout, and a
                    // mutation is refused from another worktree, so the
                    // metadata directory of this worktree owns the private
                    // paths for the mutation that is actually permitted.
                    git_dir: self.git_dir.clone(),
                    target,
                    root,
                })
            }
        }
    }

    /// Refuse a Claude mutation from a worktree that is not the destination.
    fn require_mutable(
        &self,
        target: AgentTarget,
        destination: &Destination,
    ) -> Result<(), HookFailure> {
        if target == AgentTarget::Claude && destination.root != self.worktree {
            return Err(conflict(
                "hook_configuration_outside_worktree",
                format!(
                    "Claude local settings for this repository live at {}. Memoria does not silently write to another checkout. Run the command again with --root {}.",
                    destination.configuration.display(),
                    destination.root.display()
                ),
            ));
        }
        Ok(())
    }

    /// Probe the selected client and enforce the frozen version floor.
    ///
    /// This runs only for an installation, and only for the target being
    /// installed. Ordinary commands, hook status, and hook uninstall never
    /// execute a client executable.
    fn require_client(&self, target: AgentTarget) -> Result<(), HookFailure> {
        let floor = client_probe::floor(target);
        match self.clients.probe(target) {
            client_probe::Probe::Supported(_) => Ok(()),
            client_probe::Probe::TooOld(found) => Err(unsupported(
                "hook_client_unsupported",
                format!(
                    "{} reports version {found}, below the supported floor {floor}. Upgrade the client, then run `memoria agent hook install --target {}` again.",
                    target.as_str(),
                    target.as_str()
                ),
            )),
            client_probe::Probe::Unavailable(reason) => Err(unsupported(
                "hook_client_unsupported",
                format!(
                    "no supported {} client was found ({reason}). This release requires at least {floor}. Install or upgrade the client, then run `memoria agent hook install --target {}` again.",
                    target.as_str(),
                    target.as_str()
                ),
            )),
        }
    }

    fn load_record(&self, destination: &Destination) -> Result<Option<Ownership>, HookFailure> {
        match self.read_record_bytes(destination)? {
            None => Ok(None),
            Some(bytes) => ownership_from_bytes(&bytes).map(Some),
        }
    }

    /// The ownership record's current bytes, or `None` when there is none.
    fn read_record_bytes(&self, destination: &Destination) -> Result<Option<Vec<u8>>, HookFailure> {
        if !self.require_plain(&destination.record)? {
            return Ok(None);
        }
        Ok(Some(self.read_bounded(&destination.record)?))
    }

    /// The identity of the ownership record as it is now: the digest of its
    /// bytes, or `None` for absence, which is an identity too.
    fn record_digest(&self, destination: &Destination) -> Result<Option<String>, HookFailure> {
        Ok(self
            .read_record_bytes(destination)?
            .as_deref()
            .map(bytes_digest))
    }

    /// The configuration's current bytes, or `None` when it does not exist.
    ///
    /// Every caller that both parses and hashes the configuration works from
    /// one read. Hashing a second read could describe different bytes from
    /// the ones transformed, and a compare-and-swap against that digest
    /// would authorize a replacement derived from content nobody examined.
    fn read_configuration_bytes(
        &self,
        destination: &Destination,
    ) -> Result<Option<Vec<u8>>, HookFailure> {
        if !self.require_plain(&destination.configuration)? {
            return Ok(None);
        }
        Ok(Some(self.read_bounded(&destination.configuration)?))
    }

    /// Parse configuration bytes into the shape ownership hashing uses.
    fn parse_configuration(
        &self,
        destination: &Destination,
        bytes: &[u8],
    ) -> Result<Value, HookFailure> {
        match destination.representation {
            Representation::Json => parse_json(bytes).map_err(|e| {
                conflict(
                    "hook_configuration_invalid",
                    format!("{}: {e}", destination.configuration.display()),
                )
            }),
            Representation::InlineToml => {
                // Unrelated values and comments survive the edit because the
                // document itself is edited. For inspection, the `hooks`
                // table is projected into the same JSON shape the ownership
                // record stores, so one entry hash compares both
                // representations.
                let document = self.parse_inline(destination, bytes)?;
                let mut root = Map::new();
                if let Some(hooks) = document.get("hooks") {
                    root.insert("hooks".into(), toml_to_json(hooks));
                }
                Ok(Value::Object(root))
            }
        }
    }

    fn build_plan(
        &self,
        target: AgentTarget,
        operation: &'static str,
    ) -> Result<HookPlan, HookFailure> {
        let destination = self.destination(target)?;
        let command = hook_command(
            &self.executable.display().to_string(),
            target.as_str(),
            &destination.root.display().to_string(),
        );
        let desired = owned_entry(&command);
        let record = self.load_record(&destination)?;
        let bytes = self.read_configuration_bytes(&destination)?;
        let file_exists = bytes.is_some();
        let configuration = match &bytes {
            Some(bytes) => self.parse_configuration(&destination, bytes)?,
            None => Value::Object(Map::new()),
        };
        let expected_digest = bytes.as_deref().map(bytes_digest);
        let groups = hook_config::stop_groups(&configuration)
            .cloned()
            .unwrap_or_default();

        let mut state = "absent";
        if let Some(record) = &record {
            if record.target != target.as_str() || record.configuration != destination.relative {
                state = "conflict";
            } else {
                let owned: Vec<&Value> = groups
                    .iter()
                    .filter(|group| entry_hash(group) == record.entry_hash)
                    .collect();
                match owned.len() {
                    // The record exists but its entry is gone or edited.
                    // Either way this is not Memoria's to change silently.
                    0 => state = "modified",
                    // Ownership is the stored exact group, not agreement
                    // with the command this executable would install now.
                    // A relocated executable still owns its unchanged
                    // entry, so removal and reinstallation stay available.
                    1 => {
                        state = if record.entry_hash == entry_hash(&desired) {
                            "installed"
                        } else {
                            "relocated"
                        };
                    }
                    _ => state = "conflict",
                }
            }
        } else if groups.iter().any(resembles_owned) {
            // A user entry that looks like Memoria's is never adopted.
            state = "unmanaged";
        } else if destination.representation == Representation::InlineToml {
            state = "absent";
        }

        // An interrupted operation must reach apply, where recovery runs
        // under the integration lock. Reporting a no-op here would leave the
        // transaction record behind forever.
        let recovery_needed = transaction_path(&destination).exists();
        let no_change = match operation {
            "install" => state == "installed" && !recovery_needed,
            "uninstall" => state == "absent" && !recovery_needed,
            _ => true,
        };
        let mut writes = Vec::new();
        let mut removals = Vec::new();
        if operation == "install" && !no_change {
            writes.push(destination.configuration.display().to_string());
            writes.push(destination.record.display().to_string());
        }
        if operation == "uninstall" && !no_change {
            removals.push(destination.record.display().to_string());
            writes.push(destination.configuration.display().to_string());
        }
        let _ = file_exists;
        Ok(HookPlan {
            target,
            configuration: destination.configuration.display().to_string(),
            record: destination.record.display().to_string(),
            state: state.to_string(),
            writes,
            removals,
            no_change,
            recovery_needed,
            activation: "requires-client-review".to_string(),
            command,
            expected_digest,
        })
    }
}

impl Representation {
    /// Keep the representation the destination selected.
    fn min_with(self, other: Representation) -> Representation {
        let _ = self;
        other
    }
}

/// Project a TOML item into the JSON shape the ownership record stores.
///
/// Only the hook table is projected. The conversion keeps object keys, array
/// order, and scalar values, so an owned group hashes identically whether it
/// lives in `hooks.json` or in an inline `[[hooks.Stop]]` table.
fn toml_to_json(item: &toml_edit::Item) -> Value {
    match item {
        toml_edit::Item::None => Value::Null,
        toml_edit::Item::Value(value) => toml_value_to_json(value),
        toml_edit::Item::Table(table) => Value::Object(
            table
                .iter()
                .map(|(key, item)| (key.to_string(), toml_to_json(item)))
                .collect(),
        ),
        toml_edit::Item::ArrayOfTables(tables) => Value::Array(
            tables
                .iter()
                .map(|table| {
                    Value::Object(
                        table
                            .iter()
                            .map(|(key, item)| (key.to_string(), toml_to_json(item)))
                            .collect(),
                    )
                })
                .collect(),
        ),
    }
}

fn toml_value_to_json(value: &toml_edit::Value) -> Value {
    match value {
        toml_edit::Value::String(text) => Value::String(text.value().clone()),
        toml_edit::Value::Integer(number) => Value::from(*number.value()),
        toml_edit::Value::Float(number) => Value::from(*number.value()),
        toml_edit::Value::Boolean(flag) => Value::Bool(*flag.value()),
        toml_edit::Value::Datetime(stamp) => Value::String(stamp.value().to_string()),
        toml_edit::Value::Array(array) => {
            Value::Array(array.iter().map(toml_value_to_json).collect())
        }
        toml_edit::Value::InlineTable(table) => Value::Object(
            table
                .iter()
                .map(|(key, value)| (key.to_string(), toml_value_to_json(value)))
                .collect(),
        ),
    }
}

/// One durable intent, written before a shared configuration changes.
///
/// It records both identities of the transition: the bytes the operation
/// requires before its write, and the bytes and ownership it intends to
/// leave behind. Recovery compares the file it finds with both, so it can
/// tell an interrupted operation from an outside edit.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Intent {
    operation: String,
    target: String,
    configuration: String,
    /// The configuration digest before the change. `None` when the
    /// configuration file did not exist.
    expected_digest: Option<String>,
    /// The configuration digest the operation writes. `None` when the
    /// operation removes a file that Memoria created.
    desired_digest: Option<String>,
    entry_hash: String,
    /// The digest of the ownership record as the operation found it, or
    /// `None` when there was no record. Recovery accepts only this identity
    /// or the desired one, so an ownership record edited during the
    /// interruption is a conflict rather than something to repair.
    expected_record: Option<String>,
    /// The exact ownership record the operation intends to leave, with its
    /// container provenance. `None` for an uninstall, which leaves none.
    desired_record: Option<Ownership>,
    phase: String,
}

/// The transaction record format this release writes and accepts.
const INTENT_VERSION: u64 = 3;

/// A stable digest of raw configuration bytes, for compare and swap.
fn bytes_digest(bytes: &[u8]) -> String {
    format!("{:016x}", crate::hash::xxh3_64(bytes))
}

/// The worktree-private intent record for one target.
///
/// The path comes from the resolved Git metadata directory, not from a
/// literal `.git` component: in a linked worktree `.git` is a file, and a
/// hardcoded path fails. The target discriminator keeps two installations
/// in one worktree independent.
fn transaction_path(destination: &Destination) -> PathBuf {
    destination.git_dir.join("memoria").join(format!(
        "hook-{}.transaction.json",
        destination.target.as_str()
    ))
}

/// The worktree-private integration lock for one target.
fn integration_lock_path(destination: &Destination) -> PathBuf {
    destination
        .git_dir
        .join("memoria")
        .join(format!("hook-{}.lock", destination.target.as_str()))
}

/// Validate the container shapes that an installation would use.
///
/// An unsupported shape is a configuration error, not authorization to
/// replace user data. Even an invalid client configuration is the user's
/// file: Memoria reports it and writes nothing.
fn validate_containers(configuration: &Value) -> Result<(), HookFailure> {
    let unsupported = |what: &str, found: &Value| {
        conflict(
            "hook_configuration_invalid",
            format!(
                "{what} is {}, which Memoria cannot extend. Nothing was changed. Correct the client configuration, then install again.",
                describe(found)
            ),
        )
    };
    if configuration.is_null() {
        return Ok(());
    }
    let Some(object) = configuration.as_object() else {
        return Err(unsupported("the configuration root", configuration));
    };
    let Some(hooks) = object.get("hooks") else {
        return Ok(());
    };
    let Some(hooks) = hooks.as_object() else {
        return Err(unsupported("the `hooks` value", hooks));
    };
    match hooks.get(EVENT) {
        None => Ok(()),
        Some(stop) if stop.is_array() => Ok(()),
        Some(stop) => Err(unsupported("the `hooks.Stop` value", stop)),
    }
}

/// A short name for a JSON value's type, for diagnostics.
fn describe(value: &Value) -> &'static str {
    match value {
        Value::Null => "null",
        Value::Bool(_) => "a boolean",
        Value::Number(_) => "a number",
        Value::String(_) => "a string",
        Value::Array(_) => "an array",
        Value::Object(_) => "an object",
    }
}

/// Insert the owned group, recording which containers Memoria created.
///
/// Every container shape was validated first, so this creates only absent
/// containers and never replaces an existing value.
fn insert_group(configuration: &mut Value, entry: Value) -> Result<Created, HookFailure> {
    validate_containers(configuration)?;
    let mut created = Created::default();
    if configuration.is_null() {
        *configuration = Value::Object(Map::new());
    }
    let object = configuration
        .as_object_mut()
        .expect("validated as an object");
    if !object.contains_key("hooks") {
        object.insert("hooks".into(), Value::Object(Map::new()));
        created.hooks = true;
    }
    let hooks = object
        .get_mut("hooks")
        .expect("hooks")
        .as_object_mut()
        .expect("validated as an object");
    if !hooks.contains_key(EVENT) {
        hooks.insert(EVENT.into(), Value::Array(Vec::new()));
        created.stop = true;
    }
    hooks
        .get_mut(EVENT)
        .expect("stop")
        .as_array_mut()
        .expect("validated as an array")
        .push(entry);
    Ok(created)
}

/// Remove the owned group and the containers Memoria created, once they are
/// empty. Every unrelated handler, event, and key survives.
fn remove_group(configuration: &mut Value, hash: &str, created: Created) {
    let Some(object) = configuration.as_object_mut() else {
        return;
    };
    let Some(hooks) = object.get_mut("hooks").and_then(Value::as_object_mut) else {
        return;
    };
    if let Some(stop) = hooks.get_mut(EVENT).and_then(Value::as_array_mut) {
        stop.retain(|group| entry_hash(group) != hash);
        if stop.is_empty() && created.stop {
            hooks.remove(EVENT);
        }
    }
    if hooks.is_empty() && created.hooks {
        object.remove("hooks");
    }
}

/// Refuse to apply a plan that no longer describes the destination.
///
/// Planning and application are separate steps with a lock between them.
/// A configuration edited in that window must produce a conflict, not an
/// overwrite of somebody else's change.
fn agrees_with_plan(plan: &HookPlan, fresh: &HookPlan) -> Result<(), HookFailure> {
    // Recovery deliberately changes the destination, so a plan that asked
    // for recovery is compared against nothing: the fresh plan is
    // authoritative from there.
    if plan.recovery_needed {
        return Ok(());
    }
    if plan.expected_digest == fresh.expected_digest
        && plan.command == fresh.command
        && plan.state == fresh.state
    {
        return Ok(());
    }
    Err(conflict(
        "hook_conflict",
        format!(
            "{} changed after the plan was made. Nothing was changed. Run the command again.",
            fresh.configuration
        ),
    ))
}

/// The install eligibility gate, shared by planning and application.
fn gate_install(plan: &HookPlan) -> Result<(), HookFailure> {
    match plan.state.as_str() {
        "unmanaged" => Err(conflict(
            "hook_unmanaged",
            format!(
                "{} already contains a Stop hook that runs Memoria, but no ownership record exists. Memoria never adopts a user entry. Remove that entry, or leave it and skip installation.",
                plan.configuration
            ),
        )),
        "modified" | "conflict" => Err(conflict(
            "hook_conflict",
            format!(
                "the owned Stop hook in {} was edited, duplicated, or its record is inconsistent. Nothing was changed. Restore the entry or remove the record at {}, then reinstall.",
                plan.configuration, plan.record
            ),
        )),
        _ => Ok(()),
    }
}

/// The uninstall eligibility gate, shared by planning and application.
fn gate_uninstall(plan: &HookPlan) -> Result<(), HookFailure> {
    match plan.state.as_str() {
        "modified" | "conflict" => Err(conflict(
            "hook_conflict",
            format!(
                "the owned Stop hook in {} no longer matches its ownership record. Every file is preserved. Restore the entry or remove {} by hand.",
                plan.configuration, plan.record
            ),
        )),
        _ => Ok(()),
    }
}

impl HookStore for FsHookStore {
    fn plan_install(&self, target: AgentTarget) -> Result<HookPlan, HookFailure> {
        self.require_client(target)?;
        let destination = self.destination(target)?;
        self.require_mutable(target, &destination)?;
        let plan = self.build_plan(target, "install")?;
        // An interrupted operation is settled under the lock in apply, not
        // judged here: the state this plan sees is the interrupted one.
        // Planning stays read-only, so status and a dry run change nothing.
        if !plan.recovery_needed {
            gate_install(&plan)?;
        }
        Ok(plan)
    }

    fn status(&self, target: AgentTarget) -> Result<HookPlan, HookFailure> {
        // Status works without a valid project and never writes. It can
        // report the effective Claude location from a linked worktree.
        self.build_plan(target, "status")
    }

    fn plan_uninstall(&self, target: AgentTarget) -> Result<HookPlan, HookFailure> {
        let destination = self.destination(target)?;
        self.require_mutable(target, &destination)?;
        let plan = self.build_plan(target, "uninstall")?;
        if !plan.recovery_needed {
            gate_uninstall(&plan)?;
        }
        Ok(plan)
    }

    fn apply_install(&self, plan: &HookPlan) -> Result<(), HookFailure> {
        let destination = self.destination(plan.target)?;
        self.require_mutable(plan.target, &destination)?;
        // One integration lock for each target serializes mutations, and
        // recovery of an interrupted operation runs under it.
        let _guard = self.integration_lock(&destination)?;
        self.recover(&destination)?;
        // Re-plan under the lock, without probing the client again.
        let fresh = self.build_plan(plan.target, "install")?;
        gate_install(&fresh)?;
        if fresh.no_change {
            return Ok(());
        }
        agrees_with_plan(plan, &fresh)?;
        if destination.representation == Representation::InlineToml {
            return self.apply_inline_install(&destination, plan);
        }
        let current = self.read_configuration_bytes(&destination)?;
        let existed = current.is_some();
        // The digest describes exactly the bytes that are parsed here.
        let before = current.as_deref().map(bytes_digest);
        let mut configuration = match &current {
            Some(bytes) => self.parse_configuration(&destination, bytes)?,
            None => Value::Object(Map::new()),
        };
        let entry = owned_entry(&plan.command);
        let hash = entry_hash(&entry);
        if hook_config::stop_groups(&configuration)
            .is_some_and(|groups| groups.iter().any(|group| entry_hash(group) == hash))
        {
            return Ok(());
        }
        // A relocated installation replaces its own recorded group, so the
        // documented reinstall path works after the executable moves.
        let previous_bytes = self.read_record_bytes(&destination)?;
        let expected_record = previous_bytes.as_deref().map(bytes_digest);
        let previous = match &previous_bytes {
            Some(bytes) => Some(ownership_from_bytes(bytes)?),
            None => None,
        };
        if let Some(previous) = &previous
            && fresh.state == "relocated"
        {
            remove_group(&mut configuration, &previous.entry_hash, previous.created);
        }
        let mut created = insert_group(&mut configuration, entry.clone())?;
        created.file = !existed;
        if let Some(previous) = &previous {
            // Containers the earlier installation created stay owned by it.
            created.file |= previous.created.file;
            created.hooks |= previous.created.hooks;
            created.stop |= previous.created.stop;
        }
        let record = Ownership {
            target: plan.target.as_str().to_string(),
            configuration: destination.relative.clone(),
            entry,
            entry_hash: hash.clone(),
            created,
        };
        let bytes = write_json(&configuration)
            .map_err(|e| conflict("hook_configuration_invalid", e.message))?;
        let intent = Intent {
            operation: "install".into(),
            target: plan.target.as_str().to_string(),
            configuration: destination.relative.clone(),
            expected_digest: before.clone(),
            desired_digest: Some(bytes_digest(&bytes)),
            expected_record: expected_record.clone(),
            entry_hash: hash,
            desired_record: Some(record.clone()),
            phase: "started".into(),
        };
        self.write_intent(&destination, &intent)?;
        // Compare and swap: the configuration must still be what planning
        // saw. An outside edit in this window is a conflict, not a silent
        // overwrite.
        self.compare_and_swap(&destination, before.as_deref(), &bytes)?;
        self.write_intent(
            &destination,
            &Intent {
                phase: "configuration-written".into(),
                ..intent
            },
        )?;
        // The ownership record is compared the same way before replacement.
        self.compare_and_write_record(
            &destination,
            expected_record.as_deref(),
            &ownership_to_bytes(&record)?,
        )?;
        self.clear_intent(&destination)
    }

    fn apply_uninstall(&self, plan: &HookPlan) -> Result<(), HookFailure> {
        let destination = self.destination(plan.target)?;
        self.require_mutable(plan.target, &destination)?;
        let _guard = self.integration_lock(&destination)?;
        self.recover(&destination)?;
        let fresh = self.build_plan(plan.target, "uninstall")?;
        gate_uninstall(&fresh)?;
        agrees_with_plan(plan, &fresh)?;
        let Some(record_bytes) = self.read_record_bytes(&destination)? else {
            // An absent installation creates no directory and no record.
            return Ok(());
        };
        let expected_record = Some(bytes_digest(&record_bytes));
        let record = ownership_from_bytes(&record_bytes)?;
        if destination.representation == Representation::InlineToml {
            return self.apply_inline_uninstall(&destination, &record, expected_record.as_deref());
        }
        let current = self.read_configuration_bytes(&destination)?;
        if let Some(current) = current {
            let before = Some(bytes_digest(&current));
            let mut configuration = self.parse_configuration(&destination, &current)?;
            remove_group(&mut configuration, &record.entry_hash, record.created);
            let empty = configuration
                .as_object()
                .is_some_and(serde_json::Map::is_empty);
            let removes_file = empty && record.created.file;
            let bytes = if removes_file {
                None
            } else {
                Some(
                    write_json(&configuration)
                        .map_err(|e| conflict("hook_configuration_invalid", e.message))?,
                )
            };
            let intent = Intent {
                operation: "uninstall".into(),
                target: plan.target.as_str().to_string(),
                configuration: destination.relative.clone(),
                expected_digest: before.clone(),
                desired_digest: bytes.as_deref().map(bytes_digest),
                expected_record: expected_record.clone(),
                entry_hash: record.entry_hash.clone(),
                desired_record: None,
                phase: "started".into(),
            };
            self.write_intent(&destination, &intent)?;
            match &bytes {
                // Memoria created the file and nothing else uses it.
                None => self.compare_and_remove(&destination, before.as_deref())?,
                Some(bytes) => self.compare_and_swap(&destination, before.as_deref(), bytes)?,
            }
            self.write_intent(
                &destination,
                &Intent {
                    phase: "configuration-written".into(),
                    ..intent
                },
            )?;
        }
        self.compare_and_remove_record(&destination, expected_record.as_deref())?;
        self.clear_intent(&destination)
    }
}

impl FsHookStore {
    fn write_file(&self, path: &Path, bytes: &[u8]) -> Result<(), HookFailure> {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).map_err(|e| io("create_dir", parent, e))?;
        }
        let mode = {
            use std::os::unix::fs::PermissionsExt;
            fs::metadata(path)
                .ok()
                .map(|m| m.permissions().mode())
                .or(Some(0o600))
        };
        durable_replace(path, bytes, mode).map_err(HookFailure::Io)
    }

    /// Acquire the worktree-private integration lock for this target.
    ///
    /// Hook mutations are serialized per target, so two commands cannot
    /// interleave their configuration and record writes. The lock lives
    /// under Git metadata, never in the worktree.
    fn integration_lock(
        &self,
        destination: &Destination,
    ) -> Result<Box<dyn memoria_application::ports::WriteGuard>, HookFailure> {
        let path = integration_lock_path(destination);
        match crate::fs::lock_file_within(&path, &destination.git_dir) {
            Ok(guard) => Ok(Box::new(guard)),
            Err(memoria_application::ports::LockFailure::Busy) => Err(conflict(
                "hook_conflict",
                format!(
                    "another Memoria hook operation holds {}; retry shortly",
                    path.display()
                ),
            )),
            Err(memoria_application::ports::LockFailure::Io(err)) => Err(HookFailure::Io(err)),
        }
    }

    /// The digest of the configuration's current bytes, or `None` when the
    /// file does not exist.
    fn configuration_digest(
        &self,
        destination: &Destination,
    ) -> Result<Option<String>, HookFailure> {
        if !self.require_plain(&destination.configuration)? {
            return Ok(None);
        }
        Ok(Some(bytes_digest(
            &self.read_bounded(&destination.configuration)?,
        )))
    }

    /// Replace the configuration only when its bytes still match `expected`.
    fn compare_and_swap(
        &self,
        destination: &Destination,
        expected: Option<&str>,
        bytes: &[u8],
    ) -> Result<(), HookFailure> {
        let current = self.configuration_digest(destination)?;
        if current.as_deref() != expected {
            return Err(conflict(
                "hook_conflict",
                format!(
                    "{} changed while Memoria was preparing its edit. Nothing was changed. The recovery record is {}. Run the command again.",
                    destination.configuration.display(),
                    transaction_path(destination).display()
                ),
            ));
        }
        self.write_file(&destination.configuration, bytes)
    }

    /// Remove the configuration only when its bytes still match `expected`.
    fn compare_and_remove(
        &self,
        destination: &Destination,
        expected: Option<&str>,
    ) -> Result<(), HookFailure> {
        let current = self.configuration_digest(destination)?;
        if current.as_deref() != expected {
            return Err(conflict(
                "hook_conflict",
                format!(
                    "{} changed while Memoria was preparing its edit. Nothing was changed.",
                    destination.configuration.display()
                ),
            ));
        }
        self.remove_file(&destination.configuration)
    }

    /// Record the intent before shared configuration changes, so an
    /// interrupted operation can be recognized and finished.
    fn write_intent(&self, destination: &Destination, intent: &Intent) -> Result<(), HookFailure> {
        let digest =
            |value: &Option<String>| value.clone().map(Value::String).unwrap_or(Value::Null);
        let mut map = Map::new();
        map.insert("version".into(), Value::from(INTENT_VERSION));
        map.insert("operation".into(), Value::String(intent.operation.clone()));
        map.insert("target".into(), Value::String(intent.target.clone()));
        map.insert(
            "configuration".into(),
            Value::String(intent.configuration.clone()),
        );
        map.insert("expected_digest".into(), digest(&intent.expected_digest));
        map.insert("desired_digest".into(), digest(&intent.desired_digest));
        map.insert("expected_record".into(), digest(&intent.expected_record));
        map.insert(
            "entry_hash".into(),
            Value::String(intent.entry_hash.clone()),
        );
        map.insert(
            "desired_record".into(),
            match &intent.desired_record {
                None => Value::Null,
                Some(record) => parse_json(&ownership_to_bytes(record)?)
                    .map_err(|e| conflict("hook_configuration_invalid", e.message))?,
            },
        );
        map.insert("phase".into(), Value::String(intent.phase.clone()));
        let bytes = write_json(&Value::Object(map))
            .map_err(|e| conflict("hook_configuration_invalid", e.message))?;
        self.write_file(&transaction_path(destination), &bytes)
    }

    /// Read a validated intent, if one remains from an interrupted run.
    fn read_intent(&self, destination: &Destination) -> Result<Option<Intent>, HookFailure> {
        let path = transaction_path(destination);
        if !self.require_plain(&path)? {
            return Ok(None);
        }
        let bytes = self.read_bounded(&path)?;
        let bad = |what: &str| {
            conflict(
                "hook_conflict",
                format!("the recovery record {} is {what}", path.display()),
            )
        };
        let value = parse_json(&bytes).map_err(|_| bad("not readable"))?;
        let object = value.as_object().ok_or_else(|| bad("not an object"))?;
        if object.get("version").and_then(Value::as_u64) != Some(INTENT_VERSION) {
            return Err(bad("an unsupported version"));
        }
        // Every field is known and correctly typed, or the record is not a
        // transaction this release wrote.
        const FIELDS: [&str; 10] = [
            "version",
            "operation",
            "target",
            "configuration",
            "expected_digest",
            "desired_digest",
            "expected_record",
            "entry_hash",
            "desired_record",
            "phase",
        ];
        for key in object.keys() {
            if !FIELDS.contains(&key.as_str()) {
                return Err(bad("about an unknown field"));
            }
        }
        let text = |key: &str| -> Result<String, HookFailure> {
            object
                .get(key)
                .and_then(Value::as_str)
                .map(str::to_string)
                .ok_or_else(|| bad("missing a required field"))
        };
        // A digest is a string or an explicit null. A number or an object
        // is malformed, never a silently absent file.
        let digest = |key: &str| -> Result<Option<String>, HookFailure> {
            match object.get(key) {
                Some(Value::String(text)) => Ok(Some(text.clone())),
                Some(Value::Null) => Ok(None),
                _ => Err(bad("a malformed digest")),
            }
        };
        let desired_record = match object.get("desired_record") {
            Some(Value::Null) | None => None,
            Some(record @ Value::Object(_)) => {
                let bytes =
                    write_json(record).map_err(|_| bad("an unreadable ownership record"))?;
                Some(ownership_from_bytes(&bytes).map_err(|_| bad("an invalid ownership record"))?)
            }
            _ => return Err(bad("a malformed ownership record")),
        };
        let intent = Intent {
            operation: text("operation")?,
            target: text("target")?,
            configuration: text("configuration")?,
            expected_digest: digest("expected_digest")?,
            desired_digest: digest("desired_digest")?,
            expected_record: digest("expected_record")?,
            entry_hash: text("entry_hash")?,
            desired_record,
            phase: text("phase")?,
        };
        if intent.operation != "install" && intent.operation != "uninstall" {
            return Err(bad("an unknown operation"));
        }
        if intent.phase != "started" && intent.phase != "configuration-written" {
            return Err(bad("an unknown phase"));
        }
        if intent.target != destination.target.as_str()
            || intent.configuration != destination.relative
        {
            return Err(bad("about a different target or configuration"));
        }
        // The two identities must agree with each other and with the
        // operation: an install leaves a record, an uninstall leaves none.
        match (intent.operation.as_str(), &intent.desired_record) {
            ("install", Some(record)) if record.entry_hash == intent.entry_hash => {}
            ("uninstall", None) => {}
            _ => return Err(bad("inconsistent with its own operation")),
        }
        Ok(Some(intent))
    }

    /// Finish a verified interrupted operation, or refuse safely.
    ///
    /// The intent records the bytes the operation required and the bytes and
    /// ownership it intended to leave. Recovery compares the configuration it
    /// finds with both identities:
    ///
    /// - the expected bytes: the write never landed, so the record side is
    ///   rolled back and the file is left exactly as found;
    /// - the desired bytes: the write landed, so the remaining record step
    ///   is finished with the recorded provenance;
    /// - neither: someone edited the file, so nothing is changed at all.
    ///
    /// The recorded phase says which of these transitions is possible, so a
    /// contradictory record is refused rather than guessed at.
    fn recover(&self, destination: &Destination) -> Result<(), HookFailure> {
        let Some(intent) = self.read_intent(destination)? else {
            return Ok(());
        };
        let current = self
            .read_configuration_bytes(destination)?
            .as_deref()
            .map(bytes_digest);
        let landed = current == intent.desired_digest;
        let untouched = current == intent.expected_digest;
        let outside_edit = || {
            Err(conflict(
                "hook_conflict",
                format!(
                    "{} changed while an interrupted Memoria operation was pending. Nothing was changed. The recovery record is {}. Restore that file or remove the recovery record, then run the command again.",
                    destination.configuration.display(),
                    transaction_path(destination).display()
                ),
            ))
        };
        match intent.phase.as_str() {
            // The write may or may not have landed.
            "started" if landed || untouched => {}
            // The write landed before the interruption, so the file must
            // still hold exactly those bytes.
            "configuration-written" if landed => {}
            _ => return outside_edit(),
        }
        // The ownership record has its own two identities: the one the
        // operation found, and the one it intended to leave. Recovery
        // accepts only the states its phase permits. A record edited during
        // the interruption is a conflict, never something to repair.
        let desired_record_bytes = match &intent.desired_record {
            Some(desired) => Some(ownership_to_bytes(desired)?),
            None => None,
        };
        let desired_record = desired_record_bytes.as_deref().map(bytes_digest);
        let current_record = self.record_digest(destination)?;
        let record_is_expected = current_record == intent.expected_record;
        let record_is_desired = current_record == desired_record;
        let record_edited = || {
            Err(conflict(
                "hook_conflict",
                format!(
                    "{} changed while an interrupted Memoria operation was pending. Nothing was changed. The recovery record is {}. Restore that file or remove the recovery record, then run the command again.",
                    destination.record.display(),
                    transaction_path(destination).display()
                ),
            ))
        };
        match intent.phase.as_str() {
            // The record step had not started, so the record must still be
            // the one the operation found.
            "started" if record_is_expected => {}
            // The record step may have completed.
            "configuration-written" if record_is_expected || record_is_desired => {}
            _ => return record_edited(),
        }
        if landed && !record_is_desired {
            match &desired_record_bytes {
                // Finish the install: restore the exact intended record,
                // including which containers this operation created.
                Some(bytes) => self.compare_and_write_record(
                    destination,
                    intent.expected_record.as_deref(),
                    bytes,
                )?,
                // Finish the uninstall: the owned entry is gone, so its
                // record goes too.
                None => {
                    self.compare_and_remove_record(destination, intent.expected_record.as_deref())?
                }
            }
        }
        // When the configuration was never written, the record is already
        // the one the operation found, so there is nothing to undo.
        self.clear_intent(destination)
    }

    /// Refuse when the ownership record is no longer the one this operation
    /// examined. Absence is one of the identities it can expect.
    fn require_record(
        &self,
        destination: &Destination,
        expected: Option<&str>,
    ) -> Result<(), HookFailure> {
        if self.record_digest(destination)?.as_deref() == expected {
            return Ok(());
        }
        Err(conflict(
            "hook_conflict",
            format!(
                "{} changed while Memoria was preparing its edit. Nothing was changed. Run the command again.",
                destination.record.display()
            ),
        ))
    }

    /// Replace the ownership record only when it is still `expected`.
    fn compare_and_write_record(
        &self,
        destination: &Destination,
        expected: Option<&str>,
        bytes: &[u8],
    ) -> Result<(), HookFailure> {
        self.require_record(destination, expected)?;
        self.write_file(&destination.record, bytes)
    }

    /// Remove the ownership record only when it is still `expected`.
    fn compare_and_remove_record(
        &self,
        destination: &Destination,
        expected: Option<&str>,
    ) -> Result<(), HookFailure> {
        self.require_record(destination, expected)?;
        self.remove_file(&destination.record)
    }

    /// Remove one file and make the removal durable.
    fn remove_file(&self, path: &Path) -> Result<(), HookFailure> {
        if !self.require_plain(path)? {
            return Ok(());
        }
        fs::remove_file(path).map_err(|e| io("remove", path, e))?;
        if let Some(parent) = path.parent() {
            crate::fs::sync_dir(parent).map_err(|e| io("sync_dir", parent, e))?;
        }
        Ok(())
    }

    fn clear_intent(&self, destination: &Destination) -> Result<(), HookFailure> {
        self.remove_file(&transaction_path(destination))
    }

    /// The inline Codex representation, edited through `toml_edit` so
    /// unrelated values and comments survive.
    fn apply_inline_install(
        &self,
        destination: &Destination,
        plan: &HookPlan,
    ) -> Result<(), HookFailure> {
        let current = self.read_configuration_bytes(destination)?;
        let before = current.as_deref().map(bytes_digest);
        let mut document = match &current {
            Some(bytes) => self.parse_inline(destination, bytes)?,
            None => toml_edit::DocumentMut::new(),
        };
        let entry = owned_entry(&plan.command);
        let hash = entry_hash(&entry);
        let previous_bytes = self.read_record_bytes(destination)?;
        let expected_record = previous_bytes.as_deref().map(bytes_digest);
        let previous = match &previous_bytes {
            Some(bytes) => Some(ownership_from_bytes(bytes)?),
            None => None,
        };

        let hooks_absent = document.get("hooks").is_none();
        let hooks = document
            .entry("hooks")
            .or_insert(toml_edit::Item::Table(toml_edit::Table::new()));
        let hooks = hooks.as_table_mut().ok_or_else(|| {
            conflict(
                "hook_configuration_invalid",
                "the inline [hooks] value is not a table, and Memoria cannot extend it. Nothing was changed.",
            )
        })?;
        let stop_absent = hooks.get(EVENT).is_none();
        let stop = hooks.entry(EVENT).or_insert(toml_edit::Item::ArrayOfTables(
            toml_edit::ArrayOfTables::new(),
        ));
        let stop = stop.as_array_of_tables_mut().ok_or_else(|| {
            conflict(
                "hook_configuration_invalid",
                "the inline [[hooks.Stop]] value is not an array of tables, and Memoria cannot extend it. Nothing was changed.",
            )
        })?;
        // A relocated installation replaces its own recorded group.
        if let Some(previous) = &previous {
            stop.retain(|group| inline_group_hash(group) != previous.entry_hash);
        }
        let mut handler = toml_edit::Table::new();
        handler["type"] = toml_edit::value("command");
        handler["command"] = toml_edit::value(plan.command.clone());
        handler["timeout"] = toml_edit::value(hook_config::TIMEOUT_SECONDS as i64);
        let mut handlers = toml_edit::ArrayOfTables::new();
        handlers.push(handler);
        let mut group = toml_edit::Table::new();
        group.insert("hooks", toml_edit::Item::ArrayOfTables(handlers));
        stop.push(group);

        // Container provenance is recorded for the inline representation
        // too, so uninstall removes only what Memoria created.
        let mut created = Created {
            file: false,
            hooks: hooks_absent,
            stop: stop_absent,
        };
        if let Some(previous) = &previous {
            created.hooks |= previous.created.hooks;
            created.stop |= previous.created.stop;
        }
        let record = Ownership {
            target: plan.target.as_str().to_string(),
            configuration: destination.relative.clone(),
            entry,
            entry_hash: hash.clone(),
            created,
        };
        let rendered = document.to_string();
        let intent = Intent {
            operation: "install".into(),
            target: plan.target.as_str().to_string(),
            configuration: destination.relative.clone(),
            expected_digest: before.clone(),
            desired_digest: Some(bytes_digest(rendered.as_bytes())),
            expected_record: expected_record.clone(),
            entry_hash: hash,
            desired_record: Some(record.clone()),
            phase: "started".into(),
        };
        self.write_intent(destination, &intent)?;
        self.compare_and_swap(destination, before.as_deref(), rendered.as_bytes())?;
        self.write_intent(
            destination,
            &Intent {
                phase: "configuration-written".into(),
                ..intent
            },
        )?;
        self.compare_and_write_record(
            destination,
            expected_record.as_deref(),
            &ownership_to_bytes(&record)?,
        )?;
        self.clear_intent(destination)
    }

    fn apply_inline_uninstall(
        &self,
        destination: &Destination,
        record: &Ownership,
        expected_record: Option<&str>,
    ) -> Result<(), HookFailure> {
        let current = self.read_configuration_bytes(destination)?;
        let before = current.as_deref().map(bytes_digest);
        let mut document = match &current {
            Some(bytes) => self.parse_inline(destination, bytes)?,
            None => toml_edit::DocumentMut::new(),
        };
        if let Some(hooks) = document
            .get_mut("hooks")
            .and_then(|h| h.as_table_like_mut())
            && let Some(stop) = hooks.get_mut(EVENT)
        {
            // Exactly the owned group, identified by its canonical hash.
            // A user group that happens to run the same command has its own
            // matcher and timeout, so its hash differs and it survives.
            let emptied = if let Some(tables) = stop.as_array_of_tables_mut() {
                tables.retain(|group| inline_group_hash(group) != record.entry_hash);
                tables.is_empty()
            } else if let Some(array) = stop.as_array_mut() {
                array.retain(|value| {
                    value
                        .as_inline_table()
                        .map(|table| inline_table_hash(table) != record.entry_hash)
                        .unwrap_or(true)
                });
                array.is_empty()
            } else {
                false
            };
            // A container is removed only when Memoria created it.
            if emptied && record.created.stop {
                hooks.remove(EVENT);
                if hooks.iter().next().is_none() && record.created.hooks {
                    document.remove("hooks");
                }
            }
        }
        let rendered = document.to_string();
        let intent = Intent {
            operation: "uninstall".into(),
            target: record.target.clone(),
            configuration: destination.relative.clone(),
            expected_digest: before.clone(),
            desired_digest: Some(bytes_digest(rendered.as_bytes())),
            expected_record: expected_record.map(str::to_string),
            entry_hash: record.entry_hash.clone(),
            desired_record: None,
            phase: "started".into(),
        };
        self.write_intent(destination, &intent)?;
        self.compare_and_swap(destination, before.as_deref(), rendered.as_bytes())?;
        self.write_intent(
            destination,
            &Intent {
                phase: "configuration-written".into(),
                ..intent
            },
        )?;
        self.compare_and_remove_record(destination, expected_record)?;
        self.clear_intent(destination)
    }

    /// Parse the inline configuration document.
    fn parse_inline(
        &self,
        destination: &Destination,
        bytes: &[u8],
    ) -> Result<toml_edit::DocumentMut, HookFailure> {
        let text = std::str::from_utf8(bytes).map_err(|_| {
            conflict(
                "hook_configuration_invalid",
                format!("{} is not valid UTF-8", destination.configuration.display()),
            )
        })?;
        text.parse().map_err(|err| {
            conflict(
                "hook_configuration_invalid",
                format!(
                    "{} is not valid TOML: {err}",
                    destination.configuration.display()
                ),
            )
        })
    }
}

/// The ownership hash of one inline TOML group.
///
/// The group is projected into the same JSON shape the ownership record
/// stores, so one hash identifies an owned group in either representation.
/// A user group that shares only a command hashes differently.
fn inline_group_hash(group: &toml_edit::Table) -> String {
    let mut map = Map::new();
    for (key, item) in group.iter() {
        map.insert(key.to_string(), toml_to_json(item));
    }
    entry_hash(&Value::Object(map))
}

fn inline_table_hash(table: &toml_edit::InlineTable) -> String {
    let mut map = Map::new();
    for (key, value) in table.iter() {
        map.insert(key.to_string(), toml_value_to_json(value));
    }
    entry_hash(&Value::Object(map))
}

/// Read the ownership record's declared entry through the strict project
/// JSON parser, for tests that need the exact stored shape.
pub fn read_record_json(bytes: &[u8]) -> Result<Json, String> {
    json::parse(bytes, Limits::STATE).map_err(|e| e.message)
}

/// The exact keys the ownership record declares.
pub fn record_keys(bytes: &[u8]) -> Result<Vec<String>, String> {
    let value = read_record_json(bytes)?;
    let reader = ObjectReader::new(value, "record").map_err(|e| e.message)?;
    Ok(reader.into_fields().keys().cloned().collect())
}
