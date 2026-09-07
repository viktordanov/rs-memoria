# Memoria command reference

This reference describes Memoria commands, their arguments, and their effects on documentation state.

Each command either reports project information or changes a specific part of the project.
A human or agent remains responsible for the prose.

Read the [workflow](workflow.md) for a first review with commands in task order.

## Terms used in this reference

A document boundary groups the selected files that one README explains.
That README is their owner.
An export is a marked section that another README can copy.
An import declares the managed copy of that section.
The supplier is the provider, and the recipient is the consumer.

A review packet contains one README and the exact inputs for its review.
An acknowledgement records the reviewer, result, and reason that the explanation is correct.
A revision counts successful acknowledgements for one README.
An invalidation is an explicit review request with a recorded reason.
The token identifies the document, revision, inputs, and invalidations that the acknowledgement must match.

Pending means that a README requires review.
Current means that it has no remaining review cause.
A consumer waits until its providers finish review.
The `render` command updates imported text without recording a review result.

The `lint` command examines documentation structure.
The `check` command also requires current reviews and current imported text.
Neither command changes files.

## Invocation and paths

| Argument | Meaning |
| --- | --- |
| `--root <directory>` | The directory must equal the Git worktree root. |
| `--format human\|json` | Human text is the default. JSON produces one envelope on stdout. |
| `--help`, `--version` | These arguments produce plain text. |

Commands discover the worktree root from the current directory.
Document and scope paths are relative to the project root.
The `--packet` and `--path` arguments resolve from the process directory.
Arguments must contain valid UTF-8 text.
The [CLI grammar](../src/presentation/cli.rs#L13) defines the command arguments.

## Configuration and ownership

| File | Purpose |
| --- | --- |
| `memoria.toml` | The root configuration defines project rules. |
| `README.md` | Each README defines a documentation boundary. |
| `README.memoria.toml` | An optional sidecar defines local rules beside a README. |
| `.memoria/state.json` | The state contains the latest review for each README and active invalidations. |
| `.memoria/write.lock` | This file supports the advisory lock for project mutations. |

The nearest README owns each selected file.
A child README creates a new boundary.
The parent does not hash that child README as an ordinary source input.
Declared imports carry the relevant export content into consumer inputs.

The root configuration supports these defaults:

```toml
version = 1
ignore = []
include = []

[documentation]
instructions = []
instruction_files = []

[fingerprints]
default = "raw"
languages = {}

[lint]
missing_import_hint = true
```

The supported sidecar fields are `ignore`, `include`, and `documentation`.
Writing instructions enter packet context without changing fingerprints.
A sidecar with only instructions does not add a selection scope to the fingerprint policy.
This rule also applies to a sidecar beside the root README.
An explicit invalidation requests review after a writing-policy change.

Source evidence: [configuration reader](../crates/memoria-infrastructure/src/config.rs#L104) and [effective policy](../crates/memoria-application/src/snapshot.rs#L1193).

<details>
<summary>Configuration syntax and glob rules</summary>

The TOML reader rejects unknown fields, duplicate keys, invalid value types, and unsupported versions.
Root configuration sections must be TOML tables.
Arrays accept string values.
Basic strings support backslash escapes.
Literal strings preserve backslashes and `#` characters.
TOML comments do not change a rule.
The configuration requires an empty `fingerprints.languages` map in this release.
Only raw byte hashing exists.

Glob patterns match whole paths relative to their configuration directory.
`*` and `?` remain within one path component.
`[...]` supplies a character class, and `**` matches directories.
The pattern `directory/**` selects contents within that directory.
The parser rejects negation of a whole pattern, absolute patterns, and `..`.

Source evidence: [TOML parser](../crates/memoria-infrastructure/src/config.rs#L104) and [glob parser](../crates/memoria-domain/src/glob.rs#L78).

</details>

### Selection order

Selection has four stages:

1. Git supplies tracked files and untracked files that Git does not ignore.
2. Memoria removes reserved inputs from source selection.
3. Root rules apply before sidecar rules, with nearer scopes last.
4. Ownership assigns each selected file to its nearest README.

Within one scope, `ignore` rules apply before `include` rules.
An include can restore a Memoria exclusion, but it cannot restore a Git exclusion.
If Git ignores a tracked path, that file remains eligible.
Unowned selected files cause lint and check errors.

<details>
<summary>Reserved inputs and repository boundaries</summary>

Reserved source inputs include configuration files, READMEs, sidecars, `.gitignore` files, referenced instruction files, and `.memoria/**`.
Managed Memoria packages and their transaction artifacts are also reserved.
The tool discovers neither sources nor documentation boundaries within its reserved trees.

The default package parents are `.agents/skills` and `.claude/skills`.
A custom in-project `memoria` package requires a valid `.memoria-install.json` for discovery as managed guidance.
The reserved sibling names are exact:

```text
memoria
memoria.staging
memoria.removing
memoria.install-txn.json
memoria.install.lock
memoria.backup
memoria.backup-<n>
```

Other names, such as `memoria.rs` and `memoria.config`, remain ordinary source candidates.
Nested repositories and submodules remain opaque boundaries.
This rule also applies to contents that the parent index still tracks.

Memoria rejects selected symlinks, selected paths behind symlink ancestors, and selected special files.
Excluded symlinks do not cause source errors.
A symlink at `.memoria`, its lock, or its state file prevents mutations.
A missing tracked README or sidecar counts as a deletion.
The current files determine the resulting ownership and local rules.

Source evidence: [reserved paths](../crates/memoria-application/src/snapshot.rs#L137) and [filesystem path rules](../crates/memoria-infrastructure/src/fs.rs#L22).

</details>

<details>
<summary>Git policy and inspection limits</summary>

The fingerprint policy includes effective global excludes, repository excludes, and applicable `.gitignore` rules.
Git traversal determines which directory ignore files are active.
Ignored directories, nested repositories, and reserved trees stop that traversal.
If Git ignores an active `.gitignore` file, its rules can still affect policy.
An active directory can contribute rules without eligible source files.

An empty or comment-only ignore file contributes no rules.
The reader removes a leading UTF-8 byte order mark.
A later byte order mark remains pattern content.
Effective rule bytes retain their exact identity, even outside UTF-8.

A relative `core.excludesFile` resolves from the worktree root.
Its filename retains leading and trailing spaces.
An explicitly empty value disables global excludes without an XDG fallback.
Otherwise, Git and Memoria use the applicable configured or default path.

The Git adapter disables filesystem monitors, pagers, external diff programs, and lazy object retrieval.
It clears `core.attributesFile` and uses the empty tree as the attribute source.
Submodule content remains opaque during Git context collection.
Only a change to the recorded submodule commit counts as a parent change.

If `info/attributes` declares filters, the adapter compares raw bytes for dirty-worktree context.
It compares symlink text without opening the target.
A changed path kind or symlink ancestor counts as a change without content access.
A missing historical blob makes a diff unavailable without a remote fetch.
The command surface requires Git support for `--attr-source`.

Source evidence: [Git command construction](../crates/memoria-infrastructure/src/git.rs#L16), [Git adapter](../crates/memoria-infrastructure/src/git.rs#L260), and [ignore policy](../crates/memoria-application/src/snapshot.rs#L1193).

</details>

## Inspection commands

| Command | Result |
| --- | --- |
| `memoria status [--explain <path>]` | It shows ownership, selected bytes, review state, and exclusions. |
| `memoria lint` | It examines configuration, coverage, markers, exports, imports, cycles, and navigation. |
| `memoria review` | It shows pending documents in dependency order and the next action. |
| `memoria check` | It requires valid structure, current reviews, and current import copies. |
| `memoria graph` | It shows README ownership, import, and navigation edges with review state. |

`status` also shows active invalidations, unowned files, disconnected READMEs, and repository boundaries.
Its explanation names the rule chain for one path.
A normal Markdown link supplies navigation without creating a review dependency.
Local navigation decodes URL path escapes once and leaves `+` literal.
Invalid encodings or paths outside the root do not create navigation edges.

`lint` fails with exit 1 for structural errors.
Its `imports_outdated` and `navigation_disconnected` warnings do not fail lint.
Its `missing_import_hint` diagnostics remain optional hints.
The configuration key `lint.missing_import_hint: false` suppresses those hints.
`check` treats outdated imports as errors and fails for pending reviews.

The review plan orders providers before consumers, with path order for ties.
`data.next_ready` names the first ready document.
`data.next_action` names the required `render` or `review` action for that document.
If its imports are outdated, a ready document still requires `render` before a packet.
The plan shows input byte counts before packet creation.

Source evidence: [plan](../crates/memoria-application/src/usecases/plan.rs#L43), [lint](../crates/memoria-application/src/usecases/lint.rs#L26), and [check](../crates/memoria-application/src/usecases/check.rs#L37).

## Focused review packets

```sh
memoria review <README.md> [--max-bytes <n>] --format json
```

A focused review requires a pending document whose providers permit review and whose import copies are current.
The packet contains the document, every owned file, imported exports, previous review, writing instructions, and covered invalidations.
It also identifies changed inputs, exports, consumers, and available diffs.
Text content uses UTF-8, and other content uses base64.

The token contains 21 bytes with the form `mrv1.<16 hex>`.
A JSON packet supports acknowledgement from a file or stdin.
Human output supports reading only.
The [workflow](workflow.md#2-capture-the-packet) stores the packet outside the project.

| Limit | Value |
| --- | --- |
| Default raw inputs | 8 MiB |
| Maximum raw inputs through `--max-bytes` | 32 MiB |
| Serialized packet | 64 MiB |
| Decoded packet content | 32 MiB |
| Array elements and nesting | 100,000 elements and 32 containers |

Raw inputs contain the README, selected files, and imported export bodies.
Decoded content also counts instruction text, available old content, and generated diff text.
`size.record_count` counts array elements across the entire envelope and its diagnostics.
The producer encodes the packet before either output format can use it.
Acknowledgement recomputes the counts and rejects inconsistent values.

<details>
<summary>Refusals and unavailable history</summary>

A size refusal returns `packet_too_large` without a token.
If the refusal envelope fits the hard limits, it contains the full manifest.
Otherwise, `data.manifest_omitted` is true and `data.manifest_summary` contains bounded counts and hashes.
The diagnostic supplies the same counts in human and JSON output.
The tool does not omit required content to force a successful packet within a limit.

Old file content comes from the base commit in the previous review.
The content must match the reviewed hash before it can supply a diff.
Without that match, the packet marks the diff unavailable and supplies current content.
The state does not store old export bodies.
A rejected acknowledgement can compare imports against the bytes in the supplied packet.

Source evidence: [packet construction](../crates/memoria-application/src/usecases/prepare_review.rs#L105) and [packet codec](../crates/memoria-infrastructure/src/packet.rs#L268).

</details>

## Review mutations

A mutation changes stored files or saved review state.

### `memoria init`

The command creates missing root configuration, root README, and empty state files.
It validates existing files before writes.
An invalid existing configuration, README, or state prevents initialization.
Unresolved imports and coverage gaps remain lint concerns.
The printed checklist identifies generated output, snapshots, tests, and fixtures for scope decisions.

### `memoria render [<README.md>] [--dry-run]`

The command replaces only declared import bodies with current provider export bytes.
It preserves the other document bytes and does not change review state.
Each README replacement is atomic.
A second invocation with identical inputs makes no writes.
Invalid imports or cycles prevent the operation.

After a partial write failure, `render_incomplete` identifies the remaining documents.
The command does not treat several README replacements as one transaction.
The dry run describes the changes without applying them.

### `memoria invalidate <scope> --reason <text>`

The scope is `all`, `doc:<README.md>`, or `subtree:<directory>`.
The command captures the currently discovered READMEs in that scope.
It records the reason without changing their text.
An empty scope causes a usage error.
The reason appears in status, the plan, and each affected packet.

### `memoria ack`

```sh
memoria ack <README.md> --packet <file|-> --token <token> \
  --reviewer <name> --result updated|no-update --note <text>
```

The reviewer name permits 1–128 characters.
The note permits 12–1000 characters and requires at least three words.
Generic notes such as `done` are invalid.
The `--packet -` argument selects stdin.
Without that argument, acknowledgement does not read stdin.

Acknowledgement has five stages:

1. The application validates arguments and decodes the packet.
2. It compares packet content hashes and the token before the write lock.
3. Under the lock, it compares current inputs and provider state with the packet.
4. The domain compares the review revision and covered invalidations.
5. A final snapshot comparison precedes the atomic state save.

A changed manifest causes `snapshot_changed` with exit 3 and exact differences.
A deleted packet document in an otherwise valid project causes the same conflict.
A later document revision causes `revision_conflict` with exit 3.
A pending provider causes `dependencies_pending` with exit 3.
These conflicts leave the stored review unchanged.

The final comparison reports changed inputs before provider readiness errors.
Only covered invalidations clear for this document.
Newer invalidations remain pending and cause `still_pending: true` in a successful acknowledgement.
The state contains the latest review per README, rather than an append-only journal.

Source evidence: [acknowledgement](../crates/memoria-application/src/usecases/ack.rs#L193), [render](../crates/memoria-application/src/usecases/render.rs#L114), and [invalidation](../crates/memoria-application/src/usecases/invalidate.rs#L50).

## Agent packages

```sh
memoria agent install|uninstall [--target codex|claude] [--path <skills-directory>] [--dry-run]
```

The binary embeds `skills/memoria/SKILL.md` at build time.
The default destination is `.agents/skills/memoria` for Codex or `.claude/skills/memoria` for Claude.
Without `--target`, exactly one of `.agents` and `.claude` must exist.
A custom `--path` requires `--target` and names the package parent.
The dry run makes no changes.

The package contains `SKILL.md` and `.memoria-install.json`.
The installer preserves a backup before replacement.
Identical installed content requires no change.
Locally edited managed content causes a conflict with exit 3.
Changed file kinds also cause conflicts without content access through symlinks or special files.

<details>
<summary>Installation transactions and recovery</summary>

Installation uses the parent lock `memoria.install.lock`.
It prepares the package in `memoria.staging` and records the transaction in `memoria.install-txn.json`.
It moves an existing package to `memoria.backup` or a numbered sibling backup.
Then it renames the prepared package into place and synchronizes the parent directory.
Uninstall uses `memoria.removing` before package removal and backup restoration.

The installation and transaction records identify backups by sibling name.
A record cannot direct restoration outside its package parent.
Recovery examines the schema, phase, paths, backup identity, and managed content before a change.
Unknown content in temporary directories remains in place and causes a conflict.
A temporary directory without a valid transaction also remains in place.

If a package rename succeeds but the next directory synchronization fails, the transaction supports recovery on the next mutation.
The error identifies the transaction record and preserved backup.
A rename that did not occur permits immediate restoration.
An ordinary installation error attempts restoration of the previous package.
A failed restoration reports both errors and the backup location.

The dry run reports `recovery_needed` for an interrupted transaction.
The next installation or removal recovers the transaction under the lock before it determines the requested change.
Recovery of an interrupted removal satisfies an uninstall request.

Source evidence: [skill adapter](../crates/memoria-infrastructure/src/skill.rs#L618).

</details>

## JSON and diagnostics

Every JSON response uses this envelope:

```json
{"schema_version": 1, "command": "status", "ok": true, "data": {}, "diagnostics": []}
```

Diagnostics contain a code, severity, message, and structured details.
Optional `path`, `line`, and `column` fields identify a location.
Severity is `error`, `warning`, or `hint`.
The application sorts diagnostics by path, location, and code.
Human diagnostics go to stderr.
Mutation progress also goes to stderr before writes.

Argument errors honor an explicit JSON format request.
They return `ok: false`, `data: null`, and a `usage_error` diagnostic with exit 2.
Help and version requests still produce plain text.
If response delivery fails, the process exits 4 with `io_error` on stderr.
A completed mutation remains in place after an output error.

Source evidence: [output delivery](../src/main.rs#L312) and [diagnostic types](../crates/memoria-application/src/error.rs#L107).

## Exit statuses

| Exit | Meaning |
| --- | --- |
| 0 | The command succeeded. New invalidations can still remain after acknowledgement. |
| 1 | Project validation failed or required review work remains. |
| 2 | Arguments, review text, token, or packet validation failed. |
| 3 | A lock, snapshot, revision, or installation conflict prevents the operation. |
| 4 | An I/O error, unsupported Git state, or corrupt state prevents success. |

<details>
<summary>Diagnostic identifiers</summary>

These identifiers appear in the command contract:

```text
usage_error configuration_missing configuration_invalid sidecar_invalid sidecar_orphan
instruction_file_missing instruction_file_invalid path_invalid path_unsupported
root_readme_missing coverage_unowned markdown_invalid marker_malformed marker_nested
marker_mismatch marker_unclosed export_invalid export_duplicate import_invalid
import_missing_document import_missing_export import_self import_duplicate import_cycle
imports_outdated navigation_disconnected missing_import_hint review_pending
review_not_pending dependencies_pending document_not_found packet_too_large max_bytes_invalid
token_invalid token_mismatch reviewer_invalid result_invalid note_invalid packet_unreadable
packet_source_invalid packet_limit_exceeded packet_schema_invalid packet_integrity_failed
packet_token_mismatch packet_content_mismatch packet_document_mismatch snapshot_changed
revision_conflict invalidation_not_active invalidation_reason_mismatch scope_invalid
scope_empty reason_invalid state_busy state_conflict state_corrupt state_unreadable
git_unavailable git_unsupported root_mismatch io_error render_incomplete target_required
target_ambiguous skill_conflict skill_not_installed
```

</details>

## Fingerprints and limits of the result

Hashes use XXH3-64 with the default secret, seed zero, and 16 lowercase hexadecimal digits.
Canonical byte encodings use fixed schemas and big-endian lengths.
The token covers the document path, document revision, manifest, and covered invalidations.
Timestamps, commits, worktree locations, writing instructions, and unrelated reviews do not affect the token.
The token detects accidental change and does not authenticate a reviewer.

This release requires Git worktrees and Linux local filesystems.
It rejects bare repositories, sparse checkouts, and unmerged index entries.
It does not support language filters or hostile concurrent writers.
Raw fingerprints include whitespace, comments, and exact newlines.
Corrupt review state requires recovery from a known valid copy.

<details>
<summary>Export syntax limits</summary>

Export bodies accept only absolute `https://`, `http://`, or `mailto:` link destinations.
They reject raw HTML, footnotes, relative links, and reference-style syntax outside literal code.
Malformed inline links and nested brackets in link text also cause errors.
These restrictions prevent imported text from acquiring a different meaning in the consumer.

For literal brackets, use `\[text\]`.
For an export link, use a full inline link with an absolute destination.

Source evidence: [Markdown codec](../crates/memoria-infrastructure/src/markdown.rs#L332).

</details>

The [specification](specification.md) retains its original draft status and proposed decisions.
This reference describes the implementation in this checkout.

## Continue

Invoke `memoria review` to find the next documentation action.
