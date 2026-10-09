# Section guidance

Load this file when a document names a section guide, when you add or move one, or when a `section_guidance_*` diagnostic appears. Go back to [SKILL.md](SKILL.md) for the stages.

## What a section guide is

A section guide is a reusable Markdown file with writing rules for one kind of section. It adds to project guidance for the sections that name it. It never adds an input, never makes a document pending, and has no acknowledgement of its own. An edit to a guide is a guidance change for the documents that name it, and only for those documents.

## Register a guide

1. Add the path to `section_guidance_files` under `[documentation]` in the root `memoria.toml`. The path is relative to the project root.
2. Make sure that the file is a regular UTF-8 file of at most 65,536 bytes.
3. Keep Memoria markers out of the guide. Marker examples inside fenced or inline code are permitted.

A registration reserves the file. It is never a source and never a tracked document. A registration alone applies the guide to no document. Only the root `memoria.toml` can register a guide. A sidecar registration is `configuration_invalid`.

## Name a guide in a section

Put the `guidance` attribute last in the section marker. Its path is relative to the document's folder, like an import `src`:

```markdown
<!-- memoria:section id="commands" files="justfile Cargo.toml" guidance="docs/templates/agent-commands.md" -->
## Commands
<!-- /memoria:section -->
```

A section names one guide at most. A section with `guidance` and no `files` is a guide-only section. A change never suggests a guide-only section. The first appearance of a guide-only section forces one full baseline review.

## Fix the diagnostics

| Code | Cause | Fix |
| --- | --- | --- |
| `section_guidance_invalid` | The `guidance` attribute is malformed, the path leaves the project, or a document names more than 64 distinct guides. | Write one relative path as the last attribute. Merge guides if a document names too many. |
| `section_guidance_unregistered` | The path resolves to a file that `section_guidance_files` does not list. | Correct the path, or register the file. |
| `guidance_file_missing` | A registered guide does not exist. | Restore the file, or correct the registration and every marker that names it. |
| `guidance_file_invalid` | A registered guide breaks a registration rule. | Read the message. Remove its markers, shorten it, or remove the duplicate registration. |
| `section_mapping_invalid` | The section's `id` or `files` are wrong. The guide still applies. | Correct the marker. |
| `section_guidance_unused` | No section names a registered guide. This hint appears only in `memoria lint` and with `--verbose`. | Name the guide in a section, or remove the registration. |

## Lifecycle

- A guide edit makes no document pending. `memoria guidance --changed` lists the documents that name the guide.
- An edit to any guide of a document after `review --save` makes `ack` fail with `guidance_changed`. Capture again and reconcile.
- A moved guide needs the registration and every marker updated. The marker edits make the documents pending.
- A deleted guide is `guidance_file_missing` until you fix it.

## Conflicts

Project guidance applies to the whole document. A section guide adds to it for the sections that name it. If they conflict, follow project guidance and report the conflict.

1. Follow project guidance in the edited text.
2. State the conflict in the `ack` note and to the owner.
3. Do not edit a guide or a registration only to pass a review.

A conflict alone does not block an acknowledgement. The owner settles a real exception in project guidance.

## Complete example

The [agent instructions cookbook](https://github.com/viktordanov/rs-memoria/blob/v0.9.0/docs/cookbooks/agent-instructions/README.md) shows a complete example. The link opens the Memoria repository on GitHub. It is not a file in this project. The example has these parts:

1. `AGENTS.md` and `CLAUDE.md` share two guides: one for the `commands` section and one for a guide-only `boundaries` section.
2. A `justfile` change makes both files and the root README pending. Each file gets its own review and its own `ack`.
3. A guide edit lists only `AGENTS.md` and `CLAUDE.md` in `memoria guidance --changed`. The reviewer invalidates the affected file.
4. A typo in a guide path fails `memoria lint` with `section_guidance_unregistered`.
