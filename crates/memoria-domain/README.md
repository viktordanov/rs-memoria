# memoria-domain

The domain crate decides which READMEs require review and which review comes first.

The domain receives values and returns values.
It uses only the Rust standard library at runtime.
The application supplies repository facts.

Read [ownership.rs](src/ownership.rs#L18) to start with the rule that assigns files to READMEs.

## On this page

- [Role in the project](#role-in-the-project)
- [Files and shared text](#files-and-shared-text-have-different-relationships)
- [Manifest comparison](#manifests-make-changes-visible)
- [Review states and acknowledgement](#pending-and-waiting-describe-different-states)
- [Boundaries and next step](#boundaries-and-errors)

## Role in the project

<!-- memoria:export id="summary" -->
The domain crate assigns selected files to their nearest README.
It compares recorded review inputs with current inputs.
Its rules determine which READMEs require review and their order.
<!-- /memoria:export -->

## Files and shared text have different relationships

A document boundary groups the selected files that one README explains.
That README is their owner.
An export is a marked section that another README can copy.
The README that supplies the section is the provider.
The README that copies it is the consumer.

| Relationship | Meaning | Effect on review |
| --- | --- | --- |
| Ownership | The nearest README owns a selected file. | A file change affects its owner. |
| Import | A consumer names an export in a provider README. | An export change affects its consumers. |
| Navigation | A normal Markdown link connects readers to another README. | The link creates no review dependency. |

A pending README requires a review.
`crates/memoria-domain/README.md` owns `crates/memoria-domain/src/review.rs`.
The root README imports the domain summary.
A change to `review.rs` makes this README pending.
If the summary stays unchanged, that change does not make the root README pending.
The root still waits for this provider review before its own review can proceed.

Source evidence: [ownership.rs:18](src/ownership.rs#L18), [graph.rs:102](src/graph.rs#L102), and [schedule.rs:66](src/schedule.rs#L66).

## Manifests make changes visible

A manifest lists the identities, sizes, and hashes that define one review.
`InputManifest` contains the document identity, document hash, policy hash, selected files, and imported exports.
Each file entry contains its path, byte count, and hash.
Each import entry also names the provider and export identifier.
The constructor sorts these entries and rejects duplicate identities.

The manifest comparison separates document changes from changes to files, imports, or policy.
The canonical encoder gives these values a stable byte representation.
The application requests hashes from its hasher port.
The domain itself does not calculate XXH3-64 hashes.

The policy scopes hold repository `.gitignore` paths only.
Host ignore sources have no identity here, so they cannot enter a policy hash.
The encoder names the algorithms it assumes: `git-worktree-v2`, `repository-ignore-v1`, and `nearest-readme-v1`.

Source evidence: [manifest.rs](src/manifest.rs#L73), [policy.rs](src/policy.rs#L1), and [canonical.rs](src/canonical.rs#L1).

## Pending and waiting describe different states

An invalidation is an explicit request for a review with a recorded reason.
A document is pending after a first review becomes necessary, an input changes, or an invalidation applies.
An edit to the document itself also makes it pending.

A current document has no remaining review cause.
A document waits while a provider is pending or waits for another provider.
Thus, a current document can still wait.
A ready document is pending and has no provider that prevents its review.

## An acknowledgement advances one review

A review packet captures the README and its input bytes for one review.
An acknowledgement records the reviewer, result, and reason that the explanation is correct.
A revision is the counter that increases after each acknowledgement for that README.

`ReviewState::acknowledge` compares the manifests, document revision, and covered invalidations before it changes the state value.
It increments the document revision and records the reviewer, result, note, and guidance digest.
It clears only the covered invalidations for that document.
The application owns the lock, readiness validation, and durable save.

The record stores the guidance digest as a value.
The domain does not decide whether that guidance is good or compatible.
The application compares the packet context with the current context before it calls the aggregate.

Source evidence: [schedule.rs](src/schedule.rs#L38) and [review.rs](src/review.rs#L1).

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
| `guidance` | The guidance digest value type and its entry kinds |
| `review` | Review records and invalidation transitions |

The [crate manifest](Cargo.toml) declares no runtime dependencies.
The [crate exports](src/lib.rs#L13) identify the implemented modules.

## Continue

Read [ownership.rs:18](src/ownership.rs#L18) to trace one file to its owner.
