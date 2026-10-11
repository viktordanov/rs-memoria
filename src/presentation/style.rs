//! Color for human output.
//!
//! Renderers write plain text. When color is on, [`Painter::paint`] styles
//! that text line by line, by role: Memoria commands take the accent, paths
//! a warm white, labels and separators a muted grey, states and diff lines
//! their own colors. When color is off, the text passes through unchanged,
//! so agents, pipes, CI logs, JSON, and tests see the exact plain bytes.

use super::cli::{ColorChoice, Format};

/// What a span of text is. Every role maps to one color in each theme.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Role {
    Accent,
    AccentBold,
    Path,
    Muted,
    Attention,
    Good,
    Added,
    Removed,
    Error,
    Bold,
    Link,
}

/// The environment that decides color, read once by the caller.
#[derive(Debug, Clone, Default)]
pub struct ColorEnv {
    /// `NO_COLOR`: any non-empty value turns automatic color off.
    pub no_color: Option<String>,
    pub term: Option<String>,
    /// `COLORTERM`: `truecolor` or `24bit` selects 24-bit colors.
    pub colorterm: Option<String>,
    /// `MEMORIA_THEME`: `light` or `dark`. Overrides `COLORFGBG`.
    pub theme: Option<String>,
    /// `COLORFGBG`, set by some terminals: a background of 7 or 15 is light.
    pub colorfgbg: Option<String>,
}

impl ColorEnv {
    pub fn from_process() -> ColorEnv {
        let var = |name: &str| std::env::var(name).ok();
        ColorEnv {
            no_color: var("NO_COLOR"),
            term: var("TERM"),
            colorterm: var("COLORTERM"),
            theme: var("MEMORIA_THEME"),
            colorfgbg: var("COLORFGBG"),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Palette {
    light: bool,
    truecolor: bool,
}

/// Paints rendered human text, or passes it through when color is off.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Painter {
    palette: Option<Palette>,
}

impl Painter {
    pub fn off() -> Painter {
        Painter { palette: None }
    }

    /// Decide color for one output stream. JSON is never colored.
    pub fn decide(
        choice: ColorChoice,
        format: Format,
        is_terminal: bool,
        env: &ColorEnv,
    ) -> Painter {
        let on = format == Format::Human
            && match choice {
                ColorChoice::Never => false,
                ColorChoice::Always => true,
                ColorChoice::Auto => {
                    is_terminal
                        && env.no_color.as_deref().unwrap_or("").is_empty()
                        && env.term.as_deref() != Some("dumb")
                }
            };
        if !on {
            return Painter::off();
        }
        Painter {
            palette: Some(Palette::detect(env)),
        }
    }

    pub fn paint(&self, text: &str) -> String {
        let Some(palette) = self.palette else {
            return text.to_string();
        };
        let mut out = String::with_capacity(text.len() + text.len() / 4);
        let mut in_hunk = false;
        for (index, line) in text.split('\n').enumerate() {
            if index > 0 {
                out.push('\n');
            }
            let roles = roles(line, &mut in_hunk);
            emit(&mut out, line, &roles, palette);
        }
        out
    }
}

impl Palette {
    fn detect(env: &ColorEnv) -> Palette {
        let light = match env.theme.as_deref() {
            Some("light") => true,
            Some("dark") => false,
            _ => env
                .colorfgbg
                .as_deref()
                .and_then(|value| value.rsplit(';').next())
                .is_some_and(|background| matches!(background.trim(), "7" | "15")),
        };
        let truecolor = matches!(
            env.colorterm
                .as_deref()
                .map(str::to_ascii_lowercase)
                .as_deref(),
            Some("truecolor") | Some("24bit")
        );
        Palette { light, truecolor }
    }

    fn color(self, role: Role) -> Option<clap::builder::styling::Color> {
        use clap::builder::styling::{Ansi256Color, Color, RgbColor};
        let (r, g, b) = rgb(role, self.light)?;
        Some(if self.truecolor {
            Color::Rgb(RgbColor(r, g, b))
        } else {
            Color::Ansi256(Ansi256Color(xterm256(r, g, b)))
        })
    }

    fn style(self, role: Role) -> clap::builder::styling::Style {
        let style = clap::builder::styling::Style::new().fg_color(self.color(role));
        if matches!(role, Role::AccentBold | Role::Error | Role::Bold) {
            style.bold()
        } else {
            style
        }
    }
}

const VERSION: &str = env!("CARGO_PKG_VERSION");

/// Clap's help colors in the same palette: commands and options take the
/// accent, placeholders the muted grey. Clap drops them when its output is
/// not colored, so plain help keeps its exact text.
pub fn help_styles(env: &ColorEnv) -> clap::builder::Styles {
    let palette = Palette::detect(env);
    clap::builder::Styles::styled()
        .header(palette.style(Role::Bold))
        .usage(palette.style(Role::Bold))
        .literal(palette.style(Role::Accent))
        .placeholder(palette.style(Role::Muted))
        .valid(palette.style(Role::Accent))
        .invalid(palette.style(Role::Removed))
        .error(palette.style(Role::Error))
        .context(palette.style(Role::Muted))
        .context_value(palette.style(Role::Accent))
}

/// The line above the top-level help: the name, the version, and the claim.
pub fn help_title(env: &ColorEnv) -> clap::builder::StyledStr {
    use std::fmt::Write as _;
    let palette = Palette::detect(env);
    let (brand, muted) = (palette.style(Role::AccentBold), palette.style(Role::Muted));
    let mut title = clap::builder::StyledStr::new();
    let _ = write!(
        title,
        "{brand}Memoria {VERSION}{brand:#}{muted} — CI for your docs{muted:#}"
    );
    title
}

/// The lines below the top-level help: where to start, and the guides for
/// this release.
pub fn help_footer(env: &ColorEnv) -> clap::builder::StyledStr {
    use std::fmt::Write as _;
    let palette = Palette::detect(env);
    let (bold, accent, muted) = (
        palette.style(Role::Bold),
        palette.style(Role::Accent),
        palette.style(Role::Link),
    );
    let mut footer = clap::builder::StyledStr::new();
    let _ = write!(
        footer,
        "{bold}Start here:{bold:#} {accent}memoria onboard{accent:#}\n\
         {bold}Guides:{bold:#}     {muted}https://github.com/viktordanov/rs-memoria/tree/v{VERSION}/docs{muted:#}"
    );
    footer
}

fn emit(out: &mut String, line: &str, roles: &[Option<Role>], palette: Palette) {
    let mut current: Option<Role> = None;
    for (index, ch) in line.char_indices() {
        let role = roles[index];
        if role != current {
            if current.is_some() {
                out.push_str("\x1b[0m");
            }
            if let Some(role) = role {
                out.push_str(&sgr(role, palette));
            }
            current = role;
        }
        out.push(ch);
    }
    if current.is_some() {
        out.push_str("\x1b[0m");
    }
}

/// The chosen palette: an amber accent with warm neutrals. The light values
/// keep each hue and lower its lightness for a light background.
fn rgb(role: Role, light: bool) -> Option<(u8, u8, u8)> {
    let hex: u32 = match (role, light) {
        (Role::Accent | Role::AccentBold, false) => 0xffc000,
        (Role::Accent | Role::AccentBold, true) => 0xa67c00,
        (Role::Path, false) => 0xe9e4da,
        (Role::Path, true) => 0x3f3a33,
        (Role::Muted | Role::Link, false) => 0x707070,
        (Role::Muted | Role::Link, true) => 0x6b6b6b,
        (Role::Attention, false) => 0xe3a72f,
        (Role::Attention, true) => 0x9a6a12,
        (Role::Good, false) => 0x69e871,
        (Role::Good, true) => 0x1f8a2a,
        (Role::Added, false) => 0x5dbb63,
        (Role::Added, true) => 0x2f7d35,
        (Role::Removed | Role::Error, false) => 0xf9645d,
        (Role::Removed | Role::Error, true) => 0xc0352e,
        (Role::Bold, _) => return None,
    };
    Some(((hex >> 16) as u8, (hex >> 8) as u8, hex as u8))
}

fn sgr(role: Role, palette: Palette) -> String {
    let mut codes: Vec<String> = Vec::new();
    if matches!(role, Role::AccentBold | Role::Error | Role::Bold) {
        codes.push("1".into());
    }
    if role == Role::Link {
        codes.push("4".into());
    }
    if let Some((r, g, b)) = rgb(role, palette.light) {
        if palette.truecolor {
            codes.push(format!("38;2;{r};{g};{b}"));
        } else {
            codes.push(format!("38;5;{}", xterm256(r, g, b)));
        }
    }
    format!("\x1b[{}m", codes.join(";"))
}

/// The nearest color in the xterm 256-color cube or grey ramp.
fn xterm256(r: u8, g: u8, b: u8) -> u8 {
    const LEVELS: [u8; 6] = [0, 95, 135, 175, 215, 255];
    let nearest = |value: u8| {
        LEVELS
            .iter()
            .enumerate()
            .min_by_key(|(_, level)| (i32::from(**level) - i32::from(value)).abs())
            .map(|(index, _)| index)
            .unwrap_or(0)
    };
    let (ri, gi, bi) = (nearest(r), nearest(g), nearest(b));
    let distance = |(x, y, z): (u8, u8, u8)| {
        let d = |a: u8, b: u8| (i32::from(a) - i32::from(b)).pow(2);
        d(x, r) + d(y, g) + d(z, b)
    };
    let cube = (LEVELS[ri], LEVELS[gi], LEVELS[bi]);
    let grey_index = ((i32::from(r) + i32::from(g) + i32::from(b)) / 3 - 8).clamp(0, 230) / 10;
    let grey_level = (8 + grey_index * 10) as u8;
    if distance((grey_level, grey_level, grey_level)) < distance(cube) {
        232 + grey_index as u8
    } else {
        16 + 36 * ri as u8 + 6 * gi as u8 + bi as u8
    }
}

/// The role of every byte of one line. Later rules win.
fn roles(line: &str, in_hunk: &mut bool) -> Vec<Option<Role>> {
    let mut roles = vec![None; line.len()];
    let set = |start: usize, end: usize, role: Role, roles: &mut Vec<Option<Role>>| {
        for slot in &mut roles[start..end.min(line.len())] {
            *slot = Some(role);
        }
    };

    // A review hunk: six-space indented lines from an `@@` header on.
    const HUNK: &str = "      ";
    if *in_hunk && !(line.starts_with(HUNK) || line.is_empty()) {
        *in_hunk = false;
    }
    if !*in_hunk && line.starts_with("      @@ ") {
        *in_hunk = true;
    }
    if *in_hunk {
        if !line.is_empty() {
            let role = match line.as_bytes().get(HUNK.len()) {
                Some(b'+') => Role::Added,
                Some(b'-') => Role::Removed,
                _ => Role::Muted,
            };
            set(0, line.len(), role, &mut roles);
        }
        return roles;
    }

    let indent = line.len() - line.trim_start_matches(' ').len();
    let body = &line[indent..];

    // A section header: a capitalized line at column 0 that ends with a colon.
    if indent == 0
        && body.ends_with(':')
        && body.starts_with(|c: char| c.is_ascii_uppercase())
        && body[..body.len() - 1]
            .chars()
            .all(|c| c.is_ascii_alphabetic() || matches!(c, ' ' | ',' | '\'' | '-'))
    {
        set(0, line.len(), Role::Bold, &mut roles);
    }
    if indent == 0 {
        if body.starts_with("Review plan:") {
            set(0, "Review plan:".len(), Role::Bold, &mut roles);
        } else {
            for verb in [
                "Review", "Explain", "Saved:", "Recorded", "preview:", "proposed",
            ] {
                if body.starts_with(verb)
                    && body[verb.len()..].chars().next().is_none_or(|c| c == ' ')
                {
                    set(0, verb.len(), Role::Bold, &mut roles);
                }
            }
        }
    }

    // A label column: a short label of words, two spaces, then a value.
    if let Some(end) = column_label(body) {
        set(indent, indent + end, Role::Muted, &mut roles);
    }
    const LABELS: [&str; 15] = [
        "Then read the rest of the scope",
        "Whole document pass",
        "Suggested section",
        "handed off by",
        "covered by",
        "Read first",
        "Baseline",
        "Guidance",
        "Concepts",
        "Column",
        "Scope",
        "Guide",
        "Mode",
        "Path",
        "Line",
    ];
    for label in LABELS {
        if let Some(rest) = body.strip_prefix(label)
            && rest.chars().next().is_none_or(|c| c == ':' || c == ' ')
        {
            let end = label.len() + usize::from(rest.starts_with(':'));
            set(indent, indent + end, Role::Muted, &mut roles);
        }
    }
    // Onboarding headings sit in a label column, so they come after it.
    for heading in ["This project", "Start here", "Good to know", "Go deeper"] {
        if indent == 0 && body.starts_with(heading) {
            set(0, heading.len(), Role::Bold, &mut roles);
        }
    }
    // A numbered step: `  1. ...`.
    let digits = body.bytes().take_while(u8::is_ascii_digit).count();
    if indent > 0 && digits > 0 && body[digits..].starts_with(". ") {
        set(indent, indent + digits + 1, Role::Muted, &mut roles);
    }

    // Quiet punctuation and metadata.
    for mark in [" · ", "—", "→"] {
        for (at, _) in line.match_indices(mark) {
            set(at, at + mark.len(), Role::Muted, &mut roles);
        }
    }
    for (start, end) in parentheticals(line) {
        set(start, end, Role::Muted, &mut roles);
    }
    for (start, end) in bracketed_codes(line) {
        set(start, end, Role::Muted, &mut roles);
    }

    // Paths, then backticks, then commands: a command wins over a path in it.
    for (start, end) in paths(line) {
        set(start, end, Role::Path, &mut roles);
    }
    for (start, end) in backtick_pairs(line) {
        set(start, start + 1, Role::Muted, &mut roles);
        set(end - 1, end, Role::Muted, &mut roles);
    }
    for (start, end) in commands(line) {
        set(start, end, Role::Accent, &mut roles);
    }
    for (start, end) in urls(line) {
        set(start, end, Role::Link, &mut roles);
    }

    // States. Attention is gold, not red; absence is neutral.
    for (word, role) in [
        ("never reviewed", Role::Attention),
        ("not installed", Role::Muted),
        ("not set up", Role::Muted),
        ("outdated", Role::Attention),
        ("pending", Role::Attention),
        ("current", Role::Good),
        ("passes", Role::Good),
        ("absent", Role::Muted),
        ("ready", Role::Good),
    ] {
        for (start, end) in state_words(line, word) {
            set(start, end, role, &mut roles);
        }
    }
    if indent == 0 && body.starts_with("OK:") {
        set(0, 3, Role::Good, &mut roles);
    }

    // Diagnostics and failures.
    for (severity, role) in [
        ("error", Role::Error),
        ("warning", Role::Attention),
        ("hint", Role::Muted),
    ] {
        if indent == 0
            && body.starts_with(severity)
            && matches!(body.as_bytes().get(severity.len()), Some(b' ' | b'['))
        {
            set(0, severity.len(), role, &mut roles);
        }
    }
    if indent == 0 && body.starts_with("memoria ") && body.contains(" failed with exit status ") {
        set(0, line.len(), Role::Removed, &mut roles);
    }
    // A generated workflow's comments.
    if line.starts_with("  # ") {
        set(0, line.len(), Role::Muted, &mut roles);
    }

    // The onboarding title and the one next step.
    if indent == 0
        && let Some(rest) = body.strip_prefix("Memoria ")
        && rest.starts_with(|c: char| c.is_ascii_digit())
    {
        let version_end = "Memoria ".len() + rest.find(' ').unwrap_or(rest.len());
        set(0, version_end, Role::AccentBold, &mut roles);
        set(version_end, line.len(), Role::Muted, &mut roles);
    }
    if indent == 0 && body.starts_with("Next: memoria ") {
        set(0, "Next:".len(), Role::Bold, &mut roles);
        set("Next: ".len(), line.len(), Role::AccentBold, &mut roles);
    }
    roles
}

/// The end of a label at the start of `body`: letters and single spaces,
/// at most 24 bytes, followed by two or more spaces and a value.
fn column_label(body: &str) -> Option<usize> {
    if !body.starts_with(|c: char| c.is_ascii_alphabetic()) {
        return None;
    }
    let gap = body.find("  ")?;
    let label = &body[..gap];
    let after = body[gap..].trim_start_matches(' ');
    (gap <= 24 && !after.is_empty() && label.chars().all(|c| c.is_ascii_alphabetic() || c == ' '))
        .then_some(gap)
}

fn parentheticals(line: &str) -> Vec<(usize, usize)> {
    let mut spans = Vec::new();
    let mut open: Option<usize> = None;
    for (index, ch) in line.char_indices() {
        match ch {
            '(' => open = Some(index),
            ')' => {
                if let Some(start) = open.take()
                    && index - start <= 80
                    // `$(command)` is shell, not an aside.
                    && !line[..start].ends_with('$')
                {
                    spans.push((start, index + 1));
                }
            }
            _ => {}
        }
    }
    spans
}

/// `[snake_case]` diagnostic and reason codes.
fn bracketed_codes(line: &str) -> Vec<(usize, usize)> {
    let mut spans = Vec::new();
    let mut from = 0;
    while let Some(offset) = line[from..].find('[') {
        let start = from + offset;
        let Some(len) = line[start + 1..].find(']') else {
            break;
        };
        let inner = &line[start + 1..start + 1 + len];
        if !inner.is_empty()
            && inner
                .bytes()
                .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_')
        {
            spans.push((start, start + len + 2));
        }
        from = start + 1;
    }
    spans
}

fn is_path_byte(b: u8) -> bool {
    b.is_ascii_alphanumeric() || matches!(b, b'_' | b'.' | b'~' | b'/' | b'-')
}

/// Words that look like project paths: they contain a slash, or end with a
/// file extension Memoria reads or writes.
fn paths(line: &str) -> Vec<(usize, usize)> {
    const EXTENSIONS: [&str; 7] = [".md", ".rs", ".toml", ".lock", ".yml", ".yaml", ".json"];
    let bytes = line.as_bytes();
    let mut spans = Vec::new();
    let mut index = 0;
    while index < bytes.len() {
        if !is_path_byte(bytes[index]) {
            index += 1;
            continue;
        }
        let start = index;
        while index < bytes.len() && is_path_byte(bytes[index]) {
            index += 1;
        }
        let mut end = index;
        while end > start && matches!(bytes[end - 1], b'.' | b'-') {
            end -= 1;
        }
        let word = &line[start..end];
        let preceded_by_scheme = line[..start].ends_with(':');
        let looks_like_path = (word.contains('/')
            && !word.starts_with("//")
            && word.bytes().any(|b| b.is_ascii_alphabetic()))
            || EXTENSIONS.iter().any(|extension| {
                word.len() > extension.len()
                    && word.ends_with(extension)
                    && word[..word.len() - extension.len()]
                        .bytes()
                        .any(|b| b.is_ascii_alphabetic())
            });
        if looks_like_path && !preceded_by_scheme {
            spans.push((start, end));
        }
    }
    spans
}

fn backtick_pairs(line: &str) -> Vec<(usize, usize)> {
    let ticks: Vec<usize> = line.match_indices('`').map(|(at, _)| at).collect();
    ticks
        .as_chunks::<2>()
        .0
        .iter()
        .map(|[open, close]| (*open, *close + 1))
        .collect()
}

/// `memoria <command> ...` up to two spaces, `;`, `)`, a backtick, ` · `, or the end.
fn commands(line: &str) -> Vec<(usize, usize)> {
    const COMMANDS: [&str; 19] = [
        "onboard",
        "completions",
        "explain",
        "packet",
        "init",
        "status",
        "guidance",
        "state",
        "lint",
        "review",
        "render",
        "ack",
        "invalidate",
        "check",
        "graph",
        "agent",
        "integrations",
        "--version",
        "--help",
    ];
    let mut spans = Vec::new();
    for (start, _) in line.match_indices("memoria ") {
        if line[..start]
            .chars()
            .next_back()
            .is_some_and(|c| c.is_alphanumeric() || matches!(c, '_' | '.' | '-' | '/' | '$'))
        {
            continue;
        }
        let rest = &line[start + "memoria ".len()..];
        let word_len = rest
            .find(|c: char| !(c.is_ascii_alphanumeric() || c == '-'))
            .unwrap_or(rest.len());
        if !COMMANDS.contains(&&rest[..word_len]) {
            continue;
        }
        let mut end = line.len();
        for stop in ["  ", ";", ")", "`", " · "] {
            if let Some(at) = line[start..].find(stop) {
                end = end.min(start + at);
            }
        }
        let trimmed = line[start..end].trim_end_matches([' ', '.', ',']);
        spans.push((start, start + trimmed.len()));
    }
    spans
}

fn urls(line: &str) -> Vec<(usize, usize)> {
    let mut spans = Vec::new();
    for scheme in ["https://", "http://"] {
        for (start, _) in line.match_indices(scheme) {
            let end = line[start..]
                .find(char::is_whitespace)
                .map_or(line.len(), |at| start + at);
            spans.push((start, end));
        }
    }
    spans
}

/// A state word used as a value: after the indent, two spaces, a digit and
/// a space, `, `, `· `, `→ `, or `(`, and before a space, `,`, `)`, `;`,
/// `.`, or the end. Prose such as "the current scope" stays plain.
fn state_words(line: &str, word: &str) -> Vec<(usize, usize)> {
    let mut spans = Vec::new();
    for (start, _) in line.match_indices(word) {
        let end = start + word.len();
        let before = &line[..start];
        let leading = before.trim_start_matches(' ').is_empty();
        let preceded = leading
            || before.ends_with("  ")
            || before.ends_with(", ")
            || before.ends_with("· ")
            || before.ends_with("→ ")
            || before.ends_with('(')
            || (before.ends_with(' ')
                && before[..before.len() - 1]
                    .chars()
                    .next_back()
                    .is_some_and(|c| c.is_ascii_digit()));
        let followed = line[end..]
            .chars()
            .next()
            .is_none_or(|c| matches!(c, ' ' | ',' | ')' | ';' | '.'));
        if preceded && followed {
            spans.push((start, end));
        }
    }
    spans
}

#[cfg(test)]
mod tests {
    use super::*;

    fn on() -> Painter {
        Painter {
            palette: Some(Palette {
                light: false,
                truecolor: true,
            }),
        }
    }

    /// Strip every escape sequence this module writes.
    fn strip(text: &str) -> String {
        let mut out = String::new();
        let mut chars = text.chars();
        while let Some(ch) = chars.next() {
            if ch == '\x1b' {
                for next in chars.by_ref() {
                    if next == 'm' {
                        break;
                    }
                }
            } else {
                out.push(ch);
            }
        }
        out
    }

    const SAMPLE: &str = "Review billing/design.md — opted-in document, pending since revision 2\n\
Scope: 2 sources in billing/ and below\n\
What changed since that review:\n\
  changed  billing/tax.rs · scope source\n\
      @@ -1,2 +1,2 @@\n\
      -old line\n\
      +new line\n\
       context\n\
Next:\n\
  1. Run memoria review billing/design.md --save \"$dir\"\n\
memoria review failed with exit status 3\n";

    #[test]
    fn painting_only_adds_escape_sequences() {
        let painted = on().paint(SAMPLE);
        assert_ne!(painted, SAMPLE);
        assert_eq!(strip(&painted), SAMPLE);
    }

    #[test]
    fn off_passes_text_through_unchanged() {
        assert_eq!(Painter::off().paint(SAMPLE), SAMPLE);
    }

    #[test]
    fn commands_take_the_accent_and_stop_at_their_end() {
        let line =
            "  2. After any edit, save: dir=$(mktemp -d); memoria review a.md --save \"$dir\"";
        let spans = commands(line);
        assert_eq!(spans.len(), 1);
        assert_eq!(
            &line[spans[0].0..spans[0].1],
            "memoria review a.md --save \"$dir\""
        );
        let line = "run `memoria check` before you push.";
        let (start, end) = commands(line)[0];
        assert_eq!(&line[start..end], "memoria check");
        assert!(commands("memoria.toml and memoria.lock").is_empty());
        assert!(commands("Memoria never edits prose").is_empty());
    }

    #[test]
    fn hunk_lines_are_added_removed_or_context() {
        let mut in_hunk = false;
        assert!(
            roles("What changed:", &mut in_hunk)
                .iter()
                .all(|r| *r != Some(Role::Added))
        );
        roles("      @@ -1 +1 @@", &mut in_hunk);
        assert!(in_hunk);
        assert_eq!(roles("      +new", &mut in_hunk)[0], Some(Role::Added));
        assert_eq!(roles("      -old", &mut in_hunk)[0], Some(Role::Removed));
        assert_eq!(roles("       same", &mut in_hunk)[0], Some(Role::Muted));
        roles("Also pending:", &mut in_hunk);
        assert!(!in_hunk);
        // An indented line outside a hunk is not a diff line.
        assert_eq!(roles("      -flag", &mut in_hunk)[6], None);
    }

    #[test]
    fn state_words_color_values_not_prose() {
        assert_eq!(
            state_words("  current          README.md", "current").len(),
            1
        );
        assert_eq!(
            state_words("Reviews  12 current, 0 pending", "pending").len(),
            1
        );
        assert!(state_words("against its complete current scope", "current").is_empty());
        assert!(state_words("the concurrent writer", "current").is_empty());
    }

    #[test]
    fn paths_skip_urls_and_bare_words() {
        let line = "see docs/cli.md and https://example.com/a/b, then edit memoria.toml.";
        let found: Vec<&str> = paths(line).iter().map(|(a, b)| &line[*a..*b]).collect();
        assert_eq!(found, ["docs/cli.md", "memoria.toml"]);
    }

    fn env() -> ColorEnv {
        ColorEnv {
            term: Some("xterm-256color".into()),
            ..ColorEnv::default()
        }
    }

    #[test]
    fn auto_colors_only_human_output_on_a_terminal() {
        let decide = |choice, format, tty, env: &ColorEnv| {
            Painter::decide(choice, format, tty, env).palette.is_some()
        };
        assert!(decide(ColorChoice::Auto, Format::Human, true, &env()));
        assert!(!decide(ColorChoice::Auto, Format::Human, false, &env()));
        assert!(!decide(ColorChoice::Auto, Format::Json, true, &env()));
        assert!(!decide(ColorChoice::Always, Format::Json, true, &env()));
        assert!(decide(ColorChoice::Always, Format::Human, false, &env()));
        assert!(!decide(ColorChoice::Never, Format::Human, true, &env()));
        let no_color = ColorEnv {
            no_color: Some("1".into()),
            ..env()
        };
        assert!(!decide(ColorChoice::Auto, Format::Human, true, &no_color));
        assert!(decide(ColorChoice::Always, Format::Human, true, &no_color));
        let empty_no_color = ColorEnv {
            no_color: Some(String::new()),
            ..env()
        };
        assert!(decide(
            ColorChoice::Auto,
            Format::Human,
            true,
            &empty_no_color
        ));
        let dumb = ColorEnv {
            term: Some("dumb".into()),
            ..env()
        };
        assert!(!decide(ColorChoice::Auto, Format::Human, true, &dumb));
    }

    #[test]
    fn theme_and_depth_follow_the_environment() {
        let light = ColorEnv {
            colorfgbg: Some("0;15".into()),
            colorterm: Some("truecolor".into()),
            ..env()
        };
        let painter = Painter::decide(ColorChoice::Always, Format::Human, true, &light);
        assert_eq!(
            painter.palette,
            Some(Palette {
                light: true,
                truecolor: true
            })
        );
        let forced_dark = ColorEnv {
            theme: Some("dark".into()),
            ..light
        };
        let painter = Painter::decide(ColorChoice::Always, Format::Human, true, &forced_dark);
        assert!(!painter.palette.unwrap().light);
        let plain_term = Painter::decide(ColorChoice::Always, Format::Human, true, &env());
        assert!(!plain_term.palette.unwrap().truecolor);
        assert!(plain_term.paint("memoria check").contains("38;5;"));
    }

    #[test]
    fn the_256_color_fallback_picks_the_nearest_entry() {
        assert_eq!(xterm256(0xff, 0xc0, 0x00), 214);
        assert_eq!(xterm256(0x70, 0x70, 0x70), 242);
        assert_eq!(xterm256(0, 0, 0), 16);
    }
}
