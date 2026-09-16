# memoria-application

The application crate coordinates Memoria commands and returns their results.

A use case is the application function for a command.
A port is a contract for an external operation, such as file access.
An adapter implements that contract.
The `memoria` executable supplies the adapters through `Services`.
This crate depends on `memoria-domain` and the Rust standard library.

Read [plan.rs](src/usecases/plan.rs) to start with a command that makes no writes.

## On this page

- [Role in the project](#role-in-the-project)
- [Review planning](#inputs-become-a-review-plan)
- [Review artifacts and acknowledgements](#a-review-states-its-requirements)
- [Mutation boundaries](#mutation-boundaries-differ)
- [File map and next step](#file-map)

## Role in the project

<!-- memoria:export id="summary" -->
The application crate coordinates Memoria commands through domain rules.
Its ports describe external operations, and adapters supply those operations.
Each command returns structured results for the executable.
<!-- /memoria:export -->

## Inputs become a review plan

A snapshot is a collected view of the repository inputs and saved review state.
`snapshot::build` collects these facts twice through the ports.
If those collections differ, it collects the facts once more.
If the last two collections differ, it returns `snapshot_changed`.
The analysis uses the stable collection to determine file ownership and review order.
The snapshot also carries Git context and diagnostics.

An export is a marked section that one README supplies to another README.
An import is the declared copy of that section.
The supplier is the provider, and the recipient is the consumer.
The `render` command updates those copies from their providers.

The plan names one next action in `data.next_action`.
A pending README requires a review.
If a provider is pending, the consumer waits for that provider.
That restriction also applies to consumers whose inputs still match their previous review.

Source evidence: [snapshot.rs](src/snapshot.rs) and [plan.rs](src/usecases/plan.rs).

## A review states its requirements

A document boundary groups the selected files that one README explains.
A review states what the reviewer must read for that boundary, and why the
reviewer cannot read less. A human or agent judges the prose against those
inputs.

`requirements.rs` decides the review mode. Focused reading is a candidate only
when the whole picture holds: a prior declaration exists, the bytes that
declaration named can still be verified, the mapping associations are valid
and unchanged, and nothing outside the mapped sources moved. Any doubt
produces a full baseline with an explicit reason.

`prepare_review.rs` returns one of two representations. The default manifest
carries requirements and no bodies. `--full` adds every reviewed byte and the
canonical binding descriptors. Both carry the same token, because both
describe one stable snapshot.

`review_context.rs` assembles the context that the token binds: the ownership
boundaries, the effective selection, the mapping associations, the guidance
digest, and the transitive provider closure.

After a document edit, a new artifact represents the changed bytes.
An outdated import requires `render` before the consumer can receive a review.
A review makes no change to the saved review state.

Application command envelopes use schema version 3.
The native hook runner uses its separate native JSON contract.
An artifact from an older envelope is invalid. The reader names the received
version and tells the user to produce a new artifact. Nothing is converted.

## Acknowledgement saves the review result

An acknowledgement records who reviewed one README and why its explanation is correct.
A revision is the counter that increases after each successful acknowledgement for that README.
The token identifies the document, the revision, the complete inputs, the
prior review, the complete review context, and the explicit review requests
that the artifact covers.
An invalidation is an explicit review request with a recorded reason.

The acknowledgement path has five stages:

1. Argument validation accepts the reviewer, result, note, and token.
2. Artifact validation examines the schema, limits, and digest before the write lock. A full export also validates its content hashes and its self-contained token.
3. Under the write lock, a snapshot rebuilds the complete input manifest and the complete review context, then recomputes the token.
4. The domain transition compares the revision and covered invalidations.
5. A final rebuild and token recomputation precede the state save through `StateStore`.

Stage 3 never validates only the suggested reads, the suggested sections, or
the changed files. A small artifact narrows the reading list. It never narrows
the validated state.

Changed inputs cause `snapshot_changed` with exit 3. A full export supplies
exact per-input differences, because it carries the reviewed bytes. A manifest
names the changed digest categories instead.
Changed documentation guidance causes `guidance_changed` with exit 3.
A pending provider causes `dependencies_pending` with exit 3.
A later document revision causes `revision_conflict` with exit 3.
These conflicts leave the stored review unchanged.

The token recomputation uses the artifact's own covered set. A target
invalidation raised after the review stays pending instead of blocking this
acknowledgement. The domain refuses a covered reason that is no longer active
or whose text changed.

The current-guidance comparison runs at stage 3 and again at stage 5.
Guidance is review context, so a change invalidates the artifact without
making a current document stale.

Source evidence: [requirements.rs](src/usecases/requirements.rs), [prepare_review.rs](src/usecases/prepare_review.rs), and [ack.rs](src/usecases/ack.rs).

## Saved views and historical evidence

`packet_view` validates a saved full export through the acknowledgement
integrity checks before selection. It returns a separate reading contract and
makes no state change. For a manifest it reports `packet_content_unavailable`,
because a manifest carries no content.
Legacy P1 preparation lists required examination and reuse candidates.
It never asserts completed review, and it never reduces the scope that the
review mode requires.

`history` bounds local commit lookup and compares historical lengths and hashes with acknowledged content.
Acknowledgement stores a commit only when the README, all owned files, and imported export bodies match one candidate.
Partial coverage produces a diagnostic and a null reference.
Dirty acknowledgement remains valid.
Later evidence recovery never changes the previous review attribution.
The [CLI reference](../../docs/cli.md#historical-coverage) defines the budgets and unavailable-evidence reasons.

## Mutation boundaries differ

`render` changes only declared import bodies and leaves saved review state unchanged.
Its writer compares the expected bytes before each README replacement.
Each replacement is atomic, but a set of README replacements is not one transaction.
After a write error, `render_incomplete` identifies the remaining documents.

`invalidate` records a reason for the documents in its captured scope.
It does not edit those documents.
The `lint` command examines documentation structure without writes.
The `check` command also requires current reviews and current imported text.
Neither command records an acknowledgement.

The read-only commands include `status`, `guidance`, `state_inspect`, `state_diff`, `explain`, `plan`, `lint`, `check`, and `graph`.
An `init` preview is read-only too; only `init --apply` writes.

`explain` uses the stable snapshot without a readiness gate, so current and waiting documents also produce evidence.
The shared evidence helper compares Git bytes with the saved length and hash before a hunk.
Full exports retain their existing evidence contract through that helper.
`state_diff` compares explicit snapshots through `StateInspector` and matches logical records by identity.
Neither command writes source contents or hunks into lock state.

`guidance` reports a configuration error for a README that exists.
It returns the errors with the partial report instead of hiding them.
`agent hooks install` validates the root configuration before its first write.
If that configuration is invalid, the command fails and changes nothing.

`github_workflow` validates the path, the version, the Action reference, and the runner label before it reaches the store.
It requires `--apply` for every change, so the default result is a preview that writes nothing.
The use case never downloads a release asset and never contacts GitHub, so a preview states that it did not confirm publication.
`WorkflowStore` does the filesystem work, and the use case keeps no path or template detail.

Install and upgrade also read three initialization facts through `ProjectFiles`, `ConfigurationReader`, and `StateStore`.
The generated job runs `memoria check`, so a workflow without a root README, a valid configuration, and readable state could only fail.
A preview reports each missing fact and stays read-only.
An apply returns the failure before the store opens a lock, settles a transaction, or writes a file.
A pending review is permitted, because review follows the workflow change.
Status and uninstall read none of those facts, so they work in an uninitialized project.

Source evidence: [render.rs](src/usecases/render.rs), [invalidate.rs](src/usecases/invalidate.rs), and [check.rs](src/usecases/check.rs).

## File map

| File | Ownership |
| --- | --- |
| [ports.rs](src/ports.rs) | `Services` and the adapter contracts |
| [github_workflow.rs](src/usecases/github_workflow.rs) | Workflow arguments, preview policy, and reports |
| [snapshot.rs](src/snapshot.rs) | Repository facts, section resolution, and derived review state |
| [review.rs](src/review.rs) | The review manifest and its display shape |
| [review_context.rs](src/review_context.rs) | The context descriptors that a token binds |
| [packet.rs](src/packet.rs) | Artifact values, limits, and token construction |
| [guidance.rs](src/guidance.rs) | Effective guidance and its digest |
| [usecases](src/usecases/mod.rs) | Command coordination and outcomes |
| [error.rs](src/error.rs) | Diagnostics and application exit classes |

The ports separate two questions that the release keeps apart.
`GitRepository` decides which files are eligible, under the host settings.
`RepositoryIgnoreMatcher` decides which repository rules are active, without host settings.
This separation is what makes freshness portable between machines.

The binary owns argument parsing and output delivery.
Infrastructure owns Git processes, file access, and format parsers.
The domain owns the rules that these use cases apply.

## Continue

Read the [review procedure](../../docs/workflow.md#review-one-document).
