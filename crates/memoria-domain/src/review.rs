//! Latest review records, explicit invalidations, and acknowledgement rules.

use std::collections::BTreeMap;
use std::fmt;

use crate::manifest::{Hash64, InputManifest, ManifestDiff};
use crate::path::{DirPath, DocumentId};
use crate::text;

/// An opaque timestamp supplied by the application clock. Context only.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct Timestamp(pub String);

impl fmt::Display for Timestamp {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TextError {
    TooShort { field: &'static str, min: usize },
    TooLong { field: &'static str, max: usize },
    TooFewWords { field: &'static str, min: usize },
    ControlCharacter { field: &'static str },
    Generic { field: &'static str, text: String },
    Empty { field: &'static str },
}

impl fmt::Display for TextError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            TextError::TooShort { field, min } => {
                write!(f, "{field} must have at least {min} characters")
            }
            TextError::TooLong { field, max } => {
                write!(f, "{field} must have at most {max} characters")
            }
            TextError::TooFewWords { field, min } => {
                write!(f, "{field} must contain at least {min} words")
            }
            TextError::ControlCharacter { field } => {
                write!(f, "{field} contains a control character")
            }
            TextError::Generic { field, text } => {
                write!(f, "{field} {text:?} is too generic to explain a review")
            }
            TextError::Empty { field } => write!(f, "{field} must not be empty"),
        }
    }
}

impl std::error::Error for TextError {}

fn validate_explanation(field: &'static str, raw: &str) -> Result<String, TextError> {
    let trimmed = raw.trim();
    let chars = trimmed.chars().count();
    if chars < 12 {
        return Err(TextError::TooShort { field, min: 12 });
    }
    if chars > 1000 {
        return Err(TextError::TooLong { field, max: 1000 });
    }
    if text::word_count(trimmed) < 3 {
        return Err(TextError::TooFewWords { field, min: 3 });
    }
    if text::has_disallowed_control(trimmed) {
        return Err(TextError::ControlCharacter { field });
    }
    if text::is_generic(trimmed) {
        return Err(TextError::Generic {
            field,
            text: trimmed.to_string(),
        });
    }
    Ok(trimmed.to_string())
}

/// A trimmed review note: 12–1000 characters and at least three words.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReviewNote(String);

impl ReviewNote {
    pub fn parse(raw: &str) -> Result<ReviewNote, TextError> {
        validate_explanation("note", raw).map(ReviewNote)
    }

    /// Accept already-stored text without re-validation (state loading).
    pub fn from_stored(raw: String) -> ReviewNote {
        ReviewNote(raw)
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// A trimmed invalidation reason with the same rules as a note.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct Reason(String);

impl Reason {
    pub fn parse(raw: &str) -> Result<Reason, TextError> {
        validate_explanation("reason", raw).map(Reason)
    }

    pub fn from_stored(raw: String) -> Reason {
        Reason(raw)
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// A reviewer name: 1–128 characters without control characters.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReviewerName(String);

impl ReviewerName {
    pub fn parse(raw: &str) -> Result<ReviewerName, TextError> {
        let trimmed = raw.trim();
        if trimmed.is_empty() {
            return Err(TextError::Empty { field: "reviewer" });
        }
        if trimmed.chars().count() > 128 {
            return Err(TextError::TooLong {
                field: "reviewer",
                max: 128,
            });
        }
        if trimmed.chars().any(char::is_control) {
            return Err(TextError::ControlCharacter { field: "reviewer" });
        }
        Ok(ReviewerName(trimmed.to_string()))
    }

    pub fn from_stored(raw: String) -> ReviewerName {
        ReviewerName(raw)
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReviewResult {
    Updated,
    NoUpdate,
}

impl ReviewResult {
    pub fn as_str(&self) -> &'static str {
        match self {
            ReviewResult::Updated => "updated",
            ReviewResult::NoUpdate => "no-update",
        }
    }

    pub fn parse(raw: &str) -> Option<ReviewResult> {
        match raw {
            "updated" => Some(ReviewResult::Updated),
            "no-update" => Some(ReviewResult::NoUpdate),
            _ => None,
        }
    }
}

/// Git facts recorded for context only.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct GitContext {
    pub base_commit: Option<String>,
    pub worktree_dirty: bool,
}

/// The latest acknowledgement for one document.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReviewRecord {
    pub revision: u64,
    pub manifest: InputManifest,
    pub input_fingerprint: Hash64,
    pub token_digest: Hash64,
    pub reviewed_at: Timestamp,
    pub reviewer: ReviewerName,
    pub result: ReviewResult,
    pub note: ReviewNote,
    pub git: GitContext,
    pub acknowledged_invalidations: Vec<u64>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum InvalidationScope {
    All,
    Document(DocumentId),
    Subtree(DirPath),
}

impl InvalidationScope {
    /// Whether a document belongs to this scope, by stored identity alone.
    pub fn covers(&self, document: &DocumentId) -> bool {
        match self {
            InvalidationScope::All => true,
            InvalidationScope::Document(target) => document == target,
            InvalidationScope::Subtree(dir) => document.directory().is_within(dir),
        }
    }
}

impl fmt::Display for InvalidationScope {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            InvalidationScope::All => f.write_str("all"),
            InvalidationScope::Document(doc) => write!(f, "doc:{doc}"),
            InvalidationScope::Subtree(dir) => write!(f, "subtree:{}", dir.as_str()),
        }
    }
}

/// An explicit semantic review request.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Invalidation {
    pub id: u64,
    pub scope: InvalidationScope,
    pub reason: Reason,
    pub created_at: Timestamp,
    /// Documents captured when the invalidation was created, sorted.
    pub targets: Vec<DocumentId>,
    /// Targets that have not yet been reviewed against it, sorted.
    pub pending: Vec<DocumentId>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StateError {
    InvalidationIdOrder {
        id: u64,
    },
    InvalidationIdBeyondCounter {
        id: u64,
        next: u64,
    },
    PendingNotTarget {
        id: u64,
        document: DocumentId,
    },
    DuplicateTarget {
        id: u64,
        document: DocumentId,
    },
    TargetOrder {
        id: u64,
        document: DocumentId,
    },
    TargetOutsideScope {
        id: u64,
        document: DocumentId,
    },
    DuplicatePending {
        id: u64,
        document: DocumentId,
    },
    PendingOrder {
        id: u64,
        document: DocumentId,
    },
    EmptyTargets {
        id: u64,
    },
    ZeroRevision {
        document: DocumentId,
    },
    ManifestDocumentMismatch {
        document: DocumentId,
        manifest_document: DocumentId,
    },
    AcknowledgedIdBeyondCounter {
        document: DocumentId,
        id: u64,
    },
    AcknowledgedIdZero {
        document: DocumentId,
    },
    AcknowledgedIdOrder {
        document: DocumentId,
        id: u64,
    },
    CounterOverflow,
    ZeroCounter {
        counter: &'static str,
    },
    RevisionBelowCounter {
        revision: u64,
        minimum: u64,
    },
}

impl fmt::Display for StateError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            StateError::InvalidationIdOrder { id } => write!(
                f,
                "invalidation ids must increase; {id} is out of order or duplicated"
            ),
            StateError::InvalidationIdBeyondCounter { id, next } => write!(
                f,
                "invalidation id {id} is not below next_invalidation_id {next}"
            ),
            StateError::PendingNotTarget { id, document } => write!(
                f,
                "invalidation {id} lists pending document {document} that is not a target"
            ),
            StateError::DuplicateTarget { id, document } => {
                write!(f, "invalidation {id} lists {document} twice")
            }
            StateError::TargetOrder { id, document } => write!(
                f,
                "invalidation {id} targets are not in canonical sorted order at {document}"
            ),
            StateError::TargetOutsideScope { id, document } => write!(
                f,
                "invalidation {id} targets {document}, which its scope does not cover"
            ),
            StateError::DuplicatePending { id, document } => {
                write!(
                    f,
                    "invalidation {id} lists pending document {document} twice"
                )
            }
            StateError::PendingOrder { id, document } => write!(
                f,
                "invalidation {id} pending documents are not in canonical sorted order at {document}"
            ),
            StateError::EmptyTargets { id } => write!(f, "invalidation {id} has no targets"),
            StateError::ZeroRevision { document } => {
                write!(f, "review record for {document} has revision 0")
            }
            StateError::ManifestDocumentMismatch {
                document,
                manifest_document,
            } => write!(
                f,
                "review record for {document} stores a manifest for {manifest_document}"
            ),
            StateError::AcknowledgedIdBeyondCounter { document, id } => write!(
                f,
                "review record for {document} acknowledges unknown invalidation {id}"
            ),
            StateError::AcknowledgedIdZero { document } => write!(
                f,
                "review record for {document} acknowledges invalidation 0; ids start at 1"
            ),
            StateError::AcknowledgedIdOrder { document, id } => write!(
                f,
                "review record for {document} acknowledged ids must increase; {id} is out of order or duplicated"
            ),
            StateError::CounterOverflow => write!(f, "a state counter cannot increase further"),
            StateError::ZeroCounter { counter } => {
                write!(f, "{counter} must start at 1; zero is impossible")
            }
            StateError::RevisionBelowCounter { revision, minimum } => write!(
                f,
                "state revision {revision} is below the minimum {minimum} implied by document revisions and invalidation ids"
            ),
        }
    }
}

impl std::error::Error for StateError {}

/// The versioned latest-review state of a project.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReviewState {
    pub revision: u64,
    pub next_invalidation_id: u64,
    pub reviews: BTreeMap<DocumentId, ReviewRecord>,
    pub invalidations: Vec<Invalidation>,
}

impl Default for ReviewState {
    fn default() -> ReviewState {
        ReviewState::empty()
    }
}

/// What acknowledgement needs beyond the current state.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AckRequest {
    pub document: DocumentId,
    /// The document review revision bound into the packet token.
    pub packet_revision: u64,
    /// The manifest carried by the packet.
    pub packet_manifest: InputManifest,
    /// The manifest recomputed from the current project under the lock.
    pub current_manifest: InputManifest,
    /// Invalidations covered by the packet: `(id, exact reason)`.
    pub covered: Vec<(u64, String)>,
    pub input_fingerprint: Hash64,
    pub token_digest: Hash64,
    pub reviewed_at: Timestamp,
    pub reviewer: ReviewerName,
    pub result: ReviewResult,
    pub note: ReviewNote,
    pub git: GitContext,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AckOutcome {
    pub document: DocumentId,
    pub revision: u64,
    pub cleared: Vec<u64>,
    /// Newer invalidations still pending for the document: `(id, reason)`.
    pub still_pending: Vec<(u64, String)>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AckError {
    SnapshotChanged(Box<ManifestDiff>),
    RevisionConflict {
        packet: u64,
        current: u64,
    },
    InvalidationNotActive {
        id: u64,
    },
    InvalidationReasonMismatch {
        id: u64,
        packet: String,
        stored: String,
    },
    Counter(StateError),
}

impl fmt::Display for AckError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            AckError::SnapshotChanged(_) => {
                write!(f, "the review inputs changed after the packet was created")
            }
            AckError::RevisionConflict { packet, current } => write!(
                f,
                "the packet was created for document revision {packet}, but the current revision is {current}"
            ),
            AckError::InvalidationNotActive { id } => {
                write!(f, "invalidation {id} is no longer active for this document")
            }
            AckError::InvalidationReasonMismatch { id, .. } => write!(
                f,
                "invalidation {id} has a different stored reason than the packet"
            ),
            AckError::Counter(err) => write!(f, "{err}"),
        }
    }
}

impl std::error::Error for AckError {}

impl ReviewState {
    pub fn empty() -> ReviewState {
        ReviewState {
            revision: 0,
            next_invalidation_id: 1,
            reviews: BTreeMap::new(),
            invalidations: Vec::new(),
        }
    }

    /// Structural validation of loaded state.
    pub fn validate(&self) -> Result<(), StateError> {
        if self.next_invalidation_id == 0 {
            return Err(StateError::ZeroCounter {
                counter: "next_invalidation_id",
            });
        }
        if self.next_invalidation_id == u64::MAX || self.revision == u64::MAX {
            return Err(StateError::CounterOverflow);
        }
        let mut last = 0;
        for invalidation in &self.invalidations {
            if invalidation.id <= last {
                return Err(StateError::InvalidationIdOrder {
                    id: invalidation.id,
                });
            }
            last = invalidation.id;
            if invalidation.id >= self.next_invalidation_id {
                return Err(StateError::InvalidationIdBeyondCounter {
                    id: invalidation.id,
                    next: self.next_invalidation_id,
                });
            }
            if invalidation.targets.is_empty() {
                return Err(StateError::EmptyTargets {
                    id: invalidation.id,
                });
            }
            // Targets and pending documents are canonical sets: strictly
            // increasing, so any duplicate (adjacent or not) or unsorted
            // entry is corruption. Every target must belong to the stored
            // scope; whether it still exists today is irrelevant.
            for pair in invalidation.targets.windows(2) {
                if pair[0] == pair[1] {
                    return Err(StateError::DuplicateTarget {
                        id: invalidation.id,
                        document: pair[0].clone(),
                    });
                }
                if pair[0] > pair[1] {
                    return Err(StateError::TargetOrder {
                        id: invalidation.id,
                        document: pair[1].clone(),
                    });
                }
            }
            for document in &invalidation.targets {
                if !invalidation.scope.covers(document) {
                    return Err(StateError::TargetOutsideScope {
                        id: invalidation.id,
                        document: document.clone(),
                    });
                }
            }
            for pair in invalidation.pending.windows(2) {
                if pair[0] == pair[1] {
                    return Err(StateError::DuplicatePending {
                        id: invalidation.id,
                        document: pair[0].clone(),
                    });
                }
                if pair[0] > pair[1] {
                    return Err(StateError::PendingOrder {
                        id: invalidation.id,
                        document: pair[1].clone(),
                    });
                }
            }
            for document in &invalidation.pending {
                if !invalidation.targets.contains(document) {
                    return Err(StateError::PendingNotTarget {
                        id: invalidation.id,
                        document: document.clone(),
                    });
                }
            }
        }
        for (document, record) in &self.reviews {
            if record.revision == 0 {
                return Err(StateError::ZeroRevision {
                    document: document.clone(),
                });
            }
            if record.revision == u64::MAX {
                return Err(StateError::CounterOverflow);
            }
            if record.revision > self.revision {
                return Err(StateError::RevisionBelowCounter {
                    revision: self.revision,
                    minimum: record.revision,
                });
            }
            if &record.manifest.document != document {
                return Err(StateError::ManifestDocumentMismatch {
                    document: document.clone(),
                    manifest_document: record.manifest.document.clone(),
                });
            }
            // Acknowledged ids form a canonical set of historical ids: each
            // is positive, below the counter, and strictly increasing. They
            // need not resolve to an active record: completed invalidations
            // leave active storage.
            let mut last_acknowledged = 0;
            for id in &record.acknowledged_invalidations {
                if *id == 0 {
                    return Err(StateError::AcknowledgedIdZero {
                        document: document.clone(),
                    });
                }
                if *id >= self.next_invalidation_id {
                    return Err(StateError::AcknowledgedIdBeyondCounter {
                        document: document.clone(),
                        id: *id,
                    });
                }
                if *id <= last_acknowledged {
                    return Err(StateError::AcknowledgedIdOrder {
                        document: document.clone(),
                        id: *id,
                    });
                }
                last_acknowledged = *id;
            }
        }
        if self.revision < self.next_invalidation_id - 1 {
            return Err(StateError::RevisionBelowCounter {
                revision: self.revision,
                minimum: self.next_invalidation_id - 1,
            });
        }
        Ok(())
    }

    /// Current review revision of a document (zero when never reviewed).
    pub fn document_revision(&self, document: &DocumentId) -> u64 {
        self.reviews.get(document).map(|r| r.revision).unwrap_or(0)
    }

    /// Active invalidations that still list the document as pending.
    pub fn active_invalidations_for(&self, document: &DocumentId) -> Vec<&Invalidation> {
        self.invalidations
            .iter()
            .filter(|inv| inv.pending.contains(document))
            .collect()
    }

    /// Record a new invalidation and return its id.
    pub fn add_invalidation(
        &mut self,
        scope: InvalidationScope,
        reason: Reason,
        mut targets: Vec<DocumentId>,
        created_at: Timestamp,
    ) -> Result<u64, StateError> {
        targets.sort();
        targets.dedup();
        let id = self.next_invalidation_id;
        let next = id.checked_add(1).ok_or(StateError::CounterOverflow)?;
        let revision = self
            .revision
            .checked_add(1)
            .ok_or(StateError::CounterOverflow)?;
        self.invalidations.push(Invalidation {
            id,
            scope,
            reason,
            created_at,
            targets: targets.clone(),
            pending: targets,
        });
        self.next_invalidation_id = next;
        self.revision = revision;
        Ok(id)
    }

    /// Apply an acknowledgement. The caller has already verified readiness,
    /// packet integrity, and the token.
    pub fn acknowledge(&mut self, request: AckRequest) -> Result<AckOutcome, AckError> {
        let diff = request.packet_manifest.diff(&request.current_manifest);
        if !diff.is_empty() {
            return Err(AckError::SnapshotChanged(Box::new(diff)));
        }
        let current_revision = self.document_revision(&request.document);
        if current_revision != request.packet_revision {
            return Err(AckError::RevisionConflict {
                packet: request.packet_revision,
                current: current_revision,
            });
        }
        for (id, reason) in &request.covered {
            let Some(inv) = self.invalidations.iter().find(|inv| inv.id == *id) else {
                return Err(AckError::InvalidationNotActive { id: *id });
            };
            if !inv.pending.contains(&request.document) {
                return Err(AckError::InvalidationNotActive { id: *id });
            }
            if inv.reason.as_str() != reason {
                return Err(AckError::InvalidationReasonMismatch {
                    id: *id,
                    packet: reason.clone(),
                    stored: inv.reason.as_str().to_string(),
                });
            }
        }
        let new_revision = current_revision
            .checked_add(1)
            .ok_or(AckError::Counter(StateError::CounterOverflow))?;
        let state_revision = self
            .revision
            .checked_add(1)
            .ok_or(AckError::Counter(StateError::CounterOverflow))?;

        let mut cleared: Vec<u64> = request.covered.iter().map(|(id, _)| *id).collect();
        cleared.sort();
        cleared.dedup();
        for inv in &mut self.invalidations {
            if cleared.contains(&inv.id) {
                inv.pending.retain(|doc| doc != &request.document);
            }
        }
        self.invalidations.retain(|inv| !inv.pending.is_empty());
        let still_pending: Vec<(u64, String)> = self
            .active_invalidations_for(&request.document)
            .iter()
            .map(|inv| (inv.id, inv.reason.as_str().to_string()))
            .collect();

        self.reviews.insert(
            request.document.clone(),
            ReviewRecord {
                revision: new_revision,
                manifest: request.current_manifest,
                input_fingerprint: request.input_fingerprint,
                token_digest: request.token_digest,
                reviewed_at: request.reviewed_at,
                reviewer: request.reviewer,
                result: request.result,
                note: request.note,
                git: request.git,
                acknowledged_invalidations: cleared.clone(),
            },
        );
        self.revision = state_revision;
        Ok(AckOutcome {
            document: request.document,
            revision: new_revision,
            cleared,
            still_pending,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::manifest::FileInput;
    use crate::path::ProjectPath;

    fn doc(p: &str) -> DocumentId {
        DocumentId::parse(p).unwrap()
    }

    fn manifest(d: &str, hash: u64) -> InputManifest {
        InputManifest::new(
            doc(d),
            Hash64(1),
            4,
            Hash64(2),
            vec![FileInput {
                path: ProjectPath::parse("a.rs").unwrap(),
                bytes: 1,
                hash: Hash64(hash),
            }],
            vec![],
        )
        .unwrap()
    }

    fn request(
        d: &str,
        revision: u64,
        packet: u64,
        current: u64,
        covered: Vec<(u64, String)>,
    ) -> AckRequest {
        AckRequest {
            document: doc(d),
            packet_revision: revision,
            packet_manifest: manifest(d, packet),
            current_manifest: manifest(d, current),
            covered,
            input_fingerprint: Hash64(9),
            token_digest: Hash64(8),
            reviewed_at: Timestamp("2026-09-07T12:00:00Z".into()),
            reviewer: ReviewerName::parse("fixture").unwrap(),
            result: ReviewResult::NoUpdate,
            note: ReviewNote::parse("The current summary describes all reviewed inputs.").unwrap(),
            git: GitContext::default(),
        }
    }

    #[test]
    fn note_rules() {
        assert!(ReviewNote::parse("done").is_err());
        assert!(ReviewNote::parse("Looks good.").is_err());
        assert!(ReviewNote::parse("twelve chars").is_err()); // 2 words
        assert!(ReviewNote::parse("a b c d e f").is_err()); // too short
        assert!(ReviewNote::parse("The summary still matches.").is_ok());
        assert!(ReviewNote::parse(&"x ".repeat(600)).is_err());
        assert!(ReviewNote::parse("The summary\tstill matches here.").is_err());
        assert!(ReviewNote::parse("The summary\nstill matches here.").is_ok());
        assert!(ReviewerName::parse("  ").is_err());
        assert!(ReviewerName::parse("agent:codex").is_ok());
    }

    #[test]
    fn acknowledgement_clears_only_covered_invalidations() {
        let mut state = ReviewState::empty();
        let id1 = state
            .add_invalidation(
                InvalidationScope::All,
                Reason::parse("Use Simplified English everywhere.").unwrap(),
                vec![doc("README.md"), doc("a/README.md")],
                Timestamp("t".into()),
            )
            .unwrap();
        let id2 = state
            .add_invalidation(
                InvalidationScope::All,
                Reason::parse("Replace outdated terminology now.").unwrap(),
                vec![doc("README.md")],
                Timestamp("t".into()),
            )
            .unwrap();
        assert_eq!((id1, id2), (1, 2));
        assert_eq!(state.revision, 2);
        let outcome = state
            .acknowledge(request(
                "README.md",
                0,
                1,
                1,
                vec![(1, "Use Simplified English everywhere.".into())],
            ))
            .unwrap();
        assert_eq!(outcome.revision, 1);
        assert_eq!(outcome.cleared, vec![1]);
        assert_eq!(
            outcome.still_pending,
            vec![(2, "Replace outdated terminology now.".to_string())]
        );
        assert_eq!(state.invalidations.len(), 2);
        assert_eq!(state.invalidations[0].pending, vec![doc("a/README.md")]);
        assert_eq!(state.revision, 3);
        assert!(state.validate().is_ok());
    }

    #[test]
    fn changed_inputs_reject_and_clear_nothing() {
        let mut state = ReviewState::empty();
        state
            .add_invalidation(
                InvalidationScope::All,
                Reason::parse("Use Simplified English everywhere.").unwrap(),
                vec![doc("README.md")],
                Timestamp("t".into()),
            )
            .unwrap();
        let before = state.clone();
        let err = state
            .acknowledge(request(
                "README.md",
                0,
                1,
                2,
                vec![(1, "Use Simplified English everywhere.".into())],
            ))
            .unwrap_err();
        assert!(matches!(err, AckError::SnapshotChanged(_)));
        assert_eq!(state, before);
    }

    #[test]
    fn replay_is_a_revision_conflict() {
        let mut state = ReviewState::empty();
        state
            .acknowledge(request("README.md", 0, 1, 1, vec![]))
            .unwrap();
        let err = state
            .acknowledge(request("README.md", 0, 1, 1, vec![]))
            .unwrap_err();
        assert_eq!(
            err,
            AckError::RevisionConflict {
                packet: 0,
                current: 1
            }
        );
        // Another document does not conflict.
        state
            .acknowledge(request("a/README.md", 0, 1, 1, vec![]))
            .unwrap();
        assert_eq!(state.revision, 2);
    }

    #[test]
    fn reason_mismatch_and_inactive_ids_are_rejected() {
        let mut state = ReviewState::empty();
        state
            .add_invalidation(
                InvalidationScope::All,
                Reason::parse("Use Simplified English everywhere.").unwrap(),
                vec![doc("README.md")],
                Timestamp("t".into()),
            )
            .unwrap();
        let err = state
            .acknowledge(request(
                "README.md",
                0,
                1,
                1,
                vec![(1, "Different reason text here.".into())],
            ))
            .unwrap_err();
        assert!(matches!(
            err,
            AckError::InvalidationReasonMismatch { id: 1, .. }
        ));
        let err = state
            .acknowledge(request(
                "README.md",
                0,
                1,
                1,
                vec![(7, "Use Simplified English everywhere.".into())],
            ))
            .unwrap_err();
        assert_eq!(err, AckError::InvalidationNotActive { id: 7 });
    }

    #[test]
    fn completed_invalidations_are_removed() {
        let mut state = ReviewState::empty();
        state
            .add_invalidation(
                InvalidationScope::Document(doc("README.md")),
                Reason::parse("Use Simplified English everywhere.").unwrap(),
                vec![doc("README.md")],
                Timestamp("t".into()),
            )
            .unwrap();
        state
            .acknowledge(request(
                "README.md",
                0,
                1,
                1,
                vec![(1, "Use Simplified English everywhere.".into())],
            ))
            .unwrap();
        assert!(state.invalidations.is_empty());
        assert_eq!(
            state.reviews[&doc("README.md")].acknowledged_invalidations,
            vec![1]
        );
        assert!(state.validate().is_ok());
    }

    #[test]
    fn validation_rejects_impossible_counters() {
        let mut state = ReviewState::empty();
        state.next_invalidation_id = 0;
        assert!(matches!(
            state.validate(),
            Err(StateError::ZeroCounter { .. })
        ));
        let mut state = ReviewState::empty();
        state.next_invalidation_id = u64::MAX;
        assert!(matches!(state.validate(), Err(StateError::CounterOverflow)));
        let mut state = ReviewState::empty();
        state.revision = u64::MAX;
        assert!(matches!(state.validate(), Err(StateError::CounterOverflow)));
        let mut state = ReviewState::empty();
        state.next_invalidation_id = 3;
        assert!(matches!(
            state.validate(),
            Err(StateError::RevisionBelowCounter {
                revision: 0,
                minimum: 2
            })
        ));
        let mut state = ReviewState::empty();
        state
            .acknowledge(request("README.md", 0, 1, 1, vec![]))
            .unwrap();
        assert!(state.validate().is_ok());
        state.revision = 0;
        assert!(matches!(
            state.validate(),
            Err(StateError::RevisionBelowCounter { .. })
        ));
    }

    #[test]
    fn validation_rejects_inconsistent_state() {
        let mut state = ReviewState::empty();
        state.invalidations.push(Invalidation {
            id: 1,
            scope: InvalidationScope::All,
            reason: Reason::from_stored("x".into()),
            created_at: Timestamp("t".into()),
            targets: vec![doc("README.md")],
            pending: vec![doc("a/README.md")],
        });
        assert!(matches!(
            state.validate(),
            Err(StateError::InvalidationIdBeyondCounter { .. })
        ));
        state.next_invalidation_id = 2;
        assert!(matches!(
            state.validate(),
            Err(StateError::PendingNotTarget { .. })
        ));
    }

    // MEM-045: references and sets behind valid counters.
    fn state_with(scope: InvalidationScope, targets: &[&str], pending: &[&str]) -> ReviewState {
        let mut state = ReviewState::empty();
        state.next_invalidation_id = 2;
        state.revision = 1;
        state.invalidations.push(Invalidation {
            id: 1,
            scope,
            reason: Reason::from_stored("x".into()),
            created_at: Timestamp("t".into()),
            targets: targets.iter().map(|d| doc(d)).collect(),
            pending: pending.iter().map(|d| doc(d)).collect(),
        });
        state
    }

    #[test]
    fn validation_requires_canonical_sets_and_scope_membership() {
        let all = || InvalidationScope::All;
        assert!(
            state_with(all(), &["README.md", "a/README.md"], &["a/README.md"])
                .validate()
                .is_ok()
        );
        assert!(matches!(
            state_with(all(), &["README.md", "a/README.md", "README.md"], &[]).validate(),
            Err(StateError::TargetOrder { .. }) | Err(StateError::DuplicateTarget { .. })
        ));
        assert!(matches!(
            state_with(all(), &["README.md", "b/README.md", "a/README.md"], &[]).validate(),
            Err(StateError::TargetOrder { .. })
        ));
        assert!(matches!(
            state_with(
                all(),
                &["README.md", "a/README.md"],
                &["README.md", "README.md"]
            )
            .validate(),
            Err(StateError::DuplicatePending { .. })
        ));
        assert!(matches!(
            state_with(
                all(),
                &["README.md", "a/README.md"],
                &["a/README.md", "README.md"]
            )
            .validate(),
            Err(StateError::PendingOrder { .. })
        ));
        assert!(matches!(
            state_with(
                InvalidationScope::Document(doc("a/README.md")),
                &["README.md", "a/README.md"],
                &[]
            )
            .validate(),
            Err(StateError::TargetOutsideScope { .. })
        ));
        assert!(
            state_with(
                InvalidationScope::Document(doc("a/README.md")),
                &["a/README.md"],
                &["a/README.md"]
            )
            .validate()
            .is_ok()
        );
        let subtree = || InvalidationScope::Subtree(DirPath::parse("a").unwrap());
        assert!(matches!(
            state_with(subtree(), &["a/README.md", "b/README.md"], &[]).validate(),
            Err(StateError::TargetOutsideScope { .. })
        ));
        assert!(
            state_with(
                subtree(),
                &["a/README.md", "a/b/README.md"],
                &["a/b/README.md"]
            )
            .validate()
            .is_ok()
        );
        assert!(InvalidationScope::All.covers(&doc("x/README.md")));
        assert!(subtree().covers(&doc("a/README.md")));
        assert!(!subtree().covers(&doc("ab/README.md")));
    }

    #[test]
    fn validation_requires_canonical_acknowledged_ids() {
        let mut state = ReviewState::empty();
        state
            .acknowledge(request("README.md", 0, 1, 1, vec![]))
            .unwrap();
        state.next_invalidation_id = 4;
        state.revision = 3;
        let set = |state: &mut ReviewState, ids: &[u64]| {
            state
                .reviews
                .get_mut(&doc("README.md"))
                .unwrap()
                .acknowledged_invalidations = ids.to_vec();
        };
        // Completed invalidations leave active storage: ids need no record.
        set(&mut state, &[1, 3]);
        assert!(state.validate().is_ok());
        set(&mut state, &[0]);
        assert!(matches!(
            state.validate(),
            Err(StateError::AcknowledgedIdZero { .. })
        ));
        set(&mut state, &[2, 1]);
        assert!(matches!(
            state.validate(),
            Err(StateError::AcknowledgedIdOrder { id: 1, .. })
        ));
        set(&mut state, &[1, 1]);
        assert!(matches!(
            state.validate(),
            Err(StateError::AcknowledgedIdOrder { id: 1, .. })
        ));
        set(&mut state, &[4]);
        assert!(matches!(
            state.validate(),
            Err(StateError::AcknowledgedIdBeyondCounter { id: 4, .. })
        ));
    }
}
