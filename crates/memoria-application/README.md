# memoria-application

The application crate coordinates Memoria commands and returns their results.

A use case is the application function for a command.
A port is a contract for an external operation, such as file access.
An adapter implements that contract.
The `memoria` executable supplies the adapters through `Services`.
This crate depends on `memoria-domain` and the Rust standard library.

Read [plan.rs](src/usecases/plan.rs#L43) to start with a command that makes no writes.

## On this page

- [Role in the project](#role-in-the-project)
- [Review planning](#inputs-become-a-review-plan)
- [Packets and acknowledgements](#a-packet-supplies-the-review-evidence)
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

Source evidence: [snapshot.rs:100](src/snapshot.rs#L100), [snapshot.rs:772](src/snapshot.rs#L772), and [plan.rs:43](src/usecases/plan.rs#L43).

## A packet supplies the review evidence

A document boundary groups the selected files that one README explains.
A review packet contains that README, its files, imported text, and the project documentation guidance for one review.
A human or agent judges the prose against those inputs.
After a document edit, a new packet represents the changed bytes.

An outdated import requires `render` before the consumer can receive a packet.
Packet creation makes no change to the saved review state.

Application command envelopes use schema version 2.
The native hook runner uses its separate native JSON contract.
A packet from an older envelope is invalid, and the reader names the received version.

## Acknowledgement saves the review result

An acknowledgement records who reviewed one README and why its explanation is correct.
A revision is the counter that increases after each successful acknowledgement for that README.
The packet token identifies the document, revision, inputs, guidance digest, and explicit review requests that the packet covers.
An invalidation is an explicit review request with a recorded reason.

The manifest lists input paths, sizes, and hashes for comparison.
The acknowledgement path has five stages:

1. Argument validation accepts the reviewer, result, note, and token.
2. Packet validation examines the schema, limits, digest, content hashes, and token before the write lock.
3. A snapshot under the write lock supplies the current manifest and provider state.
4. The domain transition compares the revision and covered invalidations.
5. A final snapshot comparison precedes the state save through `StateStore`.

Changed inputs cause `snapshot_changed` with exit 3.
Changed documentation guidance causes `guidance_changed` with exit 3.
A pending provider causes `dependencies_pending` with exit 3.
These conflicts leave the stored review unchanged.
New invalidations remain pending unless the packet includes them.

The current-guidance comparison runs at stage 3 and again at stage 5.
Guidance is review context, so a change invalidates the packet without making a current document stale.

Source evidence: [prepare_review.rs:105](src/usecases/prepare_review.rs#L105) and [ack.rs:193](src/usecases/ack.rs#L193).

## Saved views and historical evidence

`packet_view` validates a complete saved packet through the acknowledgement integrity checks before selection.
It returns a separate reading contract and makes no state change.
Experimental P1 preparation lists required examination and reuse candidates.
It never asserts completed review.
The shipped skill retains full review by default until an independent model-quality evaluation passes.

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
Packets retain their existing evidence contract through that helper.
`state_diff` compares explicit snapshots through `StateInspector` and matches logical records by identity.
Neither command writes source contents or hunks into lock state.

`guidance` reports a configuration error for a README that exists.
It returns the errors with the partial report instead of hiding them.
`agent hooks install` validates the root configuration before its first write.
If that configuration is invalid, the command fails and changes nothing.

Source evidence: [render.rs:114](src/usecases/render.rs#L114), [invalidate.rs:50](src/usecases/invalidate.rs#L50), and [check.rs:37](src/usecases/check.rs#L37).

## File map

| File | Ownership |
| --- | --- |
| [ports.rs](src/ports.rs#L1) | `Services` and the adapter contracts |
| [snapshot.rs](src/snapshot.rs#L1) | Repository facts and derived review state |
| [packet.rs](src/packet.rs#L1) | Packet values, limits, and token construction |
| [guidance.rs](src/guidance.rs#L1) | Effective guidance and its digest |
| [usecases](src/usecases/mod.rs#L1) | Command coordination and outcomes |
| [error.rs](src/error.rs#L185) | Diagnostics and application exit classes |

The ports separate two questions that the release keeps apart.
`GitRepository` decides which files are eligible, under the host settings.
`RepositoryIgnoreMatcher` decides which repository rules are active, without host settings.
This separation is what makes freshness portable between machines.

The binary owns argument parsing and output delivery.
Infrastructure owns Git processes, file access, and format parsers.
The domain owns the rules that these use cases apply.

## Continue

Read the [packet procedure](../../docs/workflow.md#review-one-document).
