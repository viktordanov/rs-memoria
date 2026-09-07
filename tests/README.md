# Binary tests

These tests exercise the Memoria command contract in temporary Git worktrees.

The `Project` harness supplies a sample repository, invokes the binary, and compares the results.
This README owns the test harness and integration suites in this directory.
The crate directories own their unit, property, and adapter tests.

## Role in the project

<!-- memoria:export id="summary" -->
The integration tests exercise the binary in temporary Git worktrees.
They cover ownership, review order, packets, imports, invalidation, and installation.
Regression cases compare diagnostics and stored bytes after errors.
The sample repositories stay outside this project documentation scope.
<!-- /memoria:export -->

## A fixture becomes a repository

`Project::seed` copies the three-level fixture into a temporary directory.
It restores the fixture document names to `README.md` and `README.memoria.yml`.
It creates a local Git repository for the test.
Packets go into a separate temporary directory, outside that repository.

The fixture names prevent sample READMEs from becoming real boundaries in this project.
The root `memoria.yml` also excludes `tests/fixtures/**` from selected source inputs.
This decision leaves the real test suites inside the documentation scope.

Source evidence: [common/mod.rs:29](common/mod.rs#L29), [common/mod.rs:52](common/mod.rs#L52), and [memoria.yml](../memoria.yml).

## The assertions describe observable behavior

The baseline scenario starts with pending documents and empty import bodies.
After import updates and packet acknowledgements, `memoria check` succeeds.
Navigation warnings remain visible without causing that check to fail.

The source-change scenario makes one owner pending while its consumers wait.
An acknowledgement with an unchanged export ends that review path.
The snapshot-conflict scenario changes a file after packet creation.
It expects exit 3, exact differences, and unchanged state bytes.

Source evidence: [workflow.rs:9](workflow.rs#L9), [workflow.rs:86](workflow.rs#L86), and [workflow.rs:554](workflow.rs#L554).

## Suite map

| Suite | Coverage |
| --- | --- |
| [workflow.rs](workflow.rs) | Ownership, review order, imports, and invalidation |
| [packets.rs](packets.rs) | Packet transport, limits, integrity, replay, and concurrency |
| [edges.rs](edges.rs) | Discovery, paths, configuration, and corrupt state |
| [agent.rs](agent.rs) | Installation, local edits, backups, and removal |
| `repairs.rs` through `repairs15.rs` | Regression cases for recorded defects |

The repair suites contain multiple cases and supporting controls.
Their source headers identify the related defect numbers.
The tests describe specific cases, not a proof of all possible filesystem behavior.

## Test boundaries

The harness invokes the binary that Cargo supplies through `CARGO_BIN_EXE_memoria`.
Some corruption tests construct invalid state deliberately.
Normal documentation review uses packets and the Memoria CLI.
Test results do not establish whether a human explanation is correct.

## Continue

Read [workflow.rs:86](workflow.rs#L86) to examine the unchanged-export case.
