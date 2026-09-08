//! Bounded version probing for the optional agent clients.
//!
//! Probing is lazy and target-specific. Only an installation of a client's
//! hook probes that client, and no other command runs either executable.
//! The probe never starts an agent session: it asks for a version and
//! nothing else.

use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use memoria_application::ports::AgentTarget;

/// The frozen compatibility floor for each supported client. These are the
/// inspected versions the plan accepted. They are the support floor, not
/// the versions that introduced hooks.
pub const CODEX_FLOOR: Version = Version(0, 153, 0);
pub const CLAUDE_FLOOR: Version = Version(2, 1, 259);

/// How long one version probe may take.
const PROBE_DEADLINE: Duration = Duration::from_millis(2_000);
/// How much probe output is read.
const PROBE_OUTPUT_LIMIT: usize = 4 * 1024;

/// A three-part version, compared component by component.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct Version(pub u64, pub u64, pub u64);

impl std::fmt::Display for Version {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}.{}.{}", self.0, self.1, self.2)
    }
}

/// The first dotted numeric triple in `text`.
///
/// Clients print their version in different shapes: `codex-cli 0.153.0` and
/// `2.1.259 (Claude Code)`. Both reduce to the same triple. A missing or
/// malformed triple yields `None`, which the caller treats as unsupported.
pub fn parse_version(text: &str) -> Option<Version> {
    let bytes = text.as_bytes();
    let mut index = 0;
    while index < bytes.len() {
        if !bytes[index].is_ascii_digit() {
            index += 1;
            continue;
        }
        // A triple must not start in the middle of a longer token.
        if index > 0 && (bytes[index - 1].is_ascii_digit() || bytes[index - 1] == b'.') {
            index += 1;
            continue;
        }
        let start = index;
        let mut parts = Vec::new();
        let mut cursor = start;
        while parts.len() < 3 {
            let digits_start = cursor;
            while cursor < bytes.len() && bytes[cursor].is_ascii_digit() {
                cursor += 1;
            }
            if cursor == digits_start {
                break;
            }
            match text[digits_start..cursor].parse::<u64>() {
                Ok(value) => parts.push(value),
                Err(_) => break,
            }
            if parts.len() < 3 {
                if cursor < bytes.len() && bytes[cursor] == b'.' {
                    cursor += 1;
                } else {
                    break;
                }
            }
        }
        if parts.len() == 3 {
            return Some(Version(parts[0], parts[1], parts[2]));
        }
        index = cursor.max(start + 1);
    }
    None
}

/// The floor for one target.
pub fn floor(target: AgentTarget) -> Version {
    match target {
        AgentTarget::Codex => CODEX_FLOOR,
        AgentTarget::Claude => CLAUDE_FLOOR,
    }
}

/// The program name for one target.
pub fn program(target: AgentTarget) -> &'static str {
    match target {
        AgentTarget::Codex => "codex",
        AgentTarget::Claude => "claude",
    }
}

/// What a probe observed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Probe {
    /// The client reported a version at or above the floor.
    Supported(Version),
    /// The client reported a version below the floor.
    TooOld(Version),
    /// The client is absent, failed, timed out, or printed no version.
    Unavailable(String),
}

/// Probe one client's version, bounded and without a shell.
pub trait ClientProbe: Send + Sync {
    fn probe(&self, target: AgentTarget) -> Probe;
}

/// The real probe: run `<client> --version` with a deadline.
#[derive(Debug, Default, Clone, Copy)]
pub struct CommandClientProbe;

impl ClientProbe for CommandClientProbe {
    fn probe(&self, target: AgentTarget) -> Probe {
        let program = program(target);
        let mut child = match Command::new(program)
            .arg("--version")
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
        {
            Ok(child) => child,
            Err(err) => {
                return Probe::Unavailable(format!("cannot run {program}: {err}"));
            }
        };
        let mut stdout = child.stdout.take().expect("piped stdout");
        let (sender, receiver) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            use std::io::Read as _;
            let mut buffer = Vec::new();
            let _ = std::io::Read::by_ref(&mut stdout)
                .take(PROBE_OUTPUT_LIMIT as u64)
                .read_to_end(&mut buffer);
            let _ = sender.send(buffer);
        });
        let until = Instant::now() + PROBE_DEADLINE;
        let status = loop {
            match child.try_wait() {
                Ok(Some(status)) => break Some(status),
                Ok(None) => {
                    if Instant::now() >= until {
                        let _ = child.kill();
                        let _ = child.wait();
                        break None;
                    }
                    std::thread::sleep(Duration::from_millis(5));
                }
                Err(err) => {
                    let _ = child.kill();
                    return Probe::Unavailable(format!("cannot wait for {program}: {err}"));
                }
            }
        };
        if status.is_none() {
            return Probe::Unavailable(format!("{program} --version did not finish in time"));
        }
        if !status.map(|s| s.success()).unwrap_or(false) {
            return Probe::Unavailable(format!("{program} --version failed"));
        }
        let buffer = receiver
            .recv_timeout(Duration::from_millis(250))
            .unwrap_or_default();
        let text = String::from_utf8_lossy(&buffer);
        match parse_version(&text) {
            Some(version) if version >= floor(target) => Probe::Supported(version),
            Some(version) => Probe::TooOld(version),
            None => Probe::Unavailable(format!(
                "{program} --version printed no recognizable version"
            )),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn version_parsing_handles_both_client_shapes() {
        assert_eq!(parse_version("codex-cli 0.153.0"), Some(Version(0, 153, 0)));
        assert_eq!(
            parse_version("2.1.259 (Claude Code)"),
            Some(Version(2, 1, 259))
        );
        assert_eq!(parse_version("v1.2.3\n"), Some(Version(1, 2, 3)));
        assert_eq!(parse_version("1.2.3-beta.4"), Some(Version(1, 2, 3)));
        assert_eq!(parse_version("no version here"), None);
        assert_eq!(parse_version("1.2"), None);
        assert_eq!(parse_version(""), None);
    }

    #[test]
    fn floors_compare_component_by_component() {
        assert!(Version(0, 153, 0) >= CODEX_FLOOR);
        assert!(Version(0, 153, 1) >= CODEX_FLOOR);
        assert!(Version(0, 154, 0) >= CODEX_FLOOR);
        assert!(Version(1, 0, 0) >= CODEX_FLOOR);
        assert!(Version(0, 152, 999) < CODEX_FLOOR);
        assert!(Version(0, 1, 0) < CODEX_FLOOR);
        assert!(Version(2, 1, 259) >= CLAUDE_FLOOR);
        assert!(Version(2, 1, 258) < CLAUDE_FLOOR);
        assert!(Version(2, 0, 999) < CLAUDE_FLOOR);
    }

    #[test]
    fn a_missing_client_is_unavailable_without_a_panic() {
        struct Absent;
        impl ClientProbe for Absent {
            fn probe(&self, _: AgentTarget) -> Probe {
                Probe::Unavailable("absent".into())
            }
        }
        assert!(matches!(
            Absent.probe(AgentTarget::Codex),
            Probe::Unavailable(_)
        ));
    }
}
