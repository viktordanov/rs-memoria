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
    fn head_commit(&self) -> Result<Option<String>, AdapterError>;
    fn worktree_dirty(&self) -> Result<bool, AdapterError>;
    /// Bytes of `path` at `commit`, or `None` when unavailable.
    fn read_blob(&self, commit: &str, path: &str) -> Result<Option<Vec<u8>>, AdapterError>;
    /// Bounded raw historical read. No filters, replacement objects, or network fetches.
    fn historical_blob(
        &self,
        commit: &str,
        path: &str,
        limit: u64,
        deadline: std::time::Instant,
    ) -> Result<Option<Vec<u8>>, AdapterError> {
        if std::time::Instant::now() >= deadline {
            return Err(AdapterError::new(
                "history_limit",
                None,
                "Historical deadline exhausted.",
            ));
        }
        let bytes = self.read_blob(commit, path)?;
        if bytes.as_ref().is_some_and(|b| b.len() as u64 > limit) {
            return Err(AdapterError::new(
                "history_limit",
                None,
                "Historical byte budget exhausted.",
            ));
        }
        Ok(bytes)
    }
    /// Local HEAD ancestry only. The caller requests one extra item to detect exhaustion.
    fn recent_commits(
        &self,
        _limit: usize,
        _deadline: std::time::Instant,
    ) -> Result<Vec<String>, AdapterError> {
        Ok(vec![])
    }
    /// Why Git ignores a path, when it does. Reporting only: this answer
    /// includes host rules and never enters deterministic policy.
    fn explain_ignore(&self, path: &str) -> Result<Option<String>, AdapterError>;
    /// The absolute per-worktree Git metadata directory. Linked worktrees
    /// have their own. This is the containment boundary for every private
    /// path Memoria creates.
    fn git_dir(&self) -> Result<String, AdapterError>;
    /// The worktree-private path for `relative` under Git metadata.
    ///
    /// The path is built from [`GitRepository::git_dir`] rather than from
    /// `git rev-parse --git-path`, which resolves symlinked components and
    /// would hand back a location outside the metadata directory.
    fn private_path(&self, relative: &str) -> Result<String, AdapterError>;
    /// The absolute main-checkout worktree root of this repository family.
    /// A linked worktree resolves to the checkout that owns the common
    /// Git directory.
    fn main_worktree(&self) -> Result<String, AdapterError>;
}

/// One repository ignore source: a project-relative `.gitignore` path and
/// its exact bytes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IgnoreScope {
    /// Project-relative path, for example `.gitignore` or `src/.gitignore`.
    pub path: String,
    pub bytes: Vec<u8>,
}

/// Repository-only ignore matching for the deterministic policy inventory.
///
/// This port answers with repository rule bytes alone. It reads no host
/// configuration, no `core.excludesFile`, no XDG fallback, and no
/// `.git/info/exclude`, so an irrelevant host rule cannot hide a nested
/// `.gitignore` and change a project's policy hash.
pub trait RepositoryIgnoreMatcher {
    /// The subset of `directories` that `scopes` exclude as directories.
    /// Paths are project-relative without a trailing slash. The adapter
    /// performs no filesystem or Git access.
    fn ignored_directories(
        &self,
        scopes: &[IgnoreScope],
        directories: &[String],
    ) -> Result<Vec<String>, AdapterError>;
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

/// XXH3-64 with the default secret and seed zero.
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
    /// The file exists but is malformed, truncated, or fails its checksum.
    Corrupt(String),
    /// The stored bytes differ from the expected bytes.
    Conflict,
    /// Only the version 1 `.memoria/state.json` exists. The release has one
    /// clean cutover and no automatic migration.
    Legacy(String),
    /// Both the legacy state file and `memoria.lock` exist.
    Ambiguous(String),
    /// A read, payload, or expansion limit would be exceeded.
    LimitExceeded(String),
    /// The format version is outside this release's support.
    UnsupportedSchema(String),
    /// The codec identifier is outside this release's support.
    UnsupportedCodec(String),
    /// Inspection was asked for a file that does not exist.
    Missing(String),
}

impl StateFailure {
    /// The stable diagnostic code for this failure.
    pub fn code(&self) -> &'static str {
        match self {
            StateFailure::Io(_) => "state_unreadable",
            StateFailure::Corrupt(_) => "state_corrupt",
            StateFailure::Conflict => "state_conflict",
            StateFailure::Legacy(_) => "state_legacy",
            StateFailure::Ambiguous(_) => "state_ambiguous",
            StateFailure::LimitExceeded(_) => "state_limit_exceeded",
            StateFailure::UnsupportedSchema(_) => "state_unsupported_schema",
            StateFailure::UnsupportedCodec(_) => "state_unsupported_codec",
            StateFailure::Missing(_) => "state_missing",
        }
    }

    pub fn message(&self) -> String {
        match self {
            StateFailure::Io(err) => err.to_string(),
            StateFailure::Conflict => "state changed while loading".to_string(),
            StateFailure::Corrupt(m)
            | StateFailure::Legacy(m)
            | StateFailure::Ambiguous(m)
            | StateFailure::LimitExceeded(m)
            | StateFailure::UnsupportedSchema(m)
            | StateFailure::UnsupportedCodec(m)
            | StateFailure::Missing(m) => m.clone(),
        }
    }
}

/// Versioned state persistence with compare-and-swap.
pub trait StateStore {
    /// Load state; `None` when the file does not exist.
    fn load(&self) -> Result<Option<LoadedState>, StateFailure>;
    /// Encode state canonically without writing it.
    fn encode(&self, state: &ReviewState) -> Result<Vec<u8>, StateFailure>;
    /// Write state atomically when the stored bytes equal `expected`
    /// (`None` means the file must not exist). Returns the written bytes.
    fn save(&self, state: &ReviewState, expected: Option<&[u8]>) -> Result<Vec<u8>, StateFailure>;
}

/// Framing facts and typed state from one read-only inspection.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InspectedState {
    /// Original bounded frame, for exact file-to-file byte comparison only.
    pub encoded: Vec<u8>,
    /// The inspected path, as the caller named or the project resolved it.
    pub path: String,
    pub file_bytes: u64,
    pub payload_bytes: u64,
    pub format_version: u64,
    /// `raw` or `zstd-v1`.
    pub codec: &'static str,
    /// The XXH3-128 frame checksum, 32 lowercase hexadecimal characters.
    pub checksum: String,
    pub state: ReviewState,
    /// Guidance digest recorded with each review, by document path.
    pub guidance: Vec<(String, String)>,
}

/// Bounded, read-only decoding of a committed state artifact.
pub trait StateInspector {
    /// Inspect `memoria.lock` in the selected project.
    fn inspect_project(&self) -> Result<InspectedState, StateFailure>;
    /// Inspect an explicit path resolved against the invocation directory.
    /// This mode works outside Git and needs no configuration.
    fn inspect_file(&self, path: &str) -> Result<InspectedState, StateFailure>;
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
    Status,
    Upgrade,
    Uninstall,
}

impl SkillOperation {
    pub fn as_str(self) -> &'static str {
        match self {
            SkillOperation::Install => "install",
            SkillOperation::Status => "status",
            SkillOperation::Upgrade => "upgrade",
            SkillOperation::Uninstall => "uninstall",
        }
    }
}

/// A file the lifecycle leaves in place, with the reason it stays.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RetainedArtifact {
    pub path: String,
    /// `synchronization_lock`, `user_backup`, or `unknown_content`.
    pub reason: &'static str,
    pub removable_by_uninstall: bool,
}

/// Another package with the same name, found in a different scope.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OverlappingPackage {
    /// `local`, `global`, or `legacy`.
    pub scope: String,
    pub destination: String,
    pub state: String,
    /// How the client resolves the overlap.
    pub note: String,
}

/// What one lifecycle operation would do, or what `status` observed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SkillPlan {
    pub operation: SkillOperation,
    pub target: AgentTarget,
    pub scope: AgentScope,
    /// Absolute destination package directory.
    pub destination: String,
    /// `absent`, `current`, `outdated`, `modified`, `unmanaged`, or
    /// `conflict`.
    pub state: String,
    /// The installed package version, when a record exists.
    pub package_version: Option<String>,
    /// The version this executable would install.
    pub embedded_version: String,
    /// Absolute backup directory used or restored, when any.
    pub backup: Option<String>,
    /// Files created or replaced, relative to the destination.
    pub writes: Vec<String>,
    /// Files removed, relative to the destination.
    pub removals: Vec<String>,
    /// Files replaced whose previous content is backed up.
    pub replaced: Vec<String>,
    /// Managed files whose bytes changed after installation.
    pub modified_paths: Vec<String>,
    /// Files inside the package that no record lists.
    pub unknown_paths: Vec<String>,
    /// Files the operation deliberately leaves in place.
    pub retained_artifacts: Vec<RetainedArtifact>,
    /// Same-name packages discovered in other scopes.
    pub overlapping: Vec<OverlappingPackage>,
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
    /// `upgrade` or an explicit removal found no managed package.
    NotInstalled(String),
    /// `install` found an older managed package. Upgrading is explicit.
    UpgradeRequired(String),
    /// `install` found unmanaged content and no `--replace-existing`.
    ReplacementRequired(String),
}

/// One lifecycle request against a resolved destination.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SkillRequest {
    pub operation: SkillOperation,
    pub target: AgentTarget,
    pub scope: AgentScope,
    /// Absolute skills parent directory. The package sits inside it.
    pub parent: String,
    /// Permit replacing an unmanaged package after a verified backup.
    pub replace_existing: bool,
    /// Other absolute parents to report as overlapping installations,
    /// as `(scope label, absolute parent)`.
    pub other_parents: Vec<(String, String)>,
}

/// Managed skill package transactions.
pub trait SkillPackageStore {
    /// Whether the project-relative directory exists.
    fn directory_exists(&self, relative: &str) -> bool;
    /// Whether the project-relative directory holds a package with a valid
    /// Memoria installation record.
    fn is_managed_package(&self, relative: &str) -> bool;
    /// Inspect the destination and describe what the operation would do.
    /// This never writes and never creates a directory or lock.
    fn plan(&self, request: &SkillRequest) -> Result<SkillPlan, SkillFailure>;
    /// Apply a plan that `plan` produced for the same request.
    fn apply(&self, request: &SkillRequest, plan: &SkillPlan) -> Result<(), SkillFailure>;
}

/// Where a managed skill package is installed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AgentScope {
    Local,
    Global,
}

impl AgentScope {
    pub fn as_str(self) -> &'static str {
        match self {
            AgentScope::Local => "local",
            AgentScope::Global => "global",
        }
    }
}

/// A destination that cannot be resolved, with an actionable message.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LocationError {
    pub code: &'static str,
    pub message: String,
}

/// Absolute skill destinations for one target and scope.
pub trait AgentLocations {
    /// The absolute skills parent directory.
    fn skills_parent(
        &self,
        target: AgentTarget,
        scope: AgentScope,
    ) -> Result<String, LocationError>;
    /// A recognized legacy user location, for reporting only.
    fn legacy_parent(&self, target: AgentTarget) -> Option<String>;
    /// Resolve an explicit `--path` for the selected scope.
    fn resolve_custom(&self, raw: &str, scope: AgentScope) -> Result<String, LocationError>;
    /// The selected worktree root, when the command runs inside one.
    fn worktree_root(&self) -> Option<String>;
}

/// A native `Stop` hook that Memoria owns in a client configuration.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HookPlan {
    pub target: AgentTarget,
    /// Absolute configuration file that receives the owned group.
    pub configuration: String,
    /// Absolute ownership record path.
    pub record: String,
    /// `absent`, `installed`, `modified`, `unmanaged`, `ambiguous`, or
    /// `conflict`.
    pub state: String,
    /// Every planned write, removal, and retained artifact.
    pub writes: Vec<String>,
    pub removals: Vec<String>,
    pub no_change: bool,
    pub recovery_needed: bool,
    /// `requires-client-review` until the client itself approves the hook.
    pub activation: String,
    /// The exact shell command the owned handler runs.
    pub command: String,
    /// A digest of the client configuration's bytes when the plan was made.
    /// Application compares it again before it writes, so a concurrent edit
    /// is a conflict rather than a silent overwrite. `None` means the file
    /// did not exist.
    pub expected_digest: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HookFailure {
    Conflict { code: &'static str, message: String },
    Unsupported { code: &'static str, message: String },
    Io(AdapterError),
}

/// Reversible, project-level native hook configuration.
pub trait HookStore {
    fn plan_install(&self, target: AgentTarget) -> Result<HookPlan, HookFailure>;
    fn apply_install(&self, plan: &HookPlan) -> Result<(), HookFailure>;
    fn status(&self, target: AgentTarget) -> Result<HookPlan, HookFailure>;
    fn plan_uninstall(&self, target: AgentTarget) -> Result<HookPlan, HookFailure>;
    fn apply_uninstall(&self, plan: &HookPlan) -> Result<(), HookFailure>;
}

/// The result of one bounded child process.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BoundedOutput {
    pub exit_code: Option<i32>,
    pub stdout: Vec<u8>,
    /// The deadline elapsed before the child finished.
    pub timed_out: bool,
    /// The child produced more stdout than the caller allowed.
    pub output_truncated: bool,
}

/// Run this executable's own bounded status inspection without a shell.
pub trait BoundedStatusProcess {
    /// Invoke `memoria --root <root> status --summary --format json` with a
    /// deadline in milliseconds and a stdout byte limit.
    fn run_summary(
        &self,
        root: &str,
        deadline_ms: u64,
        stdout_limit: u64,
    ) -> Result<BoundedOutput, AdapterError>;
}

/// Progress messages shown before writes.
pub trait Progress {
    fn note(&self, message: &str);
}

/// Every port a use case can need.
pub struct Services<'a> {
    pub files: &'a dyn ProjectFiles,
    pub git: &'a dyn GitRepository,
    pub ignore: &'a dyn RepositoryIgnoreMatcher,
    pub config: &'a dyn ConfigurationReader,
    pub markdown: &'a dyn MarkdownCodec,
    pub hasher: &'a dyn FingerprintHasher,
    pub state: &'a dyn StateStore,
    pub inspector: &'a dyn StateInspector,
    pub clock: &'a dyn Clock,
    pub locks: &'a dyn WriteCoordinator,
    pub writer: &'a dyn AtomicWriter,
    pub packets: &'a dyn ReviewPacketCodec,
    pub packet_input: &'a dyn PacketInput,
    pub skills: &'a dyn SkillPackageStore,
    pub locations: &'a dyn AgentLocations,
    pub hooks: &'a dyn HookStore,
    pub progress: &'a dyn Progress,
}
