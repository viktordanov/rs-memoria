# Memoria command reference

This reference describes Memoria commands, their arguments, and their effects on documentation state.

Each command either reports project information or changes a specific part of the project.
A human or agent remains responsible for the prose.

Read the [workflow](workflow.md) for a first review with commands in task order.

## On this page

- [Terms, invocation, and paths](#terms-used-in-this-reference)
- [Configuration and ownership](#configuration-and-ownership)
- [Inspection commands and packets](#inspection-commands)
- [Mutations and agent packages](#review-mutations)
- [Diagnostics, exits, and limits](#json-and-diagnostics)

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
| `memoria.lock` | This generated file contains the latest review for each README and the active invalidations. |
| `.gitattributes` | The rule `/memoria.lock binary` prevents text merging and newline conversion. |

Commit `memoria.toml` and `memoria.lock` together.
Memoria owns the bytes of `memoria.lock`; do not edit it.
The write lock is not in the worktree.
It uses the path that `git rev-parse --git-path memoria/write.lock` returns, so linked worktrees receive separate locks.

The nearest README owns each selected file.
A child README creates a new boundary.
The parent does not hash that child README as an ordinary source input.
Declared imports carry the relevant export content into consumer inputs.

The root configuration supports these defaults:

```toml
version = 2
ignore = []
include = []

[documentation]
guidance = []
guidance_files = []

[fingerprints]
default = "raw"
languages = {}

[lint]
missing_import_hint = true
```

The supported sidecar fields are `ignore`, `include`, and `documentation`.
Documentation guidance enters packet context without changing fingerprints.
A sidecar with only guidance does not add a selection scope to the fingerprint policy.
This rule also applies to a sidecar beside the root README.
An explicit invalidation requests review after a guidance change.

Guidance appends from the root scope toward the document scope.
Within each scope, inline entries come before file entries, and each list keeps its authored order.
Memoria does not override, deduplicate, or rank conflicting prose.

A `guidance_files` entry resolves relative to its declaring configuration file.
The destination must stay inside the project and hold a regular UTF-8 file.
These destinations are invalid: a symlink, a missing file, Git metadata, a state artifact, a configuration file, and a README boundary file.

The retired keys `instructions` and `instruction_files` fail with the exact replacement name.
A configuration that holds both spellings fails; Memoria never selects one silently.

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

Reserved source inputs include configuration files, READMEs, sidecars, `.gitignore` files, referenced guidance files, `memoria.lock`, `.memoria.lock.tmp.<32 hex>`, and `.memoria/**`.
The agent integration files are also reserved, whether installed or absent: `.codex/hooks.json`, `.codex/config.toml`, `.codex/memoria-hook.json`, `.claude/settings.local.json`, and `.claude/memoria-hook.json`.
Unrelated files in those directories stay ordinary inputs.
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

README discovery precedes Memoria ignore/include selection.
An eligible nested README remains a boundary even when Memoria excludes its surrounding source files.
A Memoria ignore rule does not hide a README boundary.
A tracked README absent from the worktree is not a current boundary.

Source evidence: [reserved paths](../crates/memoria-application/src/snapshot.rs#L137) and [filesystem path rules](../crates/memoria-infrastructure/src/fs.rs#L22).

</details>

<details>
<summary>Git policy and inspection limits</summary>

The fingerprint policy includes applicable repository `.gitignore` rules and Memoria selection scopes.
Host excludes affect Git eligibility but never enter the policy hash.
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

### Shell completions

`memoria completions bash|zsh|fish` prints a script without project discovery or installation.
JSON output contains `data.shell` and the same full script in `data.script`.

Create the script for your shell:

```sh
# Bash
memoria completions bash > /tmp/memoria.bash
source /tmp/memoria.bash

# Zsh
mkdir -p ~/.zsh/completions
memoria completions zsh > ~/.zsh/completions/_memoria
fpath=(~/.zsh/completions $fpath)
autoload -Uz compinit
compinit

# Fish
mkdir -p ~/.config/fish/completions
memoria completions fish > ~/.config/fish/completions/memoria.fish
```

### `memoria explain <README.md>`

This read-only command explains whole-file freshness for a current, pending, waiting, or never-reviewed boundary.
It reports changed paths, hashes, lengths, policy, guidance, imports, invalidations, and locally verified Git hunks.
It does not acquire a write lock, fetch Git objects, or acknowledge a review.
`status --explain <path>` remains the separate source-selection explanation.

The JSON result has `kind="freshness_explanation"`.
It includes state fields, `changes`, `policy`, `guidance`, `evidence`, `before_manifest`, and `current_manifest`.
A missing previous review produces a null previous manifest.
Evidence records give baseline verification, expected and observed hashes and lengths, and a fixed unavailable-hunk reason code.
Only a verified baseline permits a hunk.
Unavailable hunks alone do not fail the command.

The lock stores previous policy and guidance hashes, not previous rules or prose.
The result labels that missing context `not_stored`.
Current policy scopes retain their order and exact rule bytes, with base64 for non-UTF-8 rules.
Guidance remains advisory, and a guidance-only change does not create a pending cause.
The command preserves whole-file freshness even for a one-line comment change.

Within the lookup budgets, equal inputs and equal local Git evidence produce equal JSON.
Limits are 32 MiB of decoded evidence, 100,000 records, and 64 MiB of buffered output.
Old bytes and generated hunks count toward the evidence limit.
A whole-result refusal returns `explain_limit_exceeded` with exit 1 and no partial success result.

### `memoria state diff <OLD_LOCK> <NEW_LOCK>`

This read-only command compares two explicit lock snapshots without project discovery.
Both paths resolve from the invocation directory, even with `--root`.
It compares saved records, not current source freshness.
It does not convert files or store history.

The JSON result has `kind="state_diff"`, frame metadata, `byte_equal`, `logical_equal`, and ordered `changes`.
Each change has a path, presence flags, and before/after values.
Presence flags distinguish absent values from stored nulls.
Files match by path, imports by provider and export ID, and invalidations by ID.
Both old and new notes remain visible.

Equal and different snapshots both succeed with exit 0.
Invalid snapshots retain state diagnostics and exit 4, with the failing operand identified.
Each input retains the existing 64 MiB limits and expansion checks.
The comparison has a 256 MiB buffered output limit.
An output refusal returns `state_comparison_limit_exceeded` with exit 4.

| Command | Result |
| --- | --- |
| `memoria status [--explain <path>]` | It shows ownership, selected bytes, review state, guidance counts, and exclusions. |
| `memoria status --summary` | It emits bounded counts only. |
| `memoria guidance [<README.md>]` | It shows the documentation guidance that applies to a boundary. |
| `memoria lint` | It examines configuration, coverage, markers, exports, imports, cycles, and navigation. |
| `memoria review` | It shows pending documents in dependency order and the next action. |
| `memoria check` | It requires valid structure, current reviews, and current import copies. |
| `memoria graph` | It shows README ownership, import, and navigation edges with review state. |
| `memoria state inspect [--file <path>]` | It decodes committed state and shows its framing. |

Every command in this table is read-only.
None of them creates a lock, a temporary file, or a directory.

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
A changed-guidance count is a hint in `check`, and it never fails the command.

### `memoria status --summary`

Summary mode uses the same snapshot and freshness logic as ordinary status.
It emits counts without per-document manifests, source content, full guidance, or exclusion explanations.
It rejects `--explain` as a usage error.

Its `data` object holds `readmes`, `selected_files`, `selected_bytes`, the four review counts, guidance counts, unowned and disconnected counts, invalidation counts, and diagnostic counts.
Waiting can overlap current, pending, or never-reviewed status.
These counters are not disjoint categories.

### `memoria guidance [<README.md>]`

Without a path, the command shows the effective guidance of the root README.
It also lists each scope that adds guidance, with the command that inspects that scope.
With a path, it shows that boundary's entries, exact sources, digest, and applicable scope.

The command works for current documents.
It needs no review packet, and it changes no state.

Its `data` object holds `document`, `digest`, `entries`, `sources`, `scopes`, `reviewed_digest`, and `changed_since_review`.
Each entry holds `scope`, `source`, `kind`, and `text`.
The empty scope string is the repository root.
Entry kinds are exactly `inline` and `file`.

### `memoria state inspect [--file <path>]`

Without `--file`, the command inspects `memoria.lock` in the selected project.
A missing file returns `state_missing` with exit 4.
With `--file`, it resolves the path against the invocation directory, works outside Git, and needs no configuration.

Inspection decodes stored bytes.
It makes no freshness claim, and it offers no export, import, reset, or migration.
The [state guide](state.md) describes the format, the limits, and the recovery procedure.

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
The packet contains the document, every owned file, imported exports, previous review, documentation guidance, and covered invalidations.
It also identifies changed inputs, exports, consumers, and available diffs.
Text content uses UTF-8, and other content uses base64.

The guidance shape is `data.context.guidance`:

```json
{
  "guidance": {
    "digest": "0123456789abcdef",
    "entries": [
      {
        "scope": "",
        "source": "memoria.toml",
        "kind": "inline",
        "text": "Explain the operational workflow before implementation details."
      }
    ]
  }
}
```

The packet uses schema version 2.
Memoria rejects a version 1 packet before acknowledgement.
The token contains 21 bytes with the form `mrv2.<16 hex>`.
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
Decoded content also counts guidance text, available old content, and generated diff text.
Large guidance produces an explicit limit error.
It never disappears from the packet through silent truncation.
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

Old file content comes from a verified local commit.
The bounded lookup examines the previous reference first.
The content must match the reviewed length and hash before it can supply a diff.
Without that match, the packet marks the diff unavailable and supplies current content.
The state does not store old export bodies.
The bounded lookup can recover matching export bodies from historical provider READMEs.
A rejected acknowledgement can compare imports against the bytes in the supplied packet.

Source evidence: [packet construction](../crates/memoria-application/src/usecases/prepare_review.rs#L105) and [packet codec](../crates/memoria-infrastructure/src/packet.rs#L268).

</details>

## Saved packet views

The [method and results](development/review-context.md) explain the measured presentation savings and their limits.

The default human review and explanation show changes, available hunks, scope, and the next command.
`memoria review README.md --full` retains the detailed human packet.
`memoria explain README.md --full` retains the detailed human explanation.
`--format json` always retains complete schema-v2 output, regardless of `--full`.
The human view never relaxes packet limits.

1. Save the canonical packet outside the project:

   ```sh
   memoria review README.md --format json > /tmp/review.json
   ```

2. Invoke the saved-packet reader:

   ```sh
   memoria packet view /tmp/review.json
   memoria packet view /tmp/review.json --section guidance
   memoria packet view /tmp/review.json --file src/example.rs --format json
   ```

The reader requires no project discovery and makes no writes.
`-` selects stdin.
It validates the canonical packet before selection.
The selected bodies come from that snapshot, even when the live files change.
File bodies retain their declared UTF-8 or base64 encoding.
A missing path causes `packet_view_file_missing` with exit 2.

| Selection | Contents |
| --- | --- |
| `summary` (default), `changes` | Changes, hunks, missing-evidence reasons, scope counts, and a guidance cue |
| `guidance`, `content` | Complete guidance or current README, files, and imports |
| `history`, `inventory` | Previous review and diffs, or the current manifest |
| `--file PATH` | One current README or owned file from the packet |
| `incremental` | Experimental P1 preparation, with explicit coverage and fallback reasons |

`--file` and an explicit `--section` are mutually exclusive.
Imports remain accessible through `content` or `incremental`.
JSON views use envelope schema 2, `data.kind=packet_view`, and `data.view_version=1`.
They carry `canonical=false`, `snapshot_token`, and `source_packet_digest`.
They cannot substitute for the canonical acknowledgement packet.
New view fields do not change the existing packet schema or decoder.
A view that exceeds 64 MiB in human or JSON rendering causes `packet_view_limit_exceeded` with exit 1.
The reader refuses that result instead of truncating it.

Experimental P1 requires explicit owner approval of reliance on a trusted prior review:

```sh
memoria packet view /tmp/review.json --section incremental --trust-prior-review --format json
```

Without that trust flag, preparation requires full review.
The flag has no meaning for other sections and causes `packet_view_trust_invalid` with exit 2.
An unknown section causes `packet_view_section_invalid` with exit 2.
The [shipped skill](../skills/memoria/SKILL.md) defines trust, context retrieval, coverage, and full-review fallback.
`model_quality_gate_passed=false` remains explicit.
No projection records an inspection or approves an input.

## Historical coverage

Acknowledgement supplies a `historical_coverage` hint after a successful state save.
The hint reports `verified`, `partial`, or `unavailable`, plus counts and budget status.
Coverage includes the README, owned files, and imported export bodies at one commit.
Only complete coverage supplies a new saved `git.base_commit`.
Dirty acknowledgements remain valid with a null reference.

History retrieval first examines the saved reference, then at most 64 commits from local HEAD ancestry.
The shared operation budget permits 1,024 blob reads, 32 MiB of historical bytes, and four seconds, plus bounded process cleanup.
Git never fetches missing objects for this operation.
Every historical body must match the acknowledged length and XXH3 fingerprint.
A later matching commit can supply a hunk without changing the earlier review attribution.
`explain` names the actual matched commit in its evidence.
Shallow or missing history cannot establish that matching bytes never existed elsewhere.

`budget_exhausted` identifies incomplete coverage inspection.
`inspection_failed` distinguishes an adapter error from an exhausted budget.
Unavailable explanation evidence uses `history_limit_exceeded`, `git_read_failed`, `no_base_commit`, `reviewed_bytes_mismatch`, or `blob_unavailable`.
These reasons describe local evidence, not review validity.
Committing source before acknowledgement can improve future evidence, but it is optional.
The [state guide](state.md#2-what-the-file-holds) describes the unchanged storage format.

## Review mutations

A mutation changes stored files or saved review state.

### `memoria init [--apply]`

Without `--apply`, the command is a read-only preview.
It validates setup inputs, not the full project or the meaning of prose.
The preview reads root README markers and import reference syntax, existing root configuration, referenced guidance, and existing state.
It records no acknowledgement and does not certify nested documentation.
Invoke `memoria status` and `memoria lint` for the broader project view.
It explains the documentation model, gives three example strategies, and lists the two committed files.
It reports a missing root README without failing.
It writes nothing, not even in an empty project.

With `--apply`, the command creates the missing `memoria.toml` and `memoria.lock` files.
It requires an existing root `README.md`, and it returns `root_readme_missing` with exit 1 before any write when that file is absent.
It never creates README prose, exports, imports, or nested boundaries.

Apply validates existing files first.
An invalid existing configuration, README, or state prevents initialization.
An existing valid installation produces no changes, and existing files keep their bytes and modes.
A partial write failure still reports exactly which files reached disk.

CAUTION: An existing unrelated `memoria.lock` is a conflict to resolve by hand.
Initialization never overwrites a file because its name matches.

The generated configuration holds an empty guidance list and useful comments.
It imposes no writing standard on your project.

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
An explicit `--reviewer` takes precedence over the optional `MEMORIA_REVIEWER` environment value.
An absent or blank environment value without `--reviewer` produces `reviewer_required` with exit 2.
An invalid explicit label never falls back to the environment.
Labels use trim and the existing validation rules.
Memoria never infers identity from the OS, Git, a model, or shared configuration.
Success output confirms the resolved label, which provides attribution but no authentication or review authority.
Managed agents must supply an explicit `--reviewer` label.

After trim, the note permits 12–1000 Unicode characters and requires at least three whitespace-separated words.
CR/LF are allowed, but tabs and other controls are forbidden.
Normalization rejects generic notes: `done`, `reviewed`, `looks good`, `ok`, `okay`, `lgtm`, `fine`, `no changes`, `no change`, and `updated`.
The note explains why this README is correct for this packet.
It is not an instruction, an override, or proof that the reviewer read every input.
Reviewer and note validation precede packet reads and state mutation.
The `--packet -` argument selects stdin.
Without that argument, acknowledgement does not read stdin.

Acknowledgement has five stages:

1. The application validates arguments and decodes the packet.
2. It compares packet content hashes and the token before the write lock.
3. Under the lock, it compares current inputs and provider state with the packet.
4. The domain compares the review revision and covered invalidations.
5. A final snapshot comparison precedes the atomic state save.

Changed documentation guidance causes `guidance_changed` with exit 3.
That conflict writes no review state and does not make a current document stale.
The application compares current guidance under the write lock and again before the state save.

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
memoria agent install|status|upgrade|uninstall [--target codex|claude] [--scope local|global] [--path <skills-directory>] [--replace-existing] [--dry-run]
memoria agent hook install|status|uninstall --target codex|claude [--dry-run]
memoria agent hook run --target codex|claude --protocol 1 --configuration-root <directory>
```

The binary embeds `skills/memoria/SKILL.md` at build time.
The default scope is `local`, inside the selected worktree.
The local destination is `.agents/skills/memoria` for Codex or `.claude/skills/memoria` for Claude.
Without `--target`, exactly one of `.agents` and `.claude` must exist.

A global operation requires `--scope global` and an explicit `--target`.
It works outside Git and without `memoria.toml`, and it rejects `--root`.
A custom `--path` requires `--target` and names the package parent.
A local custom path must stay inside the worktree, and a global custom path must be absolute.
A path outside the worktree never implies a global installation.

The dry run makes no changes.
`status` never writes and exits 0 for any readable inspection.

The package contains `SKILL.md` and `.memoria-install.json`, at record schema 2.
An older managed package returns `skill_upgrade_required` with exit 1, so replacement is explicit.
An unmanaged package needs `--replace-existing`, which creates a verified sibling backup first.
Locally edited managed content causes `skill_conflict` with exit 3.
Changed file kinds also cause conflicts without content access through symlinks or special files.

Uninstall of an absent package exits 0 with `no_change`.
It creates no directory and no lock.
The zero-byte parent lock stays in place, reported as `synchronization_lock`.
The [agent integrations guide](agents.md) explains the scopes, the backups, and the hook contract.

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
{"schema_version": 2, "command": "status", "ok": true, "data": {}, "diagnostics": []}
```

The native hook runner is the one exception.
It speaks native hook JSON on stdin and stdout, and it does not accept `--format`.

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
guidance_file_missing guidance_file_invalid guidance_changed path_invalid path_unsupported
root_readme_missing coverage_unowned markdown_invalid marker_malformed marker_nested
marker_mismatch marker_unclosed export_invalid export_duplicate import_invalid
import_missing_document import_missing_export import_self import_duplicate import_cycle
imports_outdated navigation_disconnected missing_import_hint review_pending
review_not_pending dependencies_pending document_not_found packet_too_large max_bytes_invalid
summary_invalid token_invalid token_mismatch reviewer_invalid result_invalid note_invalid
packet_unreadable packet_source_invalid packet_limit_exceeded packet_schema_invalid
packet_integrity_failed packet_token_mismatch packet_content_mismatch
packet_document_mismatch snapshot_changed revision_conflict invalidation_not_active
invalidation_reason_mismatch scope_invalid scope_empty reason_invalid state_busy
state_conflict state_corrupt state_unreadable state_missing state_legacy state_ambiguous
state_limit_exceeded state_unsupported_schema state_unsupported_codec
state_inspection_limit_exceeded git_unavailable git_unsupported root_mismatch root_invalid
io_error render_incomplete target_required target_ambiguous home_unset worktree_required
path_not_absolute path_outside_worktree claude_config_dir_relative skill_conflict
skill_not_installed skill_upgrade_required skill_replace_required hook_client_unsupported
hook_conflict hook_unmanaged hook_configuration_ambiguous hook_configuration_invalid
hook_configuration_too_large hook_configuration_outside_worktree hook_location_unsupported
```

</details>

## Fingerprints and limits of the result

Hashes use XXH3-64 with the default secret, seed zero, and 16 lowercase hexadecimal digits.
The `memoria.lock` frame checksum uses XXH3-128.
Canonical byte encodings use fixed schemas and big-endian lengths.

The canonical domains are `memoria.policy.v2`, `memoria.inputs.v2`, `memoria.review-token.v2`, `memoria.packet.v2`, and `memoria.guidance.v1`.
The selection tag is `git-worktree-v2`, and the repository ignore inventory is `repository-ignore-v1`.

The token covers the document path, document revision, manifest, guidance digest, and covered invalidations.
Timestamps, commits, worktree locations, host ignore settings, and unrelated reviews do not affect the token.
The token detects accidental change and does not authenticate a reviewer.

Host ignore settings decide which files Git reports as eligible.
They never enter the policy hash.
Two hosts with equal selected inputs and equal repository rules produce equal freshness.
The policy inventory uses fixed case-sensitive matching, so it does not normalize case-only paths, Unicode filenames, or line endings.

This release requires Git worktrees and Linux or macOS local filesystems.
It rejects bare repositories, sparse checkouts, and unmerged index entries.
It does not support language filters or hostile concurrent writers.
Raw fingerprints include whitespace, comments, and exact newlines.
Corrupt review state requires recovery from a known valid copy, as the [state guide](state.md#6-errors-and-recovery) describes.

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

The [specification](specification.md) records the approved 0.2.0 contract and earlier proposed decisions.
This reference describes the implementation in this checkout.

## Continue

Invoke `memoria review` to find the next documentation action.
