//! `memoria agent hook`: explicit, reversible project-level `Stop` hooks and
//! the bounded status runner they invoke.
//!
//! The hook reports an advisory message. It never creates a review, never
//! continues an agent turn, and never marks failed inspection as clean.

use crate::error::{AppError, Detail, DetailMap, Diagnostic, ExitClass, Outcome};
use crate::ports::{AgentTarget, BoundedStatusProcess, HookFailure, HookPlan, Services};

/// Event JSON limit on stdin: 64 KiB.
pub const MAX_EVENT_BYTES: u64 = 64 * 1024;
/// The subprocess deadline, in milliseconds.
pub const STATUS_DEADLINE_MS: u64 = 2_000;
/// The outer runner deadline, in milliseconds.
pub const RUNNER_DEADLINE_MS: u64 = 3_000;
/// The subprocess stdout limit: 16 KiB.
pub const STATUS_STDOUT_LIMIT: u64 = 16 * 1024;
/// The advisory message limit, in UTF-8 bytes.
pub const MAX_MESSAGE_BYTES: usize = 512;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HookReport {
    pub plan: HookPlan,
    pub applied: bool,
    pub dry_run: bool,
}

pub fn plan_detail(plan: &HookPlan) -> Detail {
    DetailMap::default()
        .text("target", plan.target.as_str())
        .text("event", "Stop")
        .text("configuration", plan.configuration.clone())
        .text("record", plan.record.clone())
        .text("state", plan.state.clone())
        .text("activation", plan.activation.clone())
        .text("command", plan.command.clone())
        .with("writes", Detail::texts(plan.writes.clone()))
        .with("removals", Detail::texts(plan.removals.clone()))
        .bool("no_change", plan.no_change)
        .bool("recovery_needed", plan.recovery_needed)
        .build()
}

impl HookReport {
    pub fn to_detail(&self) -> Detail {
        DetailMap::default()
            .with("plan", plan_detail(&self.plan))
            .bool("applied", self.applied)
            .bool("dry_run", self.dry_run)
            .build()
    }
}

fn hook_error(failure: HookFailure) -> AppError {
    match failure {
        HookFailure::Conflict { code, message } => {
            AppError::new(ExitClass::Conflict, Diagnostic::error(code, message))
        }
        HookFailure::Unsupported { code, message } => AppError::validation(code, message),
        HookFailure::Io(err) => AppError::io("io_error", err.to_string()),
    }
}

/// `memoria agent hook install`.
pub fn install(
    services: &Services<'_>,
    target: AgentTarget,
    dry_run: bool,
) -> Result<Outcome<HookReport>, AppError> {
    // Install requires valid version 2 project configuration. Parsing the
    // TOML file is not enough: every ignore and include pattern must
    // compile, and every guidance reference must resolve inside the project.
    // Nothing is written when that preflight fails.
    let config = crate::snapshot::read_root_config(services)?;
    let problems = crate::snapshot::validate_root_config(services, &config)?;
    if !problems.is_empty() {
        return Err(AppError::many(ExitClass::Validation, problems));
    }
    let plan = services.hooks.plan_install(target).map_err(hook_error)?;
    if dry_run || plan.no_change {
        return Ok(Outcome::new(
            HookReport {
                plan,
                applied: false,
                dry_run,
            },
            vec![],
        ));
    }
    services.progress.note(&format!(
        "agent hook install: target {} configuration {}",
        target.as_str(),
        plan.configuration
    ));
    services.hooks.apply_install(&plan).map_err(hook_error)?;
    Ok(Outcome::new(
        HookReport {
            plan,
            applied: true,
            dry_run: false,
        },
        vec![],
    ))
}

/// `memoria agent hook status`. Available without valid configuration.
pub fn status(
    services: &Services<'_>,
    target: AgentTarget,
) -> Result<Outcome<HookReport>, AppError> {
    let plan = services.hooks.status(target).map_err(hook_error)?;
    Ok(Outcome::new(
        HookReport {
            plan,
            applied: false,
            dry_run: false,
        },
        vec![],
    ))
}

/// `memoria agent hook uninstall`. Available without valid configuration.
pub fn uninstall(
    services: &Services<'_>,
    target: AgentTarget,
    dry_run: bool,
) -> Result<Outcome<HookReport>, AppError> {
    let plan = services.hooks.plan_uninstall(target).map_err(hook_error)?;
    if dry_run || plan.no_change {
        return Ok(Outcome::new(
            HookReport {
                plan,
                applied: false,
                dry_run,
            },
            vec![],
        ));
    }
    services.progress.note(&format!(
        "agent hook uninstall: target {} configuration {}",
        target.as_str(),
        plan.configuration
    ));
    services.hooks.apply_uninstall(&plan).map_err(hook_error)?;
    Ok(Outcome::new(
        HookReport {
            plan,
            applied: true,
            dry_run: false,
        },
        vec![],
    ))
}

// ------------------------------------------------------------- Stop runner

/// The fields the runner reads from a native event. It never opens the
/// transcript and never inspects the last assistant message.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct StopEvent {
    pub event_name: String,
    pub working_directory: Option<String>,
    pub stop_hook_active: bool,
}

/// What the runner decided, before rendering.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RunnerResult {
    /// `{}`: nothing to say.
    Silent,
    /// One short advisory message.
    Message(String),
}

impl RunnerResult {
    /// The native JSON object, always a single buffered value.
    pub fn to_json(&self) -> String {
        match self {
            RunnerResult::Silent => "{}".to_string(),
            RunnerResult::Message(text) => {
                let mut escaped = String::with_capacity(text.len() + 2);
                for character in text.chars() {
                    match character {
                        '"' => escaped.push_str("\\\""),
                        '\\' => escaped.push_str("\\\\"),
                        '\n' => escaped.push_str("\\n"),
                        '\r' => escaped.push_str("\\r"),
                        '\t' => escaped.push_str("\\t"),
                        c if (c as u32) < 0x20 => escaped.push_str(&format!("\\u{:04x}", c as u32)),
                        c => escaped.push(c),
                    }
                }
                format!("{{\"systemMessage\":\"{escaped}\"}}")
            }
        }
    }
}

/// Truncate on a character boundary so the message never exceeds its limit.
fn bounded(mut text: String) -> String {
    if text.len() <= MAX_MESSAGE_BYTES {
        return text;
    }
    let mut cut = MAX_MESSAGE_BYTES;
    while cut > 0 && !text.is_char_boundary(cut) {
        cut -= 1;
    }
    text.truncate(cut);
    text
}

/// Decide the runner's answer from the event and one bounded status run.
///
/// The result is always exit 0 with a single JSON object. It never emits a
/// blocking decision, an extra prompt, or a tool request.
pub fn run_stop(
    process: &dyn BoundedStatusProcess,
    protocol: u32,
    event: &StopEvent,
    resolved_root: Option<&str>,
) -> RunnerResult {
    if protocol != 1 {
        return RunnerResult::Silent;
    }
    // Recursion guard, wrong event, and an unregistered project all stay silent.
    if event.stop_hook_active || event.event_name != "Stop" {
        return RunnerResult::Silent;
    }
    let Some(root) = resolved_root else {
        return RunnerResult::Silent;
    };
    let output = match process.run_summary(root, STATUS_DEADLINE_MS, STATUS_STDOUT_LIMIT) {
        Ok(output) => output,
        Err(_) => {
            return RunnerResult::Message(bounded(
                "Memoria could not inspect this project. Run `memoria status` to see the documentation state.".into(),
            ));
        }
    };
    if output.timed_out || output.output_truncated {
        return RunnerResult::Message(bounded(
            "Memoria did not finish its documentation inspection in time. Run `memoria status` when convenient.".into(),
        ));
    }
    let text = String::from_utf8_lossy(&output.stdout).into_owned();
    let Some(summary) = parse_summary(&text) else {
        let code = first_diagnostic_code(&text).unwrap_or_else(|| "status_failed".to_string());
        return RunnerResult::Message(bounded(format!(
            "Memoria status failed ({code}). Run `memoria status` to see the diagnostic."
        )));
    };
    let due = summary.pending + summary.never_reviewed;
    if due > 0 || summary.waiting > 0 || summary.open_invalidations > 0 {
        return RunnerResult::Message(bounded(format!(
            "Memoria: {due} document(s) need review, {} waiting, {} open invalidation(s). Run `memoria review`.",
            summary.waiting, summary.open_invalidations
        )));
    }
    if summary.guidance_changed > 0 {
        return RunnerResult::Message(bounded(format!(
            "Memoria: documentation guidance changed for {} reviewed document(s); byte freshness is separate. Run `memoria guidance <README.md>`.",
            summary.guidance_changed
        )));
    }
    RunnerResult::Silent
}

/// The counters the runner reads out of the summary envelope.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct RunnerSummary {
    pub pending: u64,
    pub never_reviewed: u64,
    pub waiting: u64,
    pub open_invalidations: u64,
    pub guidance_changed: u64,
}

/// A minimal reader for the summary envelope. The runner needs six numbers,
/// so it does not depend on a full JSON value model.
fn parse_summary(text: &str) -> Option<RunnerSummary> {
    if !text.contains("\"ok\": true") && !text.contains("\"ok\":true") {
        return None;
    }
    Some(RunnerSummary {
        pending: number_after(text, "\"pending\"")?,
        never_reviewed: number_after(text, "\"never_reviewed\"")?,
        waiting: number_after(text, "\"waiting\"")?,
        open_invalidations: number_after(text, "\"open_invalidations\"")?,
        guidance_changed: number_after(text, "\"changed_documents\"")?,
    })
}

fn number_after(text: &str, key: &str) -> Option<u64> {
    let start = text.find(key)? + key.len();
    let rest = &text[start..];
    let colon = rest.find(':')? + 1;
    let digits: String = rest[colon..]
        .chars()
        .skip_while(|c| c.is_whitespace())
        .take_while(char::is_ascii_digit)
        .collect();
    digits.parse().ok()
}

fn first_diagnostic_code(text: &str) -> Option<String> {
    let key = "\"code\"";
    let start = text.find(key)? + key.len();
    let rest = &text[start..];
    let open = rest.find('"')?;
    let after = &rest[open + 1..];
    let close = after.find('"')?;
    Some(after[..close].to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ports::{AdapterError, BoundedOutput};

    struct Fake(Result<BoundedOutput, ()>);

    impl BoundedStatusProcess for Fake {
        fn run_summary(&self, _: &str, _: u64, _: u64) -> Result<BoundedOutput, AdapterError> {
            self.0
                .clone()
                .map_err(|_| AdapterError::new("spawn", None, "unavailable"))
        }
    }

    fn ok(stdout: &str) -> Fake {
        Fake(Ok(BoundedOutput {
            exit_code: Some(0),
            stdout: stdout.as_bytes().to_vec(),
            timed_out: false,
            output_truncated: false,
        }))
    }

    fn event() -> StopEvent {
        StopEvent {
            event_name: "Stop".into(),
            working_directory: Some("/work".into()),
            stop_hook_active: false,
        }
    }

    const CURRENT: &str = r#"{"ok": true,"data":{"reviews":{"current":6,"pending":0,"never_reviewed":0,"waiting":0},"guidance":{"documents_with_guidance":6,"changed_documents":0,"unreviewed_documents":0},"open_invalidations":0,"missing_invalidation_targets":0}}"#;

    #[test]
    fn a_current_project_produces_an_empty_object() {
        assert_eq!(
            run_stop(&ok(CURRENT), 1, &event(), Some("/work")),
            RunnerResult::Silent
        );
        assert_eq!(RunnerResult::Silent.to_json(), "{}");
    }

    #[test]
    fn recursion_wrong_events_and_unknown_projects_stay_silent() {
        let mut recursive = event();
        recursive.stop_hook_active = true;
        assert_eq!(
            run_stop(&ok(CURRENT), 1, &recursive, Some("/work")),
            RunnerResult::Silent
        );
        let mut other = event();
        other.event_name = "PreToolUse".into();
        assert_eq!(
            run_stop(&ok(CURRENT), 1, &other, Some("/work")),
            RunnerResult::Silent
        );
        assert_eq!(
            run_stop(&ok(CURRENT), 1, &event(), None),
            RunnerResult::Silent
        );
        // An unsupported protocol is silent, never a usage exit.
        assert_eq!(
            run_stop(&ok(CURRENT), 2, &event(), Some("/work")),
            RunnerResult::Silent
        );
    }

    #[test]
    fn pending_work_produces_one_bounded_message_with_the_next_command() {
        let due = CURRENT.replace("\"pending\":0", "\"pending\":3");
        let RunnerResult::Message(text) = run_stop(&ok(&due), 1, &event(), Some("/work")) else {
            panic!("expected an advisory message");
        };
        assert!(text.contains("3 document(s) need review"), "{text}");
        assert!(text.contains("memoria review"), "{text}");
        assert!(text.len() <= MAX_MESSAGE_BYTES);
    }

    #[test]
    fn changed_guidance_alone_points_at_the_guidance_command() {
        let changed = CURRENT.replace("\"changed_documents\":0", "\"changed_documents\":2");
        let RunnerResult::Message(text) = run_stop(&ok(&changed), 1, &event(), Some("/work"))
        else {
            panic!("expected an advisory message");
        };
        assert!(text.contains("memoria guidance"), "{text}");
        assert!(!text.contains("memoria review"), "{text}");
    }

    #[test]
    fn failures_and_deadlines_never_report_a_clean_project() {
        let broken = r#"{"ok": false,"diagnostics":[{"severity":"error","code":"state_corrupt","message":"x"}]}"#;
        let RunnerResult::Message(text) = run_stop(&ok(broken), 1, &event(), Some("/work")) else {
            panic!("expected an advisory message");
        };
        assert!(text.contains("state_corrupt"), "{text}");
        let timed_out = Fake(Ok(BoundedOutput {
            exit_code: None,
            stdout: vec![],
            timed_out: true,
            output_truncated: false,
        }));
        let RunnerResult::Message(text) = run_stop(&timed_out, 1, &event(), Some("/work")) else {
            panic!("expected an advisory message");
        };
        assert!(text.contains("did not finish"), "{text}");
        let RunnerResult::Message(text) = run_stop(&Fake(Err(())), 1, &event(), Some("/work"))
        else {
            panic!("expected an advisory message");
        };
        assert!(text.contains("could not inspect"), "{text}");
    }

    #[test]
    fn the_rendered_object_escapes_control_characters() {
        let result = RunnerResult::Message("a\"b\\c\nd\u{1}".into());
        assert_eq!(
            result.to_json(),
            "{\"systemMessage\":\"a\\\"b\\\\c\\nd\\u0001\"}"
        );
    }
}
