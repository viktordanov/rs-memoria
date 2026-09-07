//! Bounded strict JSON parsing and canonical serialization.
//!
//! The parser rejects duplicate keys, nesting deeper than the configured
//! limit, more array elements than the configured limit, numbers that are not
//! unsigned 64-bit integers, and trailing values. The serializer writes sorted
//! keys, two-space indentation, and a final line feed.

use std::collections::BTreeMap;
use std::fmt::Write as _;

use memoria_application::error::Detail;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Json {
    Null,
    Bool(bool),
    Number(u64),
    String(String),
    Array(Vec<Json>),
    Object(BTreeMap<String, Json>),
}

#[derive(Debug, Clone, Copy)]
pub struct Limits {
    pub max_depth: usize,
    pub max_records: u64,
}

impl Limits {
    pub const PACKET: Limits = Limits {
        max_depth: 32,
        max_records: 100_000,
    };
    pub const STATE: Limits = Limits {
        max_depth: 32,
        max_records: 50_000_000,
    };
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JsonError {
    pub code: &'static str,
    pub message: String,
}

impl JsonError {
    fn new(code: &'static str, message: impl Into<String>) -> JsonError {
        JsonError {
            code,
            message: message.into(),
        }
    }
}

impl std::fmt::Display for JsonError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.message)
    }
}

struct Parser<'a> {
    bytes: &'a [u8],
    pos: usize,
    depth: usize,
    records: u64,
    limits: Limits,
}

pub fn parse(bytes: &[u8], limits: Limits) -> Result<Json, JsonError> {
    if std::str::from_utf8(bytes).is_err() {
        return Err(JsonError::new("json_invalid", "input is not valid UTF-8"));
    }
    let mut parser = Parser {
        bytes,
        pos: 0,
        depth: 0,
        records: 0,
        limits,
    };
    parser.skip_ws();
    let value = parser.value()?;
    parser.skip_ws();
    if parser.pos != parser.bytes.len() {
        return Err(JsonError::new(
            "json_trailing",
            format!("unexpected trailing content at byte {}", parser.pos),
        ));
    }
    Ok(value)
}

impl Parser<'_> {
    fn skip_ws(&mut self) {
        while self.pos < self.bytes.len()
            && matches!(self.bytes[self.pos], b' ' | b'\t' | b'\n' | b'\r')
        {
            self.pos += 1;
        }
    }

    fn peek(&self) -> Option<u8> {
        self.bytes.get(self.pos).copied()
    }

    fn expect(&mut self, byte: u8) -> Result<(), JsonError> {
        if self.peek() == Some(byte) {
            self.pos += 1;
            Ok(())
        } else {
            Err(JsonError::new(
                "json_invalid",
                format!("expected {:?} at byte {}", byte as char, self.pos),
            ))
        }
    }

    fn enter(&mut self) -> Result<(), JsonError> {
        if self.depth >= self.limits.max_depth {
            return Err(JsonError::new(
                "json_depth",
                format!("nesting exceeds {} containers", self.limits.max_depth),
            ));
        }
        self.depth += 1;
        Ok(())
    }

    fn leave(&mut self) {
        self.depth -= 1;
    }

    fn value(&mut self) -> Result<Json, JsonError> {
        match self.peek() {
            None => Err(JsonError::new("json_invalid", "unexpected end of input")),
            Some(b'{') => self.object(),
            Some(b'[') => self.array(),
            Some(b'"') => self.string().map(Json::String),
            Some(b't') => self.literal("true", Json::Bool(true)),
            Some(b'f') => self.literal("false", Json::Bool(false)),
            Some(b'n') => self.literal("null", Json::Null),
            Some(b'0'..=b'9') => self.number(),
            Some(b'-') => Err(JsonError::new(
                "json_number",
                format!("negative numbers are not permitted at byte {}", self.pos),
            )),
            Some(other) => Err(JsonError::new(
                "json_invalid",
                format!("unexpected byte {:?} at {}", other as char, self.pos),
            )),
        }
    }

    fn literal(&mut self, text: &str, value: Json) -> Result<Json, JsonError> {
        if self.bytes[self.pos..].starts_with(text.as_bytes()) {
            self.pos += text.len();
            Ok(value)
        } else {
            Err(JsonError::new(
                "json_invalid",
                format!("invalid literal at byte {}", self.pos),
            ))
        }
    }

    fn number(&mut self) -> Result<Json, JsonError> {
        let start = self.pos;
        while self.pos < self.bytes.len() && self.bytes[self.pos].is_ascii_digit() {
            self.pos += 1;
        }
        let digits = &self.bytes[start..self.pos];
        if digits.len() > 1 && digits[0] == b'0' {
            return Err(JsonError::new(
                "json_number",
                format!("leading zeros at byte {start}"),
            ));
        }
        if matches!(self.peek(), Some(b'.') | Some(b'e') | Some(b'E')) {
            return Err(JsonError::new(
                "json_number",
                format!(
                    "fractions and exponents are not permitted at byte {}",
                    self.pos
                ),
            ));
        }
        let text = std::str::from_utf8(digits).expect("ascii digits");
        text.parse::<u64>().map(Json::Number).map_err(|_| {
            JsonError::new(
                "json_number",
                format!("number at byte {start} exceeds 64 bits"),
            )
        })
    }

    fn string(&mut self) -> Result<String, JsonError> {
        self.expect(b'"')?;
        let mut out = String::new();
        loop {
            let Some(byte) = self.peek() else {
                return Err(JsonError::new("json_invalid", "unterminated string"));
            };
            self.pos += 1;
            match byte {
                b'"' => return Ok(out),
                b'\\' => {
                    let Some(escape) = self.peek() else {
                        return Err(JsonError::new("json_invalid", "unterminated escape"));
                    };
                    self.pos += 1;
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
                            let first = self.hex4()?;
                            let ch = if (0xD800..0xDC00).contains(&first) {
                                if self.bytes[self.pos..].starts_with(b"\\u") {
                                    self.pos += 2;
                                    let second = self.hex4()?;
                                    if !(0xDC00..0xE000).contains(&second) {
                                        return Err(JsonError::new(
                                            "json_invalid",
                                            "invalid low surrogate",
                                        ));
                                    }
                                    let code =
                                        0x10000 + ((first - 0xD800) << 10) + (second - 0xDC00);
                                    char::from_u32(code).ok_or_else(|| {
                                        JsonError::new("json_invalid", "invalid surrogate pair")
                                    })?
                                } else {
                                    return Err(JsonError::new(
                                        "json_invalid",
                                        "lone high surrogate",
                                    ));
                                }
                            } else if (0xDC00..0xE000).contains(&first) {
                                return Err(JsonError::new("json_invalid", "lone low surrogate"));
                            } else {
                                char::from_u32(first).ok_or_else(|| {
                                    JsonError::new("json_invalid", "invalid unicode escape")
                                })?
                            };
                            out.push(ch);
                        }
                        other => {
                            return Err(JsonError::new(
                                "json_invalid",
                                format!("invalid escape \\{}", other as char),
                            ));
                        }
                    }
                }
                0x00..=0x1f => {
                    return Err(JsonError::new(
                        "json_invalid",
                        "control character in string",
                    ));
                }
                _ => {
                    // Copy one UTF-8 sequence.
                    let start = self.pos - 1;
                    let width = utf8_width(byte);
                    let end = start + width;
                    if end > self.bytes.len() {
                        return Err(JsonError::new("json_invalid", "truncated UTF-8 sequence"));
                    }
                    out.push_str(
                        std::str::from_utf8(&self.bytes[start..end])
                            .map_err(|_| JsonError::new("json_invalid", "invalid UTF-8"))?,
                    );
                    self.pos = end;
                }
            }
        }
    }

    fn hex4(&mut self) -> Result<u32, JsonError> {
        if self.pos + 4 > self.bytes.len() {
            return Err(JsonError::new("json_invalid", "truncated unicode escape"));
        }
        let text = std::str::from_utf8(&self.bytes[self.pos..self.pos + 4])
            .map_err(|_| JsonError::new("json_invalid", "invalid unicode escape"))?;
        let value = u32::from_str_radix(text, 16)
            .map_err(|_| JsonError::new("json_invalid", "invalid unicode escape"))?;
        self.pos += 4;
        Ok(value)
    }

    fn array(&mut self) -> Result<Json, JsonError> {
        self.expect(b'[')?;
        self.enter()?;
        let mut items = Vec::new();
        self.skip_ws();
        if self.peek() == Some(b']') {
            self.pos += 1;
            self.leave();
            return Ok(Json::Array(items));
        }
        loop {
            self.skip_ws();
            self.records += 1;
            if self.records > self.limits.max_records {
                return Err(JsonError::new(
                    "json_records",
                    format!("more than {} array elements", self.limits.max_records),
                ));
            }
            items.push(self.value()?);
            self.skip_ws();
            match self.peek() {
                Some(b',') => self.pos += 1,
                Some(b']') => {
                    self.pos += 1;
                    break;
                }
                _ => {
                    return Err(JsonError::new(
                        "json_invalid",
                        format!("expected `,` or `]` at byte {}", self.pos),
                    ));
                }
            }
        }
        self.leave();
        Ok(Json::Array(items))
    }

    fn object(&mut self) -> Result<Json, JsonError> {
        self.expect(b'{')?;
        self.enter()?;
        let mut map = BTreeMap::new();
        self.skip_ws();
        if self.peek() == Some(b'}') {
            self.pos += 1;
            self.leave();
            return Ok(Json::Object(map));
        }
        loop {
            self.skip_ws();
            let key = self.string()?;
            self.skip_ws();
            self.expect(b':')?;
            self.skip_ws();
            let value = self.value()?;
            if map.insert(key.clone(), value).is_some() {
                return Err(JsonError::new(
                    "json_duplicate_key",
                    format!("duplicate key {key:?}"),
                ));
            }
            self.skip_ws();
            match self.peek() {
                Some(b',') => self.pos += 1,
                Some(b'}') => {
                    self.pos += 1;
                    break;
                }
                _ => {
                    return Err(JsonError::new(
                        "json_invalid",
                        format!("expected `,` or `}}` at byte {}", self.pos),
                    ));
                }
            }
        }
        self.leave();
        Ok(Json::Object(map))
    }
}

fn utf8_width(first: u8) -> usize {
    match first {
        0x00..=0x7f => 1,
        0xc0..=0xdf => 2,
        0xe0..=0xef => 3,
        _ => 4,
    }
}

/// Serialize with sorted keys, two-space indentation, and a final LF.
pub fn to_pretty(value: &Json) -> String {
    let mut out = String::new();
    write_value(&mut out, value, 0, true);
    out.push('\n');
    out
}

/// Serialize compactly without whitespace.
pub fn to_compact(value: &Json) -> String {
    let mut out = String::new();
    write_value(&mut out, value, 0, false);
    out
}

fn write_value(out: &mut String, value: &Json, indent: usize, pretty: bool) {
    match value {
        Json::Null => out.push_str("null"),
        Json::Bool(true) => out.push_str("true"),
        Json::Bool(false) => out.push_str("false"),
        Json::Number(n) => {
            let _ = write!(out, "{n}");
        }
        Json::String(s) => write_string(out, s),
        Json::Array(items) => {
            if items.is_empty() {
                out.push_str("[]");
                return;
            }
            out.push('[');
            for (index, item) in items.iter().enumerate() {
                if index > 0 {
                    out.push(',');
                }
                if pretty {
                    out.push('\n');
                    push_indent(out, indent + 1);
                }
                write_value(out, item, indent + 1, pretty);
            }
            if pretty {
                out.push('\n');
                push_indent(out, indent);
            }
            out.push(']');
        }
        Json::Object(map) => {
            if map.is_empty() {
                out.push_str("{}");
                return;
            }
            out.push('{');
            for (index, (key, item)) in map.iter().enumerate() {
                if index > 0 {
                    out.push(',');
                }
                if pretty {
                    out.push('\n');
                    push_indent(out, indent + 1);
                }
                write_string(out, key);
                out.push(':');
                if pretty {
                    out.push(' ');
                }
                write_value(out, item, indent + 1, pretty);
            }
            if pretty {
                out.push('\n');
                push_indent(out, indent);
            }
            out.push('}');
        }
    }
}

fn push_indent(out: &mut String, level: usize) {
    for _ in 0..level {
        out.push_str("  ");
    }
}

fn write_string(out: &mut String, text: &str) {
    out.push('"');
    for ch in text.chars() {
        match ch {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            '\u{8}' => out.push_str("\\b"),
            '\u{c}' => out.push_str("\\f"),
            c if (c as u32) < 0x20 => {
                let _ = write!(out, "\\u{:04x}", c as u32);
            }
            c => out.push(c),
        }
    }
    out.push('"');
}

/// Convert application detail values to JSON.
pub fn from_detail(detail: &Detail) -> Json {
    match detail {
        Detail::Null => Json::Null,
        Detail::Bool(b) => Json::Bool(*b),
        Detail::Number(n) => Json::Number(*n),
        Detail::Text(s) => Json::String(s.clone()),
        Detail::List(items) => Json::Array(items.iter().map(from_detail).collect()),
        Detail::Map(map) => Json::Object(
            map.iter()
                .map(|(k, v)| (k.clone(), from_detail(v)))
                .collect(),
        ),
    }
}

/// Total array elements across the tree.
pub fn count_records(value: &Json) -> u64 {
    match value {
        Json::Array(items) => items.len() as u64 + items.iter().map(count_records).sum::<u64>(),
        Json::Object(map) => map.values().map(count_records).sum(),
        _ => 0,
    }
}

/// Maximum container nesting depth (the root container has depth 1).
pub fn max_depth(value: &Json) -> usize {
    match value {
        Json::Array(items) => 1 + items.iter().map(max_depth).max().unwrap_or(0),
        Json::Object(map) => 1 + map.values().map(max_depth).max().unwrap_or(0),
        _ => 0,
    }
}

/// Helper for strict object field consumption.
pub struct ObjectReader {
    fields: BTreeMap<String, Json>,
    context: String,
}

impl ObjectReader {
    pub fn new(value: Json, context: &str) -> Result<ObjectReader, JsonError> {
        match value {
            Json::Object(fields) => Ok(ObjectReader {
                fields,
                context: context.to_string(),
            }),
            _ => Err(JsonError::new(
                "json_schema",
                format!("{context} must be an object"),
            )),
        }
    }

    pub fn take(&mut self, key: &str) -> Result<Json, JsonError> {
        self.fields.remove(key).ok_or_else(|| {
            JsonError::new(
                "json_schema",
                format!("{} is missing required field {key:?}", self.context),
            )
        })
    }

    pub fn take_u64(&mut self, key: &str) -> Result<u64, JsonError> {
        match self.take(key)? {
            Json::Number(n) => Ok(n),
            _ => Err(self.type_error(key, "an unsigned integer")),
        }
    }

    pub fn take_string(&mut self, key: &str) -> Result<String, JsonError> {
        match self.take(key)? {
            Json::String(s) => Ok(s),
            _ => Err(self.type_error(key, "a string")),
        }
    }

    pub fn take_optional_string(&mut self, key: &str) -> Result<Option<String>, JsonError> {
        match self.take(key)? {
            Json::String(s) => Ok(Some(s)),
            Json::Null => Ok(None),
            _ => Err(self.type_error(key, "a string or null")),
        }
    }

    pub fn take_bool(&mut self, key: &str) -> Result<bool, JsonError> {
        match self.take(key)? {
            Json::Bool(b) => Ok(b),
            _ => Err(self.type_error(key, "a boolean")),
        }
    }

    pub fn take_array(&mut self, key: &str) -> Result<Vec<Json>, JsonError> {
        match self.take(key)? {
            Json::Array(items) => Ok(items),
            _ => Err(self.type_error(key, "an array")),
        }
    }

    pub fn take_object(&mut self, key: &str) -> Result<ObjectReader, JsonError> {
        let value = self.take(key)?;
        ObjectReader::new(value, &format!("{}.{key}", self.context))
    }

    pub fn take_optional_object(&mut self, key: &str) -> Result<Option<ObjectReader>, JsonError> {
        match self.take(key)? {
            Json::Null => Ok(None),
            value => ObjectReader::new(value, &format!("{}.{key}", self.context)).map(Some),
        }
    }

    pub fn context(&self) -> &str {
        &self.context
    }

    /// The remaining fields (used for maps keyed by arbitrary strings).
    pub fn into_inner(self) -> BTreeMap<String, Json> {
        self.fields
    }

    pub fn type_error(&self, key: &str, expected: &str) -> JsonError {
        JsonError::new(
            "json_schema",
            format!("{}.{key} must be {expected}", self.context),
        )
    }

    /// Fail when unknown fields remain.
    pub fn finish(self) -> Result<(), JsonError> {
        if let Some(key) = self.fields.keys().next() {
            return Err(JsonError::new(
                "json_schema",
                format!("{} has unknown field {key:?}", self.context),
            ));
        }
        Ok(())
    }
}

pub fn expect_string(value: Json, context: &str) -> Result<String, JsonError> {
    match value {
        Json::String(s) => Ok(s),
        _ => Err(JsonError::new(
            "json_schema",
            format!("{context} must be a string"),
        )),
    }
}

pub fn expect_u64(value: Json, context: &str) -> Result<u64, JsonError> {
    match value {
        Json::Number(n) => Ok(n),
        _ => Err(JsonError::new(
            "json_schema",
            format!("{context} must be an unsigned integer"),
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips_canonical_form() {
        let text = "{\n  \"a\": [\n    1,\n    \"x\\ny\"\n  ],\n  \"b\": {},\n  \"c\": null,\n  \"d\": true\n}\n";
        let value = parse(text.as_bytes(), Limits::PACKET).unwrap();
        assert_eq!(to_pretty(&value), text);
        assert_eq!(
            to_compact(&value),
            "{\"a\":[1,\"x\\ny\"],\"b\":{},\"c\":null,\"d\":true}"
        );
    }

    #[test]
    fn rejects_duplicates_trailing_and_numbers() {
        assert_eq!(
            parse(b"{\"a\":1,\"a\":2}", Limits::PACKET)
                .unwrap_err()
                .code,
            "json_duplicate_key"
        );
        assert_eq!(
            parse(b"{} {}", Limits::PACKET).unwrap_err().code,
            "json_trailing"
        );
        assert_eq!(
            parse(b"-1", Limits::PACKET).unwrap_err().code,
            "json_number"
        );
        assert_eq!(
            parse(b"1.5", Limits::PACKET).unwrap_err().code,
            "json_number"
        );
        assert_eq!(
            parse(b"1e3", Limits::PACKET).unwrap_err().code,
            "json_number"
        );
        assert_eq!(
            parse(b"18446744073709551616", Limits::PACKET)
                .unwrap_err()
                .code,
            "json_number"
        );
        assert!(parse(b"18446744073709551615", Limits::PACKET).is_ok());
        assert_eq!(
            parse(b"01", Limits::PACKET).unwrap_err().code,
            "json_number"
        );
        assert!(parse(b"{} \n", Limits::PACKET).is_ok());
    }

    #[test]
    fn enforces_depth_and_records() {
        let deep32 = format!("{}{}", "[".repeat(32), "]".repeat(32));
        let deep33 = format!("{}{}", "[".repeat(33), "]".repeat(33));
        assert!(parse(deep32.as_bytes(), Limits::PACKET).is_ok());
        assert_eq!(
            parse(deep33.as_bytes(), Limits::PACKET).unwrap_err().code,
            "json_depth"
        );
        let limits = Limits {
            max_depth: 32,
            max_records: 4,
        };
        assert!(parse(b"[1,[2,3]]", limits).is_ok());
        assert_eq!(
            parse(b"[1,[2,3],4]", limits).unwrap_err().code,
            "json_records"
        );
        assert_eq!(
            count_records(&parse(b"[1,[2,3],{\"a\":[4]}]", Limits::PACKET).unwrap()),
            6
        );
        assert_eq!(
            max_depth(&parse(b"[1,[2,3],{\"a\":[4]}]", Limits::PACKET).unwrap()),
            3
        );
    }

    #[test]
    fn parses_escapes_and_unicode() {
        let value = parse(br#""a\u00e9\ud83d\ude00\"""#, Limits::PACKET).unwrap();
        assert_eq!(value, Json::String("aé😀\"".into()));
        assert!(parse(b"\"\x01\"", Limits::PACKET).is_err());
        assert!(parse(b"\"\\ud800\"", Limits::PACKET).is_err());
        assert!(parse(b"\"\xff\"", Limits::PACKET).is_err());
    }

    #[test]
    fn object_reader_rejects_unknown_fields() {
        let value = parse(b"{\"a\":1,\"b\":2}", Limits::PACKET).unwrap();
        let mut reader = ObjectReader::new(value, "root").unwrap();
        assert_eq!(reader.take_u64("a").unwrap(), 1);
        assert!(reader.finish().is_err());
    }
}
