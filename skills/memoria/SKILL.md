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

1. Run `memoria review --format json`.
2. If `data.next_action.kind` is `render`, run `memoria render <DOCUMENT>` for that document. Then go back to step 1.
3. If `data.next_action` is null and `data.tasks` is empty, run `memoria check`, report the result, and stop. Never acknowledge an empty plan.
4. If `data.next_action` is null and tasks remain, the remaining documents wait for pending providers. Review those providers first.
5. Otherwise, take `data.next_action.document` as the document to review.

## Capture

1. Create a directory outside the project: `dir=$(mktemp -d)`.
2. Run `memoria review <DOCUMENT> --save "$dir" --format json`. The receipt names the saved file in `data.path`.
3. Run `memoria guidance <DOCUMENT>` and read the current text.

`--save` refuses any directory inside the Git worktree, because a saved artifact there could become a review input.

## Inspect

1. Read the saved artifact: `data.changes` with each `relationship`, `data.review`, `data.inputs`, `data.scope`, `data.downstream`, and `data.covered_invalidations`.
2. Follow `data.review.mode`. For `full_baseline`, read the complete current scope. For `focused_candidate`, read [review-details.md](review-details.md) first.
3. Read the whole document. This pass is always required.
4. Treat co-covering documents and export consumers as prompts for judgment. They are not proof that a change matters.
5. Before you add or remove a link to a document in a subfolder, run `memoria status --explain <PATH>` for a file in that subfolder. The link moves coverage.
6. If the document is wrong, edit its authored text. Never edit a generated import body. Run `memoria render` instead.

## Reconcile

1. After any edit, capture a fresh artifact with the Capture steps.
2. Compare the new requirements with the inspection you already did. Read what is new.
3. Reuse an earlier inspection only when the identities and hashes match. See [review-details.md](review-details.md).

## Record

1. Run `memoria ack <DOCUMENT> --packet <saved file> --reviewer <explicit label> --result <updated|no-update> --note "<why the document is correct now>"`.
2. Use an explicit reviewer label. Do not inherit an unknown `MEMORIA_REVIEWER` value.
3. Write a note that states what you verified. Generic notes such as `looks good` are refused.
4. Go back to Select.

The token comes from the saved artifact. Pass `--token` only to check it against a value you kept.

## Finish

Finish only after `memoria check` passes. If it fails, read its diagnostics, fix the cause, and go back to Select.

## When to load the other files

- [review-details.md](review-details.md): a `focused_candidate` mode, reuse of an earlier inspection, or a fallback reason you do not understand.
- [saved-exports.md](saved-exports.md): a full export (`--full`), `memoria packet view`, or an owner who authorized the legacy P1 projection.
- [integrations.md](integrations.md): a request or an error about the skill, the agent hook, or the GitHub workflow.

## Limitation

Memoria makes no proven token-savings claim. The 0.6 reading-cost experiment stays closed and incomplete.
