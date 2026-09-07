# memoria-application

The application crate coordinates each command through domain rules and adapter contracts.

A use case reads repository facts through ports, applies domain rules, and returns an outcome.
The binary supplies the adapters through `Services`.
This crate depends on `memoria-domain` and the Rust standard library.

## Role in the project

<!-- memoria:export id="summary" -->
The application crate applies domain rules to a snapshot of repository inputs.
Its ports define the contracts for adapters.
Its use cases prepare packets, update imports, and record acknowledgements.
Each outcome carries data and diagnostics for the binary.
<!-- /memoria:export -->

## Inputs become a review plan

`snapshot::build` collects repository facts twice through the ports.
If those collections differ, it collects the facts once more.
If the last two collections differ, it returns `snapshot_changed`.
The analysis uses the stable collection to determine ownership, imports, manifests, and review order.
The snapshot also carries Git context and diagnostics.

The plan names one next action in `data.next_action`.
An outdated import requires `render` before the consumer can receive a packet.
If a provider is pending, it prevents a consumer review.
That restriction also applies to consumers whose inputs still match their previous review.

Source evidence: [snapshot.rs:100](src/snapshot.rs#L100), [snapshot.rs:772](src/snapshot.rs#L772), and [plan.rs:43](src/usecases/plan.rs#L43).

## Acknowledgement protects the reviewed inputs

The packet contains the README, owned inputs, imported exports, writing instructions, and covered invalidations.
A human or agent judges the prose against those inputs.
After a document edit, a new packet represents the changed bytes.

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

`render` changes only declared import bodies and leaves review state unchanged.
Its writer compares the expected bytes before each README replacement.
Each replacement is atomic, but a set of README replacements is not one transaction.
After a write error, `render_incomplete` identifies the remaining documents.

`invalidate` records a reason for the documents in its captured scope.
It does not edit those documents.
The `lint` and `check` commands inspect the project without writes.
`check` also fails for pending reviews and outdated imports.

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
