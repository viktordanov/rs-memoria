//! Pure domain model for Memoria.
//!
//! This crate depends only on the standard library. It contains validated
//! identities, authored relationships, selection rules, document scopes
//! with their explicit handoffs, the import and navigation graphs, immutable input manifests with their
//! canonical byte encoding, review state transitions, and the scheduling
//! rules that decide why a document needs review and whether it is ready.
//!
//! Nothing in this crate opens files, runs Git, parses TOML or Markdown,
//! hashes bytes, reads the clock, or serializes JSON. Those concerns belong
//! to the application ports and infrastructure adapters.

pub mod canonical;
pub mod document;
pub mod glob;
pub mod graph;
pub mod guidance;
pub mod manifest;
pub mod path;
pub mod policy;
pub mod review;
pub mod schedule;
pub mod scope;
pub mod section;
pub mod selection;
pub mod text;

pub use canonical::{ConsumerEdge, ImportEdge, ProviderDescriptor, ReviewContext};
pub use document::{ByteRange, Document, DocumentLink, Export, ExportId, Import, SourceLocation};
pub use glob::{Glob, GlobError};
pub use graph::{GraphError, ImportGraph, NavigationGraph};
pub use guidance::{GuidanceDigest, GuidanceEntry, GuidanceKind, GuidanceSection};
pub use manifest::{
    FileInput, Hash64, ImportInput, InputChange, InputManifest, InvalidHash, ManifestDiff,
};
pub use path::{DirPath, DocumentId, DocumentKind, PathError, ProjectPath};
pub use policy::{EffectivePolicy, GitRuleScope, PolicyRuleScope};
pub use review::{
    AckError, AckOutcome, AckRequest, CoverageEvidence, GitContext, Invalidation,
    InvalidationScope, Reason, ReviewNote, ReviewRecord, ReviewResult, ReviewState, ReviewerName,
    StateError, TextError, Timestamp,
};
pub use schedule::{DocumentStatus, PendingCause, schedule};
pub use scope::{DocumentReference, Handoff, HandoffVia, ReferenceKind, ScopeMap};
pub use section::{
    GuidancePathError, InvalidSectionId, SectionId, SectionMap, SectionMapIdentity, SectionMapping,
    SectionPathError,
};
pub use selection::{Exclusion, RuleKind, RuleScope, SelectionDecision, SelectionStep};
