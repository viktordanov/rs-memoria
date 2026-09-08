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
A review packet contains that README, its files, imported text, and writing instructions for one review.
A human or agent judges the prose against those inputs.
After a document edit, a new packet represents the changed bytes.

An outdated import requires `render` before the consumer can receive a packet.
Packet creation makes no change to the saved review state.

## Acknowledgement saves the review result

An acknowledgement records who reviewed one README and why its explanation is correct.
A revision is the counter that increases after each successful acknowledgement for that README.
The packet token identifies the document, revision, inputs, and explicit review requests that the packet covers.
An invalidation is an explicit review request with a recorded reason.

The manifest lists input paths, sizes, and hashes for comparison.
The acknowledgement path has five stages:

1. Argument validation accepts the reviewer, result, note, and token.
2. Packet validation examines the schema, limits, digest, content hashes, and token before the write lock.
3. A snapshot under the write lock supplies the current manifest and provider state.
4. The domain transition compares the revision and covered invalidations.
5. A final snapshot comparison precedes the state save through `StateStore`.

Changed inputs cause `snapshot_changed` with exit 3.
A pending provider causes `dependencies_pending` with exit 3.
These conflicts leave the stored review unchanged.
New invalidations remain pending unless the packet includes them.

Source evidence: [prepare_review.rs:105](src/usecases/prepare_review.rs#L105) and [ack.rs:193](src/usecases/ack.rs#L193).

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

Source evidence: [render.rs:114](src/usecases/render.rs#L114), [invalidate.rs:50](src/usecases/invalidate.rs#L50), and [check.rs:37](src/usecases/check.rs#L37).

## File map

| File | Ownership |
| --- | --- |
| [ports.rs](src/ports.rs#L334) | `Services` and the adapter contracts |
| [snapshot.rs](src/snapshot.rs#L82) | Repository facts and derived review state |
| [packet.rs](src/packet.rs#L150) | Packet values, limits, and token construction |
| [usecases](src/usecases/mod.rs#L3) | Command coordination and outcomes |
| [error.rs](src/error.rs#L185) | Diagnostics and application exit classes |

The binary owns argument parsing and output delivery.
Infrastructure owns Git processes, file access, and format parsers.
The domain owns the rules that these use cases apply.

## Continue

Read the [packet procedure](../../docs/workflow.md#review-one-document).
