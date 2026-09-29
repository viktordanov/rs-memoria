# Memoria command reference

This reference describes Memoria commands, their arguments, and their effects on documentation state.

Each command either reports project information or changes a specific part of the project.
A human or agent remains responsible for the prose.

Read the [workflow](workflow.md) for a first review with commands in task order.

## On this page

- [Terms, invocation, and paths](#terms-used-in-this-reference)
- [Configuration and document scopes](#configuration-and-document-scopes)
- [Inspection commands and review artifacts](#inspection-commands)
- [Mutations, agent packages, and integrations](#review-mutations)
- [Diagnostics, exits, and limits](#json-and-diagnostics)

## Terms used in this reference

A tracked document is a `README.md`, or another selected Markdown file with a Memoria marker outside code.
Its scope is the selected sources in its own folder and below, minus the subfolders it hands off.
A handoff is a link or an import from a document to a tracked document in a strict subfolder.
An export is a marked section that another document can copy.
An import declares the managed copy of that section.
The supplier is the provider, and the recipient is the consumer.

A review manifest states what one document's review must read, and why it
cannot read less. A full export adds the exact bytes of those inputs.
Both are review artifacts. Both bind the same snapshot.
A section is an optional advisory mapping from document prose to sources in its scope.
An acknowledgement records the reviewer, result, and reason that the explanation is correct.
A revision counts successful acknowledgements for one document.
An invalidation is an explicit review request with a recorded reason.
The token identifies the document, revision, inputs, and invalidations that the acknowledgement must match.

Pending means that a document requires review.
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
The `--packet`, `--save`, and `--path` arguments resolve from the process directory.
A document path names `README.md`, `*.md`, or `*.markdown`; any other path is `document_invalid` with exit 2.
Arguments must contain valid UTF-8 text.
The [CLI grammar](../src/presentation/cli.rs#L13) defines the command arguments.

## Configuration and document scopes

| File | Purpose |
| --- | --- |
| `memoria.toml` | The root configuration defines project rules. |
| `README.md` | Each README is a tracked document. |
| `*.md`, `*.markdown` with a marker | A Markdown file with an export, import, or section marker outside code is an opted-in document. |
| `README.memoria.toml` | An optional sidecar defines local rules beside a README. |
| `memoria.lock` | This generated file contains the latest review for each document and the active invalidations. |
| `.gitattributes` | The rule `/memoria.lock binary` prevents text merging and newline conversion. |

Commit `memoria.toml` and `memoria.lock` together.
Memoria owns the bytes of `memoria.lock`; do not edit it.
The write lock is not in the worktree.
It uses the path that `git rev-parse --git-path memoria/write.lock` returns, so linked worktrees receive separate locks.

Each document covers the selected sources in its own folder and below.
A nested document alone removes nothing.
A document stops covering a subfolder only when it links to or imports a tracked document strictly inside it.
No document hashes another document as an ordinary source input.
Declared imports carry the relevant export content into consumer inputs.
The [specification](specification.md#25-scope-and-handoffs) gives the complete handoff rules.

| Reference in a document | Result |
| --- | --- |
| Link or import to a tracked document in a strict subfolder | Handoff: the subfolder leaves the scope |
| Link to a same-folder, parent, or sibling document | No handoff |
| Link to unmarked Markdown, a missing path, a folder without `README.md`, or an unselected file in a subfolder | No handoff; hint `handoff_not_applied` with reason `untracked_markdown`, `missing`, `no_document_in_directory`, or `not_selected` |
| Link to a non-Markdown file, a URL, `mailto:`, or a fragment | No handoff, no hint |

A tracked document below a document that does not hand it off gets the hint `handoff_absent` on the parent.
Hints never fail a command.

The root configuration supports these defaults:

```toml
version = 3
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
Documentation guidance enters the review context without changing selection fingerprints.
A sidecar with only guidance does not add a selection scope to the fingerprint policy.
This rule also applies to a sidecar beside the root README.
An explicit invalidation requests review after a guidance change.

Guidance appends from the root scope toward the document scope.
Within each scope, inline entries come before file entries, and each list keeps its authored order.
Memoria does not override, deduplicate, or rank conflicting prose.

A `guidance_files` entry resolves relative to its declaring configuration file.
The destination must stay inside the project and hold a regular UTF-8 file.
These destinations are invalid: a symlink, a missing file, Git metadata, a state artifact, a configuration file, and a `README.md`.

Configuration version 3 is required. Version 2 fails with `configuration_invalid`, exit 1, and names the cutover: change `version = 2` to `version = 3` in `memoria.toml` and in any `README.memoria.toml` that declares a version.
A sidecar version, when present, must be 3.
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
4. Discovery turns selected Markdown with a marker into opted-in documents, and each remaining source joins the scope of every document that covers it.

Within one scope, `ignore` rules apply before `include` rules.
An include can restore a Memoria exclusion, but it cannot restore a Git exclusion.
If Git ignores a tracked path, that file remains eligible.
Selected files that no document covers cause lint and check errors (`coverage_unowned`). A root README prevents these gaps.

<details>
<summary>Reserved inputs and repository boundaries</summary>

Reserved source inputs include configuration files, READMEs, sidecars, `.gitignore` files, referenced guidance files, `memoria.lock`, `.memoria.lock.tmp.<32 hex>`, and `.memoria/**`.
The agent integration files are also reserved, whether installed or absent: `.codex/hooks.json`, `.codex/config.toml`, `.codex/memoria-hook.json`, `.claude/settings.local.json`, and `.claude/memoria-hook.json`.
Unrelated files in those directories stay ordinary inputs.
Managed Memoria packages and their transaction artifacts are also reserved.
The tool discovers neither sources nor documents within its reserved trees.

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
The current files determine the resulting scopes and local rules.

README discovery precedes Memoria ignore/include selection.
An eligible nested README remains a document even when Memoria excludes its surrounding source files.
A Memoria ignore rule does not hide a README.
Opted-in documents are discovered after selection: an unselected Markdown file is never a document.
A tracked README absent from the worktree is not a current document.
Markdown that is not valid UTF-8 stays a source, with the warning `document_encoding_invalid` when a line starts with `<!-- memoria:`.

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

### `memoria explain <DOCUMENT>`

This read-only command explains whole-file freshness for a current, pending, waiting, or never-reviewed document.
It reports changed paths, hashes, lengths, policy, guidance, imports, invalidations, and locally verified Git hunks.
It does not acquire a write lock, fetch Git objects, or acknowledge a review.
`status --explain <path>` remains the separate source-selection explanation.

The JSON result has `kind="freshness_explanation"`.
It includes state fields, `document_kind`, `scope` (files, handoffs, and incoming handoffs), `changes`, `policy`, `guidance`, `evidence`, `before_manifest`, and `current_manifest`.
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
| `memoria status [--explain <path>]` | It shows documents, handoffs, overlaps, selected bytes, review state, guidance counts, and exclusions. |
| `memoria status --summary` | It emits bounded counts only. |
| `memoria guidance [<DOCUMENT>]` | It shows the documentation guidance that applies to a document. |
| `memoria lint` | It examines configuration, coverage, markers, exports, imports, cycles, and navigation. |
| `memoria review` | It shows pending documents in dependency order and the next action. |
| `memoria check` | It requires valid structure, current reviews, and current import copies. |
| `memoria graph` | It shows documents, handoffs, overlaps, and import and navigation edges with review state. |
| `memoria state inspect [--file <path>]` | It decodes committed state and shows its framing. |

Every command in this table is read-only.
None of them creates a lock, a temporary file, or a directory.

`status` also shows active invalidations, uncovered files, disconnected documents, and repository boundaries.
Its `data` holds `documents`, `readmes`, `opted_in_documents`, `handoffs`, and `overlapping_sources`, and each document entry holds `document_kind`, `scope_files`, and `handoffs`.
The human view starts with "Documents N: M READMEs, K opted-in documents; H handoffs; S sources covered by more than one document".

`status --explain <path>` names the rule chain for one path.
For a selected source it adds `covered_by`, the documents whose scope contains it, and `handed_off`, each handoff `{by, to, subtree, via, line}` whose subtree contains it.
For a tracked document the outcome is `document`, and `document` holds its `kind`, `scope_files`, `handoffs`, and `handed_off_by`.
A normal Markdown link supplies navigation without creating a review dependency, except a link to a tracked document in a strict subfolder, which is a handoff.
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

Its `data` object holds `documents`, `readmes`, `opted_in_documents`, `handoffs`, `overlapping_sources`, `selected_files`, `selected_bytes`, the four review counts, guidance counts, the `unowned` (uncovered) and disconnected counts, invalidation counts, and diagnostic counts.
Waiting can overlap current, pending, or never-reviewed status.
These counters are not disjoint categories.

### `memoria guidance [<DOCUMENT>]`

Without a path, the command shows the effective guidance of the root README.
It also lists each scope that adds guidance, with the command that inspects that scope.
With a path, it shows that document's entries, exact sources, digest, and applicable scope.
A document's guidance comes from the configuration scopes in its folder and above it.

The command works for current documents.
It needs no review artifact, and it changes no state.

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
It reads lock formats 2 and 3 and reports `format_version`.
Each record in `data.state.reviews` has `coverage_evidence`: `null` when the record has no evidence, or the sorted list of folders that the acknowledged scope handed off.
The human view shows `coverage evidence: auth/, docs/`, `coverage evidence: none handed off`, or `coverage evidence: not recorded`.
The [state guide](state.md) describes the format, the limits, and the recovery procedure.

The review plan orders providers before consumers, with path order for ties.
`data.next_ready` names the first ready document.
`data.next_action` names the required `render` or `review` action for that document.
If its imports are outdated, a ready document still requires `render` before a review.
Each pending task adds `document_kind`, `scope_files`, and `co_covering`, the other documents that cover a source that changed for it.
The human plan starts with "Review plan: N pending documents (dependency order)" and gives one line per document with its kind, its causes, and whether it is ready.

Source evidence: [plan](../crates/memoria-application/src/usecases/plan.rs#L43), [lint](../crates/memoria-application/src/usecases/lint.rs#L26), and [check](../crates/memoria-application/src/usecases/check.rs#L37).

## Review requirements

```sh
memoria review <DOCUMENT> [--max-bytes <n>] [--full] [--save <DIR>] [--details] [--format json]
```

A review requires a pending document. Its providers must permit review, and
its import copies must be current.

The default result is a review manifest. The manifest states what the reviewer
must read and why the reviewer cannot read less. It contains no document body,
no source body, no import body, no historical content, and no guidance prose.
Read the listed paths with ordinary file tools.

The manifest (`manifest_version` 2) reports the document and its kind, its
scope and handoffs, the review revision, the token, the bound snapshot
digests, the previous review, the changed inputs with their relationships,
the review mode, the suggested sections, the suggested reads, the downstream
consumers and co-covering documents, the guidance references, the covered
invalidations, bounded counts, and the built-in workflow steps.

The human view is change-first:

1. A header: "Review auth/flows.md — opted-in document, pending since revision 1".
2. The scope: its size and each handoff with its kind and line.
3. The baseline: revision, reviewer, and result.
4. "What changed since that review", one line per change with its relationship.
5. Semantic review requests.
6. "Also pending for the same changes": co-covering documents, at most 10 lines.
7. "Downstream": export consumers, at most 10 lines.
8. "How to read": the mode and its reasons, suggested sections, the reads, the whole-document pass, and the guidance command.
9. "Next": read and edit, save a fresh artifact, and acknowledge.
10. The line "Details: memoria review DOCUMENT --details".

`--details` adds the token, the digests, per-input sizes and hashes, and counts to the human view.

### A complete manifest

This is one captured `review_manifest` envelope, exactly as the built
executable wrote it for the [worked example](../README.md#scope-and-handoffs)
of change A: `auth/login.rs` changed after every document was acknowledged.
The document is the opted-in `auth/flows.md`, whose section `login` maps that
source, so the review is a focused candidate with one suggested section.
`auth/README.md` covers the same folder, so it appears as a co-covering
document.

<!-- documented-manifest-example -->
```json
{
  "command": "review",
  "data": {
    "artifact_digest": "b7d5c9162fbc2540",
    "baseline": {
      "evidence_status": "verified",
      "recorded_commit": "ba1e8da0b5856b7050cdeda4788566d21cdd8ab5",
      "result": "no-update",
      "reviewer": "fixture",
      "revision": 1,
      "token_digest": "4f9ff5808cbf17ff"
    },
    "changes": [
      {
        "after_bytes": 19,
        "after_hash": "d34917e84a73a8f6",
        "before_bytes": 14,
        "before_hash": "3e66e2ce1ec9dd61",
        "change": "changed",
        "identity": "auth/login.rs",
        "kind": "file",
        "relationship": {
          "also_covered_by_total": 1,
          "export_id": null,
          "kind": "scope_source",
          "provider": null,
          "sections": [
            "login"
          ],
          "unrecorded_reason": null
        }
      }
    ],
    "counts": {
      "handoffs": 0,
      "imports": 0,
      "raw_input_bytes": 159,
      "scope_files": 3,
      "suggested_sources": 1
    },
    "covered_invalidations": [],
    "document": "auth/flows.md",
    "document_kind": "opted_in",
    "downstream": {
      "co_covering": [
        {
          "document": "auth/README.md",
          "document_kind": "readme",
          "status": "pending"
        }
      ],
      "co_covering_total": 1,
      "consumers": [],
      "consumers_total": 0
    },
    "guidance": {
      "changed_since_review": false,
      "command": [
        "memoria",
        "guidance",
        "auth/flows.md"
      ],
      "digest": "bb97f9223e4cba99",
      "references": []
    },
    "inputs": [
      {
        "bytes": 114,
        "export_id": null,
        "hash": "68db31dcdffd1432",
        "kind": "document",
        "path": "auth/flows.md",
        "role": "whole_document"
      },
      {
        "bytes": 19,
        "export_id": null,
        "hash": "d34917e84a73a8f6",
        "kind": "file",
        "path": "auth/login.rs",
        "role": "changed_source"
      }
    ],
    "kind": "review_manifest",
    "manifest_version": 2,
    "review": {
      "fallback_reasons": [],
      "mode": "focused_candidate",
      "sections": [
        {
          "heading": "Login",
          "id": "login",
          "lines": [
            4,
            6
          ],
          "sources": [
            "auth/login.rs"
          ]
        }
      ],
      "whole_document_pass": true
    },
    "review_revision": 1,
    "scope": {
      "files": 3,
      "handed_off_by": [],
      "handed_off_by_total": 0,
      "handoffs": [],
      "handoffs_total": 0
    },
    "snapshot": {
      "baseline_digest": "4b63f8474376f0c9",
      "context_digest": "ac6f9a256466da33",
      "guidance_digest": "bb97f9223e4cba99",
      "inputs_digest": "659dabed259ca916",
      "selection_version": 2
    },
    "token": "mrv3.a5d2ebb85aefb2fe",
    "workflow": {
      "policy": "section-review-v2",
      "steps": [
        "Read current guidance and covered reasons.",
        "Inspect the changes, their relationships, and suggested sections.",
        "Expand uncertain context or use the full baseline.",
        "Read the whole document.",
        "Capture a fresh artifact after edits and reconcile before acknowledgement."
      ]
    }
  },
  "diagnostics": [
    {
      "code": "handoff_absent",
      "column": null,
      "details": {
        "subtree": "legacy",
        "target": "legacy/README.md"
      },
      "line": null,
      "message": "legacy/README.md is a tracked document inside this document's folder, but this document neither links to it nor imports it, so both documents cover legacy/ and both are reviewed for changes there. Link to legacy/README.md to hand that folder off, or keep both reviews",
      "path": "README.md",
      "severity": "hint"
    },
    {
      "code": "missing_import_hint",
      "column": null,
      "details": {
        "target": "auth/README.md"
      },
      "line": null,
      "message": "normal link to auth/README.md has no matching import; add an import if its summary belongs here",
      "path": "README.md",
      "severity": "hint"
    },
    {
      "code": "navigation_disconnected",
      "column": null,
      "details": {
        "scope_files": 3
      },
      "line": null,
      "message": "no link or import path from the root README reaches this document",
      "path": "auth/flows.md",
      "severity": "warning"
    },
    {
      "code": "navigation_disconnected",
      "column": null,
      "details": {
        "scope_files": 1
      },
      "line": null,
      "message": "no link or import path from the root README reaches this document",
      "path": "legacy/README.md",
      "severity": "warning"
    }
  ],
  "ok": true,
  "schema_version": 3
}
```

The test suite regenerates the same fixture with the built executable and
checks that the production decoder accepts these exact bytes. A fresh run
produces different digests, a different token, and a different recorded
commit, because those bind the exact snapshot and the exact prior review.
This example is a fixture capture. Copying it acknowledges nothing.

Reading the envelope:

| Field | What the capture shows |
| --- | --- |
| `schema_version`, `kind`, `manifest_version` | `3`, `review_manifest`, `2`. This release accepts these values only. |
| `document`, `document_kind`, `scope` | `auth/flows.md`, `opted_in`, and its scope of three sources with no handoffs. |
| `token`, `artifact_digest` | The snapshot token, and the integrity digest over the envelope without that field. |
| `snapshot` | The four recorded digests, and `selection_version: 2`. The token binds three of them directly, and `context_digest` binds `guidance_digest`. |
| `baseline` | The prior review, with `evidence_status: verified`: the reviewed bytes were recoverable. |
| `changes` | One changed source, with both sides' lengths and hashes, and its relationship: a scope source that section `login` describes, also covered by one other document. |
| `review` | `focused_candidate` with no fallback reasons, one suggested section, and the always-required whole-document pass. |
| `inputs` | The document with role `whole_document`, and the changed source with role `changed_source`. Advice, not the complete inventory. |
| `downstream` | No export consumers, and `auth/README.md` as the one co-covering document. |
| `guidance` | The effective digest, the references, and the command that prints the text. No authored prose. |
| `counts` | The complete scope: three files, no imports, the raw input bytes, and no handoffs. |
| `workflow` | The built-in policy `section-review-v2` and its five fixed steps. |

An older example is regenerated, never migrated. Run `memoria review` again.

### Advisory sections

Any tracked document can map one part of its prose to the sources it describes:

```markdown
<!-- memoria:section id="persistence" files="handle.go service.go" -->
## Saving and synchronizing

Save writes a local archive. Sync also uploads the archive.
<!-- /memoria:section -->
```

Both markers sit at column zero. The attribute order is fixed. Exactly one
space separates two paths. The identifier matches
`[A-Za-z][A-Za-z0-9_-]{0,63}` and stays unique inside one document. Each path
is literal, relative to the document's folder, and must name a selected
regular source file in the document's scope: not a tracked document, not a
handed-off file. The body must open with a Markdown heading.

Sections do not nest. An export or an import can sit wholly inside a section.
A section cannot sit inside or cross an export or an import. Marker text
inside fenced or indented code is inert.

A section is advice. It adds or removes no input and has no separate
freshness. One invalid mapping withdraws the advice of the whole document,
because partial
advice cannot narrow a review. Memoria then reports `section_mapping_invalid`
as a warning with the path, the line, and a precise reason. `lint` does not
fail because of section advice alone. `review` selects the full baseline.

The [specification](specification.md#461-map-document-sections-to-sources) gives
the complete grammar and every rejection rule.

### Review mode and fallback reasons

`data.review.mode` is `focused_candidate` or `full_baseline`.

`focused_candidate` means technical eligibility only. It does not certify the
previous review. Without explicit trust in that review, use the full baseline.

`full_baseline` means all current scope sources, all current import bodies,
the whole document, the effective guidance, and every active covered reason.
`data.review.fallback_reasons` gives one entry for each cause.

| Code | Cause |
| --- | --- |
| `unmapped_change` | No valid section describes a changed source. |
| `path_set_changed` | A source entered or left the scope. A rename appears as both. |
| `baseline_missing` | The document has no previous review. |
| `baseline_unavailable` | The reviewed bytes could not be verified. The message names the history reason. |
| `mapping_invalid` | The document's section mappings are unusable. |
| `mapping_changed` | The section associations changed since the last review. |
| `handoff_changed` | A handoff appeared or disappeared, so a subtree left or entered the scope. The identity is the subtree. |
| `coverage_unrecorded` | A source entered the scope, and the last review of this document did not record its handed-off folders. The identity is the source path. |
| `document_classification_changed` | A Markdown file gained its first marker or lost its last, so it left or joined the scope. |
| `policy_changed` | The effective selection policy changed. |
| `imports_changed` | An imported contract changed. |
| `semantic_invalidation` | An explicit reason requires a semantic review. |
| `guidance_changed` | The effective guidance changed since the last review. |

`data.review.whole_document_pass` is always `true`. A suggested reading list
never replaces the whole-document pass.

### Relationships and downstream

Each entry in `data.changes` has a `relationship`:

| `kind` | Meaning |
| --- | --- |
| `own_text` | The document's own text changed. |
| `scope_source` | A source in the scope changed. `sections` names this document's valid sections that map it. |
| `handoff` | A source entered or left the scope because a handoff changed. |
| `coverage_unrecorded` | A source entered the scope, and the former coverage is not recorded. `unrecorded_reason` gives the reason. |
| `import` | An imported export body changed. `provider` and `export_id` name it. |
| `selection_policy` | The effective selection policy changed. |

`also_covered_by_total` counts the other documents that cover the same source.
`unrecorded_reason` is `null` except for `coverage_unrecorded`.

### Coverage unrecorded

`memoria ack` records the folders that the document's scope handed off. A
later review uses that record to classify a source that entered the scope:
inside a recorded folder, the result is `handoff_changed` with the shallowest
such folder. Otherwise, it is `path_set_changed`.

A record from lock format 2 (Memoria 0.6, or an earlier 0.7 build) has no
such record. For it, Memoria tries to rebuild the recorded token from each
subset of candidate folders. An exact match proves the former handoffs. The
proof exists only if all of these conditions are true. Memoria examines them
in this order, and the first one that fails is the reason:

| Condition | `unrecorded_reason` when it fails |
| --- | --- |
| The record is the document's first review. | `revision_not_first` |
| The review acknowledged no invalidation. | `acknowledged_invalidations` |
| The previous document text is available. | `previous_text_unavailable` |
| Each layout tried has at most 12 candidates. | `candidate_limit` |
| One subset reproduces the recorded token digest. | `no_matching_reconstruction` |

Each layout costs at most 4,096 token computations, so at most 8,192 for one
document. The proof runs only for a record without evidence, and only when a
source entered its scope.

If the proof is not available, the review reports `coverage_unrecorded` for
each added source, with this message: "`<path>` entered this document's
scope. This document was last reviewed by a release that did not record
handed-off folders, so Memoria cannot tell whether a handoff ended or the
source is new (`<reason>`). Review the complete current scope. The next
acknowledgement records the coverage." The human phrase is "entered the scope;
former coverage not recorded (<reason>)". The mode is `full_baseline`, as for
every fallback.

`data.scope` holds `files`, `handoffs[] {subtree, target, via, line}`, and
`handed_off_by[] {parent, via, line}`. `data.downstream` holds
`consumers[] {export_id, consumer, consumer_kind, status,
waits_for_this_document}` and `co_covering[] {document, document_kind,
status}`. Each list holds at most 64 entries, with a `*_total` beside it. The
human view shows 10 lines, then "and N more; `memoria graph` lists all".
Downstream is shown for judgment. It is not bound into the token, and it is
not a semantic-change detector.

### Suggested reads

`data.inputs` lists identities, not the complete scope inventory. Each
entry has a `role`: `whole_document`, `changed_source`, `section_context`, or
`current_import`. For the complete inventory, invoke `memoria status` or
produce a full export.

The line range in `data.review.sections[].lines` is a 1-based inclusive hint
for the current document bytes. The complete document hash binds every byte.

### Full export

```sh
memoria review <DOCUMENT> --full --format json
```

`--full` produces the complete offline export instead of the manifest. The
export contains the document under `data.content.document`, every scope file, imported exports, previous
review, documentation guidance, and covered invalidations. It also identifies
changed inputs, exports, consumers, and available diffs. Text content uses
UTF-8, and other content uses base64.

The export repeats the manifest under `data.requirements`. It adds
`data.binding`, which holds the canonical context descriptors and the prior
record. The prior record in `data.binding.baseline` includes its
`coverage_evidence`, because the baseline encoding binds it. A reader can
recompute the token from the file alone.

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

### Versions and transport

Every CLI JSON envelope uses schema version 3. The manifest uses
`kind: "review_manifest"` and `manifest_version: 2`. The full export uses
`kind: "focused_review"` and `packet_version: 4`. `packet view` uses
`view_version: 2`. The token contains 21 bytes with the form `mrv3.<16 hex>`.

Memoria accepts the current versions only. A Memoria 0.6 artifact
(`manifest_version: 1` or `packet_version: 3`) is refused with
`packet_schema_invalid` and the text "…`manifest_version` is 1; this release
accepts 2 only. Run `memoria review PATH --format json` again…". Memoria
converts nothing and upgrades nothing. Review artifacts are disposable.
Produce a new one.

### Saved artifacts: `--save <DIR>`

`memoria review <DOCUMENT> --save <DIR>` writes the JSON artifact into `DIR`.
It works with or without `--full`, and it requires a document: without one it
fails with `save_requires_document`, exit 2.

The command validates the arguments, builds the snapshot, and encodes the
artifact under the hard limits before it checks the destination, so a limit
refusal writes nothing. `DIR` must exist and resolve to a directory
(`save_destination_invalid`, exit 2). Its canonical path must not equal or lie
under the canonical Git worktree root, including ignored folders and `.git`
(`save_destination_in_project`, exit 2, with a `mktemp -d` hint). This guards
against accidental self-invalidation; it is not a security boundary.

The file name is `memoria-<manifest|full>-<slug>-<hex16>.json`. The slug
replaces every byte outside `[A-Za-z0-9._-]` with `_`, capped at 96 bytes, and
`hex16` is the token's hexadecimal part. An existing name takes a suffix `-2`
to `-99`; after that the command fails with `save_name_exhausted`, exit 3.
The file is created exclusively with mode 0600, never follows a symlink at the
final name, and is synced before close. On a write error the file is removed
and the command fails with `save_failed`, exit 4; `details.leftover` names a
file that could not be removed. A crash can leave a truncated file, which
`ack` rejects.

The saved bytes are exactly the bytes that `--format json` prints for the same
snapshot. `review` stays read-only for the project: no lock and no state.
With `--save`, JSON output is a receipt: `{kind: "saved_review_artifact",
path, artifact_kind, bytes, document, document_kind, review_revision, token,
artifact_digest, mode, ack_command}`. Human output is the review view, then
`Saved: <path>` and the `ack` command. A receipt passed to `ack` fails with
`packet_schema_invalid`.

Both artifacts acknowledge from a file or from stdin. Human output supports
reading only. The [workflow](workflow.md#2-save-the-artifact) stores the
artifact outside the project.

### Limits

| Limit | Value | Applies to |
| --- | --- | --- |
| Default raw inputs | 8 MiB | Both artifacts |
| Maximum raw inputs through `--max-bytes` | 32 MiB | Both artifacts |
| Serialized output | 64 MiB | Both artifacts |
| Array elements and nesting | 100,000 elements and 32 containers | Both artifacts |
| Decoded content | 32 MiB | Full export |

Raw inputs contain the document, its scope files, and imported export bodies.
A manifest does not remove the input-collection limits. It also does not imply
constant-memory snapshot building.

Decoded content counts the transported bytes: current content, guidance text,
available old content, and generated diff text. Only the full export carries
those bytes, so only the full export can exceed that limit. Large guidance
produces an explicit limit error on the export. It never disappears through
silent truncation.

A manifest stays proportional to the number of changes, not to the size of the
scope. Required entries are never truncated.

`size.record_count` counts array elements across the entire full-export
envelope and its diagnostics. The producer encodes the artifact before either
output format can use it. Acknowledgement recomputes the counts and rejects
inconsistent values.

<details>
<summary>Refusals and unavailable history</summary>

A size refusal returns `packet_too_large` without a token and without a
partial manifest.
If the refusal envelope fits the hard limits, it contains the full input manifest.
Otherwise, `data.manifest_omitted` is true and `data.manifest_summary` contains bounded counts and hashes.
The diagnostic supplies the same counts in human and JSON output.
The tool does not omit required content to force a successful artifact within a limit.

Old file content comes from a verified local commit.
The bounded lookup examines the previous reference first.
The content must match the reviewed length and hash before it can supply a diff.
Without that match, the review marks the evidence unavailable and supplies current content.
A manifest reports the same fact as the `baseline_unavailable` fallback reason
and as `data.baseline.evidence_status`.
The state does not store old export bodies.
The bounded lookup can recover matching export bodies from historical provider documents.
A rejected acknowledgement can compare imports against the bytes in a supplied
full export. A manifest carries no old bytes, so its conflict names the changed
digest categories instead of exact hunks.

Source evidence: [packet construction](../crates/memoria-application/src/usecases/prepare_review.rs#L105) and [packet codec](../crates/memoria-infrastructure/src/packet.rs#L268).

</details>

## Saved export views

The [method and results](development/review-context.md) report the observed
reading costs and their limits.

The default human review shows the baseline, the changes, the review mode, the
fallback reasons, the suggested reads, and the next command. It shows no file
content and no hunks.
`memoria review README.md --details` adds tokens, digests, and per-input sizes and hashes.
`memoria review README.md --full` shows the detailed human export.
`memoria explain README.md --full` shows the detailed human explanation with
verified hunks.
Both output formats follow `--full`. The default is the manifest in human and
JSON output alike.
The human view never relaxes the artifact limits.

1. Save the full export outside the project:

   ```sh
   dir=$(mktemp -d)
   memoria review README.md --full --save "$dir" --format json
   ```

2. Invoke the saved-export reader:

   ```sh
   memoria packet view "$dir"/memoria-full-README.md-*.json
   memoria packet view "$dir"/memoria-full-README.md-*.json --section guidance
   memoria packet view "$dir"/memoria-full-README.md-*.json --file src/example.rs --format json
   ```

The reader requires no project discovery and makes no writes.
`-` selects stdin.
It validates the full export before selection.
It accepts a current full export only. A manifest carries no content, so the
reader reports `packet_content_unavailable` with exit 2. The message names the
two ways to read the content: ordinary file tools, or `review --full`.
The selected bodies come from that snapshot, even when the live files change.
File bodies retain their declared UTF-8 or base64 encoding.
A missing path causes `packet_view_file_missing` with exit 2.

| Selection | Contents |
| --- | --- |
| `summary` (default), `changes` | Changes, hunks, missing-evidence reasons, scope counts, and a guidance cue |
| `guidance`, `content` | Complete guidance or the current document, files, and imports |
| `history`, `inventory` | Previous review and diffs, or the current manifest |
| `--file PATH` | The current document or one scope file from the export |
| `incremental` | Experimental P1 preparation, with explicit coverage and fallback reasons |

`--file` and an explicit `--section` are mutually exclusive.
Imports remain accessible through `content` or `incremental`.
JSON views use envelope schema 3, `data.kind=packet_view`, and `data.view_version=2`.
The document body is under the key `document`.
They carry `canonical=false`, `snapshot_token`, and `source_packet_digest`.
They cannot substitute for the canonical acknowledgement artifact.
New view fields do not change the export schema or its decoder.
A view that exceeds 64 MiB in human or JSON rendering causes `packet_view_limit_exceeded` with exit 1.
The reader refuses that result instead of truncating it.

Legacy P1 projection requires explicit owner approval of reliance on a trusted
prior review. The review mode in the manifest governs the required scope. P1
never reduces that scope.

```sh
memoria packet view "$dir"/memoria-full-README.md-*.json --section incremental --trust-prior-review --format json
```

Without that trust flag, preparation requires full review.
The flag has no meaning for other sections and causes `packet_view_trust_invalid` with exit 2.
An unknown section causes `packet_view_section_invalid` with exit 2.
The [shipped skill](../skills/memoria/SKILL.md) defines trust, context retrieval, coverage, and full-review fallback.
`model_quality_gate_passed=false` remains explicit.
No projection records an inspection or approves an input.
Memoria states no target for token savings and no bound on missed
documentation changes.

## Historical coverage

Acknowledgement supplies a `historical_coverage` hint after a successful state save.
The hint reports `verified`, `partial`, or `unavailable`, plus counts and budget status.
Coverage includes the document, its scope files, and imported export bodies at one commit.
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
It never creates README prose, exports, imports, or nested documents.
The generated configuration declares `version = 3` and states the rule: each document covers its folder and below; link or import a document in a subfolder to hand that subfolder to it.

Apply validates existing files first.
An invalid existing configuration, README, or state prevents initialization.
An existing valid installation produces no changes, and existing files keep their bytes and modes.
A partial write failure still reports exactly which files reached disk.

CAUTION: An existing unrelated `memoria.lock` is a conflict to resolve by hand.
Initialization never overwrites a file because its name matches.

The generated configuration holds an empty guidance list and useful comments.
It imposes no writing standard on your project.

### `memoria render [<DOCUMENT>] [--dry-run]`

The command replaces only declared import bodies with current provider export bytes.
It preserves the other document bytes and does not change review state.
Each document replacement is atomic.
A second invocation with identical inputs makes no writes.
Invalid imports or cycles prevent the operation.

After a partial write failure, `render_incomplete` identifies the remaining documents.
The command does not treat several document replacements as one transaction.
The dry run describes the changes without applying them.

### `memoria invalidate <scope> --reason <text>`

The scope is `all`, `doc:<DOCUMENT>`, or `subtree:<directory>`.
The command captures the currently tracked documents in that scope; `subtree:` selects documents by folder.
It records the reason without changing their text.
An empty scope causes a usage error.
The reason appears in status, the plan, and each affected review artifact.

### `memoria ack`

```sh
memoria ack <DOCUMENT> --packet <file|-> [--token <token>] \
  --reviewer <name> --result updated|no-update --note <text>
```

`--packet` accepts a review manifest or a full export, usually the file that
`review --save` wrote. Both bind the same snapshot and carry the same token.

Without `--token`, the token comes from the decoded, integrity-checked
artifact. With `--token`, the value must equal the artifact token
(`token_mismatch`, exit 2). Either way, the complete token is recomputed from
the repository under the write lock. The report adds `token_source`:
`artifact` or `argument`.

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
The note explains why this document is correct for this snapshot.
It is not an instruction, an override, or proof that the reviewer read every input.
Reviewer and note validation precede artifact reads and state mutation.
The `--packet -` argument selects stdin.
Without that argument, acknowledgement does not read stdin.

Acknowledgement has five stages:

1. The application validates arguments and decodes the artifact.
2. It validates the artifact digest, and for a full export the content hashes
   and the self-contained token, before the write lock.
3. Under the lock, it rebuilds the complete input manifest and the complete
   review context, then recomputes the v3 token.
4. The domain compares the review revision and the covered invalidations.
5. A final rebuild and token recomputation precede the atomic state save.

Stage 3 never validates only `data.inputs`, the suggested sections, or the
changed files. A small manifest narrows the reading list. It never narrows the
validated state.

The review context binds more than the input manifest. It binds the document
kind and its sorted handoffs, the nested repositories in its covered folders,
the effective selection policy and its scope path set, the
section mapping associations, the effective guidance digest, and the
transitive provider closure. A provider edit can invalidate a consumer token
even when the imported export body stays equal. That edit does not make the
consumer stale by itself.

Changed documentation guidance causes `guidance_changed` with exit 3.
That conflict writes no review state and does not make a current document stale.
The application compares current guidance under the write lock and again before the state save.

A changed input manifest causes `snapshot_changed` with exit 3.
A full export supplies exact per-input differences, because it carries the
reviewed bytes. A manifest names the changed digest categories under
`details.changed` instead. It promises no packet-time hunks it never carried.
A deleted document in an otherwise valid project causes the same conflict.
A later document revision causes `revision_conflict` with exit 3.
A pending provider causes `dependencies_pending` with exit 3.
Adding or removing a handoff, or a handoff target that appears, disappears, or
stops being tracked, changes the context, so an outstanding artifact causes
`snapshot_changed`.
These conflicts leave the stored review unchanged.

The final comparison reports changed inputs before provider readiness errors.
Only covered invalidations clear for this document.
Newer invalidations remain pending and cause `still_pending: true` in a successful acknowledgement.
The state contains the latest review per document, rather than an append-only journal.
Acknowledging one document never clears another, even one that covers the same sources.

Source evidence: [acknowledgement](../crates/memoria-application/src/usecases/ack.rs#L193), [render](../crates/memoria-application/src/usecases/render.rs#L114), and [invalidation](../crates/memoria-application/src/usecases/invalidate.rs#L50).

## Integrations

```sh
memoria integrations skill  install|status|upgrade|uninstall
memoria integrations hook   install|status|uninstall|run
memoria integrations github install|status|upgrade|uninstall
```

The umbrella groups the three integrations. The [integrations guide](integrations.md) maps the branches and their older names.

The skill and hook branches are the established `memoria agent ...` commands under a new name. They take the same arguments, produce the same data, return the same diagnostics, and exit with the same status. The JSON envelope of an equivalent command reports the established `agent ...` label under both spellings.

There is no `memoria integrations agent` level and no `memoria integrations skill hook` path. An installed hook launcher keeps the `memoria agent hook run` text, so an existing installation needs no migration.

## Agent packages

```sh
memoria agent install|status|upgrade|uninstall [--target codex|claude] [--scope local|global] [--path <skills-directory>] [--replace-existing] [--dry-run]
memoria agent hook install|status|uninstall --target codex|claude [--dry-run]
memoria agent hook run --target codex|claude --protocol 1 --configuration-root <directory>
```

The binary embeds the four files of `skills/memoria/` at build time: `SKILL.md`, `review-details.md`, `saved-exports.md`, and `integrations.md`. One body serves Claude Code and Codex.
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

The package contains those four files and `.memoria-install.json`, at record schema 2, with a hash for each file.
A package is `current` only when its version, its file names, and every file's bytes match this executable.
An upgrade replaces exactly the files that the previous record names, so a single-file 0.6 package upgrades cleanly.
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

## GitHub workflow

```sh
memoria integrations github install   [--path <file>] [--version <version>] [--action-ref <ref>] [--runner <label>] [--apply | --dry-run]
memoria integrations github status    [--path <file>]
memoria integrations github upgrade   [--path <file>] [--version <version>] [--action-ref <ref>] [--runner <label>] [--apply | --dry-run]
memoria integrations github uninstall [--path <file>] [--apply | --dry-run]
```

This branch creates and maintains one consumer workflow file. The workflow calls the first-party setup Action, which installs a verified prebuilt executable on the runner. The [GitHub Actions guide](github-actions.md) describes both parts.

The default destination is `.github/workflows/memoria.yml`. A custom `--path` must name one direct `.yml` or `.yaml` child of `.github/workflows`. Memoria rejects an absolute path, a traversal, a control character, a symlink ancestor, a symlink destination, and a special file.

Install, upgrade, and uninstall preview the change and write nothing without `--apply`. The option `--dry-run` selects the same preview. `--apply` and `--dry-run` together are an argument error. A preview creates no directory, no lock, no temporary file, and no record. An apply recomputes its plan from the current bytes, so an earlier preview never permits an overwrite of a later edit.

`status` is always read-only and rejects `--apply`, `--dry-run`, `--version`, `--action-ref`, and `--runner`.

Install and upgrade need an initialized project, because the generated job runs `memoria check`. The requirements are a regular root `README.md`, a `memoria.toml` that parses, and a readable `memoria.lock`. A preview works without them and reports each one as `github_prerequisite_missing`. An apply refuses with `github_prerequisites_missing` and exit 1, before the store takes a lock, recovers a transaction, or writes anything. A pending review is permitted: review follows the workflow change. Memoria never initializes a project and never acknowledges a review. Status and uninstall do not read the configuration at all, so they work in an uninitialized or broken project.

Memoria records what it wrote in `.github/memoria-workflows/<filename>.json`. Both files belong in the consumer's Git history. Memoria owns a workflow only when a valid record names it. It never adopts a file without that record, even when the bytes match the current template.

| State | Install apply | Upgrade apply | Uninstall apply |
| --- | --- | --- | --- |
| `absent` | Creates both files | `github_not_installed`, exit 1 | No change, exit 0 |
| `current` | No change, exit 0 | No change, exit 0 | Removes both owned files |
| `outdated` | `github_upgrade_required`, exit 1 | Rewrites both files | Removes both owned files |
| `modified` | Preserves and fails | Preserves and fails | Preserves and fails |
| `unmanaged` | Preserves and fails | Preserves and fails | Preserves and fails |
| `conflict` | Preserves and fails | Preserves and fails | Preserves and fails |

`--version` accepts an exact stable version, 0.7.0 or later, with an optional leading `v`: configuration version 3 needs Memoria 0.7.0. A recorded older pin stays readable, so `upgrade` can replace it. `--action-ref` accepts a full 40-character commit SHA or an exact `vX.Y.Z` tag. `--runner` accepts `ubuntu-24.04`, `ubuntu-latest`, and `ubuntu-24.04-arm`. An upgrade keeps the recorded runner and a recorded commit pin unless you change them explicitly, and it refuses a downgrade.

The diagnostic codes are `github_not_installed`, `github_upgrade_required`, `github_prerequisites_missing`, `github_unmanaged`, `github_modified`, `github_ownership_conflict`, `github_destination_appeared`, `github_recovery_needed`, `github_recovery_pending`, `github_recovery_unavailable`, `github_busy`, `github_downgrade_refused`, `github_path_unsafe`, and `github_template_unsupported`. The preview also emits the warning `github_prerequisite_missing` and the hints `github_sibling_workflows` and `github_publication_unverified`.

The workflow file and its ownership record are ordinary documentation inputs. A new workflow makes the documents whose scope contains it pending, so review follows the change. Memoria acknowledges nothing automatically.

<details>
<summary>Durable writes and recovery</summary>

A mutation takes a private advisory lock under the worktree's Git metadata, keyed by the destination file name. It then records the expected and the intended bytes of both files in a durable intent record.

The mutation moves each file it replaces into private recovery storage before the replacement appears. The move is an atomic rename on the same filesystem. Memoria refuses the mutation when the recovery directory is on another filesystem. After the move, Memoria compares the displaced bytes with the expected bytes. If they differ, Memoria puts the file back and reports `github_modified`.

The restoration is itself no-clobber. A test for absence followed by a rename would leave a window in which another writer creates the destination and the rename destroys it. Memoria therefore restores with a hard link, which fails when the destination exists. If a file appeared there, both byte sequences survive: the appearing file stays at its path, the displaced bytes stay in recovery storage, and `github_recovery_pending` names both paths.

Creation uses no-clobber semantics. A file that appears between the plan and the write causes `github_destination_appeared` instead of an overwrite.

An interrupted mutation leaves its intent record. Every apply settles that record first, including an apply whose files already match the target. The apply recognizes two states: the expected bytes, which mean that nothing started, and the intended bytes, which mean that the change finished. Any third state preserves both files and reports `github_recovery_needed`. An intent record that does not decode, or that names another workflow or an unknown operation, is evidence rather than a transaction: Memoria preserves it byte for byte and refuses the change. A preview and a status never settle a record.

The advisory lock serializes Memoria writers only. It does not stop an arbitrary editor. Displacement exists for that case: an editor that holds the old file keeps writing into the copy that Memoria preserved.

Source evidence: [workflow adapter](../crates/memoria-infrastructure/src/github_workflow.rs#L1).

</details>

## JSON and diagnostics

Every JSON response uses this envelope:

```json
{"schema_version": 3, "command": "status", "ok": true, "data": {}, "diagnostics": []}
```

This release moves every envelope to schema version 3 in one cutover.
The native hook runner is the one exception.
It speaks native hook JSON on stdin and stdout, and it does not accept `--format`.
Its protocol version is separate and stays unchanged.

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
| 2 | Arguments, review text, token, artifact validation, or a `--save` destination failed. |
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
review_not_pending dependencies_pending document_not_found document_invalid
document_encoding_invalid section_mapping_invalid handoff_not_applied handoff_absent
save_requires_document save_destination_invalid save_destination_in_project
save_name_exhausted save_failed packet_too_large max_bytes_invalid
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

The canonical domains are `memoria.policy.v2`, `memoria.inputs.v2`, `memoria.guidance.v1`, `memoria-review-baseline-v2`, `memoria-review-context-v2`, `memoria-review-token-v3`, and the integrity domains `memoria.packet.v3` and `memoria.review-manifest.v1`.
The selection tag is `git-worktree-v2`, and the repository ignore inventory is `repository-ignore-v1`.
The policy encoding also holds the identifier `nearest-readme-v1`. It is a frozen hash-domain identifier: its name is historical, and renaming it would change every policy hash.
The selection version is 2, with the section workflow policy `section-review-v2`.

The v3 token combines three digests with four more encoded values:

| Symbol | Contents |
| --- | --- |
| `I` | The digest of the complete input manifest. |
| `B` | The digest of the complete prior review record, or of its absence. |
| `C` | The digest of the review context: document kind, handoffs, selection, mapping, guidance, and the provider closure. |
| `T` | The encoded bytes: the domain string, the document path, the review revision, `I`, `B`, `C`, and the covered invalidations. |

The token is `mrv3.` and the 16 hexadecimal digits of the hash of `T`.

`C` binds the document identity and kind, its sorted handoffs
`(subtree, target)`, the nested-repository boundaries in its covered folders,
the effective policy hash, the scope path set, the selection version, the
mapping validity and its
sorted associations, the effective guidance digest, the target import
identities with their current export hashes, the direct consumer edges, and
the transitive provider closure. Each provider descriptor carries its input
digest, guidance digest, review revision, active invalidations, and resolved
import edges.

The provider closure is deliberately conservative. Unrelated source edits
outside the scope do not change the token. Incoming handoffs and overlaps are
not bound, because they do not change this document's inputs. Git HEAD, worktree dirty
status, and the availability of historical blobs do not define review
validity. Moving HEAD alone cannot invalidate an otherwise identical snapshot.

Timestamps, worktree locations, host ignore settings, and unrelated reviews do
not affect the token. The token and the artifact digests detect accidental
change. They are not signatures. They do not authenticate a reviewer, and they
do not prove which bytes a reviewer read.

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

The [specification](specification.md) records the approved contract, updated for 0.7.0.
This reference describes the implementation in this checkout.

## Continue

Invoke `memoria review` to find the next documentation action.
