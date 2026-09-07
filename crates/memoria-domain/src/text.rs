//! Shared text helpers for reviewer-supplied values.

/// Count whitespace-separated words.
pub fn word_count(text: &str) -> usize {
    text.split_whitespace().count()
}

/// Whether the text contains a control character other than `\n` or `\r`.
pub fn has_disallowed_control(text: &str) -> bool {
    text.chars()
        .any(|c| c.is_control() && c != '\n' && c != '\r')
}

/// Generic notes that do not explain a review.
pub const GENERIC_NOTES: &[&str] = &[
    "done",
    "reviewed",
    "looks good",
    "ok",
    "okay",
    "lgtm",
    "fine",
    "no changes",
    "no change",
    "updated",
];

pub fn is_generic(text: &str) -> bool {
    let lowered = text.trim().to_lowercase();
    let stripped = lowered.trim_end_matches(['.', '!']);
    GENERIC_NOTES.contains(&stripped)
}
