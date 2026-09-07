//! Versioned JSON state store with compare-and-swap replacement.

use std::collections::BTreeMap;
use std::path::PathBuf;

use memoria_application::packet::{manifest_detail, review_record_detail};
use memoria_application::ports::{AdapterError, LoadedState, StateFailure, StateStore};
use memoria_domain::{
    DirPath, DocumentId, ExportId, FileInput, GitContext, Hash64, ImportInput, InputManifest,
    Invalidation, InvalidationScope, ProjectPath, Reason, ReviewNote, ReviewRecord, ReviewResult,
    ReviewState, ReviewerName, Timestamp,
};

use crate::fs::{
    FaultHook, NO_FAULTS, check_state_dir, durable_replace_with, kind_of, read_regular,
};
use crate::json::{
    self, Json, JsonError, Limits, ObjectReader, expect_string, expect_u64, from_detail,
};

pub const STATE_RELATIVE: &str = ".memoria/state.json";

pub struct JsonStateStore<'a> {
    root: PathBuf,
    path: PathBuf,
    faults: FaultHook<'a>,
}

impl JsonStateStore<'static> {
    pub fn new(root: PathBuf) -> JsonStateStore<'static> {
        JsonStateStore {
            path: root.join(STATE_RELATIVE),
            root,
            faults: NO_FAULTS,
        }
    }
}

impl<'a> JsonStateStore<'a> {
    /// A store whose durable replacement consults `faults` (see
    /// [`durable_replace_with`]).
    pub fn with_faults(root: PathBuf, faults: FaultHook<'a>) -> JsonStateStore<'a> {
        JsonStateStore {
            path: root.join(STATE_RELATIVE),
            root,
            faults,
        }
    }
}

fn schema(message: impl Into<String>) -> JsonError {
    JsonError {
        code: "json_schema",
        message: message.into(),
    }
}

fn hash_from(text: String, context: &str) -> Result<Hash64, JsonError> {
    Hash64::parse(&text).map_err(|e| schema(format!("{context}: {e}")))
}

pub fn manifest_from_json(value: Json, context: &str) -> Result<InputManifest, JsonError> {
    let mut reader = ObjectReader::new(value, context)?;
    if reader.take_u64("version")? != 1 {
        return Err(schema(format!("{context}.version must be 1")));
    }
    let document = DocumentId::parse(&reader.take_string("document")?)
        .map_err(|e| schema(format!("{context}.document: {e}")))?;
    let policy_hash = hash_from(
        reader.take_string("policy_hash")?,
        &format!("{context}.policy_hash"),
    )?;
    let document_bytes = reader.take_u64("document_bytes")?;
    let document_hash = hash_from(
        reader.take_string("document_hash")?,
        &format!("{context}.document_hash"),
    )?;
    let mut files = Vec::new();
    for (index, item) in reader.take_array("files")?.into_iter().enumerate() {
        let ctx = format!("{context}.files[{index}]");
        let mut file = ObjectReader::new(item, &ctx)?;
        let path = ProjectPath::parse(&file.take_string("path")?)
            .map_err(|e| schema(format!("{ctx}.path: {e}")))?;
        let bytes = file.take_u64("bytes")?;
        let hash = hash_from(file.take_string("hash")?, &format!("{ctx}.hash"))?;
        file.finish()?;
        files.push(FileInput { path, bytes, hash });
    }
    let mut imports = Vec::new();
    for (index, item) in reader.take_array("imports")?.into_iter().enumerate() {
        let ctx = format!("{context}.imports[{index}]");
        let mut import = ObjectReader::new(item, &ctx)?;
        let document = DocumentId::parse(&import.take_string("document")?)
            .map_err(|e| schema(format!("{ctx}.document: {e}")))?;
        let export_id = ExportId::parse(&import.take_string("export_id")?)
            .map_err(|e| schema(format!("{ctx}.export_id: {e}")))?;
        let bytes = import.take_u64("bytes")?;
        let hash = hash_from(import.take_string("hash")?, &format!("{ctx}.hash"))?;
        import.finish()?;
        imports.push(ImportInput {
            document,
            export_id,
            bytes,
            hash,
        });
    }
    reader.finish()?;
    let sorted_files = files.clone();
    let sorted_imports = imports.clone();
    let manifest = InputManifest::new(
        document,
        policy_hash,
        document_bytes,
        document_hash,
        files,
        imports,
    )
    .map_err(|e| schema(format!("{context}: {e}")))?;
    if manifest.files() != sorted_files.as_slice()
        || manifest.imports() != sorted_imports.as_slice()
    {
        return Err(schema(format!(
            "{context}: files and imports must be in canonical sorted order"
        )));
    }
    Ok(manifest)
}

pub fn record_from_json(value: Json, context: &str) -> Result<ReviewRecord, JsonError> {
    let mut reader = ObjectReader::new(value, context)?;
    let revision = reader.take_u64("revision")?;
    let manifest = manifest_from_json(
        reader.take("input_manifest")?,
        &format!("{context}.input_manifest"),
    )?;
    let input_fingerprint = hash_from(
        reader.take_string("input_fingerprint")?,
        &format!("{context}.input_fingerprint"),
    )?;
    let token_digest = hash_from(
        reader.take_string("token_digest")?,
        &format!("{context}.token_digest"),
    )?;
    let reviewed_at = Timestamp(reader.take_string("reviewed_at")?);
    let reviewer = ReviewerName::from_stored(reader.take_string("reviewer")?);
    let result = ReviewResult::parse(&reader.take_string("result")?)
        .ok_or_else(|| schema(format!("{context}.result must be `updated` or `no-update`")))?;
    let note = ReviewNote::from_stored(reader.take_string("note")?);
    let mut git = reader.take_object("git")?;
    let base_commit = git.take_optional_string("base_commit")?;
    let worktree_dirty = git.take_bool("worktree_dirty")?;
    git.finish()?;
    let mut acknowledged = Vec::new();
    for (index, item) in reader
        .take_array("acknowledged_invalidations")?
        .into_iter()
        .enumerate()
    {
        acknowledged.push(expect_u64(
            item,
            &format!("{context}.acknowledged_invalidations[{index}]"),
        )?);
    }
    reader.finish()?;
    Ok(ReviewRecord {
        revision,
        manifest,
        input_fingerprint,
        token_digest,
        reviewed_at,
        reviewer,
        result,
        note,
        git: GitContext {
            base_commit,
            worktree_dirty,
        },
        acknowledged_invalidations: acknowledged,
    })
}

fn invalidation_from_json(value: Json, context: &str) -> Result<Invalidation, JsonError> {
    let mut reader = ObjectReader::new(value, context)?;
    let id = reader.take_u64("id")?;
    let scope_text = reader.take_string("scope")?;
    let scope = if scope_text == "all" {
        InvalidationScope::All
    } else if let Some(doc) = scope_text.strip_prefix("doc:") {
        InvalidationScope::Document(
            DocumentId::parse(doc).map_err(|e| schema(format!("{context}.scope: {e}")))?,
        )
    } else if let Some(dir) = scope_text.strip_prefix("subtree:") {
        InvalidationScope::Subtree(
            DirPath::parse(dir).map_err(|e| schema(format!("{context}.scope: {e}")))?,
        )
    } else {
        return Err(schema(format!(
            "{context}.scope {scope_text:?} is not recognized"
        )));
    };
    let reason = Reason::from_stored(reader.take_string("reason")?);
    let created_at = Timestamp(reader.take_string("created_at")?);
    let docs = |items: Vec<Json>, key: &str| -> Result<Vec<DocumentId>, JsonError> {
        items
            .into_iter()
            .enumerate()
            .map(|(i, item)| {
                DocumentId::parse(&expect_string(item, &format!("{context}.{key}[{i}]"))?)
                    .map_err(|e| schema(format!("{context}.{key}[{i}]: {e}")))
            })
            .collect()
    };
    let targets = docs(reader.take_array("targets")?, "targets")?;
    let pending = docs(reader.take_array("pending_documents")?, "pending_documents")?;
    reader.finish()?;
    Ok(Invalidation {
        id,
        scope,
        reason,
        created_at,
        targets,
        pending,
    })
}

pub fn state_from_json(value: Json) -> Result<ReviewState, JsonError> {
    let mut reader = ObjectReader::new(value, "state")?;
    let schema_version = reader.take_u64("schema_version")?;
    if schema_version != 1 {
        return Err(JsonError {
            code: "state_unsupported_schema",
            message: format!(
                "unsupported state schema_version {schema_version}; this release supports version 1 only"
            ),
        });
    }
    let revision = reader.take_u64("revision")?;
    let next_invalidation_id = reader.take_u64("next_invalidation_id")?;
    let mut reviews = BTreeMap::new();
    let reviews_reader = ObjectReader::new(reader.take("reviews")?, "state.reviews")?;
    for (key, value) in reviews_reader.into_fields() {
        let document = DocumentId::parse(&key)
            .map_err(|e| schema(format!("state.reviews key {key:?}: {e}")))?;
        let record = record_from_json(value, &format!("state.reviews[{key:?}]"))?;
        reviews.insert(document, record);
    }
    let mut invalidations = Vec::new();
    for (index, item) in reader.take_array("invalidations")?.into_iter().enumerate() {
        invalidations.push(invalidation_from_json(
            item,
            &format!("state.invalidations[{index}]"),
        )?);
    }
    reader.finish()?;
    Ok(ReviewState {
        revision,
        next_invalidation_id,
        reviews,
        invalidations,
    })
}

impl ObjectReader {
    pub fn into_fields(self) -> BTreeMap<String, Json> {
        self.into_inner()
    }
}

pub fn state_to_json(state: &ReviewState) -> Json {
    let reviews: BTreeMap<String, Json> = state
        .reviews
        .iter()
        .map(|(doc, record)| {
            (
                doc.as_str().to_string(),
                from_detail(&review_record_detail(record)),
            )
        })
        .collect();
    let invalidations: Vec<Json> = state
        .invalidations
        .iter()
        .map(|inv| {
            let mut map = BTreeMap::new();
            map.insert("id".into(), Json::Number(inv.id));
            map.insert("scope".into(), Json::String(inv.scope.to_string()));
            map.insert("reason".into(), Json::String(inv.reason.as_str().into()));
            map.insert("created_at".into(), Json::String(inv.created_at.0.clone()));
            map.insert(
                "targets".into(),
                Json::Array(
                    inv.targets
                        .iter()
                        .map(|d| Json::String(d.as_str().into()))
                        .collect(),
                ),
            );
            map.insert(
                "pending_documents".into(),
                Json::Array(
                    inv.pending
                        .iter()
                        .map(|d| Json::String(d.as_str().into()))
                        .collect(),
                ),
            );
            Json::Object(map)
        })
        .collect();
    let mut map = BTreeMap::new();
    map.insert("schema_version".into(), Json::Number(1));
    map.insert("revision".into(), Json::Number(state.revision));
    map.insert(
        "next_invalidation_id".into(),
        Json::Number(state.next_invalidation_id),
    );
    map.insert("reviews".into(), Json::Object(reviews));
    map.insert("invalidations".into(), Json::Array(invalidations));
    Json::Object(map)
}

pub fn manifest_to_json(manifest: &InputManifest) -> Json {
    from_detail(&manifest_detail(manifest))
}

impl StateStore for JsonStateStore<'_> {
    fn load(&self) -> Result<Option<LoadedState>, StateFailure> {
        check_state_dir(&self.root).map_err(StateFailure::Io)?;
        match kind_of(&self.path) {
            Ok(memoria_application::ports::FileKind::Missing) => return Ok(None),
            Ok(memoria_application::ports::FileKind::Regular) => {}
            Ok(other) => {
                return Err(StateFailure::Corrupt(format!(
                    "{} must be a regular file, found {other:?}",
                    self.path.display()
                )));
            }
            Err(err) => {
                return Err(StateFailure::Io(AdapterError::new(
                    "stat",
                    Some(self.path.display().to_string()),
                    err.to_string(),
                )));
            }
        }
        let bytes = read_regular(&self.path).map_err(StateFailure::Io)?;
        let value = json::parse(&bytes, Limits::STATE)
            .map_err(|e| StateFailure::Corrupt(format!("state file is not strict JSON: {e}")))?;
        let state = state_from_json(value).map_err(|e| StateFailure::Corrupt(e.message))?;
        Ok(Some(LoadedState { bytes, state }))
    }

    fn encode(&self, state: &ReviewState) -> Vec<u8> {
        json::to_pretty(&state_to_json(state)).into_bytes()
    }

    fn save(&self, state: &ReviewState, expected: Option<&[u8]>) -> Result<Vec<u8>, StateFailure> {
        check_state_dir(&self.root).map_err(StateFailure::Io)?;
        // Never serialize a state that would be rejected on the next load.
        state.validate().map_err(|e| {
            StateFailure::Corrupt(format!("refusing to write inconsistent state: {e}"))
        })?;
        let current = match kind_of(&self.path) {
            Ok(memoria_application::ports::FileKind::Missing) => None,
            Ok(_) => Some(read_regular(&self.path).map_err(StateFailure::Io)?),
            Err(err) => {
                return Err(StateFailure::Io(AdapterError::new(
                    "stat",
                    Some(self.path.display().to_string()),
                    err.to_string(),
                )));
            }
        };
        if current.as_deref() != expected {
            return Err(StateFailure::Conflict);
        }
        if let Some(parent) = self.path.parent() {
            std::fs::create_dir_all(parent).map_err(|e| {
                StateFailure::Io(AdapterError::new(
                    "create_dir",
                    Some(parent.display().to_string()),
                    e.to_string(),
                ))
            })?;
        }
        let bytes = self.encode(state);
        let mode = {
            use std::os::unix::fs::PermissionsExt;
            std::fs::metadata(&self.path)
                .ok()
                .map(|m| m.permissions().mode())
                .or(Some(0o600))
        };
        durable_replace_with(&self.path, &bytes, mode, self.faults).map_err(StateFailure::Io)?;
        Ok(bytes)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_state_round_trip_is_canonical() {
        let json = state_to_json(&ReviewState::empty());
        let text = json::to_pretty(&json);
        assert_eq!(
            text,
            "{\n  \"invalidations\": [],\n  \"next_invalidation_id\": 1,\n  \"reviews\": {},\n  \"revision\": 0,\n  \"schema_version\": 1\n}\n"
        );
        let parsed = state_from_json(json::parse(text.as_bytes(), Limits::STATE).unwrap()).unwrap();
        assert_eq!(parsed, ReviewState::empty());
    }

    #[test]
    fn rejects_future_schema_and_unknown_fields() {
        let err = state_from_json(json::parse(b"{\"schema_version\":2,\"revision\":0,\"next_invalidation_id\":1,\"reviews\":{},\"invalidations\":[]}", Limits::STATE).unwrap()).unwrap_err();
        assert_eq!(err.code, "state_unsupported_schema");
        let err = state_from_json(json::parse(b"{\"schema_version\":1,\"revision\":0,\"next_invalidation_id\":1,\"reviews\":{},\"invalidations\":[],\"extra\":1}", Limits::STATE).unwrap()).unwrap_err();
        assert!(err.message.contains("unknown field"));
    }

    #[test]
    fn save_refuses_inconsistent_state_and_symlinked_directory() {
        let dir = tempfile::tempdir().unwrap();
        let store = JsonStateStore::new(dir.path().to_path_buf());
        let mut zero = ReviewState::empty();
        zero.next_invalidation_id = 0;
        assert!(matches!(
            store.save(&zero, None),
            Err(StateFailure::Corrupt(_))
        ));
        assert!(store.load().unwrap().is_none());
        let outside = tempfile::tempdir().unwrap();
        std::os::unix::fs::symlink(outside.path(), dir.path().join(".memoria")).unwrap();
        assert!(matches!(
            store.save(&ReviewState::empty(), None),
            Err(StateFailure::Io(_))
        ));
        assert!(matches!(store.load(), Err(StateFailure::Io(_))));
        assert_eq!(std::fs::read_dir(outside.path()).unwrap().count(), 0);
    }

    #[test]
    fn injected_write_failures_keep_the_previous_state() {
        for step in ["temp-create", "sync-file", "rename"] {
            let dir = tempfile::tempdir().unwrap();
            let plain = JsonStateStore::new(dir.path().to_path_buf());
            let before = plain.save(&ReviewState::empty(), None).unwrap();
            let hook = move |name: &str| {
                if name == step {
                    Some(std::io::Error::other(format!("injected {step}")))
                } else {
                    None
                }
            };
            let faulty = JsonStateStore::with_faults(dir.path().to_path_buf(), &hook);
            let mut next = ReviewState::empty();
            next.revision = 1;
            assert!(
                matches!(faulty.save(&next, Some(&before)), Err(StateFailure::Io(_))),
                "{step}"
            );
            assert_eq!(
                std::fs::read(dir.path().join(STATE_RELATIVE)).unwrap(),
                before,
                "{step}"
            );
            let leftovers = std::fs::read_dir(dir.path().join(".memoria"))
                .unwrap()
                .count();
            assert_eq!(leftovers, 1, "{step} left temporary files");
        }
        // Directory sync failure: the complete new state is in place and the error is explicit.
        let dir = tempfile::tempdir().unwrap();
        let plain = JsonStateStore::new(dir.path().to_path_buf());
        let before = plain.save(&ReviewState::empty(), None).unwrap();
        let hook = |name: &str| {
            if name == "sync-dir" {
                Some(std::io::Error::other("injected sync-dir"))
            } else {
                None
            }
        };
        let faulty = JsonStateStore::with_faults(dir.path().to_path_buf(), &hook);
        let mut next = ReviewState::empty();
        next.revision = 1;
        let err = faulty.save(&next, Some(&before)).unwrap_err();
        assert!(
            matches!(&err, StateFailure::Io(e) if e.operation == "sync_dir"),
            "{err:?}"
        );
        assert_eq!(plain.load().unwrap().unwrap().state.revision, 1);
    }

    #[test]
    fn store_compare_and_swap() {
        let dir = tempfile::tempdir().unwrap();
        let store = JsonStateStore::new(dir.path().to_path_buf());
        assert!(store.load().unwrap().is_none());
        let bytes = store.save(&ReviewState::empty(), None).unwrap();
        assert!(matches!(
            store.save(&ReviewState::empty(), None),
            Err(StateFailure::Conflict)
        ));
        let loaded = store.load().unwrap().unwrap();
        assert_eq!(loaded.bytes, bytes);
        let mut state = ReviewState::empty();
        state.revision = 1;
        store.save(&state, Some(&bytes)).unwrap();
        assert!(matches!(
            store.save(&state, Some(&bytes)),
            Err(StateFailure::Conflict)
        ));
        std::fs::write(
            dir.path().join(STATE_RELATIVE),
            b"{\"schema_version\":1,\"schema_version\":1}",
        )
        .unwrap();
        assert!(matches!(store.load(), Err(StateFailure::Corrupt(_))));
    }
}
