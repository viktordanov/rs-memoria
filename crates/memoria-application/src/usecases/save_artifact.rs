//! `memoria review <DOCUMENT> --save <DIR>`: write the exact review artifact
//! to a file outside the project, then report a receipt.
//!
//! The artifact is encoded first, with the existing hard limits, so a limit
//! refusal writes nothing. The destination must already exist and lie
//! outside the Git worktree, including its ignored folders and `.git`: an
//! artifact inside the worktree could become a selected input and invalidate
//! its own acknowledgement. The saved bytes are exactly the bytes that
//! `--format json` prints for the same snapshot. `review` stays read-only
//! for the project: no lock, no state.

use crate::error::{AppError, Detail, DetailMap, Diagnostic, ExitClass};
use crate::ports::{SaveFailure, Services};
use crate::review::ReviewManifest;

/// The longest slug a saved file name carries.
pub const MAX_SLUG_BYTES: usize = 96;
/// Collision suffixes run from 2 to this value.
pub const MAX_COLLISION_SUFFIX: u32 = 99;

/// Which artifact was saved.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ArtifactKind {
    Manifest,
    Full,
}

impl ArtifactKind {
    pub fn as_str(self) -> &'static str {
        match self {
            ArtifactKind::Manifest => "manifest",
            ArtifactKind::Full => "full",
        }
    }
}

/// The receipt of one saved artifact.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SavedArtifact {
    pub path: String,
    pub artifact_kind: ArtifactKind,
    pub bytes: u64,
    pub document: String,
    pub document_kind: String,
    pub review_revision: u64,
    pub token: String,
    pub artifact_digest: String,
    pub mode: String,
    pub ack_command: String,
}

impl SavedArtifact {
    pub fn to_detail(&self) -> Detail {
        DetailMap::default()
            .text("kind", "saved_review_artifact")
            .text("path", self.path.clone())
            .text("artifact_kind", self.artifact_kind.as_str())
            .number("bytes", self.bytes)
            .text("document", self.document.clone())
            .text("document_kind", self.document_kind.clone())
            .number("review_revision", self.review_revision)
            .text("token", self.token.clone())
            .text("artifact_digest", self.artifact_digest.clone())
            .text("mode", self.mode.clone())
            .text("ack_command", self.ack_command.clone())
            .build()
    }
}

/// Replace every byte outside `[A-Za-z0-9._-]` with `_`, capped at
/// `MAX_SLUG_BYTES`.
pub fn slug(document: &str) -> String {
    document
        .bytes()
        .take(MAX_SLUG_BYTES)
        .map(|b| {
            if b.is_ascii_alphanumeric() || b == b'.' || b == b'_' || b == b'-' {
                b as char
            } else {
                '_'
            }
        })
        .collect()
}

/// The deterministic file name for one attempt: `memoria-<kind>-<slug>-
/// <hex16>.json`, then `-2` … `-99` before the extension.
pub fn file_name(kind: ArtifactKind, document: &str, token: &str, attempt: u32) -> String {
    let hex = token.strip_prefix("mrv3.").unwrap_or(token);
    let suffix = if attempt <= 1 {
        String::new()
    } else {
        format!("-{attempt}")
    };
    format!(
        "memoria-{}-{}-{hex}{suffix}.json",
        kind.as_str(),
        slug(document)
    )
}

fn shell_quote(text: &str) -> String {
    if !text.is_empty()
        && text
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"/._-+:@".contains(&b))
    {
        text.to_string()
    } else {
        format!("'{}'", text.replace('\'', "'\\''"))
    }
}

/// Check the destination, then write the already encoded artifact.
pub fn save(
    services: &Services<'_>,
    destination: &str,
    kind: ArtifactKind,
    requirements: &ReviewManifest,
    artifact_digest: &str,
    encoded: &[u8],
) -> Result<SavedArtifact, AppError> {
    let io = |err: crate::ports::AdapterError| AppError::io("save_failed", err.to_string());
    let Some(directory) = services
        .artifacts
        .canonical_directory(destination)
        .map_err(io)?
    else {
        return Err(AppError::usage(
            "save_destination_invalid",
            format!("--save {destination} must name an existing directory"),
        ));
    };
    let root = services.artifacts.worktree_root().map_err(io)?;
    let inside = directory == root
        || directory
            .strip_prefix(&root)
            .is_some_and(|rest| rest.starts_with('/') || rest.starts_with('\\'));
    if inside {
        return Err(AppError::new(
            ExitClass::Usage,
            Diagnostic::error(
                "save_destination_in_project",
                format!(
                    "--save {destination} resolves to {directory}, inside the Git worktree {root}. A saved artifact inside the project could become a review input and invalidate itself. Choose a directory outside the worktree, for example one from `mktemp -d`"
                ),
            )
            .with_details(
                DetailMap::default()
                    .text("destination", directory.clone())
                    .text("worktree", root.clone())
                    .text("hint", "dir=$(mktemp -d)")
                    .build(),
            ),
        ));
    }
    let document = requirements.document.as_str();
    for attempt in 1..=MAX_COLLISION_SUFFIX {
        let name = file_name(kind, document, &requirements.token, attempt);
        match services.artifacts.create_new(&directory, &name, encoded) {
            Ok(path) => {
                let ack_command = format!(
                    "memoria ack {} --packet {} --reviewer <REVIEWER> --result <updated|no-update> --note <NOTE>",
                    shell_quote(document),
                    shell_quote(&path)
                );
                return Ok(SavedArtifact {
                    path,
                    artifact_kind: kind,
                    bytes: encoded.len() as u64,
                    document: document.to_string(),
                    document_kind: requirements.document_kind().as_str().to_string(),
                    review_revision: requirements.review_revision,
                    token: requirements.token.clone(),
                    artifact_digest: artifact_digest.to_string(),
                    mode: requirements.mode.as_str().to_string(),
                    ack_command,
                });
            }
            Err(SaveFailure::Exists) => continue,
            Err(SaveFailure::Io { error, leftover }) => {
                let mut details = DetailMap::default().text("destination", directory.clone());
                if let Some(leftover) = leftover {
                    details = details.text("leftover", leftover);
                }
                return Err(AppError::new(
                    ExitClass::Io,
                    Diagnostic::error("save_failed", error.to_string())
                        .with_details(details.build()),
                ));
            }
        }
    }
    Err(AppError::new(
        ExitClass::Conflict,
        Diagnostic::error(
            "save_name_exhausted",
            format!(
                "{directory} already holds this artifact name and every suffix up to -{MAX_COLLISION_SUFFIX}; choose an empty directory"
            ),
        ),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_are_deterministic_and_safe() {
        let token = "mrv3.0123456789abcdef";
        assert_eq!(
            file_name(ArtifactKind::Manifest, "src/README.md", token, 1),
            "memoria-manifest-src_README.md-0123456789abcdef.json"
        );
        assert_eq!(
            file_name(ArtifactKind::Full, "docs/a b/ü.md", token, 7),
            "memoria-full-docs_a_b___.md-0123456789abcdef-7.json"
        );
        assert_eq!(slug(&"a".repeat(200)).len(), MAX_SLUG_BYTES);
        assert_eq!(shell_quote("/tmp/x y"), "'/tmp/x y'");
        assert_eq!(shell_quote("/tmp/a.json"), "/tmp/a.json");
    }
}
