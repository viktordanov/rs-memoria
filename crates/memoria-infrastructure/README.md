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
- [Formats and artifact limits](#formats-and-artifact-limits)
- [Skill, hook, and workflow installation](#skill-installation-has-its-own-transaction)
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
The adapters do not determine which document needs review.

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

An acknowledgement records who reviewed one document and why its explanation is correct.
Saved review state contains the latest acknowledgement and input identities for each document.
The lock codec writes format 3 and reads formats 2 and 3. Format 3 adds one coverage-evidence field to each review row; a format 2 record decodes as unrecorded. Document identities accept any tracked Markdown path, and each record's files still lie inside its document's folder.

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

Each lock attempt is nonblocking.
`LockFileCoordinator::with_wait` gives the project write lock a wait budget, and a busy lock is retried every 25 to 200 milliseconds until that budget ends.
So reviewers who acknowledge at the same moment take turns instead of failing.
The executable sets the budget from `MEMORIA_LOCK_WAIT_MS`, 10 seconds by default.
Without a budget, the coordinator reports a busy lock at once.

Source evidence: [state.rs:326](src/state.rs#L326), [fs.rs:202](src/fs.rs#L202), [fs.rs:337](src/fs.rs#L337), and [fs.rs:538](src/fs.rs#L538).

## Formats and artifact limits

A review manifest states what one document's review must read. A full export
adds the exact input bytes. A codec converts between application values and a
transport format, such as JSON. An export is a marked document section that
another document can copy.

| Adapter | Contract |
| --- | --- |
| `PulldownMarkdownCodec` | It identifies marker ranges and link locations, validates export text, parses advisory sections, and recognizes the markers that opt a Markdown file in. |
| `TomlConfigurationReader` | It accepts the supported TOML configuration. It refuses `section_guidance_files` in a sidecar with `configuration_invalid`, because only the root may register section guides. |
| `JsonPacketCodec` | It encodes and decodes both review artifacts under hard limits. |
| `LockStateStore` | It reads and writes the binary `memoria.lock` file. |
| `LockStateInspector` | It decodes a state file for read-only inspection. |
| `GixRepositoryIgnore` | It matches repository ignore rules without host settings. |
| `FsHookStore` | It installs and removes one owned native `Stop` hook. |
| `FsWorkflowStore` | It renders, records, and removes one managed GitHub workflow. |
| `FsArtifactStore` | It resolves a `--save` destination and writes one artifact exclusively, with mode 0600. |
| `Xxh3Hasher` | It calculates XXH3-64 hashes with the default secret and seed zero. |
| `EnvAgentLocations` | It resolves absolute skill destinations for a target and scope. |
| `SelfStatusProcess` | It runs one bounded status inspection without a shell. |
| `CommandClientProbe` | It measures one agent client version under a two-second bound. |
| `Supervisor` | It owns every subprocess one bounded endpoint starts. |

The codec writes envelope schema version 3 and accepts only that version.
It writes manifest version 3 and packet version 5, and it accepts only those.
It rejects a save receipt passed as an artifact with `packet_schema_invalid`.
A different version fails with the received value in the message and with
instructions to produce a new artifact. The codec converts nothing.

The version header is inspected before the integrity domain is chosen. An
artifact from an earlier release was hashed under that release's domain, so
its digest can never match the current one. Checking the digest first would
call an intact old artifact corrupt. The order therefore reports the version,
and a supported version with tampered content still fails its integrity
check.

The two artifacts use separate integrity domains. A manifest digest uses
`memoria.review-manifest.v1` under the key `artifact_digest`. A full export
digest uses `memoria.packet.v3` under the key `packet_digest`. The same
content therefore never produces the same digest for both kinds.

The codec measures every part of the JSON envelope, including diagnostics.
It rejects an invalid digest or an inconsistent record count before
acknowledgement can continue. The limits apply to both human and JSON output.
The binary encodes the artifact before it selects the output format.

Source evidence: [packet.rs](src/packet.rs) and [main.rs](../../src/main.rs).

### The Markdown parser separates two channels

Structural problems and advisory problems reach different places. A malformed
export or import marker is a structural error that invalidates the document.
A malformed section marker is an advisory problem: it withdraws the document's
focused-review advice and leaves the document valid. Either kind of marker,
even a malformed one, opts a Markdown file in: a mistake never drops the
obligation to review it.

The parser reports a section problem under `section_issues`, never under
`issues`. The exception is a malformed `guidance` attribute: it is the
structural error `section_guidance_invalid`, because dropping it would remove a
guide from review context in silence. A section guide reference whose token
parsed is kept in `guides`, even when the section's `id` or `files` are
invalid, so the guide still applies. The parser looks for the `guidance` attribute name only outside quoted
values. A source path such as `files="guidance=rules.md"` stays a literal
path, and a `guidance` name with any space or tab around its `=` is
structural. A section-like line that is not a well-formed declaration still
reaches that channel, including a wrongly indented one. It never disappears in
silence, because a mistyped mapping must not quietly narrow a review.

The parser tracks section nesting separately from export and import nesting.
An export can sit wholly inside a section. A section cannot sit inside or
cross an export or an import. Marker text inside fenced or indented code stays
inert: the inertness check reads the first non-blank byte of the line, because
an indented code block's range begins after its indent.

Source evidence: [markdown.rs](src/markdown.rs).

## Skill installation has its own transaction

An agent skill is a set of instruction files for an agent.
The installer stores the four Memoria skill files and their management record in one flat package directory.
The record hashes every file. A package is current only when its version, its names, and every file's bytes match.
An upgrade names the files that the previous record listed, so a smaller older package upgrades cleanly.

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

## The workflow adapter owns one template

`FsWorkflowStore` renders one deterministic workflow template and records what it wrote.
The record holds the template version, the exact workflow path, the versions, the Action reference, the runner label, and the expected bytes.
The adapter reads no YAML: it rebuilds the expected bytes from the recorded parameters and compares them exactly.
A file without a valid record is unmanaged, even when its bytes match the current template.

Each mutation takes a private advisory lock under the metadata directory, keyed by the destination file name.
It then writes a durable intent that names the expected and the intended bytes of both files.
An interrupted mutation resolves under the lock before any ordinary decision, and any unrecognized third state changes nothing.

The adapter moves each file it replaces into private recovery storage before the replacement appears.
The move is an atomic rename on the same filesystem, so an editor that holds the old file keeps writing into the preserved copy.
If the displaced bytes differ from the expected bytes, the adapter puts them back and reports a conflict.
Creation refuses to clobber: a destination that appears between the plan and the write fails the operation.

The restoration is no-clobber for the same reason.
A test for absence followed by a rename leaves a window in which another writer creates the destination and the rename destroys it.
The adapter therefore restores with a hard link, which fails when the destination exists, so the decision and the move are one operation.
When a file appeared there, both byte sequences survive and the conflict names both paths.

An interrupted transaction settles before any ordinary decision, including on an apply that would write nothing.
The adapter recognizes only the expected bytes and the intended bytes.
Transaction data that does not decode, or that names another workflow or an unknown operation, is evidence rather than a transaction.
The adapter preserves those bytes and refuses the change; it never invents an empty transaction from unreadable data.

An advisory lock serializes Memoria writers only. It does not stop an arbitrary editor.
The adapter therefore compares the bytes again immediately before every write and removal.

A step hook exposes each of those boundaries to tests, in the same way that `fs.rs` exposes a fault hook.
Production code passes `NO_STEPS`.

Source evidence: [github_workflow.rs](src/github_workflow.rs#L1).

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

`lock_codec` encodes `memoria.lock` at format version 3 and decodes format versions 2 and 3.
In format 3, each review row stores its coverage evidence as `0` (unrecorded) or as a vector index plus one; the vector holds strictly increasing path identifiers of handed-off folders.
A format 2 row has no such field, so its record decodes as unrecorded; the next write re-encodes it as format 3.
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
It also rejects coverage evidence that names a path outside its table or a folder that is not strictly inside the record's document folder.
It bounds the file size, the payload size, the expanded value count, and the expanded string bytes.
Every path or string that leaves a table is charged where it is materialized, so repeated references cannot expand past the limit.

Compression uses the bundled Zstandard library with explicit parameters.
The adapter does not use an installed `zstd` executable and does not select a host library.

Source evidence: [lock_codec.rs](src/lock_codec.rs#L1).

## Continue

Read [state.rs](src/state.rs#L1) to trace a state save to the atomic writer.
