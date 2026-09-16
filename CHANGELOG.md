# Changelog

This file records user-visible changes in Memoria. The project maintainer owns release decisions.

Contents:

- [0.6.0](#060---2026-09-16)
- [Migration to 0.6.0](#migration-to-060)
- [0.5.0](#050---2026-09-11)
- [Migration to 0.5.0](#migration-to-050)
- [0.4.0](#040---2026-09-10)
- [0.3.0](#030---2026-09-09)
- [0.2.0](#020).

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
