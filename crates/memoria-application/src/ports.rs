//! Inward-facing ports implemented by infrastructure adapters.

use std::fmt;

use memoria_domain::{DocumentId, Export, Hash64, ReviewState, SourceLocation, Timestamp};

use crate::config::{RootConfig, SidecarConfig};
use crate::packet::FocusedReviewPacket;

/// An adapter failure that keeps operation, path, and source text.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AdapterError {
    pub operation: String,
    pub path: Option<String>,
    pub message: String,
}

impl AdapterError {
    pub fn new(operation: &str, path: Option<String>, message: impl Into<String>) -> AdapterError {
        AdapterError {
            operation: operation.to_string(),
            path,
            message: message.into(),
        }
    }
}

impl fmt::Display for AdapterError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match &self.path {
            Some(path) => write!(f, "{} failed for {path}: {}", self.operation, self.message),
            None => write!(f, "{} failed: {}", self.operation, self.message),
        }
    }
}

impl std::error::Error for AdapterError {}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FileKind {
    Regular,
    Directory,
    Symlink,
    Other,
    Missing,
}

/// Bounded reads of files inside the project root.
pub trait ProjectFiles {
    /// The kind of the path without following symlinks.
    fn kind(&self, path: &str) -> Result<FileKind, AdapterError>;
    /// Read a regular file completely.
    fn read(&self, path: &str) -> Result<Vec<u8>, AdapterError>;
    /// Whether a directory contains a `.git` entry (nested repository).
    fn has_git_entry(&self, directory: &str) -> bool;
    /// Names of the immediate subdirectories of a root-relative directory,
    /// excluding symlinks, in sorted order.
    fn subdirectories(&self, directory: &str) -> Result<Vec<String>, AdapterError>;
    /// The absolute root path for display.
    fn root_display(&self) -> String;
}

/// Facts from the Git worktree.
pub trait GitRepository {
    /// Tracked paths plus untracked, non-ignored paths, as raw bytes.
    fn eligible_paths(&self) -> Result<Vec<Vec<u8>>, AdapterError>;
    /// Paths with unmerged index entries.
    fn unmerged_paths(&self) -> Result<Vec<String>, AdapterError>;
    fn sparse_checkout_enabled(&self) -> Result<bool, AdapterError>;
    /// Contents of the global excludes file, when configured and present.
    fn global_excludes(&self) -> Result<Option<Vec<u8>>, AdapterError>;
    /// Contents of `<gitdir>/info/exclude`, when present.
    fn repository_excludes(&self) -> Result<Option<Vec<u8>>, AdapterError>;
    fn head_commit(&self) -> Result<Option<String>, AdapterError>;
    fn worktree_dirty(&self) -> Result<bool, AdapterError>;
    /// Bytes of `path` at `commit`, or `None` when unavailable.
    fn read_blob(&self, commit: &str, path: &str) -> Result<Option<Vec<u8>>, AdapterError>;
    /// Why Git ignores a path, when it does.
    fn explain_ignore(&self, path: &str) -> Result<Option<String>, AdapterError>;
    /// The subset of the given root-relative directories that Git's ignore
    /// rules exclude as directories (Git never descends into them).
    fn ignored_directories(&self, directories: &[String]) -> Result<Vec<String>, AdapterError>;
}

/// Strict configuration parsing.
pub trait ConfigurationReader {
    fn parse_root(&self, bytes: &[u8]) -> Result<RootConfig, String>;
    fn parse_sidecar(&self, bytes: &[u8]) -> Result<SidecarConfig, String>;
}

/// A parsed import marker before reference resolution.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParsedImport {
    pub source_text: String,
    pub body: memoria_domain::ByteRange,
    pub location: SourceLocation,
}

/// A Markdown problem with a location.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MarkdownIssue {
    pub code: &'static str,
    pub message: String,
    pub location: Option<SourceLocation>,
}

/// Declarations and links found in a README.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ParsedDocument {
    pub exports: Vec<Export>,
    pub imports: Vec<ParsedImport>,
    /// Raw link destinations outside export bodies and code.
    pub links: Vec<String>,
    /// Validation issues. Errors make the document invalid.
    pub issues: Vec<MarkdownIssue>,
}

/// Markdown parsing with exact byte offsets.
pub trait MarkdownCodec {
    fn parse(&self, document: &DocumentId, bytes: &[u8]) -> ParsedDocument;
}

/// Streaming hash accumulator.
pub trait HashStream {
    fn update(&mut self, bytes: &[u8]);
    fn finish(self: Box<Self>) -> Hash64;
}

/// xxHash64 with seed zero.
pub trait FingerprintHasher {
    fn hash(&self, bytes: &[u8]) -> Hash64;
    fn stream(&self) -> Box<dyn HashStream>;
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LoadedState {
    pub bytes: Vec<u8>,
    pub state: ReviewState,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StateFailure {
    /// The file is unreadable or the directory is inaccessible.
    Io(AdapterError),
    /// The file exists but is malformed or uses an unsupported schema.
    Corrupt(String),
    /// The stored bytes differ from the expected bytes.
    Conflict,
}

/// Versioned state persistence with compare-and-swap.
pub trait StateStore {
    /// Load state; `None` when the file does not exist.
    fn load(&self) -> Result<Option<LoadedState>, StateFailure>;
    /// Encode state canonically without writing it.
    fn encode(&self, state: &ReviewState) -> Vec<u8>;
    /// Write state atomically when the stored bytes equal `expected`
    /// (`None` means the file must not exist). Returns the written bytes.
    fn save(&self, state: &ReviewState, expected: Option<&[u8]>) -> Result<Vec<u8>, StateFailure>;
}

pub trait Clock {
    fn now(&self) -> Timestamp;
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LockFailure {
    Busy,
    Io(AdapterError),
}

/// Held while a mutation runs. Dropping releases the lock.
pub trait WriteGuard {}

/// Exclusive advisory lock for project mutations.
pub trait WriteCoordinator {
    fn lock(&self) -> Result<Box<dyn WriteGuard + '_>, LockFailure>;
}

/// Durable, compare-then-replace file writes inside the project.
pub trait AtomicWriter {
    /// Replace `path` when its current bytes equal `expected`.
    fn replace(&self, path: &str, expected: &[u8], new: &[u8]) -> Result<(), AdapterError>;
    /// Create `path` only when it does not exist.
    fn create_new(&self, path: &str, bytes: &[u8]) -> Result<(), AdapterError>;
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PacketSource {
    File(String),
    Stdin,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PacketFailure {
    /// Transport failure: unreadable file, stdin error.
    Io(AdapterError),
    /// Schema, integrity, or limit failure with a stable code.
    Invalid { code: &'static str, message: String },
}

/// Bounded packet transport.
pub trait PacketInput {
    fn read(&self, source: &PacketSource) -> Result<Vec<u8>, PacketFailure>;
}

/// Strict envelope encoding and decoding for focused review packets.
pub trait ReviewPacketCodec {
    /// Encode the successful `review` envelope, computing `packet_digest`.
    fn encode(
        &self,
        packet: &FocusedReviewPacket,
        diagnostics: &[crate::error::Diagnostic],
    ) -> Result<Vec<u8>, PacketFailure>;
    /// Decode and validate schema, limits, and `packet_digest`.
    fn decode(&self, bytes: &[u8]) -> Result<FocusedReviewPacket, PacketFailure>;
    /// The complete-envelope record count (every array element across data
    /// and diagnostics) that `encode` would publish for this packet.
    fn complete_record_count(
        &self,
        packet: &FocusedReviewPacket,
        diagnostics: &[crate::error::Diagnostic],
    ) -> u64;
    /// Measure the complete generic envelope (`command`, `ok`, `data`,
    /// `diagnostics`) exactly as the JSON presentation would emit it, so a
    /// refusal can be bounded before any output exists.
    fn envelope_size(
        &self,
        command: &str,
        ok: bool,
        data: &crate::error::Detail,
        diagnostics: &[crate::error::Diagnostic],
    ) -> crate::packet::EnvelopeSize;
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AgentTarget {
    Codex,
    Claude,
}

impl AgentTarget {
    pub fn as_str(self) -> &'static str {
        match self {
            AgentTarget::Codex => "codex",
            AgentTarget::Claude => "claude",
        }
    }

    pub fn parse(raw: &str) -> Option<AgentTarget> {
        match raw {
            "codex" => Some(AgentTarget::Codex),
            "claude" => Some(AgentTarget::Claude),
            _ => None,
        }
    }

    /// Project-relative default skills parent.
    pub fn default_parent(self) -> &'static str {
        match self {
            AgentTarget::Codex => ".agents/skills",
            AgentTarget::Claude => ".claude/skills",
        }
    }

    /// Project-relative agent directory used for detection.
    pub fn detection_dir(self) -> &'static str {
        match self {
            AgentTarget::Codex => ".agents",
            AgentTarget::Claude => ".claude",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SkillOperation {
    Install,
    Uninstall,
}

/// A planned managed-package change.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SkillPlan {
    pub operation: SkillOperation,
    pub target: AgentTarget,
    /// Absolute destination package directory.
    pub destination: String,
    /// Absolute backup directory used or restored, when any.
    pub backup: Option<String>,
    /// Files created or replaced, relative to the destination.
    pub writes: Vec<String>,
    /// Files removed, relative to the destination.
    pub removals: Vec<String>,
    /// Files replaced whose previous content is backed up.
    pub replaced: Vec<String>,
    /// Nothing needs to change.
    pub no_change: bool,
    /// An interrupted transaction must be recovered first.
    pub recovery_needed: bool,
    /// Human-readable status of the existing destination.
    pub existing: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SkillFailure {
    /// User edits or unknown files prevent a safe change.
    Conflict {
        message: String,
        paths: Vec<String>,
    },
    Io(AdapterError),
    NotInstalled(String),
}

/// Managed skill package transactions.
pub trait SkillPackageStore {
    /// Whether the project-relative directory exists.
    fn directory_exists(&self, relative: &str) -> bool;
    /// Whether the project-relative directory holds a package with a valid
    /// Memoria installation record.
    fn is_managed_package(&self, relative: &str) -> bool;
    fn plan_install(&self, target: AgentTarget, parent: &str) -> Result<SkillPlan, SkillFailure>;
    fn apply_install(&self, plan: &SkillPlan) -> Result<(), SkillFailure>;
    fn plan_uninstall(&self, target: AgentTarget, parent: &str) -> Result<SkillPlan, SkillFailure>;
    fn apply_uninstall(&self, plan: &SkillPlan) -> Result<(), SkillFailure>;
}

/// Progress messages shown before writes.
pub trait Progress {
    fn note(&self, message: &str);
}

/// Every port a use case can need.
pub struct Services<'a> {
    pub files: &'a dyn ProjectFiles,
    pub git: &'a dyn GitRepository,
    pub config: &'a dyn ConfigurationReader,
    pub markdown: &'a dyn MarkdownCodec,
    pub hasher: &'a dyn FingerprintHasher,
    pub state: &'a dyn StateStore,
    pub clock: &'a dyn Clock,
    pub locks: &'a dyn WriteCoordinator,
    pub writer: &'a dyn AtomicWriter,
    pub packets: &'a dyn ReviewPacketCodec,
    pub packet_input: &'a dyn PacketInput,
    pub skills: &'a dyn SkillPackageStore,
    pub progress: &'a dyn Progress,
}
