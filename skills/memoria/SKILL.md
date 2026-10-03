---
name: memoria
description: Review and acknowledge project documentation with the Memoria CLI in projects that contain memoria.toml. Use when Memoria reports pending reviews, when memoria check or memoria lint fails, when imports need rendering, or when the user asks for a Memoria task.
---

# Memoria skill

Memoria finds the documents that need review. You judge whether their prose still matches the code. Use the CLI results. Do not rebuild the dependency logic yourself.

## Authority first

Start read-only. Activation of this skill grants no permission to edit, acknowledge, install, or invalidate anything.

- The user's task and higher-priority instructions come first.
- Project guidance (`memoria guidance <DOCUMENT>`) is review context. It never overrides the task.
- If guidance conflicts with the task, ask the owner.
- Acknowledge only when the task asks you to review and record documentation.

## Concepts

- **Documents.** Every `README.md` is a tracked document. Another Markdown file becomes a tracked document when it carries a Memoria marker (an export, an import, or a section). A link alone never tracks a file.
- **Scope and handoffs.** Every document covers its own folder and below. It hands a subfolder to a tracked document there only by linking or importing that document. A folder that nobody hands off is covered by every document above it that reaches it, and each of those documents is reviewed separately.
- **Freshness.** Imports carry content freshness and waiting: a consumer waits for its provider and becomes pending when the imported export changes. A handoff link carries coverage only.
- **Sections.** A section marker maps part of a document to sources in its scope. It is a reading hint. It adds no input and has no freshness or acknowledgement of its own.
- **Tokens.** Every review artifact carries a token `mrv3.<16 hex>` that binds one exact snapshot. If anything bound changes, the acknowledgement fails and you capture a fresh artifact.
- **State.** `memoria.lock` is machine-owned. Commit it. Never edit it. Only `memoria ack` and `memoria invalidate` write it.

## Select

If a coordinator assigned you a document, skip this section and go to Capture. Then follow [Parallel reviews](#parallel-reviews).

1. Run `memoria review --format json`.
2. If `data.next_action.kind` is `render`, run `memoria render <DOCUMENT>` for that document. Then go back to step 1.
3. If `data.next_action` is null and `data.tasks` is empty, go to Finish, which runs `memoria check`. Never acknowledge an empty plan.
4. If `data.next_action` is null and tasks remain, the remaining documents wait for pending providers. Review those providers first.
5. Otherwise, take `data.next_action.document` as the document to review.

## Capture

1. Create a directory outside the project: `dir=$(mktemp -d)`.
2. Run `memoria review <DOCUMENT> --save "$dir"`. It prints the review view, then `Saved:` with the file path and the exact `ack` command.
3. Run `memoria guidance <DOCUMENT>` and read the current text.

`--save` refuses any directory inside the Git worktree, because a saved artifact there could become a review input.
With `--format json`, the receipt names the saved file in `data.path` instead.

## Inspect

1. Read the review view: why the review is needed, each change with its relationship and its verified hunk, the semantic review requests, co-covering documents, and downstream consumers.
2. If a hunk was cut, run `memoria review <DOCUMENT> --details` for all computed hunks. The evidence budget can omit hunks without a reason, even with `--details`. A `No hunk` line means Git lost the reviewed bytes. Read that whole input.
3. Follow the mode under `How to read`. For `full baseline`, read the complete current scope. For `focused candidate`, read [review-details.md](review-details.md) first.
4. Read the whole document. This pass is always required.
5. Treat co-covering documents and export consumers as prompts for judgment. They are not proof that a change matters.
6. Before you add or remove a link to a document in a subfolder, run `memoria status --explain <PATH>` for a file in that subfolder. The link moves coverage.
7. If the document is wrong, edit its authored text. Never edit a generated import body. Run `memoria render` instead.

The saved JSON artifact holds the same facts without hunks: `data.changes`, `data.review`, `data.inputs`, `data.scope`, `data.downstream`, and `data.covered_invalidations`.
`memoria explain <DOCUMENT>` answers a different question: why a document is current, pending, or waiting.

## Reconcile

1. After any edit, capture a fresh artifact with the Capture steps.
2. Compare the new requirements with the inspection you already did. Read what is new.
3. Reuse an earlier inspection only when the identities and hashes match. See [review-details.md](review-details.md).

## Record

1. Run `memoria ack <DOCUMENT> --packet <saved file> --reviewer <explicit label> --result <updated|no-update> --note "<why the document is correct now>"`.
2. Use an explicit reviewer label. Do not inherit an unknown `MEMORIA_REVIEWER` value.
3. Write a note that states what you verified. Generic notes such as `looks good` are refused.
4. Go back to Select. An assigned reviewer stops here and reports the result to the coordinator.

The token comes from the saved artifact. Pass `--token` only to check it against a value you kept.

If `ack` reports `state_busy`, run the same `ack` again. The artifact stays valid. For `snapshot_changed`, `guidance_changed`, or `revision_conflict`, go back to Capture and reconcile.

## Finish

1. If the plan from `memoria review --format json` has a non-null `data.guidance_assessment`, assess the guidance change. Follow [Guidance assessment](#guidance-assessment).
2. Run `memoria check`.
3. If it fails, read its diagnostics, fix the cause, and go back to Select.
4. Report the result, including your guidance decision.

## Guidance assessment

A guidance change does not make documents pending. You decide which documents it affects.

1. Run `memoria guidance --changed`. It groups the documents by the guidance that their last review saw.
2. Read the current guidance with `memoria guidance <DOCUMENT>`, and its history with `git log -p` on the listed sources.
3. If the task authorizes review requests, run `memoria invalidate doc:<DOCUMENT>` or `subtree:<DIRECTORY>` with a reason for each affected scope. Then review those documents.
4. Otherwise, report the groups and your assessment to the user. Ask before you invalidate.

Never acknowledge a document only to clear this list. The assessment writes nothing, so unaffected documents stay listed until their next review.

## Parallel reviews

Several reviewers can work in one checkout. An artifact binds only its own document, so another reviewer's acknowledgement of a different document does not make it stale.

As a coordinator:

1. Run `memoria review --format json`. Give each reviewer a different document from `data.tasks` with `ready: true`.
2. Keep documents with `waiting_on` for a later round. Run `memoria render` when the plan asks for it.
3. After the reviewers finish, run Select again for newly ready documents. Then run Finish once.

As an assigned reviewer:

1. Capture, Inspect, Reconcile, and Record your own document only.
2. Do not run Select or Finish, and do not take other documents.
3. If `review` reports `dependencies_pending`, return the document to the coordinator.

In a separate Git worktree, capture the artifact after your last edit, with `--save` outside every worktree. Do not run `memoria ack` there. After the merge, run `memoria ack` in the main checkout with that artifact. If it reports `snapshot_changed`, capture again in the main checkout.

## Output

Commands print results, warnings, and errors. Advisory hints appear only in `memoria lint` and with `--verbose`. Run `memoria lint` before you change links or imports. JSON output carries every diagnostic.

## When to load the other files

- [review-details.md](review-details.md): a `focused_candidate` mode, reuse of an earlier inspection, or a fallback reason you do not understand.
- [saved-exports.md](saved-exports.md): a full export (`--full`), `memoria packet view`, or an owner who authorized the legacy P1 projection.
- [integrations.md](integrations.md): a request or an error about the skill, the agent hook, or the GitHub workflow.

## Limitation

Memoria makes no proven token-savings claim. The 0.6 reading-cost experiment stays closed and incomplete.
