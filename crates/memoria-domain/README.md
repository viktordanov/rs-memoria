# memoria-domain

The domain crate decides which documents require review and which review comes first.

The domain receives values and returns values.
At runtime it uses the Rust standard library and the [`glob`](https://crates.io/crates/glob) crate, which matches patterns without I/O.
The application supplies repository facts.

Read [scope.rs](src/scope.rs) to start with the rule that decides which files each document covers.

## On this page

- [Role in the project](#role-in-the-project)
- [Scopes and handoffs](#every-document-covers-its-folder-until-it-hands-it-off)
- [Files and shared text](#files-and-shared-text-have-different-relationships)
- [Manifests and the review context](#manifests-make-changes-visible)
- [Section mappings](#sections-map-prose-to-sources)
- [Review states, acknowledgement, and next step](#pending-and-waiting-describe-different-states)

## Role in the project

<!-- memoria:export id="summary" -->
The domain crate decides which selected files each document covers.
It compares recorded review inputs with current inputs.
Its rules determine which documents require review and their order.
<!-- /memoria:export -->

## Every document covers its folder until it hands it off

A tracked document is a `README.md` or an opted-in Markdown file.
`DocumentId` accepts both kinds of path, and `DocumentKind` names the kind from the file name.
The application decides which Markdown files are tracked.

`ScopeMap::build` applies the backbone rule in one pass:

1. Base(D) is every selected source in the document's folder and below it.
2. A handoff is a link or an import from D to a tracked document strictly below D's folder.
3. Scope(D) is Base(D) minus the subtree of every handoff target.

```text
README.md        links auth/README.md    covers app.rs
app.rs
auth/README.md                           covers auth/login.rs
auth/login.rs
```

Without the link, both documents cover `auth/login.rs`.
A nested document alone removes nothing.
Handoff edges point strictly downward, so they cannot form a cycle.
No scope depends on another scope, so the map needs no fixed point.

The map answers `scope_of`, `covering`, `handoffs_of`, `handed_off_by`, `covers_dir`, `uncovered`, `overlapping`, and `absent_handoffs`.
Its property tests generate trees and check four facts: full coverage when a root document exists, the same result for any input order, handoffs only strictly downward, and each scope inside its document's folder.

Source evidence: [scope.rs](src/scope.rs) and [properties.rs](tests/properties.rs).

## Files and shared text have different relationships

An export is a marked section that another document can copy.
The document that supplies the section is the provider.
The document that copies it is the consumer.

| Relationship | Meaning | Effect on review |
| --- | --- | --- |
| Scope | A document covers a selected file in its folder that it has not handed off. | A file change affects every document that covers it. |
| Handoff | A link or import moves a subfolder to a tracked document there. | The subfolder leaves the parent's scope. The handoff adds no waiting; an import keeps its own waiting edge. |
| Import | A consumer names an export in a provider document. | An export change affects its consumers, which wait for the provider. |
| Navigation | Any other normal Markdown link connects readers to another document. | The link creates no review dependency. |

A pending document requires a review.
This README covers `crates/memoria-domain/src/review.rs`, because the root README imports the domain summary and so hands `crates/memoria-domain/` to this README.
A change to `review.rs` makes this README pending.
If the summary stays unchanged, that change does not make the root README pending.
The root still waits for this provider review before its own review can proceed.

Source evidence: [scope.rs](src/scope.rs), [graph.rs](src/graph.rs), and [schedule.rs](src/schedule.rs).

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
`nearest-readme-v1` is a frozen hash-domain identifier: its name is historical, and renaming it would change every policy hash.

Source evidence: [manifest.rs](src/manifest.rs), [policy.rs](src/policy.rs), and [canonical.rs](src/canonical.rs).

## The review context binds more than the inputs

A review token identifies one snapshot. `encode_review_token_v3` accepts three
digests, and it encodes them with four more values in one frozen order:

| Encoded value | Contents |
| --- | --- |
| Domain and document | The separation string `memoria-review-token-v3`, then the document identity. |
| Review revision | The current review revision of that document. |
| `I`, `B`, `C` | The digests of the complete input manifest, the complete prior review record, and the review context. |
| Invalidations | The covered invalidations, sorted by id, with their exact reasons. |

The application hashes the encoded bytes. The token is `mrv3.` and the sixteen
lowercase hexadecimal digits of that hash. The domain crate computes no hash of
its own.

`B` uses the layout `memoria-review-baseline-v2`. It binds every stored field
of the prior record, and it ends with the coverage evidence: tag `0` for an
unrecorded record, or tag `1` and the sorted folders. The v1 encoding of an
absent baseline stays available only for the legacy exclusion proof, which
rebuilds tokens that earlier releases recorded.

`ReviewContext` (layout `memoria-review-context-v2`) holds the descriptors that reach beyond the manifest:

| Component | Bound state |
| --- | --- |
| Scope | The document, its kind, its sorted handoffs `(subtree, target)`, and the nested repositories inside its covered folders. |
| Selection | The effective policy hash, the scope path set, and the selection version 2. |
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

`SectionMap` is one document's advisory mapping state: `Absent`, `Valid`, or
`Invalid`. `Invalid` is total. Partial advice cannot narrow a review, because
a reader cannot tell which mapping the author meant.

`SectionMap::identity` gives the comparable association set: the sorted
identifiers, each with its sorted source set. A pattern contributes the
sources it expands to, never its text, so a literal list and a pattern that
match the same files are one association. Body edits, heading text, moved line
ranges, and a section guide do not change it. Any changed association requires
a full baseline.

A section can name one section guide. `SectionMapping.guidance` holds its
resolved path. A guide-only section has no sources, so a change never suggests
it, and it contributes its identifier with an empty source set to the
identity. `validate_guidance_path` checks the guide token grammar. The guide
text becomes a `GuidanceKind::Section` entry of the document's guidance, and
its `sections` list is presentation only.

A section adds or removes no input and has no separate freshness. The domain
validates the identifier grammar and each `files` token: `SectionRule::parse`
classifies a token as a literal path, a pattern, or a `!` exclusion.
`SectionFiles::expand` is the one expansion rule. It takes the union of the
includes over a given scope, subtracts the union of the exclusions, and reports
the tokens that match nothing. The application checks each literal path against
the project, then calls `expand` with the current scope, or with the recorded
scope when it reconstructs a previous mapping.

`glob::Glob` is the one matcher for selection rules and section patterns. It
wraps the `glob` crate and keeps the released grammar: `?` is one Unicode
character, braces are ordinary characters, a `.` component is skipped, a run of
`*` inside a component is one `*`, and `[^...]` negates like `[!...]`. Each
pattern compiles once, when its rule or marker is parsed.

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

A review artifact identifies the snapshot of one document and its inputs.
An acknowledgement records the reviewer, result, and reason that the explanation is correct.
A revision is the counter that increases after each acknowledgement for that document.

`ReviewState::acknowledge` compares the manifests, document revision, and covered invalidations before it changes the state value.
It increments the document revision and records the reviewer, result, note, and guidance digest.
It also records the coverage evidence: the sorted, unique folders that the document's scope handed off, as `CoverageEvidence::Recorded`.
The application supplies them from the snapshot whose token it revalidated last.
A record without evidence is `CoverageEvidence::Unrecorded`; the domain never invents one.
`ReviewState::validate` requires each recorded folder to lie strictly inside the document's folder.
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
| `selection`, `scope`, `policy` | Selected inputs, document scopes, and handoffs |
| `graph`, `schedule` | Dependencies, navigation, and review order |
| `manifest`, `canonical` | Input comparisons and stable byte encodings |
| `section` | Advisory mapping identities, `files` token grammar and expansion, guide path grammar, guide bounds, and mapping identity |
| `guidance` | The guidance digest value type, its entry kinds (`inline`, `file`, `section`), and the sections that name a guide |
| `review` | Review records and invalidation transitions |

The [crate manifest](Cargo.toml) declares one runtime dependency, `glob`. It has no dependencies of its own and performs no I/O, so the crate stays a pure model.
The [crate exports](src/lib.rs) identify the implemented modules.

## Continue

Read [scope.rs](src/scope.rs) to trace one file to the documents that cover it.
