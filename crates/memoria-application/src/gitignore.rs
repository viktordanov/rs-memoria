//! Git-compatible parsing of ignore-file lines for policy fingerprints.
//!
//! Lines are handled as bytes end to end: Git matches rule bytes against
//! path bytes, so two rules that differ only in bytes that are not valid
//! UTF-8 are different rules and must stay different before hashing.

const UTF8_BOM: &[u8] = b"\xef\xbb\xbf";

/// Effective pattern lines: blank lines and comments removed, trailing
/// unescaped spaces trimmed, order, escapes, and exact bytes preserved.
pub fn effective_patterns(content: &[u8]) -> Vec<Vec<u8>> {
    // Git skips a UTF-8 byte order mark at the start of an ignore file
    // before reading lines; a BOM anywhere else is pattern content.
    let content = content.strip_prefix(UTF8_BOM).unwrap_or(content);
    let mut out = Vec::new();
    for raw_line in content.split(|byte| *byte == b'\n') {
        let line = raw_line.strip_suffix(b"\r").unwrap_or(raw_line);
        if line.first() == Some(&b'#') {
            continue;
        }
        let trimmed = trim_trailing_unescaped_spaces(line);
        if trimmed.is_empty() {
            continue;
        }
        out.push(trimmed.to_vec());
    }
    out
}

fn trim_trailing_unescaped_spaces(line: &[u8]) -> &[u8] {
    let mut end = line.len();
    while end > 0 && line[end - 1] == b' ' {
        if end >= 2 && line[end - 2] == b'\\' {
            break;
        }
        end -= 1;
    }
    &line[..end]
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rules(items: &[&[u8]]) -> Vec<Vec<u8>> {
        items.iter().map(|item| item.to_vec()).collect()
    }

    #[test]
    fn leading_bom_is_skipped_but_embedded_bom_is_pattern_content() {
        // MEM-035: Git ignores a byte order mark only at the file start.
        assert_eq!(
            effective_patterns(b"\xef\xbb\xbf# comment\n*.tmp\n"),
            rules(&[b"*.tmp"])
        );
        assert!(effective_patterns(b"\xef\xbb\xbf# only a comment\n").is_empty());
        assert_eq!(
            effective_patterns(b"\xef\xbb\xbf*.tmp\n"),
            effective_patterns(b"*.tmp\n")
        );
        assert_eq!(
            effective_patterns(b"# c\n\xef\xbb\xbf# pattern for git\n"),
            rules(&[b"\xef\xbb\xbf# pattern for git"])
        );
        assert_eq!(
            effective_patterns(b"\xef\xbb\xbf\xef\xbb\xbf*.a\n"),
            rules(&[b"\xef\xbb\xbf*.a"])
        );
    }

    #[test]
    fn drops_comments_and_blanks_and_trailing_spaces() {
        let patterns = effective_patterns(b"# comment\n\n  \nbuild/   \nkeep\\ \n\\#literal\r\n");
        assert_eq!(patterns, rules(&[b"build/", b"keep\\ ", b"\\#literal"]));
    }

    #[test]
    fn rule_bytes_outside_utf8_stay_distinct() {
        // MEM-048: byte ranges that Git evaluates differently must not
        // collapse into one replacement-character rule.
        let first = effective_patterns(b"[\xc2-\xc3]*\n");
        let second = effective_patterns(b"[\xc3-\xc4]*\n");
        assert_eq!(first, rules(&[b"[\xc2-\xc3]*"]));
        assert_eq!(second, rules(&[b"[\xc3-\xc4]*"]));
        assert_ne!(first, second);
        // Trailing-space and comment rules apply to raw bytes as well.
        assert_eq!(
            effective_patterns(b"\xff\xfe   \n# \xff\n\xfe\\ \n"),
            rules(&[b"\xff\xfe", b"\xfe\\ "])
        );
        // Ordinary rules are the same bytes as their text.
        assert_eq!(
            effective_patterns("é/**\n".as_bytes()),
            rules(&["é/**".as_bytes()])
        );
    }
}
