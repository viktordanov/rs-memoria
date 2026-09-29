# Saved exports

Load this file for a full export, for `memoria packet view`, or when the owner authorized the legacy P1 projection. Go back to [SKILL.md](SKILL.md) for the stages.

## Full exports

A full export carries every reviewed byte. It is for offline reading or for a machine that cannot open the project.

1. Create a directory outside the project: `dir=$(mktemp -d)`.
2. Run `memoria review <DOCUMENT> --full --save "$dir" --format json`.
3. Acknowledge with the same saved file: `memoria ack <DOCUMENT> --packet <saved file> ...`.

## Retrieval with packet view

1. Run `memoria packet view <saved file> --section guidance` for the saved guidance.
2. Run `memoria packet view <saved file> --section content` for all saved current content.
3. Run `memoria packet view <saved file> --file <PATH>` for one saved input.
4. Run `memoria packet view <saved file> --section history` for old bodies and historical evidence.

`packet view` accepts a current full export only. A review manifest carries no content, so `packet view` reports `packet_content_unavailable`. A view is not an acknowledgement artifact. Retrieval does not prove that you read the content.

Artifacts from Memoria 0.6 are refused. Capture a new artifact. No converter exists.

## Legacy P1 projection

P1 is an older opt-in projection over a saved full export. Use it only when the owner authorizes incremental review and names a trusted prior review.

1. Run `memoria packet view <saved file> --section incremental --trust-prior-review`.
2. If `selection.full_review_required` is true, do a full review of the document.
3. Otherwise, read the document, the changed files, the imports, the guidance, the diffs, and every active reason.
4. For each unchanged candidate, justify reliance on the trusted review, or read the content.
5. Record the inspected inventory and its rationale outside the project before you acknowledge.

P1 never overrides `data.review.mode`. If the manifest reports `full_baseline`, use the full baseline.

## Reading cost

Memoria states no target for token savings. Earlier drafts proposed a savings target and a bound on missed changes. No completed evaluation supports them, and they are withdrawn. Never weaken a snapshot rule, a fallback, or the whole-document pass to reduce reading cost.
