# Command behavior tests

These tests compare Memoria command behavior with its public contract.

A test harness is shared code that prepares a test and captures its results.
The `Project` harness prepares a Git worktree, invokes Memoria commands, and captures output and file changes.
This README covers the test harness and integration suites in this directory.
The crate READMEs cover their unit, property, and adapter tests.

Read [workflow.rs](workflow.rs#L9) to start with the scenario that makes all reviews current.

## On this page

- [Role in the project](#role-in-the-project)
- [Fixture setup](#a-fixture-becomes-a-repository)
- [Observable assertions](#the-assertions-describe-observable-behavior)
- [Suite map](#suite-map)
- [Setup Action fixtures](#setup-action-fixtures)
- [Test boundaries and next step](#test-boundaries)

## Role in the project

<!-- memoria:export id="summary" -->
The integration tests invoke Memoria commands in temporary Git worktrees.
They compare output, exit statuses, and stored files.
The sample repositories stay outside this project documentation scope.
<!-- /memoria:export -->

## A fixture becomes a repository

A fixture is sample input for a test.
`Project::seed` copies the three-level fixture into a temporary directory, and `Project::seed_from` copies any named fixture.
`Project::worked_example` builds the worked example of the specification: a root that links `auth/`, imports an opted-in guide, and never mentions `legacy/`.
`Project::agent_instructions` seeds the `agent-instructions` fixture: two tracked agent files that share two registered section guides, with every document acknowledged.
It restores the fixture document names to `README.md` and `README.memoria.toml`.
It creates a local Git repository for the test.

A review artifact identifies the snapshot of one document's review. The default
manifest states requirements. The full export adds the exact input bytes.
`Project::review_packet` captures the manifest, and `Project::review_full`
captures the export. Both write into a separate temporary directory, outside
that repository.

Every `README.md` in this repository is a tracked document, even under an ignored folder.
The fixture names prevent sample READMEs from becoming real documents in this project.
The root `memoria.toml` also excludes `tests/fixtures/**` from selected source inputs.
This decision leaves the real test suites inside the documentation scope.

`common/state_vectors.rs` builds the frozen `memoria.lock` representation vectors.
It reads one frozen logical input and rebuilds the scaled fixtures in Rust, so no large expansion is stored.

Source evidence: [common/mod.rs](common/mod.rs#L1), [common/state_vectors.rs](common/state_vectors.rs#L1), and [memoria.toml](../memoria.toml).

## The assertions describe observable behavior

A pending document requires review.
An export is a marked section that another document can copy through a declared import.
An acknowledgement records who reviewed a document and why its explanation is correct.
The `check` command requires valid structure, current reviews, and current imported text.

The baseline scenario starts with pending documents and empty import bodies.
After import updates and acknowledgements, `memoria check` succeeds.
Navigation warnings remain visible without causing that check to fail.

The source-change scenario makes the document that covers the file pending while the documents that import its text wait.
The three-level fixture keeps `src/disconnected/README.md` deliberately unlinked, so the root also covers `src/disconnected/item.rs`: the built-in overlap case, with its `handoff_absent` hint.
An acknowledgement with an unchanged export ends that review path.
The snapshot-conflict scenario changes a file after the review. It expects
exit 3 and unchanged state bytes. A full export yields exact per-input
differences. A manifest yields the changed digest categories, because it never
carried the reviewed bytes.

The portability scenario varies each host ignore source without changing a selected file.
It expects unchanged policy hashes, unchanged state bytes, and a clean check.

Source evidence: [workflow.rs](workflow.rs), [portability.rs](portability.rs), and [state_format.rs](state_format.rs).

## Suite map

An invalidation requests a review with an explicit reason, even without a file change.

| Suite | Coverage |
| --- | --- |
| [workflow.rs](workflow.rs) | Scopes, review order, imports, and invalidation |
| [handoffs.rs](handoffs.rs) | The backbone rule: the `auth/` before and after, changes A–G of the worked example, hints, waiting, and fan-out |
| [documents.rs](documents.rs) | Discovery: every marker line shape, name variants, selection, encoding, symlinks, and nested repositories. Every command on an opted-in document, including the document count of a passing check |
| [document_reviews.rs](document_reviews.rs) | Sections over scope, relationships, bounded downstream lists, full exports, and render convergence |
| [save_ack.rs](save_ack.rs) | `review --save` destinations, bytes, mode, collisions, and `ack` without a token |
| [upgrade.rs](upgrade.rs) | Configuration version 3 and projects acknowledged by the real Memoria 0.6.0 |
| [review_context.rs](review_context.rs) | Saved export views, P1 fallback, and verified historical coverage |
| [sections.rs](sections.rs) | Advisory mappings, section guide markers and bounds, review mode, fallback reasons, and snapshot safety |
| [section_patterns.rs](section_patterns.rs) | Section `files` patterns and `!` exclusions: effective-scope expansion, added, deleted, and renamed matches, changed-only reads, identity with literal lists, lint-only empty-match hints, invalid tokens, and the full-baseline reading lines |
| [configuration.rs](configuration.rs) | Supported settings, section guide registration rules, `ignore` and `include` pattern meaning on the shared glob engine, and explicit configuration errors |
| [packets.rs](packets.rs) | Artifact transport, limits, integrity, replay, and concurrency |
| [parallel.rs](parallel.rs) | Independent reviewers in one checkout: simultaneous acknowledgements, shared sources, import waiting, competing reviewers, the lock wait, and worktree integration |
| [quiet_output.rs](quiet_output.rs) | Default human output on stdout and stderr together: hints and progress only on request, review hunks, `explain` next steps, and the guidance assessment |
| [edges.rs](edges.rs) | Discovery, paths, configuration, and corrupt state |
| [portability.rs](portability.rs) | Host rules against repository policy, and clean clones |
| [guidance.rs](guidance.rs) | Setup preview, guidance visibility, advisory behavior, section guide layers, and the agent-instructions cookbook run |
| `cookbook_readme_tree.rs`, `cookbook_beside_code.rs`, `cookbook_central_docs.rs` | One real run of each cookbook fixture, compared block by block with its page. The beside-code run also checks the root README's "CLI at a glance" output |
| [usability.rs](usability.rs) | Completions, reviewer identity, freshness explanations, state comparisons, and human guidance cues |
| [state_format.rs](state_format.rs) | Frozen lock vectors, corruption, limits, and inspection |
| [agent.rs](agent.rs) | Skill scopes, status, upgrade, backups, and removal |
| [agent_hooks.rs](agent_hooks.rs) | Hook ownership, interrupted transactions, and the bounded runner |
| [integrations.rs](integrations.rs) | The umbrella grammar and equality with the older command names |
| [github_workflows.rs](github_workflows.rs) | Workflow preview, apply, ownership, conflicts, and path safety |
| `repairs.rs` through `repairs15.rs` | Regression cases for recorded defects |

The repair suites contain multiple cases and supporting controls.
Their source headers identify the related defect numbers.
The tests describe specific cases, not a proof of all possible filesystem behavior.

The parallel cases start real Memoria processes at the same time.
Each reviewer captures its artifact before anyone acknowledges, as parallel reviewers do.
The worktree cases capture in a linked worktree and acknowledge in the main checkout after a merge.

Some cases rebuild an interrupted state instead of stopping a real process.
They write the exact transaction record that each durable boundary leaves, then invoke the public command.
Their edits change one identity at a time, so a refusal names one cause.
The process cases are real: a stub Git records an identifier, and the test examines that process after the endpoint exits.
One stub blocks under its own identifier, and another exits early and leaves a descendant holding its pipes.

## Setup Action fixtures

`setup_action/` holds Python fixtures for the composite setup Action. Invoke them with this command:

```sh
python3 -m unittest discover -s tests/setup_action -p 'test_*.py'
```

They need Python 3 and PyYAML. PyYAML is a test-only dependency. `scripts/setup-memoria.py` needs no YAML library.

`support.py` compiles a real ELF executable for the host with `rustc` and writes a fake `curl` that serves one fixed routing table. The fake transport records the exact requested URL, so a wrong asset selection fails a test. The fixtures replace the process architecture and the operating-system release file through module attributes, so the installer itself keeps no test-only environment variable. A poisoned `cargo` stub sits beside the fake transport: the Action must never invoke it.

`test_install.py` also covers the version check as a whole operation. One deadline must cover the child and both stream readers, and both readers must finish before a version answer is accepted. Its stubs hold one or both pipes open in a grandchild after the parent exits, so an unfinished stream is a real condition rather than a simulated one. Each such stub releases its pipes within twelve seconds.

`test_metadata.py` parses `action.yml` with a general YAML parser instead of the code that reads it. `check_consumer_workflow.py` examines the workflow that the CLI generates. PyYAML follows YAML 1.1 and reads the bare `on` key as a boolean. The checker therefore accepts both spellings of that key, and it also examines the raw bytes.

`crates/memoria-infrastructure/tests/github_workflow_races.rs` injects an edit at the exact step where an editor can interfere with a workflow change. The adapter exposes a step hook for that purpose, in the same way that `fs.rs` exposes a fault hook. The covered windows are the displacement, the restoration that follows an aborted change, the no-clobber creation, the ownership record, and an interrupted transaction. Each fixture asserts which byte sequences survive; none of them claims coverage of a window it does not inject.

## Test boundaries

The harness invokes the executable that Cargo supplies through `CARGO_BIN_EXE_memoria`.
A hook installation measures the version of the agent client it targets, so a test that installs a hook puts a stub client on `PATH`.
`agent_hooks.rs` and `integrations.rs` each define that stub.
Without it a test passes only on a machine that happens to have the real client installed.
Each fixture runs with an isolated home and XDG configuration directory.
Every Git invocation also runs with empty system and global configuration files.
The harness clears inherited Git parameters, so a host signing rule cannot change a seed commit.
It also clears `MEMORIA_LOCK_WAIT_MS`, so a host wait setting cannot change a lock test.
Tests that exercise host ignore rules override those locations explicitly.

Some corruption tests construct invalid state deliberately.
The frozen `memoria.lock` vectors under `fixtures/state-v2` measure representation and size in format 2. They stay as read-compatibility input.
Each one decodes with every record unrecorded and encodes to the committed format 3 vector of the same name under `fixtures/state-v3`, which also holds `evidence.lock` with recorded nested folders and an empty recorded set.
They are not project review records, and no test uses them as one.
`fixtures/upgrade-0.6` holds seven small projects whose `memoria.lock` the real Memoria 0.6.0 executable acknowledged: one with a linked child README, one without the link, one without the link whose child was reviewed a second after its root, one whose root was reviewed twice, one whose root review covered an invalidation, and two with 12 and 13 unlinked child READMEs.
`common/counting.rs` runs the review use case in process with a counting hasher, so a test can pin how many legacy proof recomputations a review performs.

Normal documentation review uses review artifacts and the Memoria CLI.
Test results do not establish whether a human explanation is correct.

## Review context regression cases

`review_context.rs` exercises saved-export retrieval, legacy P1 preparation, and historical commit coverage through the CLI.
Its shapes include small and large scopes, deep nesting, ordinary Markdown inputs, and shared-export fan-out.
It tests dirty acknowledgement, later exact matches, missing objects, shallow ancestry, and exhausted candidate or byte budgets.
Path changes require full-review fallback.
A semantic control needs an unchanged limit definition.
The test keeps that context accessible and does not claim model quality.
Existing artifact, state, portability, process, and corruption suites retain their integrity checks.

## Snapshot safety cases

`sections.rs` is the release gate for advisory sections. It states what the
CLI must refuse, not what it saves.

The suite covers the section grammar through the CLI, every fallback reason,
baseline eligibility with and without verifiable bytes, and mapping validation
with its advisory diagnostic. It checks that one invalid mapping withdraws
every suggestion, and that a new mapping cannot reduce the required scope.

`section_patterns.rs` adds the same guarantees for patterns: an added match is
still a full baseline, and only the reading list is limited to changed matches.
Unit tests in `crates/memoria-application/src/usecases/requirements.rs` check
that the current resolver and the prior-mapping resolver expand the same tokens
to the same identity, because both call one domain function.

It also checks the snapshot binding itself. Each bound component of the review
context must change the token: an unrelated scope source, the document body, the
mapping associations, the effective guidance, and the effective selection
policy. A provider context change refuses a consumer artifact even when the
imported export body stays byte-identical.

Stale-acknowledgement refusal has its own cases. An edit outside the suggested
sections, a guidance edit, and a section edit each refuse the captured
artifact and write nothing.

Three further groups guard the artifact contract itself:

1. A manifest that violates its own schema cannot acknowledge. The cases are
   an unsupported selection version, an absent document input, an inconsistent
   scope-file count, a repeated reading identity, and a wrong document. A
   full export's embedded requirements go through the same rules.
2. An artifact from an earlier release is refused with regeneration
   instructions, never converted and never called corrupt. A supported
   version with tampered content still fails its integrity check.
3. An added source and an added import each appear exactly once in the
   suggested reads. A removed source never becomes a current read.

`agent.rs` pins the empty-plan control flow of the installed skill: the empty
plan reaches the clean check and never reaches the acknowledgement step.

`sections.rs` also pins the complete manifest example in the CLI reference
against the production decoder, so the documentation and the schema cannot
drift apart. In the same way, each cookbook test drives its fixture through
the built binary and compares every file block and output block of its page
under `docs/cookbooks/` with that run. `guidance.rs` holds the agent-instructions
run, and each `cookbook_*.rs` suite holds one more. They share `cookbook_run`
and `cookbook_block` from `common/mod.rs`. Run a cookbook test with
`MEMORIA_PRINT_COOKBOOK=1` and `--nocapture` to print fresh output blocks.

A section guide is bound like project guidance. `save_ack.rs` checks that an
edit to any guide of a document after `--save` refuses the artifact, including
the guide of a section that the review does not suggest.

No reading-cost result may weaken any rule in this suite.

## Continue

Read [workflow.rs:86](workflow.rs#L86) to examine the unchanged-export case.
