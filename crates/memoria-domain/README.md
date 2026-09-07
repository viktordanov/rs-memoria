# memoria-domain

The domain crate defines the rules that determine documentation ownership and review state.

The domain receives values and returns values.
It uses only the Rust standard library at runtime.
Application use cases supply repository facts through adapters.

## Role in the project

<!-- memoria:export id="summary" -->
The domain crate assigns selected files to their nearest README.
It separates ownership, imports, and navigation.
Its manifests describe review inputs, and its review rules determine pending work.
It orders providers before consumers without file access or Git processes.
<!-- /memoria:export -->

## The model has three relationships

| Relationship | Meaning | Effect on review |
| --- | --- | --- |
| Ownership | The nearest README owns a selected file. | A file change affects its owner. |
| Import | A consumer names an export in a provider README. | An export change affects its consumers. |
| Navigation | A normal Markdown link connects readers to another README. | The link creates no review dependency. |

`crates/memoria-domain/README.md` owns `crates/memoria-domain/src/review.rs`.
The root README imports the domain summary.
A change to `review.rs` makes this README pending.
If the summary stays unchanged, that change does not make the root README pending.
The root still waits for this provider review before its own review can proceed.

Source evidence: [ownership.rs:18](src/ownership.rs#L18), [graph.rs:102](src/graph.rs#L102), and [schedule.rs:66](src/schedule.rs#L66).

## Manifests make changes visible

`InputManifest` contains the document identity, document hash, policy hash, selected files, and imported exports.
Each file entry contains its path, byte count, and hash.
Each import entry also names the provider and export identifier.
The constructor sorts these entries and rejects duplicate identities.

The manifest comparison separates document changes from changes to files, imports, or policy.
The canonical encoder gives these values a stable byte representation.
The application requests hashes from its hasher port.
The domain itself does not calculate xxHash64 hashes.

Source evidence: [manifest.rs:73](src/manifest.rs#L73) and [canonical.rs:110](src/canonical.rs#L110).

## Pending and waiting describe different states

A document is pending after a first review becomes necessary, an input changes, or an explicit invalidation applies.
An edit to the document itself also makes it pending.
A document waits while a provider is pending or waits for another provider.
Thus, a current document can still wait.
A ready document is pending and has no provider that prevents its review.

`ReviewState::acknowledge` compares the manifests, document revision, and covered invalidations before it changes the state value.
It increments the document revision and records the reviewer, result, and note.
It clears only the covered invalidations for that document.
The application owns the lock, readiness validation, and durable save.

Source evidence: [schedule.rs:38](src/schedule.rs#L38) and [review.rs:645](src/review.rs#L645).

## Boundaries and errors

The domain contains validated identities, algorithms, and state transitions.
It contains no database repositories, event bus, or entity service framework.
Its errors describe invalid paths, duplicate inputs, import cycles, and invalid state transitions.
The application maps these errors to diagnostics and exit classes.

| Modules | Ownership |
| --- | --- |
| `path`, `glob`, `document`, `text` | Identities, declarations, patterns, and review text rules |
| `selection`, `ownership`, `policy` | Selected inputs and their owners |
| `graph`, `schedule` | Dependencies, navigation, and review order |
| `manifest`, `canonical` | Input comparisons and stable byte encodings |
| `review` | Review records and invalidation transitions |

The [crate manifest](Cargo.toml) declares no runtime dependencies.
The [crate exports](src/lib.rs#L13) identify the implemented modules.

## Continue

Read [ownership.rs:18](src/ownership.rs#L18) to trace one file to its owner.
