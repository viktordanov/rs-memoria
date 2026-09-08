//! Native hook configuration editing for Codex and Claude.
//!
//! This adapter is deliberately separate from the state parser, which accepts
//! only unsigned integer values. Client configuration holds arbitrary JSON
//! numbers, so this module uses `serde_json` with `arbitrary_precision` and
//! `preserve_order`, plus duplicate-key rejection. It preserves unrelated
//! values and key order; pretty serialization can still change whitespace.

use std::fmt;

use serde_json::{Map, Value};

/// Configuration byte limit: 1 MiB.
pub const MAX_CONFIGURATION_BYTES: u64 = 1024 * 1024;
/// Ownership record byte limit: 64 KiB.
pub const MAX_RECORD_BYTES: u64 = 64 * 1024;

/// The hook event Memoria owns.
pub const EVENT: &str = "Stop";
/// The synchronous handler deadline, in seconds.
pub const TIMEOUT_SECONDS: u64 = 5;
/// The native hook protocol this release speaks.
pub const PROTOCOL: u32 = 1;
/// Domain separation for the owned-entry hash.
pub const ENTRY_DOMAIN: &str = "memoria.hook-entry.v1";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConfigError {
    pub code: &'static str,
    pub message: String,
}

impl fmt::Display for ConfigError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.message)
    }
}

fn invalid(message: impl Into<String>) -> ConfigError {
    ConfigError {
        code: "hook_configuration_invalid",
        message: message.into(),
    }
}

/// Maximum nesting depth of a hook configuration document.
const MAX_DEPTH: usize = 64;

/// A strict JSON reader for client configuration.
///
/// `serde_json`'s `arbitrary_precision` feature transports a number as a
/// one-key object whose key is a private name. A configuration that
/// genuinely contains that key is then indistinguishable from a number, and
/// a deserializer rewrites the object into a number. This reader avoids the
/// ambiguity: it scans the document itself, so a number is recognized from
/// its own lexeme and every object key stays a literal key.
///
/// It preserves exact number text and key order, and rejects duplicate keys.
struct Reader<'a> {
    bytes: &'a [u8],
    position: usize,
    depth: usize,
    /// String bytes inspected while scanning. One complete pass over a
    /// document inspects each string byte once.
    examined: usize,
}

impl<'a> Reader<'a> {
    fn new(bytes: &'a [u8]) -> Reader<'a> {
        Reader {
            bytes,
            position: 0,
            depth: 0,
            examined: 0,
        }
    }

    fn error(&self, message: impl Into<String>) -> ConfigError {
        invalid(format!(
            "the configuration is not valid JSON at byte {}: {}",
            self.position,
            message.into()
        ))
    }

    fn skip_whitespace(&mut self) {
        while let Some(byte) = self.bytes.get(self.position) {
            match byte {
                b' ' | b'\t' | b'\n' | b'\r' => self.position += 1,
                _ => break,
            }
        }
    }

    fn peek(&self) -> Option<u8> {
        self.bytes.get(self.position).copied()
    }

    fn expect(&mut self, byte: u8) -> Result<(), ConfigError> {
        if self.peek() == Some(byte) {
            self.position += 1;
            Ok(())
        } else {
            Err(self.error(format!("expected {:?}", byte as char)))
        }
    }

    fn literal(&mut self, text: &str, value: Value) -> Result<Value, ConfigError> {
        if self.bytes[self.position..].starts_with(text.as_bytes()) {
            self.position += text.len();
            Ok(value)
        } else {
            Err(self.error("unrecognized literal"))
        }
    }

    fn value(&mut self) -> Result<Value, ConfigError> {
        if self.depth >= MAX_DEPTH {
            return Err(self.error("nesting is too deep"));
        }
        self.skip_whitespace();
        match self.peek() {
            None => Err(self.error("unexpected end of input")),
            Some(b'{') => self.object(),
            Some(b'[') => self.array(),
            Some(b'"') => self.string().map(Value::String),
            Some(b't') => self.literal("true", Value::Bool(true)),
            Some(b'f') => self.literal("false", Value::Bool(false)),
            Some(b'n') => self.literal("null", Value::Null),
            Some(byte) if byte == b'-' || byte.is_ascii_digit() => self.number(),
            Some(byte) => Err(self.error(format!("unexpected {:?}", byte as char))),
        }
    }

    fn object(&mut self) -> Result<Value, ConfigError> {
        self.expect(b'{')?;
        self.depth += 1;
        let mut map = Map::new();
        self.skip_whitespace();
        if self.peek() == Some(b'}') {
            self.position += 1;
            self.depth -= 1;
            return Ok(Value::Object(map));
        }
        loop {
            self.skip_whitespace();
            let key = self.string()?;
            if map.contains_key(&key) {
                return Err(invalid(format!("duplicate object key {key:?}")));
            }
            self.skip_whitespace();
            self.expect(b':')?;
            let value = self.value()?;
            map.insert(key, value);
            self.skip_whitespace();
            match self.peek() {
                Some(b',') => self.position += 1,
                Some(b'}') => {
                    self.position += 1;
                    self.depth -= 1;
                    return Ok(Value::Object(map));
                }
                _ => return Err(self.error("expected ',' or '}'")),
            }
        }
    }

    fn array(&mut self) -> Result<Value, ConfigError> {
        self.expect(b'[')?;
        self.depth += 1;
        let mut items = Vec::new();
        self.skip_whitespace();
        if self.peek() == Some(b']') {
            self.position += 1;
            self.depth -= 1;
            return Ok(Value::Array(items));
        }
        loop {
            items.push(self.value()?);
            self.skip_whitespace();
            match self.peek() {
                Some(b',') => self.position += 1,
                Some(b']') => {
                    self.position += 1;
                    self.depth -= 1;
                    return Ok(Value::Array(items));
                }
                _ => return Err(self.error("expected ',' or ']'")),
            }
        }
    }

    fn string(&mut self) -> Result<String, ConfigError> {
        self.expect(b'"')?;
        let mut out = String::new();
        loop {
            let byte = self
                .peek()
                .ok_or_else(|| self.error("unterminated string"))?;
            match byte {
                b'"' => {
                    self.position += 1;
                    return Ok(out);
                }
                b'\\' => {
                    self.position += 1;
                    let escape = self
                        .peek()
                        .ok_or_else(|| self.error("unterminated escape"))?;
                    self.position += 1;
                    match escape {
                        b'"' => out.push('"'),
                        b'\\' => out.push('\\'),
                        b'/' => out.push('/'),
                        b'b' => out.push('\u{8}'),
                        b'f' => out.push('\u{c}'),
                        b'n' => out.push('\n'),
                        b'r' => out.push('\r'),
                        b't' => out.push('\t'),
                        b'u' => {
                            let character = self.unicode_escape()?;
                            out.push(character);
                        }
                        _ => return Err(self.error("unrecognized escape")),
                    }
                }
                byte if byte < 0x20 => {
                    return Err(self.error("a control character must be escaped"));
                }
                _ => {
                    // Copy the whole run of ordinary characters at once.
                    // `parse_json` validated the document as UTF-8, and every
                    // byte that ends a run is ASCII, so the run is one
                    // complete UTF-8 slice. Each byte belongs to one run, so
                    // the total scanning work stays linear in the input.
                    let rest = &self.bytes[self.position..];
                    let run = rest
                        .iter()
                        .position(|byte| matches!(byte, b'"' | b'\\') || *byte < 0x20)
                        .unwrap_or(rest.len());
                    self.examined += run;
                    let text = std::str::from_utf8(&rest[..run])
                        .map_err(|_| self.error("invalid UTF-8"))?;
                    out.push_str(text);
                    self.position += run;
                }
            }
        }
    }

    fn unicode_escape(&mut self) -> Result<char, ConfigError> {
        let first = self.hex4()?;
        if (0xd800..0xdc00).contains(&first) {
            // A leading surrogate needs its trailing half.
            if self.peek() != Some(b'\\') {
                return Err(self.error("a leading surrogate needs a trailing surrogate"));
            }
            self.position += 1;
            self.expect(b'u')?;
            let second = self.hex4()?;
            if !(0xdc00..0xe000).contains(&second) {
                return Err(self.error("an invalid trailing surrogate"));
            }
            let combined = 0x10000 + ((first - 0xd800) as u32) * 0x400 + (second - 0xdc00) as u32;
            return char::from_u32(combined).ok_or_else(|| self.error("an invalid surrogate pair"));
        }
        char::from_u32(first as u32).ok_or_else(|| self.error("an invalid escape"))
    }

    /// Read exactly four ASCII hexadecimal digits.
    ///
    /// Integer parsing alone is too permissive here: Rust accepts a leading
    /// plus sign, which JSON does not permit in a Unicode escape.
    fn hex4(&mut self) -> Result<u16, ConfigError> {
        let slice = self
            .bytes
            .get(self.position..self.position + 4)
            .ok_or_else(|| self.error("a short unicode escape"))?;
        let mut value: u16 = 0;
        for byte in slice {
            let digit = match byte {
                b'0'..=b'9' => byte - b'0',
                b'a'..=b'f' => byte - b'a' + 10,
                b'A'..=b'F' => byte - b'A' + 10,
                _ => {
                    return Err(self.error("a unicode escape needs four hexadecimal digits"));
                }
            };
            value = (value << 4) | u16::from(digit);
        }
        self.position += 4;
        Ok(value)
    }

    /// Read one number and keep its exact source text.
    fn number(&mut self) -> Result<Value, ConfigError> {
        let start = self.position;
        if self.peek() == Some(b'-') {
            self.position += 1;
        }
        fn digits(reader: &mut Reader<'_>) -> usize {
            let from = reader.position;
            while reader.peek().is_some_and(|b| b.is_ascii_digit()) {
                reader.position += 1;
            }
            reader.position - from
        }
        if digits(self) == 0 {
            return Err(self.error("a number needs at least one digit"));
        }
        if self.peek() == Some(b'.') {
            self.position += 1;
            if digits(self) == 0 {
                return Err(self.error("a fraction needs at least one digit"));
            }
        }
        if matches!(self.peek(), Some(b'e') | Some(b'E')) {
            self.position += 1;
            if matches!(self.peek(), Some(b'+') | Some(b'-')) {
                self.position += 1;
            }
            if digits(self) == 0 {
                return Err(self.error("an exponent needs at least one digit"));
            }
        }
        let lexeme = std::str::from_utf8(&self.bytes[start..self.position])
            .map_err(|_| self.error("an invalid number"))?;
        // `arbitrary_precision` keeps the exact text of this lexeme.
        serde_json::from_str::<Value>(lexeme)
            .map_err(|err| invalid(format!("number {lexeme:?}: {err}")))
    }

    fn finish(&mut self) -> Result<(), ConfigError> {
        self.skip_whitespace();
        if self.position == self.bytes.len() {
            Ok(())
        } else {
            Err(self.error("trailing content after the document"))
        }
    }
}

/// Parse configuration JSON, rejecting duplicate keys and oversized input.
pub fn parse_json(bytes: &[u8]) -> Result<Value, ConfigError> {
    if bytes.len() as u64 > MAX_CONFIGURATION_BYTES {
        return Err(ConfigError {
            code: "hook_configuration_too_large",
            message: format!(
                "the configuration is {} bytes, above the limit of {MAX_CONFIGURATION_BYTES}",
                bytes.len()
            ),
        });
    }
    let text =
        std::str::from_utf8(bytes).map_err(|_| invalid("the configuration is not valid UTF-8"))?;
    if text.trim().is_empty() {
        return Ok(Value::Object(Map::new()));
    }
    let mut reader = Reader::new(text.as_bytes());
    let value = reader.value()?;
    reader.finish()?;
    Ok(value)
}

/// Parse and report how many string bytes the reader inspected.
#[cfg(test)]
fn parse_json_examined(bytes: &[u8]) -> Result<(Value, usize), ConfigError> {
    let text =
        std::str::from_utf8(bytes).map_err(|_| invalid("the configuration is not valid UTF-8"))?;
    let mut reader = Reader::new(text.as_bytes());
    let value = reader.value()?;
    reader.finish()?;
    Ok((value, reader.examined))
}

/// Serialize configuration JSON with two-space indentation and a newline.
pub fn write_json(value: &Value) -> Result<Vec<u8>, ConfigError> {
    let mut out = serde_json::to_vec_pretty(value)
        .map_err(|err| invalid(format!("cannot serialize the configuration: {err}")))?;
    out.push(b'\n');
    Ok(out)
}

/// The canonical JSON encoding used by the owned-entry hash: object keys
/// sorted, array order preserved, no optional whitespace.
///
/// The string encoder emits exact UTF-8 without Unicode normalization. It
/// escapes quotes, backslashes, and controls with the standard short escapes
/// where available; other U+0000–U+001F controls use lowercase `\u00xx`. It
/// does not escape `/` or any other Unicode character.
pub fn canonical_json(value: &Value) -> String {
    let mut out = String::new();
    encode_canonical(value, &mut out);
    out
}

fn encode_canonical(value: &Value, out: &mut String) {
    match value {
        Value::Null => out.push_str("null"),
        Value::Bool(true) => out.push_str("true"),
        Value::Bool(false) => out.push_str("false"),
        Value::Number(number) => out.push_str(&number.to_string()),
        Value::String(text) => encode_canonical_string(text, out),
        Value::Array(items) => {
            out.push('[');
            for (index, item) in items.iter().enumerate() {
                if index > 0 {
                    out.push(',');
                }
                encode_canonical(item, out);
            }
            out.push(']');
        }
        Value::Object(map) => {
            let mut keys: Vec<&String> = map.keys().collect();
            keys.sort_by(|a, b| a.as_bytes().cmp(b.as_bytes()));
            out.push('{');
            for (index, key) in keys.into_iter().enumerate() {
                if index > 0 {
                    out.push(',');
                }
                encode_canonical_string(key, out);
                out.push(':');
                encode_canonical(&map[key], out);
            }
            out.push('}');
        }
    }
}

fn encode_canonical_string(text: &str, out: &mut String) {
    out.push('"');
    for character in text.chars() {
        match character {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\u{8}' => out.push_str("\\b"),
            '\u{c}' => out.push_str("\\f"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out.push('"');
}

/// The owned-entry hash: `memoria.hook-entry.v1` and the canonical JSON
/// encoding, each with the canonical encoder's length prefix.
pub fn entry_hash(entry: &Value) -> String {
    let json = canonical_json(entry);
    let mut bytes = Vec::new();
    let push = |value: &[u8], bytes: &mut Vec<u8>| {
        bytes.extend_from_slice(&(value.len() as u64).to_be_bytes());
        bytes.extend_from_slice(value);
    };
    push(ENTRY_DOMAIN.as_bytes(), &mut bytes);
    push(json.as_bytes(), &mut bytes);
    format!("{:016x}", crate::hash::xxh3_64(&bytes))
}

/// Quote one argument for a POSIX shell with single quotes.
pub fn shell_quote(value: &str) -> String {
    let mut out = String::with_capacity(value.len() + 2);
    out.push('\'');
    for character in value.chars() {
        if character == '\'' {
            // Close the quote, emit an escaped quote, reopen.
            out.push_str("'\\''");
        } else {
            out.push(character);
        }
    }
    out.push('\'');
    out
}

/// The exact command the owned handler runs.
///
/// The wrapper captures the runner's output before it writes stdout. On
/// executable failure it emits `{}` and exits 0, so an older executable's
/// usage exit never becomes the client's continuation signal.
pub fn hook_command(executable: &str, target: &str, configuration_root: &str) -> String {
    format!(
        "memoria_hook_output=$({executable} agent hook run --target {target} --protocol {PROTOCOL} --configuration-root {root} 2>/dev/null) && printf '%s\\n' \"$memoria_hook_output\" || printf '{{}}\\n'",
        executable = shell_quote(executable),
        root = shell_quote(configuration_root),
    )
}

/// The owned native group: one `Stop` group with one command handler and no
/// matcher. The hook is synchronous, so no asynchronous result can enter a
/// later model turn.
pub fn owned_entry(command: &str) -> Value {
    let mut handler = Map::new();
    handler.insert("type".into(), Value::String("command".into()));
    handler.insert("command".into(), Value::String(command.to_string()));
    handler.insert("timeout".into(), Value::from(TIMEOUT_SECONDS));
    let mut group = Map::new();
    group.insert("hooks".into(), Value::Array(vec![Value::Object(handler)]));
    Value::Object(group)
}

/// Whether a group looks like Memoria's owned handler shape. Used only to
/// report an unmanaged look-alike, never to adopt it.
pub fn resembles_owned(group: &Value) -> bool {
    group
        .get("hooks")
        .and_then(Value::as_array)
        .is_some_and(|handlers| {
            handlers.iter().any(|handler| {
                handler
                    .get("command")
                    .and_then(Value::as_str)
                    .is_some_and(|command| command.contains("agent hook run"))
            })
        })
}

/// The `Stop` array of a configuration, when one exists.
pub fn stop_groups(configuration: &Value) -> Option<&Vec<Value>> {
    configuration.get("hooks")?.get(EVENT)?.as_array()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_literal_private_number_key_stays_a_literal_object() {
        // `serde_json`'s arbitrary-precision transport uses this key name.
        // A configuration that genuinely contains it must keep its object.
        for source in [
            br#"{"custom":{"$serde_json::private::Number":"123"}}"#.as_slice(),
            br#"{"$serde_json::private::Number":"123"}"#.as_slice(),
            br#"{"$serde_json::private::Number":"123","other":1}"#.as_slice(),
            br#"{"a":{"b":{"$serde_json::private::Number":"7"}}}"#.as_slice(),
            br#"[{"$serde_json::private::Number":"9"}]"#.as_slice(),
        ] {
            let value = parse_json(source)
                .unwrap_or_else(|e| panic!("{}: {e}", String::from_utf8_lossy(source)));
            let written = write_json(&value).unwrap();
            let again = parse_json(&written).unwrap();
            assert_eq!(
                canonical_json(&again),
                canonical_json(&value),
                "{}",
                String::from_utf8_lossy(source)
            );
            assert!(
                canonical_json(&value).contains("$serde_json::private::Number"),
                "the literal key survives: {}",
                canonical_json(&value)
            );
            assert!(
                !canonical_json(&value).contains(":123}")
                    || canonical_json(&value).contains("\"123\""),
                "the value stayed a string: {}",
                canonical_json(&value)
            );
        }
    }

    #[test]
    fn strict_json_rejects_malformed_documents() {
        for source in [
            &b"{"[..],
            b"{\"a\":}",
            b"{\"a\" 1}",
            b"[1,]",
            b"{,}",
            b"01",
            b"1.",
            b"1e",
            b"-",
            b"tru",
            b"{\"a\":1} trailing",
            b"\"unterminated",
            b"{\"a\":\"\x01\"}",
        ] {
            assert!(
                parse_json(source).is_err(),
                "accepted {}",
                String::from_utf8_lossy(source)
            );
        }
        // Deep nesting is bounded rather than recursing without limit.
        let deep = format!("{}1{}", "[".repeat(200), "]".repeat(200));
        assert!(parse_json(deep.as_bytes()).is_err());
    }

    #[test]
    fn a_unicode_escape_needs_four_hexadecimal_digits() {
        // Integer parsing accepts a leading plus sign. JSON does not.
        for source in [
            br#"{"custom":"\u+001"}"#.as_slice(),
            br#"{"custom":"\u+0001"}"#.as_slice(),
            br#"{"custom":"\u 001"}"#.as_slice(),
            br#"{"custom":"\u-001"}"#.as_slice(),
            br#"{"custom":"\u00"}"#.as_slice(),
            br#"{"custom":"\u00g1"}"#.as_slice(),
            br#"{"custom":"\u0_01"}"#.as_slice(),
            // A leading surrogate without its trailing half.
            br#"{"custom":"\ud83d"}"#.as_slice(),
            br#"{"custom":"\ud83d\u0041"}"#.as_slice(),
            // A lone trailing surrogate is not a character.
            br#"{"custom":"\udc00"}"#.as_slice(),
            // The trailing half also needs four hexadecimal digits.
            br#"{"custom":"\ud83d\u+c00"}"#.as_slice(),
        ] {
            assert!(
                parse_json(source).is_err(),
                "accepted {}",
                String::from_utf8_lossy(source)
            );
        }
        // Valid escapes still decode, including a surrogate pair.
        let value = parse_json(br#"{"a":"\u0041\ud83d\ude00\u00e9"}"#).unwrap();
        assert_eq!(value["a"].as_str().unwrap(), "A\u{1f600}\u{e9}");
    }

    #[test]
    fn string_scanning_work_grows_with_the_input_not_its_square() {
        // The reader inspects each string byte once. A repeated scan of the
        // remaining document would grow with the square of the input.
        let mut previous = 0usize;
        for count in [1_000usize, 10_000, 100_000] {
            let source = format!("{{\"custom\":\"{}\"}}", "a".repeat(count));
            let (value, examined) = parse_json_examined(source.as_bytes()).unwrap();
            assert_eq!(value["custom"].as_str().unwrap().len(), count);
            assert!(
                examined <= 2 * source.len(),
                "{count} characters inspected {examined} bytes of {}",
                source.len()
            );
            assert!(examined > previous, "the counter tracks real work");
            previous = examined;
        }
        // Multibyte and escaped content stays linear too.
        let source = format!(
            "{{\"custom\":\"{}\"}}",
            "\u{e9}\u{1f600}x\\n".repeat(20_000)
        );
        let (_, examined) = parse_json_examined(source.as_bytes()).unwrap();
        assert!(
            examined <= 2 * source.len(),
            "inspected {examined} bytes of {}",
            source.len()
        );
    }

    #[test]
    fn strings_and_escapes_round_trip_exactly() {
        let source = concat!(
            "{\"escapes\":\"a\\\"b\\\\c\\/d\\be\\ff\\ng\\rh\\ti\",",
            "\"unicode\":\"\\u00e9\\ud83d\\ude00\",",
            "\"plain\":\"caf\\u00e9\"}"
        );
        let value = parse_json(source.as_bytes()).unwrap();
        let written = write_json(&value).unwrap();
        let again = parse_json(&written).unwrap();
        assert_eq!(canonical_json(&again), canonical_json(&value));
        let canonical = canonical_json(&value);
        assert!(canonical.contains('\u{1f600}'), "{canonical}");
        assert!(canonical.contains('\u{e9}'), "{canonical}");
        // The short escapes decoded to their real characters.
        assert!(canonical.contains("\\n"), "{canonical}");
        assert!(canonical.contains("\\t"), "{canonical}");
        assert!(
            canonical.contains("/d"),
            "the solidus escape decoded: {canonical}"
        );
    }

    #[test]
    fn duplicate_keys_are_rejected_and_numbers_keep_their_exact_text() {
        assert!(parse_json(br#"{"a":1,"a":2}"#).is_err());
        let value = parse_json(br#"{"big":123456789012345678901234567890,"neg":-2.5e-8}"#).unwrap();
        assert_eq!(
            canonical_json(&value),
            r#"{"big":123456789012345678901234567890,"neg":-2.5e-8}"#
        );
        // The exact source text survives a parse and serialize round trip.
        let written = write_json(&value).unwrap();
        let again = parse_json(&written).unwrap();
        assert_eq!(canonical_json(&again), canonical_json(&value));
    }

    #[test]
    fn canonical_encoding_sorts_keys_and_preserves_array_order() {
        let value = parse_json(br#"{"b":[3,1,2],"a":{"z":1,"y":2}}"#).unwrap();
        assert_eq!(canonical_json(&value), r#"{"a":{"y":2,"z":1},"b":[3,1,2]}"#);
    }

    #[test]
    fn string_escapes_follow_the_documented_rules() {
        let value = Value::String("a\"b\\c/d\ne\u{1}f\u{e9}\u{1f600}".to_string());
        assert_eq!(
            canonical_json(&value),
            "\"a\\\"b\\\\c/d\\ne\\u0001f\u{e9}\u{1f600}\""
        );
    }

    #[test]
    fn shell_quoting_survives_quotes_and_metacharacters() {
        assert_eq!(shell_quote("/usr/bin/memoria"), "'/usr/bin/memoria'");
        assert_eq!(shell_quote("with space"), "'with space'");
        assert_eq!(shell_quote("it's"), "'it'\\''s'");
        assert_eq!(shell_quote("a;rm -rf /"), "'a;rm -rf /'");
        assert_eq!(shell_quote("$(x)`y`"), "'$(x)`y`'");
    }

    #[test]
    fn the_command_wrapper_absorbs_failure_with_an_empty_object() {
        let command = hook_command("/opt/mem oria", "codex", "/work/it's");
        assert!(command.contains("'/opt/mem oria' agent hook run --target codex --protocol 1"));
        assert!(command.contains("--configuration-root '/work/it'\\''s'"));
        assert!(command.ends_with("|| printf '{}\\n'"));
        assert!(command.contains("2>/dev/null"));
    }

    #[test]
    fn the_entry_hash_changes_with_the_command_only() {
        let a = owned_entry("one");
        let b = owned_entry("two");
        assert_ne!(entry_hash(&a), entry_hash(&b));
        assert_eq!(entry_hash(&a), entry_hash(&owned_entry("one")));
        assert_eq!(entry_hash(&a).len(), 16);
        assert_eq!(
            canonical_json(&a),
            r#"{"hooks":[{"command":"one","timeout":5,"type":"command"}]}"#
        );
    }

    #[test]
    fn a_look_alike_entry_is_recognized_but_never_adopted() {
        let user = parse_json(
            br#"{"hooks":[{"type":"command","command":"memoria agent hook run --target codex"}]}"#,
        )
        .unwrap();
        assert!(resembles_owned(&user));
        assert_ne!(entry_hash(&user), entry_hash(&owned_entry("x")));
    }
}
