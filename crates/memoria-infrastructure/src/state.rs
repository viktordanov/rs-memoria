//! Committed state: the `memoria.lock` store and its read-only inspector.
//!
//! The store owns the artifact's bytes. It detects the clean cutover before
//! it chooses a decoder, so a leftover version 1 file is reported instead of
//! silently ignored. Writes keep compare-and-swap, final snapshot
//! validation, atomic rename, and directory synchronization.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use memoria_application::packet::{manifest_detail, review_record_detail};
use memoria_application::ports::{
    AdapterError, FileKind, InspectedState, LoadedState, StateFailure, StateInspector, StateStore,
};
use memoria_domain::{
    DocumentId, ExportId, FileInput, GitContext, GuidanceDigest, Hash64, ImportInput,
    InputManifest, ProjectPath, ReviewNote, ReviewRecord, ReviewResult, ReviewState, ReviewerName,
    Timestamp,
};

use crate::fs::{
    FaultHook, NO_FAULTS, check_state_dir, durable_replace_with, kind_of, read_regular,
};
use crate::json::{Json, JsonError, ObjectReader, expect_u64, from_detail};
use crate::lock_codec::{self, LockError, MAX_FILE_BYTES, codec_name};

/// The generated committed state, beside `memoria.toml`.
pub const STATE_RELATIVE: &str = "memoria.lock";
/// The version 1 state file. Recognized only to report the clean cutover.
pub const LEGACY_STATE_RELATIVE: &str = ".memoria/state.json";

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
    if reader.take_u64("version")? != 2 {
        return Err(schema(format!("{context}.version must be 2")));
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
    let guidance = GuidanceDigest(hash_from(
        reader.take_string("guidance_digest")?,
        &format!("{context}.guidance_digest"),
    )?);
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
        guidance,
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

pub fn manifest_to_json(manifest: &InputManifest) -> Json {
    from_detail(&manifest_detail(manifest))
}

pub fn record_to_json(record: &ReviewRecord) -> Json {
    from_detail(&review_record_detail(record))
}

impl ObjectReader {
    pub fn into_fields(self) -> BTreeMap<String, Json> {
        self.into_inner()
    }
}

fn lock_failure(error: LockError, path: &Path) -> StateFailure {
    let where_ = format!("{}: ", path.display());
    match error {
        LockError::Corrupt(message) => StateFailure::Corrupt(format!("{where_}{message}")),
        LockError::Limit(message) => StateFailure::LimitExceeded(format!("{where_}{message}")),
        LockError::UnsupportedSchema(message) => {
            StateFailure::UnsupportedSchema(format!("{where_}{message}"))
        }
        LockError::UnsupportedCodec(message) => {
            StateFailure::UnsupportedCodec(format!("{where_}{message}"))
        }
    }
}

/// Refuse anything that is not an ordinary file, so a symlink or device
/// cannot stand in for committed state.
fn require_regular(path: &Path) -> Result<bool, StateFailure> {
    match kind_of(path) {
        Ok(FileKind::Missing) => Ok(false),
        Ok(FileKind::Regular) => Ok(true),
        Ok(other) => Err(StateFailure::Corrupt(format!(
            "{} must be a regular file, found {other:?}",
            path.display()
        ))),
        Err(err) => Err(StateFailure::Io(AdapterError::new(
            "stat",
            Some(path.display().to_string()),
            err.to_string(),
        ))),
    }
}

/// Whether a path exists at all, following no symlink.
fn exists(path: &Path) -> bool {
    std::fs::symlink_metadata(path).is_ok()
}

/// Decide which decoder applies, before reading either file.
///
/// This release has one clean cutover. A lone legacy file is reported as
/// `state_legacy`; both files present are `state_ambiguous`, even when one
/// of them is corrupt. The loader never picks a winner.
fn choose(root: &Path) -> Result<bool, StateFailure> {
    let lock = root.join(STATE_RELATIVE);
    let legacy = root.join(LEGACY_STATE_RELATIVE);
    let has_lock = exists(&lock);
    let has_legacy = exists(&legacy);
    match (has_lock, has_legacy) {
        (true, true) => Err(StateFailure::Ambiguous(format!(
            "both {STATE_RELATIVE} and the version 1 {LEGACY_STATE_RELATIVE} exist. \
Archive the legacy file outside the worktree, verify the archive, then remove it. \
See docs/releases/0.2.0.md."
        ))),
        (false, true) => Err(StateFailure::Legacy(format!(
            "only the version 1 {LEGACY_STATE_RELATIVE} exists. This release reads {STATE_RELATIVE} \
and performs no automatic migration. Archive the legacy file outside the worktree, verify the \
archive, remove it, then run `memoria init --apply`. See docs/releases/0.2.0.md."
        ))),
        (has_lock, false) => Ok(has_lock),
    }
}

/// The `memoria.lock` store.
pub struct LockStateStore<'a> {
    root: PathBuf,
    path: PathBuf,
    faults: FaultHook<'a>,
}

impl LockStateStore<'static> {
    pub fn new(root: PathBuf) -> LockStateStore<'static> {
        LockStateStore {
            path: root.join(STATE_RELATIVE),
            root,
            faults: NO_FAULTS,
        }
    }
}

impl<'a> LockStateStore<'a> {
    /// A store whose durable replacement consults `faults` (see
    /// [`durable_replace_with`]).
    pub fn with_faults(root: PathBuf, faults: FaultHook<'a>) -> LockStateStore<'a> {
        LockStateStore {
            path: root.join(STATE_RELATIVE),
            root,
            faults,
        }
    }

    pub fn path(&self) -> &Path {
        &self.path
    }
}

/// Read one state file with its size bound enforced before allocation.
fn read_bounded(path: &Path) -> Result<Vec<u8>, StateFailure> {
    let metadata = std::fs::metadata(path).map_err(|err| {
        StateFailure::Io(AdapterError::new(
            "stat",
            Some(path.display().to_string()),
            err.to_string(),
        ))
    })?;
    if metadata.len() > MAX_FILE_BYTES {
        return Err(StateFailure::LimitExceeded(format!(
            "{} is {} bytes, above the limit of {MAX_FILE_BYTES}",
            path.display(),
            metadata.len()
        )));
    }
    read_regular(path).map_err(StateFailure::Io)
}

impl StateStore for LockStateStore<'_> {
    fn load(&self) -> Result<Option<LoadedState>, StateFailure> {
        check_state_dir(&self.root).map_err(StateFailure::Io)?;
        if !choose(&self.root)? {
            return Ok(None);
        }
        require_regular(&self.path)?;
        let bytes = read_bounded(&self.path)?;
        let decoded = lock_codec::decode(&bytes).map_err(|e| lock_failure(e, &self.path))?;
        Ok(Some(LoadedState {
            bytes,
            state: decoded.state,
        }))
    }

    fn encode(&self, state: &ReviewState) -> Result<Vec<u8>, StateFailure> {
        lock_codec::encode(state).map_err(|e| lock_failure(e, &self.path))
    }

    fn save(&self, state: &ReviewState, expected: Option<&[u8]>) -> Result<Vec<u8>, StateFailure> {
        check_state_dir(&self.root).map_err(StateFailure::Io)?;
        // Never write a state that the next load would reject.
        state.validate().map_err(|e| {
            StateFailure::Corrupt(format!("refusing to write inconsistent state: {e}"))
        })?;
        // The legacy file must be gone before this release writes state.
        let present = choose(&self.root)?;
        let current = if present {
            require_regular(&self.path)?;
            Some(read_bounded(&self.path)?)
        } else {
            None
        };
        if current.as_deref() != expected {
            return Err(StateFailure::Conflict);
        }
        // Encoding, including compression, happens before replacement, so an
        // allocation or encoding failure leaves the previous state intact.
        let bytes = self.encode(state)?;
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

/// Bounded, read-only inspection of a committed state artifact.
pub struct LockStateInspector {
    root: Option<PathBuf>,
    /// The directory an explicit `--file` path resolves against.
    invocation_dir: PathBuf,
}

impl LockStateInspector {
    pub fn new(root: Option<PathBuf>, invocation_dir: PathBuf) -> LockStateInspector {
        LockStateInspector {
            root,
            invocation_dir,
        }
    }

    fn inspect_path(&self, path: &Path, display: String) -> Result<InspectedState, StateFailure> {
        if !exists(path) {
            return Err(StateFailure::Missing(format!(
                "{display} does not exist; commit state is created by `memoria init --apply` and by acknowledgements"
            )));
        }
        if !require_regular(path)? {
            return Err(StateFailure::Missing(format!("{display} does not exist")));
        }
        let bytes = read_bounded(path)?;
        let decoded = lock_codec::decode(&bytes).map_err(|e| lock_failure(e, path))?;
        Ok(InspectedState {
            encoded: bytes,
            path: display,
            file_bytes: decoded.file_bytes,
            payload_bytes: decoded.payload_bytes,
            format_version: u64::from(decoded.format_version),
            codec: codec_name(decoded.codec),
            checksum: decoded.checksum,
            guidance: decoded
                .guidance
                .iter()
                .map(|(document, digest)| (document.as_str().to_string(), digest.to_hex()))
                .collect(),
            state: decoded.state,
        })
    }
}

impl StateInspector for LockStateInspector {
    fn inspect_project(&self) -> Result<InspectedState, StateFailure> {
        let root = self.root.as_ref().ok_or_else(|| {
            StateFailure::Io(AdapterError::new(
                "inspect",
                None,
                "no project is selected; pass --file to inspect an explicit state file",
            ))
        })?;
        check_state_dir(root).map_err(StateFailure::Io)?;
        // The cutover check runs before any decoder is chosen.
        if !choose(root)? {
            return Err(StateFailure::Missing(format!(
                "{} does not exist; run `memoria init --apply` to create it",
                root.join(STATE_RELATIVE).display()
            )));
        }
        let path = root.join(STATE_RELATIVE);
        let display = STATE_RELATIVE.to_string();
        self.inspect_path(&path, display)
    }

    fn inspect_file(&self, raw: &str) -> Result<InspectedState, StateFailure> {
        let candidate = Path::new(raw);
        let path = if candidate.is_absolute() {
            candidate.to_path_buf()
        } else {
            self.invocation_dir.join(candidate)
        };
        // An explicit file needs no worktree, configuration, or scan. It is
        // still refused when it is a symlink or another special file.
        self.inspect_path(&path, raw.to_string())
    }
}
