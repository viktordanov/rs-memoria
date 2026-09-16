//! CommonMark parsing with byte offsets, marker extraction, export
//! validation, and normal-link discovery.

use std::collections::BTreeMap;

use memoria_application::ports::{
    MarkdownCodec, MarkdownIssue, ParsedDocument, ParsedImport, ParsedSection,
};
use memoria_domain::section::validate_section_path;
use memoria_domain::{ByteRange, DocumentId, Export, ExportId, SourceLocation};
use pulldown_cmark::{Event, LinkType, Options, Parser, Tag, TagEnd};

#[derive(Debug, Default, Clone, Copy)]
pub struct PulldownMarkdownCodec;

#[derive(Debug, Clone, PartialEq, Eq)]
enum Marker {
    ExportOpen(String),
    ExportClose,
    ImportOpen(String),
    ImportClose,
    /// Advisory section: the `id` attribute and the authored `files` tokens.
    SectionOpen {
        id: String,
        files: Vec<String>,
    },
    SectionClose,
}

/// A marker problem, routed to the structural or the advisory channel.
///
/// Section problems never invalidate a README. They withdraw its focused
/// advice and leave the reviewer with the full baseline.
#[derive(Debug, Clone, PartialEq, Eq)]
enum MarkerError {
    Structural(String),
    Section(String),
}

impl MarkerError {
    fn message(self) -> String {
        match self {
            MarkerError::Structural(m) | MarkerError::Section(m) => m,
        }
    }
}

fn line_of(text: &str, offset: usize) -> SourceLocation {
    let line = text[..offset.min(text.len())]
        .bytes()
        .filter(|b| *b == b'\n')
        .count()
        + 1;
    SourceLocation { line, column: 1 }
}

/// Byte ranges of fenced and indented code blocks.
fn code_ranges(text: &str) -> Vec<ByteRange> {
    let mut ranges = Vec::new();
    for (event, range) in Parser::new_ext(text, Options::empty()).into_offset_iter() {
        if let Event::Start(Tag::CodeBlock(_)) = event {
            ranges.push(ByteRange::new(range.start, range.end));
        }
    }
    ranges
}

fn in_ranges(ranges: &[ByteRange], offset: usize) -> bool {
    ranges.iter().any(|r| r.start <= offset && offset < r.end)
}

/// Whether a line that is not a well-formed marker still looks like an
/// attempt at a section declaration. Such a line must be reported, never
/// dropped in silence, so a mistyped mapping cannot quietly narrow a review.
fn looks_like_section(line: &str) -> bool {
    let start = line.trim_start_matches([' ', '\t']);
    start.starts_with("<!--") && start.contains("memoria:section")
}

/// Split the `files` attribute on exactly one ASCII space per separator and
/// check every token against the literal-path grammar.
fn parse_section_files(raw: &str) -> Result<Vec<String>, String> {
    if raw.is_empty() {
        return Err("section marker requires at least one path in `files`".to_string());
    }
    let mut files = Vec::new();
    for token in raw.split(' ') {
        if token.is_empty() {
            return Err(
                "section marker `files` paths must be separated by exactly one space".to_string(),
            );
        }
        if let Err(err) = validate_section_path(token) {
            return Err(format!("section path {token:?}: {err}"));
        }
        if files.contains(&token.to_string()) {
            return Err(format!("section lists the path {token:?} more than once"));
        }
        files.push(token.to_string());
    }
    Ok(files)
}

/// Parse one marker line. `Ok(None)` when the line is not a marker at all.
fn parse_marker(line: &str) -> Result<Option<Marker>, MarkerError> {
    let trimmed = line.trim_end_matches([' ', '\t', '\r']);
    let section_like = looks_like_section(line);
    let wrap = |message: String| {
        if section_like {
            MarkerError::Section(message)
        } else {
            MarkerError::Structural(message)
        }
    };
    if !trimmed.starts_with("<!-- memoria:") && !trimmed.starts_with("<!-- /memoria:") {
        if trimmed.starts_with("<!--") && trimmed.contains("memoria:") {
            return Err(wrap("marker must have the exact form `<!-- memoria:... -->` or `<!-- /memoria:... -->` at column zero".to_string()));
        }
        if section_like {
            return Err(MarkerError::Section("section marker must have the exact form `<!-- memoria:section id=\"ID\" files=\"PATH\" -->` at column zero".to_string()));
        }
        return Ok(None);
    }
    let Some(inner) = trimmed
        .strip_prefix("<!-- ")
        .and_then(|s| s.strip_suffix(" -->"))
    else {
        return Err(wrap(
            "marker must end with ` -->` and contain no other trailing text".to_string(),
        ));
    };
    match inner {
        "/memoria:export" => return Ok(Some(Marker::ExportClose)),
        "/memoria:import" => return Ok(Some(Marker::ImportClose)),
        "/memoria:section" => return Ok(Some(Marker::SectionClose)),
        _ => {}
    }
    if inner.starts_with('/') {
        return Err(wrap(format!("unknown closing marker {inner:?}")));
    }
    if let Some(rest) = inner.strip_prefix("memoria:section ") {
        // Attribute order is fixed: `id` then `files`, each double-quoted.
        let Some(rest) = rest.strip_prefix("id=\"") else {
            return Err(MarkerError::Section(
                "section marker requires `id=\"ID\"` as its first attribute".to_string(),
            ));
        };
        let Some((id, rest)) = rest.split_once('"') else {
            return Err(MarkerError::Section(
                "section marker attribute id is not closed".to_string(),
            ));
        };
        let Some(files) = rest
            .strip_prefix(" files=\"")
            .and_then(|s| s.strip_suffix('"'))
        else {
            return Err(MarkerError::Section(
                "section marker requires `files=\"PATH[ PATH...]\"` as its second and last attribute".to_string(),
            ));
        };
        if id.is_empty() || files.contains('"') {
            return Err(MarkerError::Section(
                "section marker attributes are malformed".to_string(),
            ));
        }
        let files = parse_section_files(files).map_err(MarkerError::Section)?;
        return Ok(Some(Marker::SectionOpen {
            id: id.to_string(),
            files,
        }));
    }
    let (kind, attribute) = match inner.strip_prefix("memoria:export ") {
        Some(rest) => ("export", rest),
        None => match inner.strip_prefix("memoria:import ") {
            Some(rest) => ("import", rest),
            None => return Err(wrap(format!("unknown marker {inner:?}"))),
        },
    };
    let expected_key = if kind == "export" { "id" } else { "src" };
    let Some(value) = attribute
        .strip_prefix(expected_key)
        .and_then(|s| s.strip_prefix("=\""))
        .and_then(|s| s.strip_suffix('"'))
    else {
        return Err(wrap(format!(
            "{kind} marker requires exactly one double-quoted `{expected_key}` attribute"
        )));
    };
    if value.contains('"') || value.is_empty() {
        return Err(wrap(format!(
            "{kind} marker attribute {expected_key} is malformed"
        )));
    }
    Ok(Some(if kind == "export" {
        Marker::ExportOpen(value.to_string())
    } else {
        Marker::ImportOpen(value.to_string())
    }))
}

/// Validate one export body using the complete document as reference
/// context, so reference-style links whose definitions sit outside the
/// export are still recognized and rejected.
fn validate_export_range(text: &str, body: ByteRange) -> Vec<String> {
    let mut problems = Vec::new();
    let mut options = Options::empty();
    options.insert(Options::ENABLE_FOOTNOTES);
    let mut literal: Vec<ByteRange> = Vec::new();
    let mut inline_links: BTreeMap<usize, usize> = BTreeMap::new();
    for (event, range) in Parser::new_ext(text, options).into_offset_iter() {
        if matches!(event, Event::Start(Tag::CodeBlock(_)) | Event::Code(_)) {
            literal.push(ByteRange::new(range.start, range.end));
        }
        if let Event::Start(Tag::Link {
            link_type: LinkType::Inline,
            ..
        })
        | Event::Start(Tag::Image {
            link_type: LinkType::Inline,
            ..
        }) = &event
        {
            inline_links.insert(range.start, range.end);
        }
        if range.start < body.start || range.start >= body.end {
            continue;
        }
        match event {
            Event::Start(Tag::Link {
                link_type,
                dest_url,
                ..
            }) => {
                if !matches!(link_type, LinkType::Inline) {
                    problems.push(format!(
                        "link {:?} must be an inline link with an absolute destination",
                        dest_url.as_ref()
                    ));
                } else if !(dest_url.starts_with("https://")
                    || dest_url.starts_with("http://")
                    || dest_url.starts_with("mailto:"))
                {
                    problems.push(format!("link destination {dest_url:?} must be an absolute https://, http://, or mailto: URL"));
                }
            }
            Event::Start(Tag::Image {
                link_type,
                dest_url,
                ..
            }) => {
                if !matches!(link_type, LinkType::Inline) {
                    problems.push(format!(
                        "image {:?} must be an inline image with an absolute destination",
                        dest_url.as_ref()
                    ));
                } else if !(dest_url.starts_with("https://") || dest_url.starts_with("http://")) {
                    problems.push(format!(
                        "image destination {dest_url:?} must be an absolute https:// or http:// URL"
                    ));
                }
            }
            Event::Html(html) | Event::InlineHtml(html) => {
                problems.push(format!(
                    "raw HTML {:?} is not permitted inside exports",
                    html.trim()
                ));
            }
            Event::FootnoteReference(_) | Event::Start(Tag::FootnoteDefinition(_)) => {
                problems.push("footnotes are not permitted inside exports".to_string());
            }
            _ => {}
        }
    }
    problems.extend(reference_syntax_problems(
        text,
        body,
        &literal,
        &inline_links,
    ));
    problems
}

/// Reject bracket syntax that takes its meaning from the surrounding
/// document: reference-style links and images (`[t][l]`, `[l][]`, `[l]`),
/// and reference definitions (`[l]: dest`). A copied fragment containing
/// them could resolve differently inside a consumer. Literal code and
/// backslash-escaped brackets are allowed.
fn reference_syntax_problems(
    text: &str,
    body: ByteRange,
    literal: &[ByteRange],
    inline_links: &BTreeMap<usize, usize>,
) -> Vec<String> {
    let bytes = text.as_bytes();
    let mut problems = Vec::new();
    let mut i = body.start;
    while i < body.end {
        if in_ranges(literal, i) {
            i += 1;
            continue;
        }
        match bytes[i] {
            b'\\' => {
                i += 2;
                continue;
            }
            b'[' => {}
            _ => {
                i += 1;
                continue;
            }
        }
        let Some(close) = matching_bracket(bytes, i, body.end) else {
            i += 1;
            continue;
        };
        let label = &text[i + 1..close];
        // A `!` starts an image only when it is not itself escaped: count the
        // backslashes before it and require even parity.
        let is_image = i > body.start && bytes[i - 1] == b'!' && {
            let mut backslashes = 0;
            let mut j = i - 1;
            while j > body.start && bytes[j - 1] == b'\\' {
                backslashes += 1;
                j -= 1;
            }
            backslashes % 2 == 0
        };
        let kind = if is_image { "image" } else { "link" };
        match bytes.get(close + 1).copied() {
            Some(b'(') => {
                // Only a bracket that the parser accepted as a complete inline
                // link or image is context-independent. Malformed inline
                // syntax stays plain text whose label a consumer could define.
                let start = if is_image { i - 1 } else { i };
                match inline_links.get(&start) {
                    None => problems.push(format!(
                        "`[{label}](…)` is not a valid inline {kind}; a consuming README could turn `[{label}]` into a reference"
                    )),
                    Some(&end) => {
                        if has_unescaped_bracket(bytes, i + 1, close, literal) {
                            problems.push(format!(
                                "inline {kind} text `[{label}]` contains nested brackets; a consuming README could turn the inner text into a reference"
                            ));
                        }
                        i = end.max(close + 1);
                        continue;
                    }
                }
            }
            Some(b'[') => {
                problems.push(format!(
                    "reference-style {kind} `[{label}][…]` is not permitted inside exports; use an inline {kind} with an absolute destination"
                ));
                if let Some(second) = matching_bracket(bytes, close + 1, body.end) {
                    i = second + 1;
                    continue;
                }
            }
            Some(b':') if at_line_start(bytes, i, body.start) => problems.push(format!(
                "reference definition `[{label}]:` is not permitted inside exports"
            )),
            _ if label.trim().is_empty() => {}
            _ => problems.push(format!(
                "bracketed text `[{label}]` could become a shortcut reference in a consuming README; escape it as `\\[{label}\\]` or use an inline link"
            )),
        }
        i = close + 1;
    }
    problems
}

/// Whether `bytes[start..end]` contains an unescaped bracket outside code.
fn has_unescaped_bracket(bytes: &[u8], start: usize, end: usize, literal: &[ByteRange]) -> bool {
    let mut i = start;
    while i < end {
        match bytes[i] {
            b'\\' => i += 1,
            b'[' | b']' if !in_ranges(literal, i) => return true,
            _ => {}
        }
        i += 1;
    }
    false
}

/// Index of the `]` that closes the bracket at `open`, honoring escapes and
/// nesting, within `end`.
fn matching_bracket(bytes: &[u8], open: usize, end: usize) -> Option<usize> {
    let mut depth = 0usize;
    let mut i = open;
    while i < end {
        match bytes[i] {
            b'\\' => i += 1,
            b'[' => depth += 1,
            b']' => {
                depth -= 1;
                if depth == 0 {
                    return Some(i);
                }
            }
            _ => {}
        }
        i += 1;
    }
    None
}

/// Whether only up to three spaces precede `index` on its line.
fn at_line_start(bytes: &[u8], index: usize, floor: usize) -> bool {
    let mut spaces = 0;
    let mut i = index;
    while i > floor {
        i -= 1;
        match bytes[i] {
            b'\n' => return spaces <= 3,
            b' ' => spaces += 1,
            _ => return false,
        }
    }
    spaces <= 3
}

fn links_outside_code(text: &str, code: &[ByteRange]) -> Vec<String> {
    let mut links = Vec::new();
    for (event, range) in Parser::new_ext(text, Options::empty()).into_offset_iter() {
        if let Event::Start(Tag::Link { dest_url, .. }) = event
            && !in_ranges(code, range.start)
        {
            links.push(dest_url.to_string());
        }
    }
    links
}

/// The text of the body's first block when that block is a heading.
///
/// A section body must open with a CommonMark heading, ATX or setext. The
/// heading text is a reading hint only; it never identifies the section.
fn leading_heading(body: &str) -> Result<String, String> {
    if body.trim().is_empty() {
        return Err("section body must contain authored Markdown".to_string());
    }
    let mut text = String::new();
    let mut depth = 0usize;
    for event in Parser::new_ext(body, Options::empty()) {
        match event {
            Event::Start(Tag::Heading { .. }) if depth == 0 => depth = 1,
            Event::Start(_) if depth == 0 => {
                return Err(
                    "the first block of a section body must be a Markdown heading".to_string(),
                );
            }
            // Terminate at the end of the heading itself. An inline span such
            // as emphasis also produces an end event, and stopping there would
            // silently drop the text that follows it.
            Event::End(TagEnd::Heading(..)) if depth == 1 => break,
            Event::Text(t) | Event::Code(t) if depth == 1 => text.push_str(&t),
            _ => {}
        }
    }
    if depth == 0 {
        return Err("the first block of a section body must be a Markdown heading".to_string());
    }
    let text = text.trim().to_string();
    if text.is_empty() {
        return Err("the section's first heading has no text".to_string());
    }
    Ok(text)
}

impl MarkdownCodec for PulldownMarkdownCodec {
    fn parse(&self, _document: &DocumentId, bytes: &[u8]) -> ParsedDocument {
        let mut parsed = ParsedDocument::default();
        let Ok(text) = std::str::from_utf8(bytes) else {
            parsed.issues.push(MarkdownIssue {
                code: "markdown_invalid",
                message: "README is not valid UTF-8".into(),
                location: None,
            });
            return parsed;
        };
        let code = code_ranges(text);
        parsed.links = links_outside_code(text, &code);

        #[derive(Clone)]
        struct Open {
            marker: Marker,
            body_start: usize,
            location: SourceLocation,
        }
        struct OpenSection {
            id: String,
            files: Vec<String>,
            body_start: usize,
            location: SourceLocation,
        }
        let mut open: Option<Open> = None;
        let mut open_section: Option<OpenSection> = None;
        // Set when an opening section marker was refused, so its matching
        // close reports nothing further. One mistake yields one message.
        let mut section_recovering = false;
        let mut offset = 0;
        let mut exports: Vec<Export> = Vec::new();
        let mut export_ids: Vec<String> = Vec::new();
        let mut sections: Vec<ParsedSection> = Vec::new();
        let mut section_ids: Vec<String> = Vec::new();
        let section_issue =
            |issues: &mut Vec<MarkdownIssue>, message: String, location: SourceLocation| {
                issues.push(MarkdownIssue {
                    code: "section_mapping_invalid",
                    message,
                    location: Some(location),
                });
            };
        for raw_line in text.split_inclusive('\n') {
            let line_start = offset;
            offset += raw_line.len();
            let line = raw_line.strip_suffix('\n').unwrap_or(raw_line);
            let has_newline = raw_line.ends_with('\n');
            // An indented code block's range begins after its indent, so the
            // first non-blank byte, not the line start, decides inertness.
            let content_start =
                line_start + (line.len() - line.trim_start_matches([' ', '\t']).len());
            if in_ranges(&code, content_start) {
                continue;
            }
            let location = line_of(text, line_start);
            let marker = match parse_marker(line) {
                Ok(None) => continue,
                Ok(Some(marker)) => marker,
                Err(MarkerError::Section(message)) => {
                    section_issue(&mut parsed.section_issues, message, location);
                    section_recovering = !line.contains("/memoria:section");
                    continue;
                }
                Err(err) => {
                    parsed.issues.push(MarkdownIssue {
                        code: "marker_malformed",
                        message: err.message(),
                        location: Some(location),
                    });
                    continue;
                }
            };
            match marker {
                Marker::SectionOpen { id, files } => {
                    // Sections never nest, and never begin inside an export
                    // or import block whose body a provider owns.
                    if let Some(previous) = &open_section {
                        section_issue(
                            &mut parsed.section_issues,
                            format!(
                                "section opened while the section at line {} is still open; sections cannot nest or overlap",
                                previous.location.line
                            ),
                            location,
                        );
                        section_recovering = true;
                        continue;
                    }
                    if let Some(previous) = &open {
                        section_issue(
                            &mut parsed.section_issues,
                            format!(
                                "section cannot start inside the export or import block opened at line {}",
                                previous.location.line
                            ),
                            location,
                        );
                        section_recovering = true;
                        continue;
                    }
                    if !has_newline {
                        section_issue(
                            &mut parsed.section_issues,
                            "an opening section marker must be followed by a line ending".into(),
                            location,
                        );
                        continue;
                    }
                    open_section = Some(OpenSection {
                        id,
                        files,
                        body_start: offset,
                        location,
                    });
                }
                Marker::SectionClose => {
                    let Some(current) = open_section.take() else {
                        if section_recovering {
                            section_recovering = false;
                        } else {
                            section_issue(
                                &mut parsed.section_issues,
                                "closing section marker without an open section".into(),
                                location,
                            );
                        }
                        continue;
                    };
                    if let Some(inner) = &open {
                        section_issue(
                            &mut parsed.section_issues,
                            format!(
                                "section closes while the export or import block opened at line {} is still open; a section cannot cross that boundary",
                                inner.location.line
                            ),
                            current.location,
                        );
                        continue;
                    }
                    if section_ids.contains(&current.id) {
                        section_issue(
                            &mut parsed.section_issues,
                            format!("duplicate section id {:?}", current.id),
                            current.location,
                        );
                        continue;
                    }
                    if let Err(err) = memoria_domain::SectionId::parse(&current.id) {
                        section_issue(
                            &mut parsed.section_issues,
                            err.to_string(),
                            current.location,
                        );
                        continue;
                    }
                    let body = ByteRange::new(current.body_start, line_start);
                    let heading = match leading_heading(&text[body.start..body.end]) {
                        Ok(heading) => heading,
                        Err(message) => {
                            section_issue(
                                &mut parsed.section_issues,
                                format!("section {:?}: {message}", current.id),
                                current.location,
                            );
                            continue;
                        }
                    };
                    section_ids.push(current.id.clone());
                    sections.push(ParsedSection {
                        id: current.id,
                        files: current.files,
                        heading,
                        body,
                        first_line: line_of(text, body.start).line,
                        last_line: line_of(text, body.end.saturating_sub(1)).line,
                        location: current.location,
                    });
                }
                Marker::ExportOpen(_) | Marker::ImportOpen(_) => {
                    if let Some(previous) = &open {
                        parsed.issues.push(MarkdownIssue {
                            code: "marker_nested",
                            message: format!("marker opened while the block at line {} is still open; blocks cannot nest or overlap", previous.location.line),
                            location: Some(location),
                        });
                        continue;
                    }
                    if !has_newline {
                        parsed.issues.push(MarkdownIssue {
                            code: "marker_malformed",
                            message: "an opening marker must be followed by a line ending".into(),
                            location: Some(location),
                        });
                        continue;
                    }
                    open = Some(Open {
                        marker,
                        body_start: offset,
                        location,
                    });
                }
                Marker::ExportClose | Marker::ImportClose => {
                    let Some(current) = open.take() else {
                        parsed.issues.push(MarkdownIssue {
                            code: "marker_mismatch",
                            message: "closing marker without an open block".into(),
                            location: Some(location),
                        });
                        continue;
                    };
                    let body = ByteRange::new(current.body_start, line_start);
                    match (&current.marker, &marker) {
                        (Marker::ExportOpen(id), Marker::ExportClose) => {
                            match ExportId::parse(id) {
                                Ok(export_id) => {
                                    if export_ids.contains(id) {
                                        parsed.issues.push(MarkdownIssue {
                                            code: "export_duplicate",
                                            message: format!("duplicate export id {id:?}"),
                                            location: Some(current.location),
                                        });
                                    } else {
                                        export_ids.push(id.clone());
                                        for problem in validate_export_range(text, body) {
                                            parsed.issues.push(MarkdownIssue {
                                                code: "export_invalid",
                                                message: format!("export {id:?}: {problem}"),
                                                location: Some(current.location),
                                            });
                                        }
                                        exports.push(Export {
                                            id: export_id,
                                            body,
                                            location: current.location,
                                        });
                                    }
                                }
                                Err(err) => parsed.issues.push(MarkdownIssue {
                                    code: "export_invalid",
                                    message: err.to_string(),
                                    location: Some(current.location),
                                }),
                            }
                        }
                        (Marker::ImportOpen(src), Marker::ImportClose) => {
                            parsed.imports.push(ParsedImport {
                                source_text: src.clone(),
                                body,
                                location: current.location,
                            });
                        }
                        _ => parsed.issues.push(MarkdownIssue {
                            code: "marker_mismatch",
                            message: format!(
                                "closing marker does not match the block opened at line {}",
                                current.location.line
                            ),
                            location: Some(location),
                        }),
                    }
                }
            }
        }
        if let Some(current) = open {
            parsed.issues.push(MarkdownIssue {
                code: "marker_unclosed",
                message: "block is never closed".into(),
                location: Some(current.location),
            });
        }
        if let Some(current) = open_section {
            section_issue(
                &mut parsed.section_issues,
                "section is never closed".into(),
                current.location,
            );
        }
        parsed.exports = exports;
        // One invalid mapping withdraws the whole README's advice. Partial
        // advice cannot narrow a review, because a reader cannot tell which
        // mapping the author meant to write.
        parsed.sections = if parsed.section_issues.is_empty() {
            sections
        } else {
            Vec::new()
        };
        parsed
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(text: &str) -> ParsedDocument {
        PulldownMarkdownCodec.parse(&DocumentId::parse("README.md").unwrap(), text.as_bytes())
    }

    fn section_codes(text: &str) -> Vec<String> {
        parse(text)
            .section_issues
            .iter()
            .map(|i| i.message.clone())
            .collect()
    }

    #[test]
    fn parses_an_advisory_section() {
        let text = "# Doc\n\n<!-- memoria:section id=\"persistence\" files=\"handle.go service.go\" -->\n## Saving and synchronizing\n\nSave writes a local archive.\n<!-- /memoria:section -->\n";
        let parsed = parse(text);
        assert!(parsed.issues.is_empty(), "{:?}", parsed.issues);
        assert!(
            parsed.section_issues.is_empty(),
            "{:?}",
            parsed.section_issues
        );
        assert_eq!(parsed.sections.len(), 1);
        let section = &parsed.sections[0];
        assert_eq!(section.id, "persistence");
        assert_eq!(section.files, vec!["handle.go", "service.go"]);
        assert_eq!(section.heading, "Saving and synchronizing");
        assert_eq!(section.location.line, 3);
        // The body range and the line hints both exclude the two markers.
        assert_eq!(
            &text[section.body.start..section.body.end],
            "## Saving and synchronizing\n\nSave writes a local archive.\n"
        );
        assert_eq!((section.first_line, section.last_line), (4, 6));
    }

    #[test]
    fn a_formatted_heading_keeps_its_trailing_text() {
        // An inline span inside the heading must not end the hint early.
        for (body, expected) in [
            ("## A *formatted* heading", "A formatted heading"),
            ("## A **strong** heading", "A strong heading"),
            ("## A `code` heading", "A code heading"),
            (
                "## A [linked](https://example.com/x) heading",
                "A linked heading",
            ),
            ("## Plain heading", "Plain heading"),
            ("## Trailing *emphasis*", "Trailing emphasis"),
            ("Setext heading *here*\n===", "Setext heading here"),
        ] {
            let text = format!(
                "<!-- memoria:section id=\"s\" files=\"a.rs\" -->\n{body}\n\nBody text.\n<!-- /memoria:section -->\n"
            );
            let parsed = parse(&text);
            assert!(
                parsed.section_issues.is_empty(),
                "{body:?}: {:?}",
                parsed.section_issues
            );
            assert_eq!(parsed.sections[0].heading, expected, "{body:?}");
        }
    }

    #[test]
    fn heading_validation_is_unchanged_by_the_termination_fix() {
        // A body that does not open with a heading is still refused.
        for body in [
            "Plain paragraph.",
            "- list item",
            "> quote",
            "```\ncode\n```",
        ] {
            let text = format!(
                "<!-- memoria:section id=\"s\" files=\"a.rs\" -->\n{body}\n<!-- /memoria:section -->\n"
            );
            assert!(
                !parse(&text).section_issues.is_empty(),
                "{body:?} must not open a section"
            );
        }
        // A heading with no text at all is still refused.
        let text = "<!-- memoria:section id=\"s\" files=\"a.rs\" -->\n##\n\nBody.\n<!-- /memoria:section -->\n";
        let parsed = parse(text);
        assert!(
            parsed
                .section_issues
                .iter()
                .any(|i| i.message.contains("no text")),
            "{:?}",
            parsed.section_issues
        );
        // Only the first heading becomes the hint; a later one does not extend it.
        let text = "<!-- memoria:section id=\"s\" files=\"a.rs\" -->\n## First *one*\n\nProse.\n\n## Second\n<!-- /memoria:section -->\n";
        assert_eq!(parse(text).sections[0].heading, "First one");
    }

    #[test]
    fn section_markers_accept_crlf_and_trailing_blanks() {
        let text = "<!-- memoria:section id=\"a\" files=\"x.rs\" -->  \r\n# Title\r\nBody.\r\n<!-- /memoria:section -->\t\r\n";
        let parsed = parse(text);
        assert!(
            parsed.section_issues.is_empty(),
            "{:?}",
            parsed.section_issues
        );
        assert_eq!(parsed.sections[0].heading, "Title");
        assert_eq!(
            (parsed.sections[0].first_line, parsed.sections[0].last_line),
            (2, 3)
        );
    }

    #[test]
    fn section_markers_inside_code_are_literal() {
        let text = "```markdown\n<!-- memoria:section id=\"x\" files=\"a.rs\" -->\n<!-- /memoria:section -->\n```\n\n    <!-- memoria:section id=\"y\" files=\"b.rs\" -->\n\ntext\n";
        let parsed = parse(text);
        assert!(parsed.issues.is_empty(), "{:?}", parsed.issues);
        assert!(
            parsed.section_issues.is_empty(),
            "{:?}",
            parsed.section_issues
        );
        assert!(parsed.sections.is_empty());
    }

    #[test]
    fn malformed_section_markers_are_advisory_not_structural() {
        for text in [
            // Wrong indentation, still visible rather than silently dropped.
            "text\n\n  <!-- memoria:section id=\"a\" files=\"x.rs\" -->\n# T\n  <!-- /memoria:section -->\n",
            // Attribute order is fixed.
            "<!-- memoria:section files=\"x.rs\" id=\"a\" -->\n# T\n<!-- /memoria:section -->\n",
            // Unknown extra attribute.
            "<!-- memoria:section id=\"a\" files=\"x.rs\" extra=\"1\" -->\n# T\n<!-- /memoria:section -->\n",
            // Single quotes.
            "<!-- memoria:section id='a' files='x.rs' -->\n# T\n<!-- /memoria:section -->\n",
            // No space padding inside the comment.
            "<!--memoria:section id=\"a\" files=\"x.rs\"-->\n# T\n<!-- /memoria:section -->\n",
            // Empty files attribute.
            "<!-- memoria:section id=\"a\" files=\"\" -->\n# T\n<!-- /memoria:section -->\n",
            // Two spaces between paths.
            "<!-- memoria:section id=\"a\" files=\"x.rs  y.rs\" -->\n# T\n<!-- /memoria:section -->\n",
            // A glob is not a literal path.
            "<!-- memoria:section id=\"a\" files=\"src/*.rs\" -->\n# T\n<!-- /memoria:section -->\n",
            // Parent components are refused.
            "<!-- memoria:section id=\"a\" files=\"../x.rs\" -->\n# T\n<!-- /memoria:section -->\n",
            // Absolute paths are refused.
            "<!-- memoria:section id=\"a\" files=\"/x.rs\" -->\n# T\n<!-- /memoria:section -->\n",
            // The same path twice in one section.
            "<!-- memoria:section id=\"a\" files=\"x.rs x.rs\" -->\n# T\n<!-- /memoria:section -->\n",
            // The id grammar.
            "<!-- memoria:section id=\"9a\" files=\"x.rs\" -->\n# T\n<!-- /memoria:section -->\n",
            // Duplicate ids inside one README.
            "<!-- memoria:section id=\"a\" files=\"x.rs\" -->\n# T\n<!-- /memoria:section -->\n<!-- memoria:section id=\"a\" files=\"y.rs\" -->\n# U\n<!-- /memoria:section -->\n",
            // A body that does not open with a heading.
            "<!-- memoria:section id=\"a\" files=\"x.rs\" -->\nPlain paragraph.\n<!-- /memoria:section -->\n",
            // An empty body.
            "<!-- memoria:section id=\"a\" files=\"x.rs\" -->\n<!-- /memoria:section -->\n",
            // Never closed.
            "<!-- memoria:section id=\"a\" files=\"x.rs\" -->\n# T\n",
            // Closed without an opening.
            "<!-- /memoria:section -->\n",
            // Sections cannot nest.
            "<!-- memoria:section id=\"a\" files=\"x.rs\" -->\n# T\n<!-- memoria:section id=\"b\" files=\"y.rs\" -->\n# U\n<!-- /memoria:section -->\n<!-- /memoria:section -->\n",
        ] {
            let parsed = parse(text);
            assert!(
                !parsed.section_issues.is_empty(),
                "expected an advisory problem for {text:?}"
            );
            assert!(
                parsed
                    .section_issues
                    .iter()
                    .all(|i| i.code == "section_mapping_invalid"),
                "{:?}",
                parsed.section_issues
            );
            // Advice is withdrawn completely, and the README stays structurally valid.
            assert!(parsed.sections.is_empty(), "{text:?}");
            assert!(parsed.issues.is_empty(), "{text:?}: {:?}", parsed.issues);
        }
    }

    #[test]
    fn one_invalid_mapping_withdraws_every_section() {
        let text = "<!-- memoria:section id=\"good\" files=\"x.rs\" -->\n# Good\n<!-- /memoria:section -->\n\n<!-- memoria:section id=\"bad\" files=\"../y.rs\" -->\n# Bad\n<!-- /memoria:section -->\n";
        let parsed = parse(text);
        assert_eq!(parsed.section_issues.len(), 1);
        assert!(parsed.sections.is_empty());
    }

    #[test]
    fn exports_may_sit_inside_a_section_but_never_cross_it() {
        // An export wholly inside a section is allowed.
        let ok = "<!-- memoria:section id=\"a\" files=\"x.rs\" -->\n# Title\n<!-- memoria:export id=\"summary\" -->\nText.\n<!-- /memoria:export -->\n<!-- /memoria:section -->\n";
        let parsed = parse(ok);
        assert!(parsed.issues.is_empty(), "{:?}", parsed.issues);
        assert!(
            parsed.section_issues.is_empty(),
            "{:?}",
            parsed.section_issues
        );
        assert_eq!(parsed.sections.len(), 1);
        assert_eq!(parsed.exports.len(), 1);
        // A section that starts inside an export is refused.
        let inside = "<!-- memoria:export id=\"summary\" -->\n<!-- memoria:section id=\"a\" files=\"x.rs\" -->\n# T\n<!-- /memoria:section -->\n<!-- /memoria:export -->\n";
        assert!(!parse(inside).section_issues.is_empty());
        // A section that closes while an export is open crosses the boundary.
        let crossing = "<!-- memoria:section id=\"a\" files=\"x.rs\" -->\n# T\n<!-- memoria:export id=\"summary\" -->\nText.\n<!-- /memoria:section -->\n<!-- /memoria:export -->\n";
        let parsed = parse(crossing);
        assert!(
            parsed
                .section_issues
                .iter()
                .any(|i| i.message.contains("cross")),
            "{:?}",
            parsed.section_issues
        );
    }

    #[test]
    fn section_like_text_in_prose_is_reported_not_dropped() {
        // An HTML comment that mentions a section but is not one must surface.
        assert!(!section_codes("<!-- memoria:section -->\n").is_empty());
        // Ordinary prose naming the marker is not a declaration.
        let prose = "Write `<!-- memoria:section id=\"a\" files=\"x.rs\" -->` at column zero.\n";
        let parsed = parse(prose);
        assert!(
            parsed.section_issues.is_empty(),
            "{:?}",
            parsed.section_issues
        );
        assert!(parsed.issues.is_empty(), "{:?}", parsed.issues);
    }

    #[test]
    fn extracts_exports_imports_and_links() {
        let text = "# Root\n\nSee [corpus](src/corpus/README.md) and [ext](https://example.com).\n\n<!-- memoria:export id=\"summary\" -->\n## Summary\nText with [abs](https://example.com/x).\n<!-- /memoria:export -->\n\n<!-- memoria:import src=\"src/retrieval/README.md#summary\" -->\nold body\n<!-- /memoria:import -->\n";
        let parsed = parse(text);
        assert!(parsed.issues.is_empty(), "{:?}", parsed.issues);
        assert_eq!(parsed.exports.len(), 1);
        let export = &parsed.exports[0];
        assert_eq!(
            &text[export.body.start..export.body.end],
            "## Summary\nText with [abs](https://example.com/x).\n"
        );
        assert_eq!(export.location.line, 5);
        assert_eq!(parsed.imports.len(), 1);
        assert_eq!(
            &text[parsed.imports[0].body.start..parsed.imports[0].body.end],
            "old body\n"
        );
        assert_eq!(parsed.imports[0].location.line, 10);
        assert_eq!(
            parsed.links,
            vec![
                "src/corpus/README.md",
                "https://example.com",
                "https://example.com/x"
            ]
        );
    }

    #[test]
    fn markers_in_code_are_literal() {
        let text = "```markdown\n<!-- memoria:export id=\"x\" -->\n```\n\n    <!-- memoria:import src=\"a/README.md#b\" -->\n\ntext\n";
        let parsed = parse(text);
        assert!(parsed.issues.is_empty(), "{:?}", parsed.issues);
        assert!(parsed.exports.is_empty());
        assert!(parsed.imports.is_empty());
    }

    #[test]
    fn crlf_and_empty_bodies() {
        let text = "<!-- memoria:export id=\"s\" -->\r\n<!-- /memoria:export -->\r\n";
        let parsed = parse(text);
        assert!(parsed.issues.is_empty(), "{:?}", parsed.issues);
        assert_eq!(parsed.exports[0].body.len(), 0);
        let text = "<!-- memoria:export id=\"s\" -->\r\nline\r\n<!-- /memoria:export -->";
        let parsed = parse(text);
        assert_eq!(
            &text.as_bytes()[parsed.exports[0].body.start..parsed.exports[0].body.end],
            b"line\r\n"
        );
    }

    #[test]
    fn reports_malformed_nested_and_unclosed_markers() {
        assert_eq!(
            parse("<!-- memoria:export id='x' -->\n<!-- /memoria:export -->\n").issues[0].code,
            "marker_malformed"
        );
        assert_eq!(
            parse("<!-- memoria:export id=\"x\" extra=\"1\" -->\n<!-- /memoria:export -->\n")
                .issues[0]
                .code,
            "marker_malformed"
        );
        assert_eq!(
            parse("<!--memoria:export id=\"x\"-->\n").issues[0].code,
            "marker_malformed"
        );
        assert_eq!(parse("<!-- memoria:export id=\"x\" -->\n<!-- memoria:import src=\"a/README.md#b\" -->\n<!-- /memoria:import -->\n").issues[0].code, "marker_nested");
        assert_eq!(
            parse("<!-- memoria:export id=\"x\" -->\n<!-- /memoria:import -->\n").issues[0].code,
            "marker_mismatch"
        );
        assert_eq!(
            parse("<!-- /memoria:export -->\n").issues[0].code,
            "marker_mismatch"
        );
        assert_eq!(
            parse("<!-- memoria:export id=\"x\" -->\ntext\n").issues[0].code,
            "marker_unclosed"
        );
        assert_eq!(
            parse("<!-- memoria:export id=\"x\" -->").issues[0].code,
            "marker_malformed"
        );
        assert_eq!(
            parse("<!-- memoria:export id=\"9x\" -->\n<!-- /memoria:export -->\n").issues[0].code,
            "export_invalid"
        );
        let dup = parse(
            "<!-- memoria:export id=\"x\" -->\n<!-- /memoria:export -->\n<!-- memoria:export id=\"x\" -->\n<!-- /memoria:export -->\n",
        );
        assert_eq!(dup.issues[0].code, "export_duplicate");
    }

    #[test]
    fn reference_definitions_outside_the_export_are_rejected() {
        for body in [
            "See [manual][guide].",
            "See [guide][].",
            "See [guide].",
            "See ![guide][img].",
        ] {
            let text = format!(
                "# Doc\n\n<!-- memoria:export id=\"s\" -->\n{body}\n<!-- /memoria:export -->\n\n[guide]: https://example.com/manual\n[img]: https://example.com/i.png\n"
            );
            let parsed = parse(&text);
            assert!(
                parsed
                    .issues
                    .iter()
                    .any(|i| i.code == "export_invalid" && i.message.contains("inline")),
                "{body}: {:?}",
                parsed.issues
            );
        }
        // Literal references inside a fenced example stay literal.
        let text = "<!-- memoria:export id=\"s\" -->\n```\n[manual][guide]\n```\n<!-- /memoria:export -->\n\n[guide]: https://example.com/manual\n";
        assert!(parse(text).issues.is_empty());
        // Reference links outside the export do not affect it.
        let text = "See [manual][guide].\n\n<!-- memoria:export id=\"s\" -->\nPlain text.\n<!-- /memoria:export -->\n\n[guide]: ../x.md\n";
        assert!(parse(text).issues.is_empty());
    }

    #[test]
    fn unresolved_references_and_definitions_are_rejected() {
        // Reference syntax without a provider definition, and definitions, are context-dependent.
        for body in [
            "See [manual][guide].",
            "See [guide][].",
            "See [guide].",
            "![alt][img]",
            "![img]",
            "[guide]: ../wrong.md",
            "  [img]: ./i.png",
        ] {
            let text =
                format!("<!-- memoria:export id=\"s\" -->\n{body}\n<!-- /memoria:export -->\n");
            let parsed = parse(&text);
            assert!(
                parsed.issues.iter().any(|i| i.code == "export_invalid"),
                "{body}: {:?}",
                parsed.issues
            );
        }
        // Escaped brackets, inline links, code spans, and code blocks stay allowed.
        let ok = "<!-- memoria:export id=\"s\" -->\nSee \\[manual\\] and [abs](https://a.b/c) and `[x][y]`.\n\n```\n[manual][guide]\n[guide]: ../x.md\n```\n\n    [indented][ref]\n<!-- /memoria:export -->\n";
        assert!(parse(ok).issues.is_empty(), "{:?}", parse(ok).issues);
        // Reference syntax outside the export does not affect it.
        let outside = "See [manual][guide].\n\n<!-- memoria:export id=\"s\" -->\nPlain.\n<!-- /memoria:export -->\n\n[guide]: ../x.md\n";
        assert!(parse(outside).issues.is_empty());
    }

    #[test]
    fn malformed_and_nested_inline_links_are_rejected() {
        for body in [
            "See [guide](not a valid URL).",
            "See [outer [guide]](https://example.com).",
            "![alt](not a valid URL)",
            "![outer [img]](https://example.com/i.png)",
            "[guide](https://example.com",
        ] {
            let text =
                format!("<!-- memoria:export id=\"s\" -->\n{body}\n<!-- /memoria:export -->\n");
            let parsed = parse(&text);
            assert!(
                parsed.issues.iter().any(|i| i.code == "export_invalid"),
                "{body}: {:?}",
                parsed.issues
            );
        }
        let ok = "<!-- memoria:export id=\"s\" -->\nSee [guide \\[v2\\]](https://example.com/a) and ![alt](https://example.com/i.png) and [`code [x]`](https://example.com/b).\n<!-- /memoria:export -->\n";
        assert!(parse(ok).issues.is_empty(), "{:?}", parse(ok).issues);
    }

    #[test]
    fn escaped_exclamation_before_a_link_is_ordinary_text() {
        let ok = "<!-- memoria:export id=\"s\" -->\nSee \\![guide](https://example.com) and \\\\![img](https://example.com/i.png).\n<!-- /memoria:export -->\n";
        let parsed = parse(ok);
        assert!(parsed.issues.is_empty(), "{:?}", parsed.issues);
        let bad = "<!-- memoria:export id=\"s\" -->\nSee \\![guide](../relative.md)\n<!-- /memoria:export -->\n";
        assert!(parse(bad).issues.iter().any(|i| i.code == "export_invalid"));
        let bad = "<!-- memoria:export id=\"s\" -->\nSee \\\\![img](not a url)\n<!-- /memoria:export -->\n";
        assert!(parse(bad).issues.iter().any(|i| i.code == "export_invalid"));
    }

    #[test]
    fn export_body_limits() {
        let bad = parse(
            "<!-- memoria:export id=\"x\" -->\n[rel](../a/README.md) ![img](./i.png) <b>x</b> [ref][r] <https://a.b>\n\n[r]: https://x.y\n<!-- /memoria:export -->\n",
        );
        let messages: Vec<&str> = bad.issues.iter().map(|i| i.message.as_str()).collect();
        assert!(
            messages.iter().any(|m| m.contains("../a/README.md")),
            "{messages:?}"
        );
        assert!(
            messages.iter().any(|m| m.contains("./i.png")),
            "{messages:?}"
        );
        assert!(
            messages.iter().any(|m| m.contains("raw HTML")),
            "{messages:?}"
        );
        assert!(
            messages
                .iter()
                .any(|m| m.contains("[ref]") || m.contains("https://x.y")),
            "{messages:?}"
        );
        assert!(
            messages.iter().any(|m| m.contains("https://a.b")),
            "{messages:?}"
        );
        let ok = parse(
            "<!-- memoria:export id=\"x\" -->\n[abs](https://a.b/c) ![i](http://a.b/i.png) [m](mailto:a@b.c)\n\n```\n<b>literal</b>\n```\n<!-- /memoria:export -->\n",
        );
        assert!(ok.issues.is_empty(), "{:?}", ok.issues);
    }
}
