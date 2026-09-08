# memoria-infrastructure

The infrastructure crate reads and writes external data for Memoria commands.

A port is an application contract for an external operation.
An adapter implements that contract with Git, file access, or a data format library.
This crate depends on the application and domain crates.
It keeps library types inside the adapters.

Read [fs.rs](src/fs.rs#L110) to start with file classification and reads.

## On this page

- [Role in the project](#role-in-the-project)
- [Repository adapters](#repository-facts-enter-through-adapters)
- [State writes](#state-writes-preserve-a-clear-error-boundary)
- [Formats, packets, and skill installation](#formats-and-packet-limits)
- [Next step](#continue)

## Role in the project

<!-- memoria:export id="summary" -->
The infrastructure crate implements Git, file, parser, and storage operations.
It supplies these operations through application ports.
Its writers compare expected content before replacement.
<!-- /memoria:export -->

## Repository facts enter through adapters

`GitCli` lists tracked files and untracked files that Git does not ignore.
It also supplies ignore rules and Git context.
Its commands disable filesystem monitors, external diff programs, pagers, and lazy object retrieval.
A missing historical blob produces unavailable diff context without a remote fetch.

`FsProjectFiles` classifies paths before it reads their contents.
It rejects symlinks within project paths.
The application uses these facts to enforce selection and repository boundaries.
The adapters do not determine which README needs review.

Source evidence: [git.rs:16](src/git.rs#L16), [git.rs:260](src/git.rs#L260), and [fs.rs:110](src/fs.rs#L110).

## State writes preserve a clear error boundary

An acknowledgement records who reviewed one README and why its explanation is correct.
Saved review state contains the latest acknowledgement and input identities for each README.

`JsonStateStore::save` validates the state value and compares the stored bytes with the expected bytes.
A mismatch returns `StateFailure::Conflict`.
The application acquires the project write lock before this save.

The atomic writer creates a temporary file beside the target.
It writes and synchronizes that file before the rename.
Then it synchronizes the parent directory.
Before the rename, an error leaves the previous target unchanged.
After the rename, a directory synchronization error leaves the new content in place and reports uncertain durability.

The advisory lock coordinates Memoria processes.
It does not prevent an editor or Git from changing files.
The operating system releases the lock after the process exits.
The lock file remains on disk.

Source evidence: [state.rs:326](src/state.rs#L326), [fs.rs:202](src/fs.rs#L202), and [fs.rs:337](src/fs.rs#L337).

## Formats and packet limits

A review packet contains one README and the exact input bytes for its review.
A codec converts between application values and a transport format, such as JSON.
An export is a marked README section that another README can copy.

| Adapter | Contract |
| --- | --- |
| `PulldownMarkdownCodec` | It identifies marker ranges and validates export text. |
| `TomlConfigurationReader` | It accepts the supported TOML configuration. |
| `JsonPacketCodec` | It encodes and decodes packets under hard limits. |
| `JsonStateStore` | It converts versioned JSON to review state. |
| `Xxh3Hasher` | It calculates XXH3-64 hashes with the default secret and seed zero. |

The packet codec measures every part of the JSON envelope.
This measurement includes diagnostics.
It rejects an invalid digest or inconsistent record count before acknowledgement can continue.
Packet limits apply to both human and JSON output.
The binary obtains the encoded packet before it selects the output format.

Source evidence: [packet.rs:268](src/packet.rs#L268) and [main.rs:145](../../src/main.rs#L145).

## Skill installation has its own transaction

An agent skill is a file of instructions for an agent.
The installer stores the Memoria skill and its management record in one package directory.

`FsSkillStore` uses a lock in the package parent directory.
It records installation progress and preserves a backup during package replacement.
It refuses changes to locally edited managed files.
Recovery examines the transaction record and managed content before it changes a package.
The command reference describes the [installation limits](../../docs/cli.md#agent-packages).

Source evidence: [skill.rs:618](src/skill.rs#L618).

## Continue

Read [state.rs:326](src/state.rs#L326) to trace a state save to the atomic writer.
