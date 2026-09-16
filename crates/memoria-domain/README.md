# memoria-domain

The domain crate decides which READMEs require review and which review comes first.

The domain receives values and returns values.
It uses only the Rust standard library at runtime.
The application supplies repository facts.

Read [ownership.rs](src/ownership.rs#L18) to start with the rule that assigns files to READMEs.

## On this page

- [Role in the project](#role-in-the-project)
- [Files and shared text](#files-and-shared-text-have-different-relationships)
- [Manifests and the review context](#manifests-make-changes-visible)
- [Section mappings](#sections-map-prose-to-sources)
- [Review states, acknowledgement, and next step](#pending-and-waiting-describe-different-states)

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

Source evidence: [manifest.rs](src/manifest.rs), [policy.rs](src/policy.rs), and [canonical.rs](src/canonical.rs).

## The review context binds more than the inputs

A review token identifies one snapshot. `encode_review_token_v3` accepts three
digests, and it encodes them with four more values in one frozen order:

| Encoded value | Contents |
| --- | --- |
| Domain and document | The separation string `memoria-review-token-v3`, then the owner identity. |
| Review revision | The current review revision of that owner. |
| `I`, `B`, `C` | The digests of the complete input manifest, the complete prior review record, and the review context. |
| Invalidations | The covered invalidations, sorted by id, with their exact reasons. |

The application hashes the encoded bytes. The token is `mrv3.` and the sixteen
lowercase hexadecimal digits of that hash. The domain crate computes no hash of
its own.

`ReviewContext` holds the descriptors that reach beyond the manifest:

| Component | Bound state |
| --- | --- |
| Ownership | The owner, its ancestor and descendant boundaries, and the nested-repository boundaries that delimit its coverage. |
| Selection | The effective policy hash, the selected owned path set, and the selection version. |
| Mapping | The validity state and the sorted associations from section identifier to sources. |
| Guidance | The effective ordered guidance digest. |
| Graph | The import edges with current export hashes, the direct consumer edges, and the transitive provider closure. |

Each provider descriptor carries its input digest, guidance digest, review
revision, active invalidations, and resolved import edges. The closure is
deliberately conservative. A provider edit can change a consumer token even
when the imported export body stays equal.

Every collection sorts before encoding, so traversal order never reaches a
token. The frozen byte layouts live with their tests in
[canonical.rs](src/canonical.rs).

## Sections map prose to sources

`SectionMap` is one README's advisory mapping state: `Absent`, `Valid`, or
`Invalid`. `Invalid` is total. Partial advice cannot narrow a review, because
a reader cannot tell which mapping the author meant.

`SectionMap::identity` gives the comparable association set: the sorted
identifiers, each with its sorted source set. Body edits, heading text, and
moved line ranges do not change it. Any changed association requires a full
baseline.

A section creates no ownership and no separate freshness. The domain validates
the identifier grammar and the literal-path grammar. The application resolves
each path against the project and rejects anything this README does not own.

Source evidence: [section.rs](src/section.rs).

## Pending and waiting describe different states

An invalidation is an explicit request for a review with a recorded reason.
A document is pending after a first review becomes necessary, an input changes, or an invalidation applies.
An edit to the document itself also makes it pending.

A current document has no remaining review cause.
A document waits while a provider is pending or waits for another provider.
Thus, a current document can still wait.
A ready document is pending and has no provider that prevents its review.

## An acknowledgement advances one review

A review artifact identifies the snapshot of one README and its inputs.
An acknowledgement records the reviewer, result, and reason that the explanation is correct.
A revision is the counter that increases after each acknowledgement for that README.

`ReviewState::acknowledge` compares the manifests, document revision, and covered invalidations before it changes the state value.
It increments the document revision and records the reviewer, result, note, and guidance digest.
It clears only the covered invalidations for that document.
The application owns the lock, readiness validation, and durable save.

The record stores the guidance digest as a value.
The domain does not decide whether that guidance is good or compatible.
The application rebuilds the complete context and recomputes the token before
it calls the aggregate. A small artifact narrows the reading list. It never
narrows the validated state.

Source evidence: [schedule.rs](src/schedule.rs) and [review.rs](src/review.rs).

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
| `section` | Advisory mapping identities, path grammar, and mapping identity |
| `guidance` | The guidance digest value type and its entry kinds |
| `review` | Review records and invalidation transitions |

The [crate manifest](Cargo.toml) declares no runtime dependencies.
The [crate exports](src/lib.rs) identify the implemented modules.

## Continue

Read [ownership.rs](src/ownership.rs) to trace one file to its owner.
