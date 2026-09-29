# Integrations

Load this file for a request or an error about the skill, the agent hook, or the GitHub workflow. Go back to [SKILL.md](SKILL.md) for the stages.

## Rules

- Do not install, upgrade, or remove an integration unless the user asks. Each one is an explicit, reversible, local change.
- Use `memoria integrations skill ...`, `memoria integrations hook ...`, and `memoria integrations github ...`. The older `memoria agent ...` names keep the same behavior.
- The skill package has four files: `SKILL.md`, `review-details.md`, `saved-exports.md`, and `integrations.md`. Memoria records a hash for each file.
- `memoria integrations github install|upgrade|uninstall` previews the change and writes nothing without `--apply`.
- The workflow installs Memoria 0.7.0 or later, because configuration version 3 needs 0.7.0.
- The workflow never acknowledges a review. A failing `memoria check` in CI still needs a local review.

## Troubleshooting

| Report | Cause | Action |
| --- | --- | --- |
| `outdated` skill package | An older Memoria installed it. | Run `memoria integrations skill upgrade` with the same target and scope. |
| `modified` skill package | A package file was edited, or an unknown file was added. | Tell the user. Memoria preserves the files. |
| `github_prerequisites_missing` | The root README, `memoria.toml`, or `memoria.lock` is missing or invalid. | Do the normal setup first. Never work around it. |
| `github_unmanaged` or `github_modified` | Memoria does not own the workflow file, or it was edited. | Tell the user. Do not remove the file. |
| `configuration_invalid` for version 2 | The project still declares configuration version 2. | Follow "Migration to 0.7.0" in the Memoria changelog. |
