# memoria-infrastructure

The infrastructure crate reads and writes external data for Memoria commands.

A port is an application contract for an external operation.
An adapter implements that contract with Git, file access, or a data format library.
This crate depends on the application and domain crates.
It keeps library types inside the adapters.

Read [fs.rs](src/fs.rs#L110) to start with file classification and reads.

## On this page

- [Role in the project](#role-in-the-project)
- [Repository adapters](#repository-facts-enter-through-adapters), [local history](#bounded-local-history), and [state writes](#state-writes-preserve-a-clear-error-boundary)
- [Formats and packet limits](#formats-and-packet-limits)
- [Skill and hook installation](#skill-installation-has-its-own-transaction)
- [Bounded processes](#one-supervisor-owns-every-bounded-subprocess) and [the lock codec](#the-lock-codec-produces-one-canonical-artifact)

## Role in the project

<!-- memoria:export id="summary" -->
The infrastructure crate implements Git, file, parser, and storage operations.
It supplies these operations through application ports.
Its writers compare expected content before replacement.
<!-- /memoria:export -->

## Repository facts enter through adapters

`GitCli` lists tracked files and untracked files that Git does not ignore.
It also supplies Git context and worktree-private paths.
It resolves the metadata directory with `--absolute-git-dir` and composes private paths itself.
A linked worktree therefore gets its own metadata directory, and a symlinked one keeps its real location.
Its commands disable filesystem monitors, external diff programs, pagers, and lazy object retrieval.
A missing historical blob produces unavailable diff context without a remote fetch.

`GixRepositoryIgnore` answers a different question.
It decides which directories the repository's own `.gitignore` rules exclude.
It reads no host configuration, so a harmless host rule cannot change a project's policy.
It uses fixed case-sensitive matching, which makes the inventory portable.

`FsProjectFiles` classifies paths before it reads their contents.
It rejects symlinks within project paths.
A lock file must stay inside the metadata directory that its boundary names.
The opener refuses a symlink, a special file, and a substituted parent directory.
The application uses these facts to enforce selection and repository boundaries.
The adapters do not determine which README needs review.

Source evidence: [git.rs:16](src/git.rs#L16), [repository_ignore.rs](src/repository_ignore.rs#L31), and [fs.rs:110](src/fs.rs#L110).

## Bounded local history

`GitCli::historical_blob` and `recent_commits` use the subprocess supervisor with a shared deadline and bounded output.
The application supplies candidate and byte budgets.
These commands disable object replacement and lazy fetching.
They make no remote request and write no historical snapshots.
Missing objects produce unavailable evidence.
Exhausted limits remain explicit.
The application compares the returned bytes with the acknowledged fingerprints before use.

## State writes preserve a clear error boundary

An acknowledgement records who reviewed one README and why its explanation is correct.
Saved review state contains the latest acknowledgement and input identities for each README.

`LockStateStore::save` validates the state value and compares the stored bytes with the expected bytes.
A mismatch returns `StateFailure::Conflict`.
The application acquires the project write lock before this save.
That lock uses a worktree-private path under Git metadata, so the committed file is never the process lock.

The store checks which state files exist before it selects a decoder.
A lone version 1 file returns `StateFailure::Legacy`, and both files return `StateFailure::Ambiguous`.
This release performs no automatic migration.

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
| `LockStateStore` | It reads and writes the binary `memoria.lock` file. |
| `LockStateInspector` | It decodes a state file for read-only inspection. |
| `GixRepositoryIgnore` | It matches repository ignore rules without host settings. |
| `FsHookStore` | It installs and removes one owned native `Stop` hook. |
| `Xxh3Hasher` | It calculates XXH3-64 hashes with the default secret and seed zero. |
| `EnvAgentLocations` | It resolves absolute skill destinations for a target and scope. |
| `SelfStatusProcess` | It runs one bounded status inspection without a shell. |
| `CommandClientProbe` | It measures one agent client version under a two-second bound. |
| `Supervisor` | It owns every subprocess one bounded endpoint starts. |

The packet codec writes envelope schema version 2 and accepts only that version.
A different version fails with the received value in the message.
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
The command reference describes the [installation limits](../../docs/cli.md#agent-packages), and the [agent integrations guide](../../docs/agents.md) explains the scopes and the hook contract.

`FsSkillStore` also repeats the operation's own eligibility test under the retained lock.
If the destination changed after the plan, the operation fails and preserves the files it found.

## Hook installation is one guarded transaction

`FsHookStore` owns exactly one native `Stop` group and one ownership record.
It never adopts a user entry, and it removes only an unchanged owned group.
Ownership comes from the recorded group, not from the command that this executable would install.
A moved executable therefore keeps a working removal and a working reinstallation.

Each hook operation holds a private lock for its own target under the metadata directory.
The store validates the container shapes first and refuses an unsupported shape without a write.
One read supplies both the parsed document and the digest, so a replacement always describes bytes that were examined.
It replaces a configuration only when the current bytes still match that digest.

A durable intent records two identities for the configuration and two for the ownership record: what the operation requires, and what it intends to leave.
The next explicit install or uninstall settles that transaction under the lock, before any ordinary eligibility decision.
Recovery finishes the operation when it finds the intended bytes, and leaves the file alone when it finds the required bytes.
Any third state is an edit from outside, so it changes nothing and names the record.
The ownership record gets the same treatment, and absence is one of its identities.
Ordinary mutations compare the record the same way before they replace or remove it.
An outstanding transaction is never reported as a successful no-op, and status and a dry run stay read-only.
Each boundary is durable: a replacement synchronizes its file and its directory, and a removal synchronizes its directory.

The JSON reader keeps every unrelated value exactly.
It preserves number lexemes and key order, rejects duplicate keys, and bounds nesting depth.
It copies each run of ordinary characters once, so its work grows with the input and not with its square.
A Unicode escape needs four hexadecimal digits, so a signed or short escape is a configuration error.
The TOML path matches an owned inline group by its exact hash, so a similar user group survives.

`SelfStatusProcess` runs the bounded status inspection with an argument array, a deadline, and an output limit.
`CommandClientProbe` measures the selected client before installation and enforces the frozen version floors.

Source evidence: [skill.rs](src/skill.rs#L618), [hooks.rs](src/hooks.rs#L1), and [client_probe.rs](src/client_probe.rs#L1).

## One supervisor owns every bounded subprocess

The native endpoint answers within one deadline and leaves nothing running.

`Supervisor` starts each child in its own session, so the child's process group holds every descendant.
Repository discovery and the status inspection both run through it, under the deadline that remains.
On expiry, on an output overflow, or on cancellation, it terminates the group and collects the child within a cleanup budget.
A group stays owned until its pipes are drained and its descendants are gone, so a leader that exits early cannot leave one behind.
Each drain wait is bounded by the cleanup budget and by the deadline that remains.
When the endpoint gives up, it cancels: every live group ends, and a later start is refused.

Source evidence: [bounded_process.rs](src/bounded_process.rs#L1) and [git.rs](src/git.rs#L1).

## The lock codec produces one canonical artifact

`lock_codec` encodes and decodes `memoria.lock` at format version 2.
One logical state always produces one byte sequence, so two hosts commit equal bytes.

The writer measures the logical expansion of the state before it builds a table.
That walk charges the same occurrences the reader charges, in the same units, so an oversized state is refused before any allocation.
The encoder then normalizes records into canonical tables and selects raw or compressed bytes.
It selects compressed bytes only when they are shorter, and a tie selects raw bytes.
It also decodes its own output with the production decoder before it returns the bytes.
The writer therefore cannot produce a file that the reader rejects.

The decoder verifies the outer checksum before decompression.
It accepts exactly one Zstandard frame with the frozen profile, and no dictionary or trailing bytes.
It then rejects noncanonical encodings: overlong integers, unsorted tables, unused entries, and invalid references.
It bounds the file size, the payload size, the expanded value count, and the expanded string bytes.
Every path or string that leaves a table is charged where it is materialized, so repeated references cannot expand past the limit.

Compression uses the bundled Zstandard library with explicit parameters.
The adapter does not use an installed `zstd` executable and does not select a host library.

Source evidence: [lock_codec.rs](src/lock_codec.rs#L1).

## Continue

Read [state.rs](src/state.rs#L1) to trace a state save to the atomic writer.
