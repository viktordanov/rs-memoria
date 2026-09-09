//! Lossless human diagnostics; JSON remains the application projection.
use std::fmt::Write as _;
use std::io::IsTerminal as _;

use memoria_application::error::{Detail, Diagnostic};
use unicode_width::UnicodeWidthStr as _;

#[derive(Clone, Copy)]
pub struct HumanOptions {
    pub width: usize,
}

impl HumanOptions {
    pub fn stderr() -> Self {
        Self {
            width: if std::io::stderr().is_terminal() {
                terminal_size::terminal_size_of(std::io::stderr())
                    .map(|(width, _)| usize::from(width.0))
                    .unwrap_or(80)
            } else {
                80
            },
        }
    }
}

fn prose(out: &mut String, value: &str, indent: &str, options: HumanOptions) {
    let width = if options.width == 0 {
        80
    } else {
        options.width
    };
    for line in value.split('\n') {
        out.push_str(indent);
        let mut used = indent.width();
        // Commands, JSON, and hunks remain byte-preserving blocks.
        if line.starts_with(['{', '[', '@', ' '])
            || line.contains('`')
            || line.starts_with("memoria ")
        {
            out.push_str(line);
        } else {
            for word in line.split_whitespace() {
                if used > indent.width() {
                    if used + 1 + word.width() > width {
                        out.push('\n');
                        out.push_str(indent);
                        used = indent.width();
                    } else {
                        out.push(' ');
                        used += 1;
                    }
                }
                out.push_str(word);
                used += word.width();
            }
        }
        out.push('\n');
    }
}

/// Evidence strings remain exact, including multiline notes, paths and hunks.
pub fn detail(out: &mut String, value: &Detail, depth: usize) {
    let indent = "  ".repeat(depth);
    match value {
        Detail::Map(map) if !map.is_empty() => {
            for (key, value) in map {
                match value {
                    Detail::Map(child) if !child.is_empty() => {
                        let _ = writeln!(out, "{indent}{key}:");
                        detail(out, value, depth + 1);
                    }
                    Detail::List(child) if !child.is_empty() => {
                        let _ = writeln!(out, "{indent}{key}:");
                        detail(out, value, depth + 1);
                    }
                    _ => {
                        let _ = write!(out, "{indent}{key}: ");
                        detail(out, value, 0);
                    }
                }
            }
        }
        Detail::List(items) if !items.is_empty() => {
            for item in items {
                let _ = writeln!(out, "{indent}-");
                detail(out, item, depth + 1);
            }
        }
        Detail::Text(text) => {
            let _ = writeln!(out, "{indent}{text}");
        }
        other => {
            let text = match other {
                Detail::Null => "null".into(),
                Detail::Bool(value) => value.to_string(),
                Detail::Number(value) => value.to_string(),
                Detail::Map(_) => "{}".into(),
                Detail::List(_) => "[]".into(),
                Detail::Text(_) => unreachable!(),
            };
            let _ = writeln!(out, "{indent}{text}");
        }
    }
}

pub fn render_diagnostics(diagnostics: &[Diagnostic], options: HumanOptions) -> String {
    let mut out = String::new();
    let mut emitted = std::collections::BTreeSet::new();
    for diagnostic in diagnostics {
        let grouped = matches!(
            diagnostic.code.as_str(),
            "navigation_disconnected" | "missing_import_hint"
        );
        if grouped && !emitted.insert((diagnostic.code.as_str(), diagnostic.severity)) {
            continue;
        }
        let items: Vec<_> = if grouped {
            diagnostics
                .iter()
                .filter(|item| item.code == diagnostic.code && item.severity == diagnostic.severity)
                .collect()
        } else {
            vec![diagnostic]
        };
        let _ = write!(
            out,
            "{} [{}]",
            diagnostic.severity.as_str(),
            diagnostic.code
        );
        if grouped {
            let _ = write!(out, " — {} item(s)", items.len());
        }
        out.push('\n');
        for item in items {
            if let Some(path) = &item.path {
                let _ = writeln!(out, "  Path: {path}");
            }
            if let Some(line) = item.line {
                let _ = writeln!(out, "  Line: {line}");
            }
            if let Some(column) = item.column {
                let _ = writeln!(out, "  Column: {column}");
            }
            prose(&mut out, &item.message, "  ", options);
            detail(&mut out, &item.details, 1);
        }
        let action = match diagnostic.code.as_str() {
            "summary_invalid" => Some(
                "Use --summary for counts or --explain README.md for the path-selection explanation.",
            ),
            "navigation_disconnected" => {
                Some("Add a normal link from an already reachable README.")
            }
            "missing_import_hint" => Some(
                "Add an import only when the provider's summary belongs here. For navigation-only links, disable optional hints with [lint] missing_import_hint = false.",
            ),
            "note_invalid" => Some(
                "Explain why this README is correct for this packet. After trim: 12–1000 Unicode characters and at least three whitespace-separated words. CR/LF are allowed; tabs and other controls are forbidden. Generic notes are rejected: done, reviewed, looks good, ok, okay, lgtm, fine, no changes, no change, updated.",
            ),
            _ => None,
        };
        if let Some(action) = action {
            out.push_str("Next action:\n");
            prose(&mut out, action, "  ", options);
        }
        out.push('\n');
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use memoria_application::error::DetailMap;

    #[test]
    fn prose_wraps_by_display_width_with_hanging_indentation() {
        let diagnostic = Diagnostic::error(
            "summary_invalid",
            "--summary emits bounded counts only; it cannot be combined with --explain",
        );
        for width in [40, 80, 120] {
            let out = render_diagnostics(std::slice::from_ref(&diagnostic), HumanOptions { width });
            assert!(out.lines().all(|line| line.width() <= width));
            for word in diagnostic.message.split_whitespace() {
                assert!(out.contains(word));
            }
        }
        let out = render_diagnostics(
            &[Diagnostic::warning(
                "unicode",
                "漢字 漢字 漢字 漢字 漢字 漢字 漢字 漢字 漢字 漢字",
            )],
            HumanOptions { width: 20 },
        );
        assert!(out.lines().all(|line| line.width() <= 20));
    }

    #[test]
    fn preserves_recursive_evidence_at_every_width() {
        let hunk = "@@ -1 +1 @@\n-before\n+after\n";
        let evidence = DetailMap::default()
            .with(
                "nested",
                Detail::list([DetailMap::default()
                    .with("empty", Detail::List(vec![]))
                    .with("null", Detail::Null)
                    .bool("false", false)
                    .number("zero", 0)
                    .text("hunk", hunk)
                    .build()]),
            )
            .build();
        for width in [40, 80, 120] {
            let rendered = render_diagnostics(&[Diagnostic::warning("unknown", "Unicode 漢字 and a long explanation that wraps without losing any words in the message").with_details(evidence.clone())], HumanOptions { width });
            for value in ["[]", "null", "false", "0", hunk, "漢字"] {
                assert!(rendered.contains(value));
            }
        }
    }

    #[test]
    fn groups_only_matching_code_and_severity() {
        let items = [
            Diagnostic::warning("navigation_disconnected", "first").at_path("a/README.md"),
            Diagnostic::hint("other", "middle"),
            Diagnostic::warning("navigation_disconnected", "second").at_path("b/README.md"),
        ];
        let out = render_diagnostics(&items, HumanOptions { width: 40 });
        assert_eq!(out.matches("[navigation_disconnected]").count(), 1);
        for value in ["first", "second", "a/README.md", "b/README.md", "middle"] {
            assert!(out.contains(value));
        }
    }
}
