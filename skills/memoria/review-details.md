# Review details

Load this file for a `focused_candidate` review, for reuse of an earlier inspection, or for a fallback reason that you do not understand. Go back to [SKILL.md](SKILL.md) for the stages.

## Review modes

`data.review.mode` is `full_baseline` or `focused_candidate`.

`full_baseline` means one thing. Read all current scope sources, all current import bodies, the whole document, the effective guidance, and every covered reason. `data.review.fallback_reasons` gives the code and the message for each cause.

`focused_candidate` means that the CLI found no technical reason for the full baseline. It is eligibility, not certification of the previous review. Decide separately whether that previous review is worth reuse. Without that explicit trust, use the full baseline.

For a focused review, do these steps:

1. Read the effective guidance and every covered reason.
2. Read each changed source in `data.inputs`, and each section in `data.review.sections`.
3. If a claim depends on content outside those sections, read that content too.
4. Read the whole document. This pass is never optional.
5. If uncertainty remains, read the full current scope.

The line range in `data.review.sections[].lines` is a hint for the current bytes. The document hash binds every byte, including the bytes outside the suggested lines.

## Fallback codes

| Code | Cause |
| --- | --- |
| `baseline_missing` | The document has no previous review. |
| `baseline_unavailable` | The previous bytes cannot be verified from local history. |
| `unmapped_change` | A changed source has no valid section. |
| `path_set_changed` | A source entered or left the scope. A rename is a removal plus an addition. |
| `handoff_changed` | A handoff appeared or disappeared, so a subfolder left or entered the scope. |
| `coverage_unrecorded` | A source entered the scope, and the last review did not record its handed-off folders. The reason is in the message. Review the complete scope; the next acknowledgement records the coverage. |
| `document_classification_changed` | A Markdown file gained its first marker or lost its last marker. |
| `mapping_invalid` | The document's section mappings are invalid. |
| `mapping_changed` | The section mappings changed since the last review. |
| `policy_changed` | The selection policy changed. |
| `imports_changed` | An imported export changed. Review the provider first, then render. |
| `semantic_invalidation` | Someone requested a semantic review with `memoria invalidate`. |
| `guidance_changed` | The project guidance changed since the last review. |

## Relationships of changes

Each entry in `data.changes` has a `relationship`:

- `own_text`: the document's own text changed.
- `scope_source`: a source in the scope changed. `sections` names the sections that describe it.
- `handoff`: a source entered or left the scope because a handoff changed.
- `coverage_unrecorded`: a source entered the scope, and the former coverage is not recorded. `unrecorded_reason` names why.
- `import`: an imported export body changed.
- `selection_policy`: the selection policy changed.

`also_covered_by_total` counts the other documents that cover the same source. Each of those documents gets its own review and its own acknowledgement. Acknowledging one never clears another.

## Co-covering documents

`data.downstream.co_covering` lists other documents whose scope contains a changed source. Two documents in the same folder always cover the same sources. A document that does not hand off a nested document's folder covers that folder too. Review each document on its own merits.

## Records to keep outside the project

Memoria stores no checklist. Keep these records yourself:

1. The document, the token, the artifact digest, and the baseline revision.
2. The sources, imports, and sections that you inspected, with their hashes.
3. Each reused earlier inspection, the review that you trusted, and why reuse is still relevant.
4. The completed whole-document pass, and the disposition of every covered reason.

## Reuse rules

Equal hashes permit reuse. Equal hashes do not transfer understanding. After a source change, open again every claim that depends on that source. Never replace unavailable history with unverified `HEAD` content.
