//! A strict YAML subset parser for Memoria configuration files.
//!
//! Supported: block mappings, block sequences, flow sequences, the empty
//! flow mapping `{}`, plain/single-quoted/double-quoted scalars, comments,
//! and one optional leading `---`. Rejected: anchors, aliases, tags, block
//! scalars, non-empty flow mappings, complex keys, multiple documents, and
//! duplicate keys.

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Yaml {
    Null,
    Bool(bool),
    Int(i64),
    Str(String),
    Seq(Vec<Yaml>),
    Map(Vec<(String, Yaml)>),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct YamlError {
    pub line: usize,
    pub message: String,
}

impl std::fmt::Display for YamlError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "line {}: {}", self.line, self.message)
    }
}

struct Line {
    number: usize,
    indent: usize,
    text: String,
}

/// Remove a trailing ` #` comment from one line. A quote is a delimiter
/// only where a scalar starts (line start, after `- `, after `: `, after
/// `[` or `,` inside a flow sequence); a quote inside a plain scalar such
/// as `build's/**` is literal text, so the comment after it still ends
/// the scalar.
fn strip_comment(raw: &str) -> String {
    let mut out = String::new();
    let mut quote: Option<char> = None;
    let mut prev_space = true;
    // Whether the next non-space character begins a scalar.
    let mut scalar_start = true;
    // An indicator (`-` or `:`) seen; whitespace after it starts a scalar.
    let mut pending_indicator = false;
    let mut flow_depth = 0usize;
    let chars: Vec<char> = raw.chars().collect();
    let mut i = 0;
    while i < chars.len() {
        let ch = chars[i];
        match quote {
            Some('"') => {
                // A backslash escapes the next character inside double quotes.
                out.push(ch);
                if ch == '\\' {
                    if let Some(next) = chars.get(i + 1) {
                        out.push(*next);
                        i += 1;
                    }
                } else if ch == '"' {
                    quote = None;
                }
            }
            Some(_) => {
                // Inside single quotes `''` is an escaped quote.
                out.push(ch);
                if ch == '\'' {
                    if chars.get(i + 1) == Some(&'\'') {
                        out.push('\'');
                        i += 1;
                    } else {
                        quote = None;
                    }
                }
            }
            None => {
                if ch == '#' && prev_space {
                    break;
                }
                let next_is_space = chars.get(i + 1).is_none_or(|next| next.is_whitespace());
                match ch {
                    c if c.is_whitespace() => {
                        if pending_indicator {
                            scalar_start = true;
                            pending_indicator = false;
                        }
                    }
                    '\'' | '"' if scalar_start => {
                        quote = Some(ch);
                        scalar_start = false;
                    }
                    '-' if scalar_start && next_is_space => {
                        pending_indicator = true;
                        scalar_start = false;
                    }
                    ':' if next_is_space => {
                        pending_indicator = true;
                        scalar_start = false;
                    }
                    '[' if scalar_start => {
                        flow_depth += 1;
                    }
                    ']' if flow_depth > 0 => {
                        flow_depth -= 1;
                        scalar_start = false;
                    }
                    ',' if flow_depth > 0 => {
                        scalar_start = true;
                    }
                    _ => {
                        scalar_start = false;
                        pending_indicator = false;
                    }
                }
                out.push(ch);
            }
        }
        prev_space = ch.is_whitespace();
        i += 1;
    }
    out.trim_end().to_string()
}

fn err(line: usize, message: impl Into<String>) -> YamlError {
    YamlError {
        line,
        message: message.into(),
    }
}

pub fn parse(text: &str) -> Result<Yaml, YamlError> {
    let mut lines = Vec::new();
    let mut seen_document_marker = false;
    for (index, raw) in text.split('\n').enumerate() {
        let number = index + 1;
        let raw = raw.strip_suffix('\r').unwrap_or(raw);
        if raw.contains('\t')
            && raw
                .trim_start_matches(|c: char| c != '\t')
                .starts_with('\t')
            && raw.starts_with([' ', '\t'])
        {
            return Err(err(number, "tabs are not permitted in indentation"));
        }
        let content = strip_comment(raw);
        if content.trim().is_empty() {
            continue;
        }
        if content.trim() == "---" {
            if seen_document_marker || !lines.is_empty() {
                return Err(err(number, "multiple documents are not supported"));
            }
            seen_document_marker = true;
            continue;
        }
        if content.trim() == "..." {
            return Err(err(number, "document end markers are not supported"));
        }
        let indent = content.len() - content.trim_start_matches(' ').len();
        lines.push(Line {
            number,
            indent,
            text: content[indent..].to_string(),
        });
    }
    if lines.is_empty() {
        return Ok(Yaml::Null);
    }
    let mut pos = 0;
    let value = parse_block(&lines, &mut pos, lines[0].indent)?;
    if pos != lines.len() {
        return Err(err(lines[pos].number, "unexpected content"));
    }
    Ok(value)
}

fn parse_block(lines: &[Line], pos: &mut usize, indent: usize) -> Result<Yaml, YamlError> {
    let line = &lines[*pos];
    if line.text.starts_with("- ") || line.text == "-" {
        parse_sequence(lines, pos, indent)
    } else {
        parse_mapping(lines, pos, indent)
    }
}

fn parse_sequence(lines: &[Line], pos: &mut usize, indent: usize) -> Result<Yaml, YamlError> {
    let mut items = Vec::new();
    while *pos < lines.len() {
        let line = &lines[*pos];
        if line.indent < indent {
            break;
        }
        if line.indent > indent {
            return Err(err(line.number, "unexpected indentation"));
        }
        let Some(rest) = line.text.strip_prefix('-') else {
            return Err(err(line.number, "expected a sequence item"));
        };
        let rest = rest.trim_start();
        *pos += 1;
        if rest.is_empty() {
            if *pos < lines.len() && lines[*pos].indent > indent {
                let child_indent = lines[*pos].indent;
                items.push(parse_block(lines, pos, child_indent)?);
            } else {
                items.push(Yaml::Null);
            }
        } else if !(rest.starts_with('"') || rest.starts_with('\'') || rest.starts_with('['))
            && (rest.contains(": ") || rest.ends_with(':'))
        {
            return Err(err(
                line.number,
                "mappings inside sequence items are not supported",
            ));
        } else {
            items.push(parse_scalar_or_flow(rest, line.number)?);
        }
    }
    Ok(Yaml::Seq(items))
}

fn parse_mapping(lines: &[Line], pos: &mut usize, indent: usize) -> Result<Yaml, YamlError> {
    let mut entries: Vec<(String, Yaml)> = Vec::new();
    while *pos < lines.len() {
        let line = &lines[*pos];
        if line.indent < indent {
            break;
        }
        if line.indent > indent {
            return Err(err(line.number, "unexpected indentation"));
        }
        if line.text.starts_with("- ") || line.text == "-" {
            return Err(err(line.number, "expected a mapping key"));
        }
        if line.text.starts_with("? ") {
            return Err(err(line.number, "complex keys are not supported"));
        }
        let (key_raw, value_raw) = split_key(&line.text, line.number)?;
        let key = parse_key(key_raw, line.number)?;
        if entries.iter().any(|(k, _)| k == &key) {
            return Err(err(line.number, format!("duplicate key {key:?}")));
        }
        *pos += 1;
        let value = if value_raw.is_empty() {
            if *pos < lines.len() && lines[*pos].indent > indent {
                let child_indent = lines[*pos].indent;
                parse_block(lines, pos, child_indent)?
            } else if *pos < lines.len()
                && lines[*pos].indent == indent
                && (lines[*pos].text.starts_with("- ") || lines[*pos].text == "-")
            {
                // Sequences may sit at the same indentation as their key.
                parse_sequence(lines, pos, indent)?
            } else {
                Yaml::Null
            }
        } else {
            parse_scalar_or_flow(value_raw, line.number)?
        };
        entries.push((key, value));
    }
    Ok(Yaml::Map(entries))
}

fn split_key(text: &str, number: usize) -> Result<(&str, &str), YamlError> {
    let mut quote: Option<char> = None;
    for (index, ch) in text.char_indices() {
        match quote {
            Some(q) => {
                if ch == q {
                    quote = None;
                }
            }
            None => {
                if (ch == '\'' || ch == '"') && index == 0 {
                    quote = Some(ch);
                } else if ch == ':' {
                    let rest = &text[index + 1..];
                    if rest.is_empty() || rest.starts_with(' ') {
                        return Ok((&text[..index], rest.trim()));
                    }
                }
            }
        }
    }
    Err(err(number, "expected `key: value`"))
}

fn parse_key(raw: &str, number: usize) -> Result<String, YamlError> {
    let key = raw.trim();
    match parse_scalar_or_flow(key, number)? {
        Yaml::Str(s) => Ok(s),
        Yaml::Bool(b) => Ok(b.to_string()),
        Yaml::Int(i) => Ok(i.to_string()),
        Yaml::Null => Err(err(number, "empty key")),
        _ => Err(err(number, "keys must be scalars")),
    }
}

fn parse_scalar_or_flow(raw: &str, number: usize) -> Result<Yaml, YamlError> {
    let raw = raw.trim();
    if raw.starts_with('&') || raw.starts_with('*') {
        return Err(err(number, "anchors and aliases are not supported"));
    }
    if raw.starts_with('!') {
        return Err(err(number, "tags are not supported"));
    }
    if raw == "|"
        || raw == ">"
        || raw.starts_with("|-")
        || raw.starts_with(">-")
        || raw.starts_with("|+")
        || raw.starts_with(">+")
    {
        return Err(err(number, "block scalars are not supported"));
    }
    if raw.starts_with('[') {
        return parse_flow_sequence(raw, number);
    }
    if raw.starts_with('{') {
        if raw == "{}" {
            return Ok(Yaml::Map(Vec::new()));
        }
        return Err(err(number, "non-empty flow mappings are not supported"));
    }
    parse_scalar(raw, number)
}

fn parse_flow_sequence(raw: &str, number: usize) -> Result<Yaml, YamlError> {
    let Some(inner) = raw.strip_prefix('[').and_then(|s| s.strip_suffix(']')) else {
        return Err(err(number, "unterminated flow sequence"));
    };
    let mut items = Vec::new();
    let mut current = String::new();
    let mut quote: Option<char> = None;
    let mut depth = 0;
    let chars: Vec<char> = inner.chars().collect();
    let mut i = 0;
    while i < chars.len() {
        let ch = chars[i];
        i += 1;
        match quote {
            Some('"') => {
                current.push(ch);
                if ch == '\\' {
                    if let Some(next) = chars.get(i) {
                        current.push(*next);
                        i += 1;
                    }
                } else if ch == '"' {
                    quote = None;
                }
            }
            Some(_) => {
                current.push(ch);
                if ch == '\'' {
                    if chars.get(i) == Some(&'\'') {
                        current.push('\'');
                        i += 1;
                    } else {
                        quote = None;
                    }
                }
            }
            None => match ch {
                // A quote delimits an item only at its start; inside a plain
                // item (`a's`) it is literal text.
                '\'' | '"' if current.trim().is_empty() => {
                    quote = Some(ch);
                    current.push(ch);
                }
                '[' => {
                    depth += 1;
                    current.push(ch);
                }
                ']' => {
                    depth -= 1;
                    current.push(ch);
                }
                ',' if depth == 0 => {
                    items.push(std::mem::take(&mut current));
                }
                _ => current.push(ch),
            },
        }
    }
    if quote.is_some() {
        return Err(err(number, "unterminated quoted scalar"));
    }
    if !current.trim().is_empty() {
        items.push(current);
    }
    let mut out = Vec::new();
    for item in items {
        let item = item.trim();
        if item.is_empty() {
            return Err(err(number, "empty flow sequence item"));
        }
        out.push(parse_scalar_or_flow(item, number)?);
    }
    Ok(Yaml::Seq(out))
}

fn parse_scalar(raw: &str, number: usize) -> Result<Yaml, YamlError> {
    if let Some(inner) = raw.strip_prefix('\'') {
        let Some(body) = inner.strip_suffix('\'') else {
            return Err(err(number, "unterminated single-quoted scalar"));
        };
        return Ok(Yaml::Str(body.replace("''", "'")));
    }
    if let Some(inner) = raw.strip_prefix('"') {
        let Some(body) = inner.strip_suffix('"') else {
            return Err(err(number, "unterminated double-quoted scalar"));
        };
        let mut out = String::new();
        let mut chars = body.chars();
        while let Some(ch) = chars.next() {
            if ch != '\\' {
                out.push(ch);
                continue;
            }
            match chars.next() {
                Some('"') => out.push('"'),
                Some('\\') => out.push('\\'),
                Some('n') => out.push('\n'),
                Some('t') => out.push('\t'),
                Some('/') => out.push('/'),
                Some(other) => return Err(err(number, format!("unsupported escape \\{other}"))),
                None => return Err(err(number, "unterminated escape")),
            }
        }
        return Ok(Yaml::Str(out));
    }
    match raw {
        "" | "~" | "null" | "Null" | "NULL" => Ok(Yaml::Null),
        "true" | "True" | "TRUE" => Ok(Yaml::Bool(true)),
        "false" | "False" | "FALSE" => Ok(Yaml::Bool(false)),
        _ => {
            if let Ok(int) = raw.parse::<i64>()
                && !raw.starts_with('+')
                && (raw == "0" || !raw.starts_with('0'))
                && (raw == "-0" || !raw.starts_with("-0"))
            {
                return Ok(Yaml::Int(int));
            }
            Ok(Yaml::Str(raw.to_string()))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn map(entries: Vec<(&str, Yaml)>) -> Yaml {
        Yaml::Map(
            entries
                .into_iter()
                .map(|(k, v)| (k.to_string(), v))
                .collect(),
        )
    }

    #[test]
    fn parses_root_configuration_shape() {
        let text = "# comment\nversion: 1\nignore:\n  - \"**/generated/**\"\n  - '**/*.snap'\ninclude: []\ndocumentation:\n  instructions:\n    - Use Simplified English.\n  instruction_files: [\".agents/a.md\", .agents/b.md]\nfingerprints:\n  default: raw\n  languages: {}\nlint:\n  missing_import_hint: false\n";
        let value = parse(text).unwrap();
        assert_eq!(
            value,
            map(vec![
                ("version", Yaml::Int(1)),
                (
                    "ignore",
                    Yaml::Seq(vec![
                        Yaml::Str("**/generated/**".into()),
                        Yaml::Str("**/*.snap".into())
                    ])
                ),
                ("include", Yaml::Seq(vec![])),
                (
                    "documentation",
                    map(vec![
                        (
                            "instructions",
                            Yaml::Seq(vec![Yaml::Str("Use Simplified English.".into())])
                        ),
                        (
                            "instruction_files",
                            Yaml::Seq(vec![
                                Yaml::Str(".agents/a.md".into()),
                                Yaml::Str(".agents/b.md".into())
                            ])
                        ),
                    ])
                ),
                (
                    "fingerprints",
                    map(vec![
                        ("default", Yaml::Str("raw".into())),
                        ("languages", Yaml::Map(vec![]))
                    ])
                ),
                (
                    "lint",
                    map(vec![("missing_import_hint", Yaml::Bool(false))])
                ),
            ])
        );
    }

    #[test]
    fn sequence_at_key_indent_and_leading_marker() {
        let value = parse("---\nignore:\n- a\n- b\n").unwrap();
        assert_eq!(
            value,
            map(vec![(
                "ignore",
                Yaml::Seq(vec![Yaml::Str("a".into()), Yaml::Str("b".into())])
            )])
        );
    }

    #[test]
    fn rejects_unsupported_constructs() {
        assert!(
            parse("a: 1\na: 2\n")
                .unwrap_err()
                .message
                .contains("duplicate")
        );
        assert!(parse("a: &x 1\n").unwrap_err().message.contains("anchors"));
        assert!(parse("a: *x\n").unwrap_err().message.contains("anchors"));
        assert!(parse("a: !!str x\n").unwrap_err().message.contains("tags"));
        assert!(
            parse("a: |\n  text\n")
                .unwrap_err()
                .message
                .contains("block scalars")
        );
        assert!(
            parse("a: {b: 1}\n")
                .unwrap_err()
                .message
                .contains("flow mappings")
        );
        assert!(
            parse("---\na: 1\n---\nb: 2\n")
                .unwrap_err()
                .message
                .contains("multiple documents")
        );
        assert!(parse("a:\n\t- x\n").is_err());
    }

    #[test]
    fn quoted_sequence_items_may_contain_colons() {
        let block =
            parse("instructions:\n  - \"Style: use short sentences.\"\n  - 'Tone: plain.'\n")
                .unwrap();
        let flow =
            parse("instructions: [\"Style: use short sentences.\", 'Tone: plain.']\n").unwrap();
        assert_eq!(block, flow);
        assert_eq!(
            block,
            map(vec![(
                "instructions",
                Yaml::Seq(vec![
                    Yaml::Str("Style: use short sentences.".into()),
                    Yaml::Str("Tone: plain.".into())
                ])
            )])
        );
        assert!(
            parse("a:\n  - key: value\n")
                .unwrap_err()
                .message
                .contains("mappings inside sequence")
        );
        assert!(
            parse("a:\n  - trailing:\n")
                .unwrap_err()
                .message
                .contains("mappings inside sequence")
        );
    }

    #[test]
    fn comments_respect_quote_escapes() {
        assert_eq!(
            parse("a: \"Use \\\" # \\\" for headings.\"\n").unwrap(),
            map(vec![("a", Yaml::Str("Use \" # \" for headings.".into()))])
        );
        assert_eq!(
            parse("a: 'it''s # not a comment' # real comment\n").unwrap(),
            map(vec![("a", Yaml::Str("it's # not a comment".into()))])
        );
        assert_eq!(
            parse("list:\n  - \"x \\\" # y\"  # trailing\n  - '# leading'\n").unwrap(),
            map(vec![(
                "list",
                Yaml::Seq(vec![
                    Yaml::Str("x \" # y".into()),
                    Yaml::Str("# leading".into())
                ])
            )])
        );
        assert_eq!(
            parse("flow: [\"a \\\" # b\", 'c # d'] # done\n").unwrap(),
            map(vec![(
                "flow",
                Yaml::Seq(vec![
                    Yaml::Str("a \" # b".into()),
                    Yaml::Str("c # d".into())
                ])
            )])
        );
    }

    #[test]
    fn literal_quotes_inside_plain_scalars_are_not_delimiters() {
        // MEM-043: a quote that does not start a scalar is text, so the
        // trailing comment is still removed in block and flow lists.
        assert_eq!(
            parse("ignore:\n  - build's/** # generated output\n  - say \"hi\"/** # quoted\n  - \"build's/**\" # whole\n").unwrap(),
            map(vec![(
                "ignore",
                Yaml::Seq(vec![
                    Yaml::Str("build's/**".into()),
                    Yaml::Str("say \"hi\"/**".into()),
                    Yaml::Str("build's/**".into())
                ])
            )])
        );
        assert_eq!(
            parse("include: [a's, \"b # c\", it''s] # flow\n").unwrap(),
            map(vec![(
                "include",
                Yaml::Seq(vec![
                    Yaml::Str("a's".into()),
                    Yaml::Str("b # c".into()),
                    Yaml::Str("it''s".into())
                ])
            )])
        );
        assert_eq!(
            parse("note: don't shout # note\n").unwrap(),
            map(vec![("note", Yaml::Str("don't shout".into()))])
        );
        assert_eq!(
            parse("ignore:\n  - build's/**\n").unwrap(),
            parse("ignore:\n  - build's/** # generated output\n").unwrap()
        );
        // Scalar-start quotes still delimit and still must terminate.
        assert!(parse("ignore:\n  - 'unterminated # x\n").is_err());
        assert!(parse("flow: [a's, 'open]\n").is_err());
    }

    #[test]
    fn scalars() {
        assert_eq!(
            parse("a: \"x#y\"\n").unwrap(),
            map(vec![("a", Yaml::Str("x#y".into()))])
        );
        assert_eq!(
            parse("a: x #y\n").unwrap(),
            map(vec![("a", Yaml::Str("x".into()))])
        );
        assert_eq!(
            parse("a: 'it''s'\n").unwrap(),
            map(vec![("a", Yaml::Str("it's".into()))])
        );
        assert_eq!(
            parse("a: \"tab\\tnl\\n\"\n").unwrap(),
            map(vec![("a", Yaml::Str("tab\tnl\n".into()))])
        );
        assert_eq!(parse("a: ~\n").unwrap(), map(vec![("a", Yaml::Null)]));
        assert_eq!(
            parse("a: 007\n").unwrap(),
            map(vec![("a", Yaml::Str("007".into()))])
        );
        assert_eq!(
            parse("http://x: 1\n").unwrap(),
            map(vec![("http://x", Yaml::Int(1))])
        );
    }
}
