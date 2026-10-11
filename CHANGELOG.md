# Changelog

This file records user-visible changes in Memoria. The project maintainer owns release decisions.

Contents:

- [0.9.0](#090---unreleased)
- [Migration to 0.9.0](#migration-to-090)
- [0.8.0](#080---2026-10-04)
- [Migration to 0.8.0](#migration-to-080)
- [0.7.0](#070---2026-09-29)
- [Migration to 0.7.0](#migration-to-070)
- [0.6.0](#060---2026-09-16)
- [Migration to 0.6.0](#migration-to-060)
- [0.5.0](#050---2026-09-11)
- [Migration to 0.5.0](#migration-to-050)
- [0.4.0](#040---2026-09-10)
- [0.3.0](#030---2026-09-09)
- [0.2.0](#020).

## 0.9.0 - unreleased

CAUTION: Update every Memoria executable, including setup-action `version:` pins, before a project adds `section_guidance_files`. Memoria 0.8 refuses the key and names it. Then follow [Migration to 0.9.0](#migration-to-090).

### Section guides

A section can now name one reusable Markdown guide with writing rules for that kind of section. The guide adds to project guidance for the sections that name it, and only for those.

- Register each guide once under `[documentation] section_guidance_files` in the root `memoria.toml`. A registration reserves the file: it is never a source and never a tracked document.
- Name a guide in a section marker with a last `guidance="PATH"` attribute. The path is relative to the document's folder, like an import `src`.
- A section with `guidance` and no `files` is a guide-only section. A change never suggests it.
- A guide adds no input and no freshness. A guide edit makes no document pending. `memoria guidance --changed` lists exactly the documents that name the guide.
- Every guide of a document is bound into its review context. An `ack` of an artifact saved before a guide edit fails with `guidance_changed` (exit 3) and writes nothing.
- `memoria guidance <DOCUMENT>` labels two layers, project guidance and section guides, and prints one rule: if they conflict, follow project guidance and report the conflict. `memoria guidance` without a document summarizes each registered guide.
- New diagnostics: `section_guidance_invalid` and `section_guidance_unregistered` (errors), and `section_guidance_unused` (a hint for `lint` and `--verbose`). A registered guide that carries a Memoria marker, is also project guidance, is listed twice, or is larger than 65,536 bytes is `guidance_file_invalid`. A sidecar registration is `configuration_invalid`.
- Bounds: one guide per section, at most 64 distinct guides per document, and at most 1,024 bytes per guide path.

### Section patterns

A section can now describe a kind of file instead of a fixed list. `files` accepts glob patterns and `!` exclusions beside literal paths:

```markdown
<!-- memoria:section id="auth" files="src/auth/** !src/auth/tests/** !src/auth/generated.rs" -->
```

- A section maps the union of its paths and patterns, minus the union of its exclusions, in any order. An exclusion also removes a literal path. Exclusions alone are invalid.
- A pattern matches only sources in the document's effective scope: never a tracked document, a guide, an ignored file, or a handed-off folder. Memoria expands it to concrete files, and the mapping identity uses those files. A literal list and a pattern that match the same files are the same association.
- A suggestion reads every literal path and only the pattern matches that changed. Review manifests add `review.sections[].files` (the authored tokens) and `review.sections[].matched` (the match count). The human review prints a pattern section as `<tokens> (N matches; read ...)`. A section with literal paths only prints as before.
- `memoria status --explain <DOCUMENT>` lists each section's tokens, every match, and the tokens that match nothing. `status --explain <SOURCE>` names each section that maps the source and the token that names it.
- A token that matches nothing keeps the mapping valid. Only `memoria lint` reports it, as the new hint `section_pattern_empty`.
- Freshness, scope, and acknowledgement do not change. An added, removed, or renamed file is still a changed path set and a full baseline. With a pattern, the mapping stays valid and the next change can be focused. With a literal path, a renamed file still withdraws the document's advice.
- Compatibility: a literal `files` path that starts with `!` is now an exclusion. Such a file can no longer be named by a literal path, but a pattern can match it. Tokens with `*`, `?`, or `[` were rejected before, so no valid 0.8 mapping changes meaning otherwise. A document whose mappings use only literal paths keeps the same review token.

### One glob engine

Selection rules and section patterns now share one matcher, built on the [`glob`](https://crates.io/crates/glob) crate. `ignore` and `include` keep their meaning: `?` matches one Unicode character, braces are ordinary characters, and `[!...]` and `[^...]` negate a class. A fuzz comparison with the 0.8 matcher found no difference in matches or errors. Configuration, selection fingerprints, and `memoria.lock` do not change.

### Clear full-baseline reading

A full-baseline review said "Read the complete current scope" and then listed only the changed inputs under `Read:`. The human view now says what each list means:

- `Read first:` holds the document and the changed inputs.
- The next line counts the unchanged sources and imports that the review must also cover, for example `Then read the rest of the scope: 3 unchanged sources. List the sources: memoria status --explain deploy/runbook.md`. When the list is already complete, the line is `Rest of the scope: none. The list above is the complete scope.`
- `memoria status --explain <DOCUMENT>` now lists every source in the document's scope. JSON adds `document.scope`.

The manifest, the token, and the review requirements do not change. The artifact still lists only the changed inputs, so it stays proportional to the changes. A focused candidate keeps its `Read:` list.

### Rust 1.99

Memoria now builds with Rust 1.99.0, up from 1.96.0, so that release builds include the miscompilation fixes from Rust 1.96.1, 1.97.1, and 1.98.1.

- `rust-toolchain.toml` pins 1.99.0, and the minimum supported Rust version (`rust-version`) is now 1.99.
- The Linux release archives build in `rust:1.99.0-bookworm`, pinned by digest in `scripts/linux-builders.json`.
- A measurement on this repository and on a 2,000-source synthetic project found no runtime difference beyond noise. Memoria's time goes mostly to Git subprocesses, not to compiled Rust code.
- Building from source now needs Rust 1.99. Prebuilt packages are not affected.

### Quiet output stays quiet

A document that names no guide keeps its guidance digest, its review lines, and its `memoria guidance` view byte for byte. During a review, the only new lines are a `· guide: <path>` suffix on a suggested section and a count on the `Guidance:` line. `status`, `check`, the plan, `graph`, hooks, and the workflow template do not change.

### Review artifacts

- Review manifests are now `manifest_version` 3. They add `review.sections[].guidance`, `review.sections[].files`, `review.sections[].matched`, and `section` entries in `guidance.references`.
- Full exports are now `packet_version` 5. They carry each section guide's exact text. `ack` now recomputes the guidance digest of a full export and refuses text that does not produce it (`packet_content_mismatch`).
- `packet view` is now `view_version` 3.

The token, the review context, `memoria.lock`, and configuration `version = 3` do not change.

### Documentation

- New cookbook: [keep agent instructions current](docs/cookbooks/agent-instructions/README.md). Its outputs come from a tested fixture.
- New [cookbook index](docs/cookbooks/README.md). Each cookbook is a folder with a `README.md` and its diagrams, and shows one way to document a project. A test compares every output block with a real run.
- New `scripts/ascii-diagram.py` renders a monospace drawing as an SVG in the Memoria diagram style, with light and dark colors. `--check` fails when an SVG is not the current render of its source.
- The agent skill has a new on-demand file, `section-guidance.md`, and teaches the conflict rule.
- `SKILL.md` now starts with the complete syntax: markers, configuration keys, and commands. A use-case table then maps each common task to its stages and reference files. The review stages and their rules do not change.
- The skill links the agent instructions cookbook and the command reference on GitHub, pinned to this release, because an installed skill cannot read Memoria's own `docs/`.
- The skill's `integrations.md` now names all five package files.
- The root README lists its use cases before the purpose and the quick start.
- The documents beside the code cookbook maps the runbook's scripts with `*.sh` and shows a new file that the pattern picks up.

### Fixes

- A human diagnostic without details no longer prints a stray `{}` line. JSON output does not change.
- A passing `memoria check` now counts every tracked document in its human message. It counted only READMEs, so opted-in documents were missing from the number. The JSON `documents` and `readmes` fields do not change.

## Migration to 0.9.0

1. Update every Memoria executable, including setup-action `version:` pins, before you add `section_guidance_files`.
2. Capture review artifacts that were in flight again. Manifest 2, packet 4, and view 2 artifacts are refused with regeneration text.
3. Run `memoria agent upgrade` for each installed skill. `memoria agent status` reports it as outdated until then.
4. A project that registers no guide sees no change: the same digests and the same output.
5. To build from source, use Rust 1.99 or later. `rustup` selects the pinned toolchain from `rust-toolchain.toml`.
6. If a section names a file whose name starts with `!`, replace that token with a pattern that matches it, for example `?keep.rs` for `!keep.rs`.

## 0.8.0 - 2026-10-04

CAUTION: This release changes default human output. If a script reads hints or progress lines from stderr, follow [Migration to 0.8.0](#migration-to-080) first. JSON envelopes, diagnostic codes, exit statuses, saved artifacts, tokens, and `memoria.lock` do not change.

No state, configuration, token, or saved-artifact migration is needed from 0.7.0.

### Quiet default output

Commands print their result, every warning, and every error. Advisory hints no longer repeat across routine commands. In the AI core evidence project, `memoria review` with an empty plan printed 1,653 stderr bytes in 50 hint lines. It now prints none.

- A hint prints only in `memoria lint` and with the new global `--verbose`, also for `review <DOCUMENT>` and `explain <DOCUMENT>`.
- Mutation progress notes (`memoria: ack: …` and the others) print only with `--verbose`.
- JSON responses still carry every diagnostic.
- The plan no longer adds "guidance present" to every line or repeats the guidance sentence.
- A link to a project guidance file now says that the file is guidance, not that it is "not a selected file". The reason code stays `not_selected`.

### One review entry point

`memoria review <DOCUMENT>` now shows the verified Git hunk under each changed input: at most 40 lines for one change and 160 in total. `--details` shows all computed hunks without the display truncation. If evidence computation exceeds its budget, hunks can be omitted without a reason, even with `--details` (OBS001). A `No hunk (<code>)` line gives the reason when Git no longer holds the reviewed bytes. The manifest, the JSON output, and the saved artifact do not change.

`memoria explain <DOCUMENT>` keeps its own purpose: why any document is current, pending, or waiting. Its default view shows one readable line for each change with its hunk, and a next step for the state. For a current document it says that no review is needed, and no longer suggests `memoria review`. When an import is older than its provider's export, it names `memoria render <DOCUMENT>` and asks you to explain the document again, because the render can leave it current.

### Guidance assessment

`memoria guidance --changed` lists the reviewed documents whose guidance changed since their review, grouped by the guidance that each review saw. It writes nothing. The review plan reports the change once, in one line and in `data.guidance_assessment`. `memoria check` keeps its `guidance_changed` hint in JSON and still passes. The agent `Stop` hook message now names `memoria guidance --changed`.

You choose the affected documents and request their review with `memoria invalidate`. Memoria records no "assessed" decision, so a document that you leave alone stays listed until its next review.

### Parallel reviews

- A write command now waits up to 10 seconds for another write command to release the lock, then reports `state_busy` (exit 3). In a test with nine reviewers who acknowledged at the same moment, 0.7 refused eight of them. 0.8 recorded all nine.
- `MEMORIA_LOCK_WAIT_MS` sets the wait (0 to 600000 milliseconds). An invalid value is `lock_wait_invalid` (exit 2) for `ack`, `invalidate`, `render`, and `init`.
- The `state_busy` message says to run the same command again with the same artifact.
- The [workflow guide](docs/workflow.md#review-documents-in-parallel) and the agent skill describe coordinators, assigned reviewers, and the worktree procedure: capture in the worktree, merge, then acknowledge in the main checkout.

### Documentation

- New [concept guide](docs/concepts.md): tracked documents, directory scope, handoffs, imports and review order, and review and acknowledgement, each with an example and its limits.
- The agent skill captures with the human `--save` view, reads hunks there, assesses guidance before it finishes, and has rules for parallel reviewers.

A preview tree-snapshot test previously failed when Git background maintenance created a lock file (OBS002). CI disables automatic maintenance for this test environment.

## Migration to 0.8.0

| If you relied on | Do this |
| --- | --- |
| Hints in the human stderr of `status`, `check`, `review`, `explain`, `graph`, and the other routine commands | Add `--verbose`, run `memoria lint`, or read `diagnostics` from `--format json`. |
| `memoria: <command>: …` progress lines on stderr | Add `--verbose`, or read the stdout result or the JSON data. |
| The `guidance_changed` hint in human `check` output | Run `memoria guidance --changed`, or read `data.guidance_assessment` from `memoria review --format json`. |
| `memoria explain <DOCUMENT> --full` for hunks during a review | Read them in `memoria review <DOCUMENT>`. `explain` still works. |
| An immediate `state_busy` | Set `MEMORIA_LOCK_WAIT_MS=0`. |
| The last line `Details: memoria review … --details` | Run `--details` directly. The line now appears only when a hunk was cut. |
| An installed agent skill | Run `memoria agent upgrade`. The skill reports `outdated` because its bytes changed. |

No state migration is needed. Existing artifacts stay valid.

## 0.7.0 - 2026-09-29

CAUTION: This is a breaking release. Update every Memoria executable to 0.7.0 together, and follow [Migration to 0.7.0](#migration-to-070) before you acknowledge anything.

### Document scopes replace nearest-README ownership

Every tracked document now covers the selected files in its own folder and below it. A document stops covering a subfolder only when it links to or imports a tracked document inside that subfolder: a handoff. A nested README alone removes nothing. READMEs and opted-in Markdown follow the same rule.

```text
README.md        links auth/README.md    covers app.rs
app.rs
auth/README.md                           covers auth/login.rs
auth/login.rs
```

Without that link, an edit to `auth/login.rs` makes both documents pending. With it, only `auth/README.md` is pending. A handoff link moves coverage and binds it into the parent's review; it creates no waiting. An import keeps its waiting edge and its content freshness.

### Added

- **Opted-in Markdown documents.** A selected Markdown file (`*.md`, `*.markdown`) with a Memoria export, import, or section marker outside code is a tracked document, reviewed and acknowledged on its own. A link never tracks a file. Every command accepts a document path.
- **Handoff hints.** `handoff_not_applied` explains a subfolder link or import that is not a handoff (`untracked_markdown`, `missing`, `no_document_in_directory`, `not_selected`). `handoff_absent` names a nested document that its parent covers too. Hints never fail a command.
- **`memoria review <DOCUMENT> --save <DIR>`** writes the exact JSON artifact into an existing directory outside the Git worktree, with mode 0600 and no clobber, and reports a receipt with the `ack` command. A destination inside the worktree, including an ignored folder, fails with `save_destination_in_project`.
- **`memoria ack` without `--token`.** The token comes from the integrity-checked artifact; the complete token is still recomputed from the repository. `--token` stays available as a check. The report adds `token_source`.
- **Change-first review view.** The human view starts with what changed and how each change relates to the document, then co-covering documents, export consumers, how to read, and three next steps. `--details` adds tokens, digests, and per-input hashes.
- **Relationships and downstream.** Each change carries a `relationship` (`own_text`, `scope_source`, `handoff`, `coverage_unrecorded`, `import`, `selection_policy`). The manifest adds `document_kind`, `scope`, and `downstream` (export consumers and co-covering documents, at most 64 each, with totals).
- **Scope reporting.** `status` counts documents, READMEs, opted-in documents, handoffs, and overlapping sources. `status --explain` names `covered_by` and `handed_off` for a source, and the scope facts for a document. `graph` lists handoffs and overlaps.
- **A four-file agent skill.** The shared skill for Claude Code and Codex is `SKILL.md` with five stages, plus `review-details.md`, `saved-exports.md`, and `integrations.md`, which load on demand. The installer hashes every file.
- **Fallback codes** `handoff_changed`, `coverage_unrecorded`, and `document_classification_changed`.
- **Handoff evidence.** `memoria ack` records the folders that the document's scope handed off. A later review uses it to tell an ended handoff (`handoff_changed`) from a new source (`path_set_changed`) exactly. `state inspect` shows it as `coverage_evidence`.

### Changed

- Configuration `version = 3` is required. Version 2 fails with `configuration_invalid` and names the cutover.
- The review manifest is `manifest_version: 2`, the full export is `packet_version: 4` with `content.document`, and `packet view` is `view_version: 2`. `whole_readme_pass` is now `whole_document_pass`, and the role `whole_readme` is `whole_document`. The count `selected_files` is `scope_files`.
- The review context is layout v2 with selection version 2 and policy `section-review-v2`. It binds the document kind and its handoffs instead of ancestor and descendant boundaries. Every outstanding 0.6 artifact is refused.
- Sections validate against the document's scope. A mapping that names a tracked document or a handed-off file is invalid, with a migration message.
- `integrations github --version` requires 0.7.0 or later.

### Removed

- The fallback code `ownership_changed`.

### Compatibility

- **Committed review history is kept.** `memoria.lock` moves to format 3: format 2 plus one coverage field in each review row. The reader accepts formats 2 and 3, and reading never writes. The first ordinary state write rewrites the lock as format 3, with every record kept unchanged and marked as having no coverage evidence. Document identities may now name opted-in Markdown. No upgrade converts, resets, prunes, or backfills the lock, and no acknowledgement is fabricated.
- A README that links or imports every tracked document directly below it keeps its 0.6 scope and policy, so it stays current.
- Memoria 0.6 refuses configuration version 3. After the first 0.7 write, its read-only `state inspect` and `state diff` report `state_unsupported_schema` for the format 3 lock. That lock is valid: keep it.
- Disposable review artifacts from 0.6 are refused with regeneration text. There is no converter.

## Migration to 0.7.0

Do these steps before any acknowledgement:

1. Update every executable — local, agents, and CI — to 0.7.0 together.
2. Change `version = 2` to `version = 3` in `memoria.toml`, and in each `README.memoria.toml` that declares a version.
3. Run `memoria status` and `memoria review`. A README that does not link or import a README in a subfolder now also covers that subfolder. Read the `handoff_absent` hints. Add the link, or accept the extra reviews. Do not acknowledge yet.
4. Decide on new opted-in documents. Each one covers its folder.
5. Regenerate every review artifact. 0.6 artifacts are refused.
6. Run `memoria integrations skill upgrade` for each installed skill.
7. After 0.7.0 is published, run `memoria integrations github upgrade --version 0.7.0 --action-ref v0.7.0 --apply`.
8. Review each document normally. There is no bulk acknowledgement or invalidation.

The first acknowledgement (or other state write) converts `memoria.lock` to format 3. After that write, Memoria 0.6 can no longer inspect the lock: its `state inspect` reports `state_unsupported_schema`. Existing records keep their values and have no coverage evidence until each document's next acknowledgement. Until then, a source that enters such a document's scope can report `coverage_unrecorded` with a reason, such as `revision_not_first` for a README that 0.6 reviewed more than once. Review the complete current scope. The next acknowledgement records the coverage.

CAUTION: Never set `version` back to 2 by hand. Memoria 0.6 would then certify the project with 0.6 rules, and nothing can prevent that. Memoria 0.6 refuses version 3, and it refuses a format 3 lock with `state_unsupported_schema`: keep that lock.

## 0.6.0 - 2026-09-16

### The default review output changed

`memoria review <README.md>` now returns a small review manifest in human and
JSON output alike. The manifest states what the review must read and why it
cannot read less. It carries no README body, no source body, no import body,
no historical content, and no authored guidance prose.

Read the listed paths with your ordinary file tools. Memoria adds no read
command, no range command, and no content API.

`memoria review <README.md> --full` produces the previous complete export. It
carries every reviewed byte and the same token.

Both artifacts acknowledge. `memoria ack --packet` accepts either one.

### Added

- **Advisory section mappings.** A README can map one part of its prose to the sources it describes with `<!-- memoria:section id="ID" files="a.rs b.rs" -->` ... `<!-- /memoria:section -->`. A section is a reading hint. It creates no ownership and no separate freshness. One invalid mapping withdraws the advice of the whole README and produces the `section_mapping_invalid` warning. `lint` does not fail because of section advice alone.
- **Review mode and fallback reasons.** `data.review.mode` is `focused_candidate` or `full_baseline`. `data.review.fallback_reasons` names every cause, with one of eleven fixed codes.
- **Baseline evidence state.** `data.baseline.evidence_status` is `verified`, `partial`, or `unavailable`. Unverifiable reviewed bytes select the full baseline instead of a fabricated diff.
- **Full exports state their own requirements.** `data.requirements` repeats the manifest, and `data.binding` carries the canonical context descriptors. A reader can recompute the token from the file alone.

### Changed

- Every CLI JSON envelope moves to `schema_version: 3` in one cutover. The native hook protocol is separate and unchanged.
- The review token becomes `mrv3.<16 hex>`. It binds the complete input manifest, the complete prior review record, and a new review context. The context binds the ownership boundaries, the effective selection policy and its selected path set, the section mapping associations, the effective guidance digest, and the transitive provider closure.
- A provider edit can now invalidate a consumer token even when the imported export body stays equal. That edit does not make the consumer stale by itself. Freshness and scheduling are unchanged.
- The full export becomes `packet_version: 3`.
- `memoria packet view` accepts a current full export only. For a manifest it reports `packet_content_unavailable` and names the two ways to read the content.
- `--full` now selects the representation in both output formats. It is no longer a human-only presentation flag.
- The decoded-content limit of 32 MiB applies to the full export, which transports the bytes. The raw-input limits of 8 MiB and 32 MiB still apply to both artifacts.
- Legacy P1 projection stays available for full exports. It never reduces the scope that `data.review.mode` requires.

### Removed

- The shipped skill no longer proposes a one-percentage-point degradation ceiling at 95% confidence or a 50% median token reduction. Those targets are withdrawn, and no completed evaluation supported them. Reading cost and missed changes are observations with stated limits, never release promises.

### Compatibility

CAUTION: Old review artifacts do not work with 0.6.0. Produce new ones.

- A version 2 envelope, a version 2 packet, and an `mrv1` or `mrv2` token are refused with instructions to run `memoria review` again. Memoria converts nothing, migrates nothing, and upgrades nothing automatically. Review artifacts are ephemeral, so regeneration is the whole procedure.
- **The state format is unchanged.** `memoria.lock` keeps format version 2 and its codec. An existing lock stays readable and stays a valid baseline candidate. There is no bulk invalidation, no state conversion, and no fabricated acknowledgement.
- No review becomes stale because of this upgrade. An outstanding artifact captured before the upgrade must be regenerated.
- An older executable cannot parse section comments correctly. Upgrade every executable before you author sections. Do not mix versions in one review workflow.

## Migration to 0.6.0

### 1. Install the new version

```sh
cargo install --locked --git https://github.com/viktordanov/rs-memoria --tag v0.6.0
memoria --version
```

### 2. Discard review artifacts captured before the upgrade

If you saved a review artifact and did not acknowledge it, delete it. Then run
`memoria review <README.md> --format json` again for a current manifest.

If you skip this step, `memoria ack` refuses the old artifact with exit 2 and
tells you to produce a new one. Nothing is written.

### 3. Adopt the new default output

If a script reads `data.content`, `data.context`, or `data.manifest` from
`memoria review <README.md> --format json`, add `--full` to that invocation.
The full export keeps those fields.

If a script only needs the token or the changed identities, read the manifest
instead. It is smaller and it carries the same token.

If you skip this step, the script reads absent fields.

### 4. Map README sections to sources, if you want the advice

This step is optional. Memoria works exactly as before without a single
section.

```markdown
<!-- memoria:section id="persistence" files="handle.go service.go" -->
## Saving and synchronizing

Save writes a local archive. Sync also uploads the archive.
<!-- /memoria:section -->
```

Add the markers, then invoke `memoria lint`. A mistyped mapping produces a
`section_mapping_invalid` warning with the line and the reason. `lint` still
exits 0.

The advice starts after the next acknowledgement of that README. A new mapping
requires one full baseline first, because a changed association cannot reduce
the required scope.

If you skip this step, every review selects the full baseline, exactly as in
0.5.0.

## 0.5.0 - 2026-09-11

### Repository maintenance

The maintainer removed co-author trailers from published Git history and re-signed the affected commits and release tags.
Release source trees, published archives, and checksums remain unchanged. Commit IDs and tag signatures changed.
Existing clones require reconciliation with the rewritten history. SHA-pinned consumers can retain their original pin or select its replacement.

The maintainer released this work as 0.5.0. The version number is a release decision, not a
consequence of a removed interface: read [Compatibility](#compatibility) for what actually
changes for an existing user, and [Migration to 0.5.0](#migration-to-050) for the steps.

### Added

- `memoria integrations skill|hook|github <operation>` groups the three integrations under one umbrella. The [integrations guide](docs/integrations.md) maps the branches.
- `memoria integrations github install|status|upgrade|uninstall` creates and maintains one consumer GitHub Actions workflow. Every change previews first and needs `--apply`. Memoria records what it wrote in `.github/memoria-workflows/<filename>.json`, never adopts a workflow without that record, and preserves a file you edited.
- A first-party `setup-memoria` Action at the repository root installs a verified prebuilt executable on an Ubuntu runner. It supports `ubuntu-24.04`, `ubuntu-latest`, and `ubuntu-24.04-arm`, and it has no Cargo fallback. The [GitHub Actions guide](docs/github-actions.md) describes both parts.
- Reproducible Linux release builds for `x86_64-unknown-linux-gnu` and `aarch64-unknown-linux-gnu`, from a builder image pinned by digest in `scripts/linux-builders.json`.

### Requirements

`memoria integrations github install` and `upgrade` need an initialized project when they write: a root README, a valid `memoria.toml`, and a readable `memoria.lock`. The generated job runs `memoria check`, so a workflow without those files could only fail. A preview works without them and names each missing item. A pending review is still permitted. `status` and `uninstall` need none of them.

The setup Action supports Ubuntu 24.04 only. It refuses another release instead of warning about it, because no validation exists for a newer image and the `ubuntu-latest` alias can move.

The setup Action installs 0.5.0 or later. It refuses an earlier version, because no earlier release carries both Linux archive pairs.

### Compatibility

This release removes no command, renames no command, and changes no existing behavior.

- Every `memoria agent ...` name keeps its arguments, data, JSON command label, diagnostics, exit statuses, and side effects. That includes the label on an argument error.
- An installed hook launcher keeps the `memoria agent hook run` text. An existing installation needs no change, and a launcher that 0.5.0 writes still works with an older executable.
- There is no `memoria integrations agent` level and no `memoria integrations skill hook` path.
- **The state format is unchanged.** `memoria.lock` keeps its format version and its codec. 0.5.0 reads a lock that 0.4.0 wrote, and 0.4.0 reads a lock that 0.5.0 writes. No conversion, no migration, and no re-review is needed.
- Canonical JSON stays at envelope schema version 2.

The generated workflow and its ownership record are ordinary documentation inputs. A new workflow makes the owning README pending, and the normal review follows. No command acknowledges a review automatically.

### What actually changes at the version boundary

Three things change, and none of them is a change to an interface you already use:

| Change | Effect on an existing 0.4.0 user |
| --- | --- |
| Package version 0.4.0 → 0.5.0 | Update the version you install or pin. |
| Release asset names carry the new version | `memoria-0.5.0-x86_64-unknown-linux-gnu.tar.gz`, the `aarch64` archive, and both `.sha256` sidecars. |
| The setup Action's version floor is 0.5.0 | Only affects the Action, which is new in this release. |

## Migration to 0.5.0

Nothing here is required to keep working. Step 1 is the whole upgrade; steps 2 to 4 adopt new
things, and each one says what happens if you skip it.

### 1. Install the new version

```sh
# Replace the executable. Your project files need no change.
memoria --version    # memoria 0.5.0
memoria check        # same result as before the upgrade
```

`memoria.lock`, `memoria.toml`, your READMEs, and your review history carry over untouched. There
is no conversion step and no `--migrate` flag, because the state format did not change.

### 2. Command names: optional, both spellings work

| You run today | Preferred in 0.5.0 |
| --- | --- |
| `memoria agent install --target codex` | `memoria integrations skill install --target codex` |
| `memoria agent status --target codex` | `memoria integrations skill status --target codex` |
| `memoria agent upgrade --target codex` | `memoria integrations skill upgrade --target codex` |
| `memoria agent uninstall --target codex` | `memoria integrations skill uninstall --target codex` |
| `memoria agent hook install --target claude` | `memoria integrations hook install --target claude` |
| `memoria agent hook status --target claude` | `memoria integrations hook status --target claude` |
| `memoria agent hook uninstall --target claude` | `memoria integrations hook uninstall --target claude` |

Skip this and every old name keeps working, with the same output and the same exit status. A
script that reads the JSON `command` field needs no change either: an equivalent invocation
reports the established `agent ...` label under both spellings.

### 3. Installed skill and hook: update when convenient

```sh
# The skill package text ships inside the executable, so a new executable has new text.
memoria integrations skill status --target codex     # reports `outdated` after the upgrade
memoria integrations skill upgrade --target codex

# The hook needs nothing. Its launcher text is unchanged.
memoria integrations hook status --target claude     # reports the same installed hook
```

Skip the skill upgrade and the old package stays in place and keeps working; your agent just
reads the older instructions. The hook launcher still calls `memoria agent hook run`, so an
installed hook needs no reinstallation.

### 4. Continuous integration: new, opt in

```sh
memoria integrations github install            # preview; works in any project, writes nothing
memoria integrations github install --apply    # writes the workflow and its ownership record
memoria integrations github status
```

The apply needs an initialized project, because the generated job runs `memoria check`. If you
already maintain your own Memoria workflow, keep it: Memoria never adopts a file it does not own,
and it will refuse rather than overwrite yours.

The maintainer published [version 0.5.0](https://github.com/viktordanov/rs-memoria/releases/tag/v0.5.0), including the tag and both Linux archives with their checksum sidecars.
The generated `@v0.5.0` reference resolves, and the job can install Memoria from those archives.

## 0.4.0 - 2026-09-10

This release adds review-context interfaces. Canonical JSON v2 and the lock format remain unchanged.
The [0.4.0 guide](docs/releases/0.4.0.md) explains the workflow and upgrade steps.

- Human review and explain output start with changes and evidence. `--full` retains detailed output.
- `memoria packet view` reads exact sections from a validated saved packet. Canonical JSON v2 acknowledgement transport remains complete.
- Experimental P1 requires explicit trust and records coverage requirements with full-review fallback. Model-quality evaluation remains unrun.
- New acknowledgements retain a Git reference only after complete content correspondence. Valid dirty reviews can report partial or unavailable historical coverage.
- Bounded local history lookup can recover later-committed matching bytes without changing prior review attribution. Source commits before acknowledgement remain optional.

## 0.3.0 - 2026-09-09

This release improves command discovery, review evidence, and human output. Whole-file freshness and the binary lock codec stay unchanged.

### Added

The new interfaces provide these capabilities:

- `memoria completions bash|zsh|fish` produces shell completion scripts without project discovery.
- `memoria explain README.md` gives deterministic, read-only freshness evidence in human text or structured JSON. Evidence includes changed paths, hashes, policy, guidance, and imports. Git hunks require a verified baseline. Unavailable hunks have explicit reasons.
- `MEMORIA_REVIEWER` supplies an optional reviewer label. An explicit `--reviewer` takes precedence. Acknowledgement reports the resolved label. Memoria never guesses identity from the OS, Git, or a model.
- `memoria state diff OLD_LOCK NEW_LOCK` compares two lock snapshots without project discovery or embedded history.
- This changelog records release changes and links earlier release notes.

### Changed

Human output and documentation have these improvements:

- Human diagnostics wrap to the available width. Nested diagnostics retain their facts, codes, and exit behavior.
- Human review plans show the guidance requirement and the next document's guidance command. Focused packets retain the full guidance.
- Human output groups navigation warnings and hints. JSON retains individual diagnostics and their semantics.
- Documentation clarifies workflow authority, acknowledgement-note constraints, and `init` and README discovery rules.

### Compatibility

Existing JSON contracts, exit behavior, and whole-file freshness remain intact. The new commands provide additive interfaces.
Lock state contains neither source contents nor hunks. This release does not migrate or convert lock files.

## 0.2.0

The [original 0.2.0 notes](docs/releases/0.2.0.md) describe that release and its explicit upgrade procedure.
The [published 0.2.0 release](https://github.com/viktordanov/rs-memoria/releases/tag/v0.2.0) contains its original assets and publication details.

Next: read the [review workflow](docs/workflow.md) for the current review procedure.
