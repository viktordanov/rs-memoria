//! An in-process `memoria review` with a counting hasher.
//!
//! The legacy exclusion proof rebuilds a recorded token once per candidate
//! subset. Each rebuild hashes one `memoria-review-token-v3` encoding whose
//! baseline is the v1 absent baseline, which only the proof binds. A v1
//! layout rebuild also hashes one `memoria-review-context-v1` encoding.
//! Counting those two shapes counts proof recomputations exactly, with the
//! real adapters and no product instrumentation.
#![allow(dead_code)]

use std::cell::Cell;
use std::path::Path;

use memoria_application::ports::{FingerprintHasher, HashStream, Progress, Services};
use memoria_application::usecases;
use memoria_domain::Hash64;
use memoria_domain::canonical::{
    LEGACY_REVIEW_CONTEXT_V1_DOMAIN, REVIEW_TOKEN_V3_DOMAIN,
    encode_legacy_absent_review_baseline_v1,
};
use memoria_infrastructure::config::TomlConfigurationReader;
use memoria_infrastructure::fs::{
    AtomicFileWriter, FsProjectFiles, LockFileCoordinator, SystemClock,
};
use memoria_infrastructure::{
    EnvAgentLocations, FsArtifactStore, FsHookStore, FsPacketInput, FsSkillStore, FsWorkflowStore,
    GitCli, GixRepositoryIgnore, JsonPacketCodec, LockStateInspector, LockStateStore,
    PulldownMarkdownCodec, Xxh3Hasher,
};

struct Quiet;

impl Progress for Quiet {
    fn note(&self, _message: &str) {}
}

/// Proof recomputations seen by one review.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct ProofCounts {
    /// Token rebuilds that bind the v1 absent baseline (both layouts).
    pub tokens: u64,
    /// Rebuilds of the 0.6 layout (`memoria-review-context-v1`).
    pub v1_layout: u64,
}

impl ProofCounts {
    /// Rebuilds of the 0.7 handoff layout.
    pub fn v2_layout(&self) -> u64 {
        self.tokens - self.v1_layout
    }
}

struct CountingHasher {
    inner: Xxh3Hasher,
    legacy_baseline: Hash64,
    tokens: Cell<u64>,
    v1_contexts: Cell<u64>,
}

fn prefixed(domain: &str) -> Vec<u8> {
    let mut out = (domain.len() as u64).to_be_bytes().to_vec();
    out.extend_from_slice(domain.as_bytes());
    out
}

impl CountingHasher {
    fn new() -> CountingHasher {
        let inner = Xxh3Hasher;
        let legacy_baseline = inner.hash(&encode_legacy_absent_review_baseline_v1());
        CountingHasher {
            inner,
            legacy_baseline,
            tokens: Cell::new(0),
            v1_contexts: Cell::new(0),
        }
    }

    /// The baseline digest of a token encoding: after the domain, the
    /// document, the revision, and the inputs digest.
    fn token_baseline(bytes: &[u8]) -> Option<u64> {
        let domain = prefixed(REVIEW_TOKEN_V3_DOMAIN);
        let rest = bytes.strip_prefix(domain.as_slice())?;
        let length = u64::from_be_bytes(rest.get(..8)?.try_into().ok()?) as usize;
        let after_document = rest.get(8 + length..)?;
        let baseline = after_document.get(16..24)?;
        Some(u64::from_be_bytes(baseline.try_into().ok()?))
    }
}

impl FingerprintHasher for CountingHasher {
    fn hash(&self, bytes: &[u8]) -> Hash64 {
        if Self::token_baseline(bytes) == Some(self.legacy_baseline.0) {
            self.tokens.set(self.tokens.get() + 1);
        }
        if bytes.starts_with(&prefixed(LEGACY_REVIEW_CONTEXT_V1_DOMAIN)) {
            self.v1_contexts.set(self.v1_contexts.get() + 1);
        }
        self.inner.hash(bytes)
    }

    fn stream(&self) -> Box<dyn HashStream> {
        self.inner.stream()
    }
}

/// Run `memoria review <document>` in process on `root` and count the proof
/// recomputations. The review only reads; it writes nothing.
pub fn review_proof_counts(root: &Path, home: &Path, document: &str) -> ProofCounts {
    let hasher = CountingHasher::new();
    let git = GitCli::discover(root).expect("a Git repository");
    let root = git.root().to_path_buf();
    let git_dir =
        std::path::PathBuf::from(memoria_application::ports::GitRepository::git_dir(&git).unwrap());
    let files = FsProjectFiles::new(root.clone());
    let config = TomlConfigurationReader;
    let markdown = PulldownMarkdownCodec;
    let ignore = GixRepositoryIgnore;
    let state = LockStateStore::new(root.clone());
    let inspector = LockStateInspector::new(Some(root.clone()), root.clone());
    let clock = SystemClock;
    let locks = LockFileCoordinator::new(root.clone(), git_dir.join("memoria/write.lock"));
    let writer = AtomicFileWriter::new(root.clone());
    let packets = JsonPacketCodec::new(&hasher);
    let packet_input = FsPacketInput;
    let skills = FsSkillStore::new(root.clone(), &[("SKILL.md", "# skill\n")], "0.7.0");
    let locations =
        EnvAgentLocations::new(Some(root.clone()), root.clone(), Some(home.into()), None);
    let hooks = FsHookStore::new(
        root.clone(),
        root.clone(),
        root.join("memoria"),
        git_dir.clone(),
        Box::new(memoria_infrastructure::CommandClientProbe),
    );
    let workflows = FsWorkflowStore::new(root.clone(), git_dir.clone(), "0.7.0");
    let artifacts = FsArtifactStore::new(root.clone(), root.clone());
    let progress = Quiet;
    let services = Services {
        files: &files,
        git: &git,
        ignore: &ignore,
        config: &config,
        markdown: &markdown,
        hasher: &hasher,
        state: &state,
        inspector: &inspector,
        clock: &clock,
        locks: &locks,
        writer: &writer,
        packets: &packets,
        packet_input: &packet_input,
        skills: &skills,
        locations: &locations,
        hooks: &hooks,
        workflows: &workflows,
        artifacts: &artifacts,
        progress: &progress,
    };
    usecases::prepare_review::run(&services, document, None, false)
        .unwrap_or_else(|err| panic!("review {document}: {err:?}"));
    ProofCounts {
        tokens: hasher.tokens.get(),
        v1_layout: hasher.v1_contexts.get(),
    }
}
