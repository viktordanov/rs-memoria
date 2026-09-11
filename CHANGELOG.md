# Changelog

This file records user-visible changes in Memoria. The project maintainer owns release decisions.

Contents:

- [0.5.0](#050---2026-09-11)
- [Migration to 0.5.0](#migration-to-050)
- [0.4.0](#040---2026-09-10)
- [0.3.0](#030---2026-09-09)
- [0.2.0](#020).

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
