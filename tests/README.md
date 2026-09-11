# Command behavior tests

These tests compare Memoria command behavior with its public contract.

A test harness is shared code that prepares a test and captures its results.
The `Project` harness prepares a Git worktree, invokes Memoria commands, and captures output and file changes.
This README owns the test harness and integration suites in this directory.
The crate directories own their unit, property, and adapter tests.

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
`Project::seed` copies the three-level fixture into a temporary directory.
It restores the fixture document names to `README.md` and `README.memoria.toml`.
It creates a local Git repository for the test.

A review packet contains one README and the exact input bytes for its review.
Packets go into a separate temporary directory, outside that repository.

A document boundary groups the selected files that one README explains.
The fixture names prevent sample READMEs from becoming real boundaries in this project.
The root `memoria.toml` also excludes `tests/fixtures/**` from selected source inputs.
This decision leaves the real test suites inside the documentation scope.

`common/state_vectors.rs` builds the frozen `memoria.lock` representation vectors.
It reads one frozen logical input and rebuilds the scaled fixtures in Rust, so no large expansion is stored.

Source evidence: [common/mod.rs](common/mod.rs#L1), [common/state_vectors.rs](common/state_vectors.rs#L1), and [memoria.toml](../memoria.toml).

## The assertions describe observable behavior

A pending README requires review.
An export is a marked section that another README can copy through a declared import.
An acknowledgement records who reviewed a README and why its explanation is correct.
The `check` command requires valid structure, current reviews, and current imported text.

The baseline scenario starts with pending documents and empty import bodies.
After import updates and packet acknowledgements, `memoria check` succeeds.
Navigation warnings remain visible without causing that check to fail.

The source-change scenario makes one README owner pending while the READMEs that import its text wait.
An acknowledgement with an unchanged export ends that review path.
The snapshot-conflict scenario changes a file after packet creation.
It expects exit 3, exact differences, and unchanged state bytes.

The portability scenario varies each host ignore source without changing a selected file.
It expects unchanged policy hashes, unchanged state bytes, and a clean check.

Source evidence: [workflow.rs](workflow.rs#L1), [portability.rs](portability.rs#L1), and [state_format.rs](state_format.rs#L1).

## Suite map

An invalidation requests a review with an explicit reason, even without a file change.

| Suite | Coverage |
| --- | --- |
| [workflow.rs](workflow.rs) | Ownership, review order, imports, and invalidation |
| [review_context.rs](review_context.rs) | Saved views, P1 fallback, and verified historical coverage |
| [configuration.rs](configuration.rs) | Supported settings and explicit configuration errors |
| [packets.rs](packets.rs) | Packet transport, limits, integrity, replay, and concurrency |
| [edges.rs](edges.rs) | Discovery, paths, configuration, and corrupt state |
| [portability.rs](portability.rs) | Host rules against repository policy, and clean clones |
| [guidance.rs](guidance.rs) | Setup preview, guidance visibility, and advisory behavior |
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
Tests that exercise host ignore rules override those locations explicitly.

Some corruption tests construct invalid state deliberately.
The frozen `memoria.lock` vectors under `fixtures/state-v2` measure representation and size.
They are not project review records, and no test uses them as one.

Normal documentation review uses packets and the Memoria CLI.
Test results do not establish whether a human explanation is correct.

## Review context regression cases

`review_context.rs` exercises saved-packet retrieval, opt-in P1 preparation, and historical commit coverage through the CLI.
Its shapes include small and large owners, deep ownership, ordinary Markdown inputs, and shared-export fan-out.
It tests dirty acknowledgement, later exact matches, missing objects, shallow ancestry, and exhausted candidate or byte budgets.
Path changes require full-review fallback.
A semantic control needs an unchanged limit definition.
The test keeps that context accessible and does not claim model quality.
Existing packet, state, portability, process, and corruption suites retain their integrity checks.

## Continue

Read [workflow.rs:86](workflow.rs#L86) to examine the unchanged-export case.
