# Committed state

**Takeaway:** `memoria.lock` is generated, machine-owned review state. Commit it. Do not edit it. Read it with `memoria state inspect`.

Memoria writes this file. The configuration in `memoria.toml` and this artifact travel together, and Git stores their historical versions.

## Contents

1. [Mental model](#1-mental-model)
2. [What the file holds](#2-what-the-file-holds)
3. [Read the state](#3-read-the-state)
4. [Format](#4-format)
5. [Size](#5-size)
6. [Errors and recovery](#6-errors-and-recovery)
7. [Git settings](#7-git-settings)
8. [Durability and locks](#8-durability-and-locks)

## 1. Mental model

The lock file holds two things: the latest acknowledged snapshot for each document, and the invalidations that still need review.

It is binary. It is not TOML, and it is not a process synchronization lock. The name follows the convention of a lock file that a tool owns and a person commits.

One logical state always produces one byte sequence. Two machines with the same review history commit the same bytes.

## 2. What the file holds

Each review record keeps these fields:

- The document identity and its review revision.
- The input manifest: the document bytes, each file in the document's scope, and each imported export.
- The review metadata: time, reviewer, result, note, and Git context.
- The token digest and the guidance digest that the reviewer saw.
- The invalidation identifiers that the review acknowledged.
- The coverage evidence: the folders that the document's scope handed off when the review was acknowledged.

### Coverage evidence

`memoria ack` records the handed-off folders of the snapshot that it revalidated last, immediately before the write. These are the `subtree` values of the review context's `handoffs` list, which the token binds. The list is sorted and unique. Each folder is strictly inside the document's folder. An empty list is valid evidence: the scope handed nothing off.

A later review uses this evidence to classify a source that entered the scope. If a recorded folder contains the source, a handoff ended: the result is `handoff_changed` with the shallowest such folder. Otherwise the source is an ordinary addition: `path_set_changed`. No previous text, earlier record, invalidation reason, or provider context is needed.

A record that was written without evidence is *unrecorded*. Every record in a format 2 lock is unrecorded. For those records only, Memoria tries the bounded token reconstruction that the [CLI reference](cli.md#coverage-unrecorded) describes. If that proof is not available, the review reports `coverage_unrecorded` and names the reason. It never guesses.

### Format 3 and the lazy migration

This release writes lock format 3. Format 3 is format 2 plus one field in each review row: the coverage evidence (§4). Release 0.7.0 also keeps the wider document identities: a record, an import provider, or an invalidation target can name an opted-in Markdown document such as `docs/workflow.md`, not only a `README.md`. Each record's files still lie inside its document's folder.

The reader accepts formats 2 and 3. A format 2 lock stays readable and stays a valid baseline candidate, with every record unrecorded.

Reading never writes. The first ordinary state write in a project (`ack`, `invalidate`, or `init --apply` on an empty project) rewrites the whole lock as format 3. Every existing record keeps its revision, manifest, digests, reviewer, note, time, Git context, and acknowledged identifiers unchanged, and stays unrecorded. Each record gets evidence at its document's next acknowledgement. `memoria state inspect` shows which records have evidence.

Memoria performs no conversion command, no backfill, no bulk invalidation, no reset, and no fabricated acknowledgement. A record whose document stops being tracked stays in the file unchanged. No command edits or deletes it.

**Memoria 0.6 and format 3.** The upgrade is one way for 0.6 tooling. After the first 0.7 write, Memoria 0.6 `state inspect` and `state diff` report `state_unsupported_schema` (exit status 4) with the message `unsupported memoria.lock format version 3; this release supports version 2 only`. Keep the lock. Do not replace it or reset it for this reason. Inspect it with Memoria 0.7. Memoria 0.6 cannot certify a project that declares configuration version 3 in any case.

The record stores no section result. Sections are advice, so they carry no
state. The token digest field holds the new v3 digest without a codec
change.

Each active invalidation keeps its identifier, scope, reason, creation time, original targets, and the targets that are still pending.

The file holds no absolute machine path, no source content, no full guidance text, no credentials, and no historical event stream. Reviewer names, notes, reasons, and Git context are ordinary committed project metadata.

The input fingerprint is not stored. Memoria computes it from the reconstructed manifest, so it cannot disagree with the manifest that it describes.

### Commit evidence for the last review

The existing `git.base_commit` field associates a commit with the last acknowledged review of inputs.
It does not track the last prose edit.
New acknowledgements retain a commit only when all reviewed content matches that commit.
The comparison covers the document, its scope files, and imported export bodies through their lengths and XXH3 fingerprints.
Selection-policy and guidance fingerprints retain their independent meaning.
A matching content reference does not certify the Git tree's selection policy or the reviewer's judgment.

If no inspected commit supplies complete coverage, acknowledgement succeeds with a null reference.
Its `historical_coverage` hint describes partial or unavailable evidence.
Partial counts describe matches at one candidate, not a union of unrelated commits.
The bounded search can stop before it finds an available match.
A null reference therefore means that Memoria established no complete reference within that inspection.

Existing records keep their bytes and attribution until the next acknowledgement.
Earlier versions can contain unverified references.
Every later hunk still requires an exact length and hash match, including hunks from those records.
Local history can recover later-committed bytes without a state write.
Git object removal can also make a previously usable reference unavailable.

The lock retains its current schema, codec, and corruption checks.
It stores no source snapshots or complete source history.
Source commits before acknowledgement remain an optional convenience.
The [CLI reference](cli.md#historical-coverage) gives the lookup limits and diagnostic fields.

## 3. Read the state

Invoke the read-only inspection command:

```sh
memoria state inspect
memoria state inspect --format json
```

To read a file that is not in the selected project, name it:

```sh
memoria state inspect --file /tmp/other.lock
```

The explicit-file mode works outside Git. It needs no configuration and no worktree scan.

Inspection decodes stored bytes. It does not compare those records with current source files, and it makes no freshness claim. For worktree freshness, invoke `memoria status`.

The JSON `data` object contains exactly these keys: `path`, `file_bytes`, `payload_bytes`, `format_version`, `codec`, `checksum`, and `state`. Codec names are `raw` and `zstd-v1`. The checksum has 32 lowercase hexadecimal characters.

Each review record in `state.reviews` has a `coverage_evidence` value: `null` for an unrecorded record, or the sorted list of handed-off folders, such as `["auth", "docs"]`. The human view shows `coverage evidence: auth/, docs/`, `coverage evidence: none handed off`, or `coverage evidence: not recorded`. `memoria state diff` reports a changed value at `reviews.<document>.coverage_evidence`.

## 4. Format

The writer writes format version 3. The reader also accepts format version 2, which has no coverage field. Any other format byte is `state_unsupported_schema`. The frame has this exact order:

| Field | Encoding |
| --- | --- |
| Magic | Four bytes `4d 4d 4c 00`, which is `MML` and then NUL. |
| Format version | One byte, `03`. A format 2 file has `02`. |
| Codec | One byte. `00` is a raw payload. `01` is one Zstandard frame. |
| Decoded payload length | One shortest unsigned LEB128 integer. |
| Body | The raw payload, or one ordinary Zstandard frame. |
| Checksum | XXH3-128, seed 0, over every preceding byte, in big-endian order. |

The writer selects compressed bytes only when they are shorter than raw bytes. A tie selects raw bytes.

The payload normalizes the records. Unique strings, path components, repeated content descriptors, guidance digests, Git contexts, and integer vectors move into canonical tables. Each record then stores small indexes instead of repeated bytes.

**The coverage field.** In format 3, each review row has one unsigned LEB128 value `coverage` immediately after the index of its acknowledged-invalidations vector:

- `0` means unrecorded.
- `n ≥ 1` means recorded evidence at integer-vector table index `n − 1`. That vector holds the path-table identifiers of the handed-off folders, strictly increasing. The folders enter the path trie like invalidation subtree scopes. The empty vector means that the scope handed nothing off.

The reader refuses these as `state_corrupt`: a coverage index outside the vector table, a path identifier outside the path table, identifiers that do not strictly increase, a path that is not a folder strictly inside the record's document folder, and a vector or path row that nothing references. The reader charges each evidence folder against the expansion limits, and the writer's preflight charges the same amounts.

Format 2 is read-only legacy input. Every logical state has exactly one format 3 byte sequence.

Codec 1 uses bundled Zstandard 1.5.7 with a fixed profile: `windowLog=20`, `chainLog=24`, `hashLog=22`, `searchLog=7`, `minMatch=3`, `targetLength=256`, strategy `btultra2`, no workers, no long-distance matching, no dictionary, and no Zstandard content checksum. The outer checksum covers the frame and all metadata, so a second checksum adds no information.

A future compressor change that changes canonical bytes needs a new codec identifier or a new format version. It cannot redefine codec 1.

## 5. Size

These are the measured sizes of the same logical records in three representations. `Current JSON` is the version 1 state file. `Rejected JSON` is a compact row-based proposal that the project did not adopt.

| Fixture | Reviews / files / imports / open invalidations | Current JSON | Rejected JSON | Lock |
| --- | --- | ---: | ---: | ---: |
| Initial empty state | 0 / 0 / 0 / 0 | 112 | 152 | 23 |
| One small document | 1 / 2 / 0 / 0 | 1,162 | 789 | 276 |
| Actual repository state | 6 / 83 / 6 / 0 | 18,216 | 9,578 | 2,198 |
| Mixed metadata and invalidations | 12 / 166 / 12 / 2 | 40,630 | 22,863 | 3,379 |
| Repeated content at scale | 600 / 8,300 / 600 / 0 | 1,931,417 | 1,056,457 | 18,463 |
| Varied content, small scale | 60 / 830 / 60 / 0 | 193,270 | 105,810 | 11,349 |
| Varied content, medium scale | 600 / 8,300 / 600 / 0 | 1,931,711 | 1,056,751 | 100,959 |
| Varied content, large scale | 6,000 / 83,000 / 6,000 / 0 | 19,316,112 | 10,566,152 | 993,262 |

The `Lock` column is format 3. The measured records predate coverage evidence, so each review row carries one coverage byte (`0`). Format 2 wrote 275, 2,194, 3,376, 18,460, 11,342, 100,933, and 992,135 bytes for the same rows. Recorded evidence adds one integer vector for each distinct folder set, at about one byte for each folder, and a path row only for a folder that is not already in the trie.

The actual state is 87.93% smaller than the version 1 file. For varied content, size grows with the number of retained manifest entries, at about 165 to 189 bytes for each review.

The Rust test `state_v3_size_vectors_match_the_measured_contract` reproduces every number in this table on Linux. The committed format 2 vectors in `tests/fixtures/state-v2` stay as read-compatibility input: each one decodes with every record unrecorded and encodes to the committed format 3 vector in `tests/fixtures/state-v3`.

## 6. Errors and recovery

Every state error uses exit status 4. No read-only command repairs, rewrites, truncates, or resets state.

| Diagnostic | Cause |
| --- | --- |
| `state_missing` | Inspection was asked for a file that does not exist. |
| `state_corrupt` | A zero-byte file, a failed checksum, a malformed frame, or an impossible state, such as a record file outside its document's folder or malformed coverage evidence. |
| `state_limit_exceeded` | A size or expansion limit would be exceeded. |
| `state_unsupported_schema` | The format version is not 2 or 3. Memoria 0.6 reports it for every format 3 lock; that lock is valid. |
| `state_unsupported_codec` | The codec identifier is outside this release. |
| `state_legacy` | Only the version 1 `.memoria/state.json` exists. |
| `state_ambiguous` | Both the legacy file and `memoria.lock` exist. |

The writer applies the same limits as the reader. It measures the logical expansion of the state before it builds a table, so an oversized state is refused before any allocation. It also decodes its own output with the production decoder before it returns bytes. A write that the reader would reject fails as `state_limit_exceeded`, and the committed file keeps its previous content.

These are the resource limits:

| Resource | Limit |
| --- | --- |
| Encoded file | 64 MiB, including the header and the checksum. |
| Decoded payload | 64 MiB. |
| Expanded scalar values | 1,000,000. |
| Expanded string bytes | 64 MiB. |
| Compression window | 1 MiB for decoding. |

### Recover from a merge conflict

The magic bytes contain NUL, so Git identifies the artifact as binary and refuses to merge it line by line. Recovery uses one intact version as the baseline:

1. Copy both conflicting versions to a directory outside the worktree.
2. Inspect both versions with `memoria state inspect --file`.
3. Select one intact version as the baseline, and put it at `memoria.lock`.
4. Reissue the missing semantic invalidations with `memoria invalidate`.
5. Review each lost or changed document through a fresh review.

At the end of the recovery, invoke `memoria lint`. Then invoke `memoria check`.

A lost review is visible again through changed input bytes. A lost semantic reason cannot be reconstructed from source bytes, which is why step 4 exists.

CAUTION: Do not merge conflicting identifier ranges by hand. If two branches created invalidations, reissue them through the CLI.

## 7. Git settings

Add this rule to `.gitattributes` in the project root:

```gitattributes
/memoria.lock binary
```

Git applies the last matching line, so put this rule after any broad `* text` rule.

The rule prevents text merging and newline conversion. Git's own binary detection already finds the NUL byte, so the rule is a safeguard for projects with broad text attributes. `memoria init --apply` creates only the configuration and the state file; it never edits an attributes file.

## 8. Durability and locks

A state write does these steps in order:

1. Compare the stored bytes with the bytes that the command loaded.
2. Validate the complete new state.
3. Write a temporary file beside `memoria.lock` on the same filesystem.
4. Synchronize the file, rename it over the target, and synchronize the directory.

The temporary file uses the reserved name `.memoria.lock.tmp.<32 lowercase hexadecimal characters>`. An existing state file keeps its mode. A new state file uses mode `0600`.

The committed file never acts as the process lock. The write lock uses the worktree-private path that this command returns:

```sh
git rev-parse --git-path memoria/write.lock
```

Linked worktrees get separate locks. Read-only commands create no lock, no temporary file, and no directory.

Memoria composes that lock path from the absolute Git directory, so a symlinked metadata directory keeps its real location. It refuses a symlink, a special file, or a substituted parent between the Git directory and the lock. A refused path ends the command with exit 4 and no write.

Local Linux and macOS filesystems are the durability boundary. Windows and network filesystem locking are outside this release.

**Next:** invoke `memoria state inspect` to read what your project has recorded.
