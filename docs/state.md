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
- The input manifest: the document bytes, each selected file, and each imported export.
- The review metadata: time, reviewer, result, note, and Git context.
- The token digest and the guidance digest that the reviewer saw.
- The invalidation identifiers that the review acknowledged.

Each active invalidation keeps its identifier, scope, reason, creation time, original targets, and the targets that are still pending.

The file holds no absolute machine path, no source content, no full guidance text, no credentials, and no historical event stream. Reviewer names, notes, reasons, and Git context are ordinary committed project metadata.

The input fingerprint is not stored. Memoria computes it from the reconstructed manifest, so it cannot disagree with the manifest that it describes.

### Commit evidence for the last review

The existing `git.base_commit` field associates a commit with the last acknowledged review of inputs.
It does not track the last prose edit.
New acknowledgements retain a commit only when all reviewed content matches that commit.
The comparison covers the README, owned files, and imported export bodies through their lengths and XXH3 fingerprints.
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

## 4. Format

The file has format version 2. Its frame has this exact order:

| Field | Encoding |
| --- | --- |
| Magic | Four bytes `4d 4d 4c 00`, which is `MML` and then NUL. |
| Format version | One byte, `02`. |
| Codec | One byte. `00` is a raw payload. `01` is one Zstandard frame. |
| Decoded payload length | One shortest unsigned LEB128 integer. |
| Body | The raw payload, or one ordinary Zstandard frame. |
| Checksum | XXH3-128, seed 0, over every preceding byte, in big-endian order. |

The writer selects compressed bytes only when they are shorter than raw bytes. A tie selects raw bytes.

The payload normalizes the records. Unique strings, path components, repeated content descriptors, guidance digests, Git contexts, and integer vectors move into canonical tables. Each record then stores small indexes instead of repeated bytes.

Codec 1 uses bundled Zstandard 1.5.7 with a fixed profile: `windowLog=20`, `chainLog=24`, `hashLog=22`, `searchLog=7`, `minMatch=3`, `targetLength=256`, strategy `btultra2`, no workers, no long-distance matching, no dictionary, and no Zstandard content checksum. The outer checksum covers the frame and all metadata, so a second checksum adds no information.

A future compressor change that changes canonical bytes needs a new codec identifier or a new format version. It cannot redefine codec 1.

## 5. Size

These are the measured sizes of the same logical records in three representations. `Current JSON` is the version 1 state file. `Rejected JSON` is a compact row-based proposal that the project did not adopt.

| Fixture | Reviews / files / imports / open invalidations | Current JSON | Rejected JSON | Lock |
| --- | --- | ---: | ---: | ---: |
| Initial empty state | 0 / 0 / 0 / 0 | 112 | 152 | 23 |
| One small boundary | 1 / 2 / 0 / 0 | 1,162 | 789 | 275 |
| Actual repository state | 6 / 83 / 6 / 0 | 18,216 | 9,578 | 2,194 |
| Mixed metadata and invalidations | 12 / 166 / 12 / 2 | 40,630 | 22,863 | 3,376 |
| Repeated content at scale | 600 / 8,300 / 600 / 0 | 1,931,417 | 1,056,457 | 18,460 |
| Varied content, small scale | 60 / 830 / 60 / 0 | 193,270 | 105,810 | 11,342 |
| Varied content, medium scale | 600 / 8,300 / 600 / 0 | 1,931,711 | 1,056,751 | 100,933 |
| Varied content, large scale | 6,000 / 83,000 / 6,000 / 0 | 19,316,112 | 10,566,152 | 992,135 |

The actual state is 87.96% smaller than the version 1 file. For varied content, size grows with the number of retained manifest entries, at about 165 to 189 bytes for each review.

The Rust test `state_v2_size_vectors_match_the_measured_contract` reproduces every number in this table on Linux.

## 6. Errors and recovery

Every state error uses exit status 4. No read-only command repairs, rewrites, truncates, or resets state.

| Diagnostic | Cause |
| --- | --- |
| `state_missing` | Inspection was asked for a file that does not exist. |
| `state_corrupt` | A zero-byte file, a failed checksum, a malformed frame, or an impossible state. |
| `state_limit_exceeded` | A size or expansion limit would be exceeded. |
| `state_unsupported_schema` | The format version is outside this release. |
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
5. Review each lost or changed boundary through a fresh packet.

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
