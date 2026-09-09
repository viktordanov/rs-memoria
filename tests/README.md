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
| [packets.rs](packets.rs) | Packet transport, limits, integrity, replay, and concurrency |
| [edges.rs](edges.rs) | Discovery, paths, configuration, and corrupt state |
| [portability.rs](portability.rs) | Host rules against repository policy, and clean clones |
| [guidance.rs](guidance.rs) | Setup preview, guidance visibility, and advisory behavior |
| [usability.rs](usability.rs) | Completions, reviewer identity, freshness explanations, state comparisons, and human guidance cues |
| [state_format.rs](state_format.rs) | Frozen lock vectors, corruption, limits, and inspection |
| [agent.rs](agent.rs) | Skill scopes, status, upgrade, backups, and removal |
| [agent_hooks.rs](agent_hooks.rs) | Hook ownership, interrupted transactions, and the bounded runner |
| `repairs.rs` through `repairs15.rs` | Regression cases for recorded defects |

The repair suites contain multiple cases and supporting controls.
Their source headers identify the related defect numbers.
The tests describe specific cases, not a proof of all possible filesystem behavior.

Some cases rebuild an interrupted state instead of stopping a real process.
They write the exact transaction record that each durable boundary leaves, then invoke the public command.
Their edits change one identity at a time, so a refusal names one cause.
The process cases are real: a stub Git records an identifier, and the test examines that process after the endpoint exits.
One stub blocks under its own identifier, and another exits early and leaves a descendant holding its pipes.

## Test boundaries

The harness invokes the executable that Cargo supplies through `CARGO_BIN_EXE_memoria`.
Each fixture runs with an isolated home and XDG configuration directory.
Every Git invocation also runs with empty system and global configuration files.
The harness clears inherited Git parameters, so a host signing rule cannot change a seed commit.
Tests that exercise host ignore rules override those locations explicitly.

Some corruption tests construct invalid state deliberately.
The frozen `memoria.lock` vectors under `fixtures/state-v2` measure representation and size.
They are not project review records, and no test uses them as one.

Normal documentation review uses packets and the Memoria CLI.
Test results do not establish whether a human explanation is correct.

## Continue

Read [workflow.rs:86](workflow.rs#L86) to examine the unchanged-export case.
