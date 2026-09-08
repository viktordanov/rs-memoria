//! Diagnostics and typed application errors.

use std::collections::BTreeMap;
use std::fmt;

/// Structured detail values attached to diagnostics and command data.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Detail {
    Null,
    Bool(bool),
    Number(u64),
    Text(String),
    List(Vec<Detail>),
    Map(BTreeMap<String, Detail>),
}

impl Detail {
    pub fn map() -> DetailMap {
        DetailMap::default()
    }

    pub fn text(value: impl Into<String>) -> Detail {
        Detail::Text(value.into())
    }

    pub fn list<I: IntoIterator<Item = Detail>>(items: I) -> Detail {
        Detail::List(items.into_iter().collect())
    }

    pub fn texts<I: IntoIterator<Item = S>, S: Into<String>>(items: I) -> Detail {
        Detail::List(items.into_iter().map(|s| Detail::Text(s.into())).collect())
    }

    pub fn option_text(value: Option<impl Into<String>>) -> Detail {
        match value {
            Some(v) => Detail::Text(v.into()),
            None => Detail::Null,
        }
    }

    /// Number of list elements across the whole tree, each counted once in
    /// its containing list (the packet record definition).
    pub fn list_elements(&self) -> u64 {
        match self {
            Detail::List(items) => {
                items.len() as u64 + items.iter().map(Detail::list_elements).sum::<u64>()
            }
            Detail::Map(map) => map.values().map(Detail::list_elements).sum(),
            _ => 0,
        }
    }

    /// A bound on the serialized size of this value. It counts every text
    /// byte, every key, and a fixed cost for punctuation, so a caller can
    /// refuse an oversized rendering before it builds the output.
    pub fn approximate_bytes(&self) -> u64 {
        match self {
            Detail::Null => 4,
            Detail::Bool(_) => 5,
            Detail::Number(_) => 20,
            Detail::Text(text) => text.len() as u64 + 2,
            Detail::List(items) => {
                2 + items
                    .iter()
                    .map(|item| item.approximate_bytes() + 1)
                    .sum::<u64>()
            }
            Detail::Map(map) => {
                2 + map
                    .iter()
                    .map(|(key, value)| key.len() as u64 + 4 + value.approximate_bytes())
                    .sum::<u64>()
            }
        }
    }

    pub fn get(&self, key: &str) -> Option<&Detail> {
        match self {
            Detail::Map(map) => map.get(key),
            _ => None,
        }
    }
}

/// Builder for a `Detail::Map`.
#[derive(Debug, Default, Clone)]
pub struct DetailMap(BTreeMap<String, Detail>);

impl DetailMap {
    pub fn with(mut self, key: &str, value: Detail) -> DetailMap {
        self.0.insert(key.to_string(), value);
        self
    }

    pub fn text(self, key: &str, value: impl Into<String>) -> DetailMap {
        self.with(key, Detail::Text(value.into()))
    }

    pub fn number(self, key: &str, value: u64) -> DetailMap {
        self.with(key, Detail::Number(value))
    }

    pub fn bool(self, key: &str, value: bool) -> DetailMap {
        self.with(key, Detail::Bool(value))
    }

    pub fn build(self) -> Detail {
        Detail::Map(self.0)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Severity {
    Error,
    Warning,
    Hint,
}

impl Severity {
    pub fn as_str(self) -> &'static str {
        match self {
            Severity::Error => "error",
            Severity::Warning => "warning",
            Severity::Hint => "hint",
        }
    }
}

/// One user-facing finding with a stable code.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Diagnostic {
    pub code: String,
    pub severity: Severity,
    pub message: String,
    pub path: Option<String>,
    pub line: Option<usize>,
    pub column: Option<usize>,
    pub details: Detail,
}

impl Diagnostic {
    pub fn new(severity: Severity, code: &str, message: impl Into<String>) -> Diagnostic {
        Diagnostic {
            code: code.to_string(),
            severity,
            message: message.into(),
            path: None,
            line: None,
            column: None,
            details: Detail::Map(BTreeMap::new()),
        }
    }

    pub fn error(code: &str, message: impl Into<String>) -> Diagnostic {
        Diagnostic::new(Severity::Error, code, message)
    }

    pub fn warning(code: &str, message: impl Into<String>) -> Diagnostic {
        Diagnostic::new(Severity::Warning, code, message)
    }

    pub fn hint(code: &str, message: impl Into<String>) -> Diagnostic {
        Diagnostic::new(Severity::Hint, code, message)
    }

    pub fn at_path(mut self, path: impl Into<String>) -> Diagnostic {
        self.path = Some(path.into());
        self
    }

    pub fn at(mut self, line: usize, column: usize) -> Diagnostic {
        self.line = Some(line);
        self.column = Some(column);
        self
    }

    pub fn with_details(mut self, details: Detail) -> Diagnostic {
        self.details = details;
        self
    }

    pub fn is_error(&self) -> bool {
        self.severity == Severity::Error
    }
}

/// Sort diagnostics by path, location, and code for stable output.
pub fn sort_diagnostics(diagnostics: &mut [Diagnostic]) {
    diagnostics.sort_by(|a, b| {
        (
            a.path.as_deref(),
            a.line,
            a.column,
            a.code.as_str(),
            a.message.as_str(),
        )
            .cmp(&(
                b.path.as_deref(),
                b.line,
                b.column,
                b.code.as_str(),
                b.message.as_str(),
            ))
    });
}

/// Process exit classes. Presentation maps them to exit codes 1–4.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum ExitClass {
    /// Project validation failed or the check found unresolved work.
    Validation = 1,
    /// Invalid arguments, options, notes, tokens, or packets.
    Usage = 2,
    /// Busy lock, snapshot change, review conflict, or installer conflict.
    Conflict = 3,
    /// I/O failure, unavailable Git, or corrupt state.
    Io = 4,
}

impl ExitClass {
    pub fn code(self) -> i32 {
        self as i32
    }
}

/// A failed use case: a class plus every diagnostic that explains it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AppError {
    pub class: ExitClass,
    pub diagnostics: Vec<Diagnostic>,
    /// Partial command data that remains useful on failure.
    pub data: Detail,
}

impl AppError {
    pub fn new(class: ExitClass, diagnostic: Diagnostic) -> AppError {
        AppError {
            class,
            diagnostics: vec![diagnostic],
            data: Detail::Null,
        }
    }

    pub fn many(class: ExitClass, mut diagnostics: Vec<Diagnostic>) -> AppError {
        sort_diagnostics(&mut diagnostics);
        AppError {
            class,
            diagnostics,
            data: Detail::Null,
        }
    }

    pub fn validation(code: &str, message: impl Into<String>) -> AppError {
        AppError::new(ExitClass::Validation, Diagnostic::error(code, message))
    }

    pub fn usage(code: &str, message: impl Into<String>) -> AppError {
        AppError::new(ExitClass::Usage, Diagnostic::error(code, message))
    }

    pub fn conflict(code: &str, message: impl Into<String>) -> AppError {
        AppError::new(ExitClass::Conflict, Diagnostic::error(code, message))
    }

    pub fn io(code: &str, message: impl Into<String>) -> AppError {
        AppError::new(ExitClass::Io, Diagnostic::error(code, message))
    }

    pub fn with_data(mut self, data: Detail) -> AppError {
        self.data = data;
        self
    }

    pub fn with_diagnostics(mut self, mut extra: Vec<Diagnostic>) -> AppError {
        self.diagnostics.append(&mut extra);
        sort_diagnostics(&mut self.diagnostics);
        self
    }
}

impl fmt::Display for AppError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        for (index, diagnostic) in self.diagnostics.iter().enumerate() {
            if index > 0 {
                writeln!(f)?;
            }
            write!(f, "{}: {}", diagnostic.code, diagnostic.message)?;
        }
        Ok(())
    }
}

impl std::error::Error for AppError {}

/// A successful use case result with non-fatal diagnostics.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Outcome<T> {
    pub data: T,
    pub diagnostics: Vec<Diagnostic>,
}

impl<T> Outcome<T> {
    pub fn new(data: T, mut diagnostics: Vec<Diagnostic>) -> Outcome<T> {
        sort_diagnostics(&mut diagnostics);
        Outcome { data, diagnostics }
    }
}
