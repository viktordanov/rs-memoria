//! Bounded, verified local evidence. Git references never establish review validity.
use super::evidence::Baseline;
use crate::{
    error::{Detail, DetailMap, Diagnostic},
    ports::Services,
};
use memoria_domain::{DocumentId, Hash64, InputManifest};
use std::time::{Duration, Instant};

const CANDIDATES: usize = 64;
const READS: usize = 1024;
const BYTES: u64 = 32 * 1024 * 1024;

pub struct History<'a, 's> {
    services: &'a Services<'s>,
    deadline: Instant,
    remaining: u64,
    reads: usize,
    candidates: Option<Vec<String>>,
    truncated: bool,
    pub exhausted: bool,
    failed: bool,
}

impl<'a, 's> History<'a, 's> {
    pub fn new(services: &'a Services<'s>) -> Self {
        Self {
            services,
            deadline: Instant::now() + Duration::from_secs(4),
            remaining: BYTES,
            reads: 0,
            candidates: None,
            truncated: false,
            exhausted: false,
            failed: false,
        }
    }

    fn candidates(&mut self) -> Vec<String> {
        if self.candidates.is_none() {
            match self
                .services
                .git
                .recent_commits(CANDIDATES + 1, self.deadline)
            {
                Ok(mut commits) => {
                    self.truncated = commits.len() > CANDIDATES;
                    commits.truncate(CANDIDATES);
                    self.candidates = Some(commits);
                }
                Err(e) => {
                    self.exhausted |= e.operation == "history_limit";
                    self.failed |= e.operation != "history_limit";
                    self.candidates = Some(vec![]);
                }
            }
        }
        self.candidates.clone().unwrap_or_default()
    }

    fn read(&mut self, commit: &str, path: &str) -> Option<Vec<u8>> {
        if self.exhausted || self.reads >= READS || Instant::now() >= self.deadline {
            self.exhausted = true;
            return None;
        }
        self.reads += 1;
        match self
            .services
            .git
            .historical_blob(commit, path, self.remaining, self.deadline)
        {
            Ok(bytes) => {
                self.remaining -= bytes.as_ref().map_or(0, |b| b.len() as u64);
                bytes
            }
            Err(e) => {
                self.exhausted |= e.operation == "history_limit";
                self.failed |= e.operation != "history_limit";
                None
            }
        }
    }

    fn at(&mut self, commit: &str, path: &str, export: Option<&str>) -> Option<Vec<u8>> {
        if let Some(export) = export {
            let document = DocumentId::parse(path).ok()?;
            let bytes = self.read(commit, path)?;
            let parsed = self.services.markdown.parse(&document, &bytes);
            if !parsed.issues.is_empty() {
                return None;
            }
            let selected = parsed.exports.iter().find(|e| e.id.as_str() == export)?;
            bytes
                .get(selected.body.start..selected.body.end)
                .map(<[u8]>::to_vec)
        } else {
            self.read(commit, path)
        }
    }

    /// Each recovered object must match the acknowledged identity, length and XXH3.
    /// Recovery never updates the original review record or attribution.
    pub fn lookup(
        &mut self,
        reference: Option<&str>,
        path: &str,
        export: Option<&str>,
        expected: (u64, Hash64),
    ) -> Baseline {
        let mut observed = None;
        if let Some(commit) = reference
            && let Some(bytes) = self.at(commit, path, export)
        {
            let actual = (bytes.len() as u64, self.services.hasher.hash(&bytes));
            observed = Some(actual);
            if actual == expected {
                return found(bytes, actual, commit);
            }
        }
        for commit in self.candidates() {
            if Some(commit.as_str()) == reference {
                continue;
            }
            if let Some(bytes) = self.at(&commit, path, export) {
                let actual = (bytes.len() as u64, self.services.hasher.hash(&bytes));
                if actual == expected {
                    return found(bytes, actual, &commit);
                }
            }
            if self.exhausted {
                break;
            }
        }
        let (code, reason) = if self.exhausted || self.truncated {
            (
                "history_limit_exceeded",
                "No matching reviewed bytes were found before the local history budget ended.",
            )
        } else if self.failed {
            (
                "git_read_failed",
                "Local Git evidence could not be read completely.",
            )
        } else if reference.is_none() {
            (
                "no_base_commit",
                "The review has no fully verified Git reference; available local history supplied no exact match.",
            )
        } else if observed.is_some() {
            (
                "reviewed_bytes_mismatch",
                "The recorded reference differs from the reviewed bytes; available local history supplied no exact match.",
            )
        } else {
            (
                "blob_unavailable",
                "The reviewed bytes are unavailable in the recorded reference and available local history.",
            )
        };
        Baseline {
            bytes: None,
            observed,
            reason_code: Some(code),
            reason: Some(reason.into()),
            commit: None,
        }
    }

    /// A single stored commit must cover all reviewed content, including imports.
    pub fn coverage(&mut self, manifest: &InputManifest, head: Option<&str>) -> Coverage {
        let mut entries = vec![(
            manifest.document.as_str().to_string(),
            None,
            manifest.document_bytes,
            manifest.document_hash,
        )];
        entries.extend(
            manifest
                .files()
                .iter()
                .map(|f| (f.path.as_str().to_string(), None, f.bytes, f.hash)),
        );
        entries.extend(manifest.imports().iter().map(|i| {
            (
                i.document.as_str().to_string(),
                Some(i.export_id.as_str().to_string()),
                i.bytes,
                i.hash,
            )
        }));
        let mut candidates = Vec::new();
        if let Some(head) = head {
            candidates.push(head.to_string());
        }
        let mut best = Coverage {
            commit: None,
            verified: 0,
            total: entries.len(),
            imports: manifest.imports().len(),
            exhausted: false,
            failed: false,
        };
        // Read HEAD before enumerating ancestry. The saved reference is concrete, never symbolic HEAD.
        let mut index = 0;
        loop {
            if index == candidates.len() {
                if index > 1 || (index == 1 && candidates.len() > 1) {
                    break;
                }
                let more = self.candidates();
                for c in more {
                    if !candidates.contains(&c) {
                        candidates.push(c);
                    }
                }
                if index == candidates.len() {
                    break;
                }
            }
            let commit = candidates[index].clone();
            let mut verified = 0;
            for (path, export, length, hash) in &entries {
                if let Some(bytes) = self.at(&commit, path, export.as_deref())
                    && bytes.len() as u64 == *length
                    && self.services.hasher.hash(&bytes) == *hash
                {
                    verified += 1;
                }
                if self.exhausted {
                    break;
                }
                // Other candidates need only prove full coverage; retain detailed HEAD counts.
                if index > 0 && verified == 0 {
                    break;
                }
            }
            best.verified = best.verified.max(verified);
            if verified == entries.len() {
                best.commit = Some(commit);
                return best;
            }
            if self.exhausted {
                break;
            }
            index += 1;
        }
        best.failed = self.failed;
        best.exhausted = self.exhausted || self.truncated;
        best
    }
}

fn found(bytes: Vec<u8>, observed: (u64, Hash64), commit: &str) -> Baseline {
    Baseline {
        bytes: Some(bytes),
        observed: Some(observed),
        reason_code: None,
        reason: None,
        commit: Some(commit.to_string()),
    }
}

pub struct Coverage {
    pub commit: Option<String>,
    pub verified: usize,
    pub total: usize,
    pub imports: usize,
    pub exhausted: bool,
    pub failed: bool,
}

impl Coverage {
    pub fn diagnostic(&self) -> Diagnostic {
        let status = if self.commit.is_some() {
            "verified"
        } else if self.verified > 0 {
            "partial"
        } else {
            "unavailable"
        };
        Diagnostic::hint("historical_coverage", format!("Historical coverage {status}: {}/{} content inputs match one inspected commit. Review validity is independent of Git history.", self.verified, self.total))
            .with_details(DetailMap::default().text("status", status)
                .with("verified_commit", Detail::option_text(self.commit.clone()))
                .number("verified_inputs", self.verified as u64).number("total_inputs", self.total as u64)
                .number("import_inputs", self.imports as u64).bool("budget_exhausted", self.exhausted).bool("inspection_failed", self.failed).build())
    }
}
