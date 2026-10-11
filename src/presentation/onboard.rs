//! `memoria onboard`: the hub a new user or agent reads first.
//!
//! Every view stays well under 1,000 tokens. Links name this executable's
//! own release, so they always match the installed behavior.

use memoria_application::usecases::onboard::{ProjectState, Setup};

use super::cli::OnboardTopic;

const VERSION: &str = env!("CARGO_PKG_VERSION");

fn docs(page: &str) -> String {
    format!("https://github.com/viktordanov/rs-memoria/blob/v{VERSION}/docs/{page}")
}

/// The main view. `project` is `None` outside a Git repository.
pub fn hub(project: Option<&ProjectState>, home: Option<&str>) -> String {
    let mut out = format!(
        "Memoria {VERSION} — CI for your docs

When code changes, Memoria marks the documents that explain it as pending and
shows exactly what changed. `memoria check` fails your build until someone has
reviewed them. You and your agents write every word; Memoria never edits prose.

How it works
  1. Code changes, and every document that covers the changed file becomes pending.
  2. `memoria review` shows what changed and what the review must read.
  3. You or an agent update the document. `memoria ack` records the review.
  4. `memoria check` in CI passes only when no document is pending.

"
    );
    match project {
        None => out.push_str(
            "This project  not a Git repository
  Memoria tracks the documents of a Git project. To begin:
  1. Open your project and write its root README.md.
  2. memoria onboard         see where the project stands
  3. memoria init            preview the setup; add --apply to write it

",
        ),
        Some(project) => {
            out.push_str(&format!("This project  {}\n", shorten(&project.root, home)));
            out.push_str(&project_rows(project));
            out.push('\n');
            out.push_str(&next_step(project));
        }
    }
    out.push_str(&format!(
        "Go deeper
  memoria onboard review     review a pending document, step by step
  memoria onboard agent      let an agent keep your docs current
  memoria onboard ci         block merges while docs are pending

Concepts  {}
",
        docs("concepts.md")
    ));
    out
}

fn shorten(root: &str, home: Option<&str>) -> String {
    match home {
        Some(home) if !home.is_empty() && root.starts_with(home) => {
            let rest = &root[home.len()..];
            if rest.is_empty() || rest.starts_with('/') {
                return format!("~{rest}");
            }
            root.to_string()
        }
        _ => root.to_string(),
    }
}

fn row(label: &str, value: &str, action: Option<&str>) -> String {
    match action {
        Some(action) => format!("  {label:<12}  {value:<17} → {action}\n"),
        None => format!("  {label:<12}  {value}\n"),
    }
}

fn project_rows(project: &ProjectState) -> String {
    let mut out = String::new();
    out.push_str(&match &project.setup {
        Setup::Missing { .. } => row("Memoria", "not set up", Some("memoria init")),
        Setup::Configured {
            documents,
            needs_review: 0,
        } => row(
            "Memoria",
            &format!(
                "{documents} {} · all current",
                plural(*documents, "document")
            ),
            None,
        ),
        Setup::Configured {
            documents,
            needs_review,
        } => row(
            "Memoria",
            &format!(
                "{documents} {} · {needs_review} pending",
                plural(*documents, "document")
            ),
            None,
        ),
        Setup::Broken { code } => row(
            "Memoria",
            &format!("needs attention ({code})"),
            Some("memoria lint"),
        ),
    });
    if project.skills.is_empty() {
        // A bare install detects its target only when exactly one agent
        // directory exists, so the action always names one, or points to
        // the topic that explains the choice.
        let action = match project.detected_agent {
            Some(target) => format!("memoria agent install --target {}", target.as_str()),
            None => "memoria onboard agent".to_string(),
        };
        out.push_str(&row("Agent skill", "not installed", Some(&action)));
    } else {
        for skill in &project.skills {
            let target = skill.target.as_str();
            let value = match &skill.problem {
                Some(code) => format!("{target} · {} ({code})", skill.state),
                None => format!("{target} · {}", skill.state),
            };
            let action = match skill.state.as_str() {
                "current" => None,
                "outdated" => Some(format!("memoria agent upgrade --target {target}")),
                _ => Some(format!("memoria agent status --target {target}")),
            };
            out.push_str(&row("Agent skill", &value, action.as_deref()));
        }
    }
    out.push_str(&match &project.workflow {
        Some(workflow) if workflow.state == "absent" => row(
            "CI check",
            "not installed",
            Some("memoria integrations github install"),
        ),
        Some(workflow) if workflow.state == "outdated" => row(
            "CI check",
            &format!("{} · outdated", workflow.path),
            Some("memoria integrations github upgrade"),
        ),
        Some(workflow) => row(
            "CI check",
            &format!("{} · {}", workflow.path, workflow.state),
            None,
        ),
        None => row(
            "CI check",
            "unknown",
            Some("memoria integrations github status"),
        ),
    });
    out
}

fn next_step(project: &ProjectState) -> String {
    match &project.setup {
        Setup::Missing { root_readme: false } => "Start here
  1. Write the root README.md yourself: the one explanation only you can give.
  2. memoria init            preview what Memoria would track; writes nothing
  3. memoria init --apply    create memoria.toml and memoria.lock; commit both

"
        .to_string(),
        Setup::Missing { root_readme: true } => "Start here
  1. memoria init            preview what Memoria would track; writes nothing
  2. memoria init --apply    create memoria.toml and memoria.lock; commit both

"
        .to_string(),
        Setup::Broken { .. } => "Next: memoria lint
  The project has a structural error. lint names it and how to fix it.

"
        .to_string(),
        Setup::Configured {
            needs_review: 0, ..
        } => "Next: nothing to review. `memoria check` passes.

"
        .to_string(),
        Setup::Configured { needs_review, .. } => format!(
            "Next: memoria review
  {needs_review} {} a review. `memoria onboard review` walks through it.

",
            if *needs_review == 1 {
                "document needs"
            } else {
                "documents need"
            }
        ),
    }
}

fn plural(count: u64, word: &str) -> String {
    if count == 1 {
        word.to_string()
    } else {
        format!("{word}s")
    }
}

pub fn topic(topic: OnboardTopic) -> String {
    match topic {
        OnboardTopic::Review => format!(
            "Review a pending document in five steps

  1. memoria review                       the next document, in dependency order
  2. memoria review <DOC> --save <DIR>    save the review artifact outside the repo
  3. Read what changed, the whole document, and `memoria guidance <DOC>`.
     Edit the document if it no longer matches the code.
  4. After any edit, save a fresh artifact: step 2 again.
  5. memoria ack <DOC> --packet <FILE> --reviewer <you> --result <updated|no-update> --note \"<why>\"

Then run `memoria check`. It passes when nothing is pending.

Good to know
  · Read the whole document, even when the review suggests one section.
  · An old artifact cannot approve new changes: ack refuses it.
  · An acknowledgement records that someone looked, not that the prose is right.

Guide  {}
",
            docs("workflow.md")
        ),
        OnboardTopic::Agent => format!(
            "Let an agent keep your docs current

Quickest: tell your agent to run `memoria onboard`. Nothing to install.

For regular reviews, install the skill: the full review procedure.
  memoria agent install      detects .claude or .agents, or pass --target claude|codex
  memoria agent status       report the installed skill
  memoria agent upgrade      run after every Memoria upgrade

Optional: a Stop hook. When the agent stops, it reports how many documents
need review and names `memoria review`. It never continues the turn. Your
agent client must approve the hook before it runs.
  memoria agent hook install --target claude

Every acknowledgement records who reviewed. Agents pass `--reviewer <agent-name>`.

Guide  {}
",
            docs("agents.md")
        ),
        OnboardTopic::Ci => format!(
            "Block merges while docs are pending

  1. memoria integrations github install            preview the workflow; writes nothing
  2. memoria integrations github install --apply    write .github/workflows/memoria.yml
  3. Commit it with memoria.toml and memoria.lock.

The job installs a checksum-verified Memoria {VERSION} (x64 or ARM64) and runs
`memoria check`. It needs no secrets and only read access.

A failing check means someone must review locally and commit memoria.lock.
CI never acknowledges anything.

After upgrading Memoria   memoria integrations github upgrade --apply
Guide  {}
",
            docs("github-actions.md")
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use memoria_application::ports::AgentTarget;
    use memoria_application::usecases::onboard::{Skill, Workflow};

    fn project(setup: Setup) -> ProjectState {
        ProjectState {
            root: "/home/ada/code/shop".into(),
            setup,
            skills: vec![Skill {
                target: AgentTarget::Claude,
                state: "outdated".into(),
                problem: None,
            }],
            detected_agent: Some(AgentTarget::Claude),
            workflow: Some(Workflow {
                path: ".github/workflows/memoria.yml".into(),
                state: "current".into(),
            }),
        }
    }

    /// Every view stays under about 1,000 tokens: 4,000 bytes is a generous
    /// upper bound at roughly four bytes per token.
    #[test]
    fn every_view_fits_the_token_budget() {
        let views = [
            hub(None, None),
            hub(
                Some(&project(Setup::Missing { root_readme: false })),
                Some("/home/ada"),
            ),
            hub(
                Some(&project(Setup::Configured {
                    documents: 12,
                    needs_review: 2,
                })),
                None,
            ),
            topic(OnboardTopic::Review),
            topic(OnboardTopic::Agent),
            topic(OnboardTopic::Ci),
        ];
        for view in views {
            assert!(view.len() < 4_000, "{} bytes:\n{view}", view.len());
        }
    }

    #[test]
    fn the_project_block_names_one_action_per_gap() {
        let text = hub(
            Some(&project(Setup::Missing { root_readme: false })),
            Some("/home/ada"),
        );
        assert!(text.contains("Write the root README.md yourself"), "{text}");
        assert!(text.contains("This project  ~/code/shop\n"), "{text}");
        assert!(
            text.contains("not set up        → memoria init\n"),
            "{text}"
        );
        assert!(
            text.contains("claude · outdated → memoria agent upgrade --target claude\n"),
            "{text}"
        );
        assert!(text.contains("Start here\n"), "{text}");
        let text = hub(
            Some(&project(Setup::Configured {
                documents: 1,
                needs_review: 1,
            })),
            None,
        );
        assert!(text.contains("1 document · 1 pending"), "{text}");
        assert!(text.contains("1 document needs a review"), "{text}");
        let text = hub(Some(&project(Setup::Missing { root_readme: true })), None);
        assert!(!text.contains("Write the root README.md"), "{text}");
        assert!(text.contains("  1. memoria init "), "{text}");
    }

    #[test]
    fn links_name_this_release() {
        for view in [hub(None, None), topic(OnboardTopic::Ci)] {
            assert!(view.contains(&format!("/blob/v{VERSION}/docs/")), "{view}");
        }
    }

    #[test]
    fn home_shortening_needs_a_path_boundary() {
        assert_eq!(shorten("/home/ada/x", Some("/home/ada")), "~/x");
        assert_eq!(shorten("/home/ada", Some("/home/ada")), "~");
        assert_eq!(shorten("/home/adam/x", Some("/home/ada")), "/home/adam/x");
    }

    /// Every command a view prints parses with the real grammar, so the hub
    /// can never point at a command or option that does not exist.
    #[test]
    fn every_printed_command_parses() {
        use clap::Parser as _;
        let configured = |needs_review| {
            hub(
                Some(&project(Setup::Configured {
                    documents: 3,
                    needs_review,
                })),
                None,
            )
        };
        let views = [
            hub(None, None),
            hub(Some(&project(Setup::Missing { root_readme: false })), None),
            hub(Some(&project(Setup::Missing { root_readme: true })), None),
            hub(
                Some(&project(Setup::Broken {
                    code: "configuration_invalid".into(),
                })),
                None,
            ),
            configured(0),
            configured(2),
            topic(OnboardTopic::Review),
            topic(OnboardTopic::Agent),
            topic(OnboardTopic::Ci),
        ];
        let mut checked = 0;
        for view in &views {
            for line in view.lines() {
                for (start, _) in line.match_indices("memoria ") {
                    if line[..start].ends_with(|c: char| c.is_alphanumeric()) {
                        continue;
                    }
                    let mut end = line.len();
                    for stop in ["  ", "`", " · ", ". "] {
                        if let Some(at) = line[start..].find(stop) {
                            end = end.min(start + at);
                        }
                    }
                    let command = line[start..end].trim_end_matches('.');
                    if command.contains("...") {
                        continue;
                    }
                    let concrete = command
                        .replace("<DOC>", "README.md")
                        .replace("<DIR>", "/tmp/artifacts")
                        .replace("<FILE>", "/tmp/artifact.json")
                        .replace("<you>", "ada")
                        .replace("<updated|no-update>", "updated")
                        .replace("\"<why>\"", "because-it-matches")
                        .replace("claude|codex", "claude");
                    let words: Vec<&str> = concrete.split_whitespace().collect();
                    // Prose may name a command without its arguments, such as
                    // `memoria ack`: that command must exist.
                    let named = words.len() == 2
                        && <super::super::cli::Cli as clap::CommandFactory>::command()
                            .find_subcommand(words[1])
                            .is_some();
                    assert!(
                        named || super::super::cli::Cli::try_parse_from(&words).is_ok(),
                        "does not parse: {command}"
                    );
                    checked += 1;
                }
            }
        }
        assert!(checked >= 20, "only {checked} commands found");
    }

    /// Every skill action names its target: a bare agent command fails with
    /// `target_ambiguous` unless exactly one agent directory exists.
    #[test]
    fn skill_actions_always_name_a_target() {
        let mut state = project(Setup::Configured {
            documents: 1,
            needs_review: 0,
        });
        state.skills.clear();
        let text = hub(Some(&state), None);
        assert!(
            text.contains("→ memoria agent install --target claude\n"),
            "{text}"
        );
        state.detected_agent = None;
        let text = hub(Some(&state), None);
        assert!(
            text.contains("not installed     → memoria onboard agent\n"),
            "{text}"
        );
        state.skills = vec![Skill {
            target: AgentTarget::Codex,
            state: "unreadable".into(),
            problem: Some("skill_record_invalid".into()),
        }];
        let text = hub(Some(&state), None);
        assert!(
            text.contains(
                "codex · unreadable (skill_record_invalid) → memoria agent status --target codex"
            ),
            "{text}"
        );
        assert!(!text.contains("not installed"), "{text}");
    }
}
