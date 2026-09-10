# Changelog

This file records user-visible changes in Memoria. The project maintainer owns release decisions.

Contents:

- [Unreleased](#unreleased)
- [0.4.0](#040---2026-09-10)
- [0.3.0](#030---2026-09-09)
- [0.2.0](#020).

## Unreleased

No changes yet.

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
