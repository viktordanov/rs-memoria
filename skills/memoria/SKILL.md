---
name: memoria
description: Keep README documentation connected to code with the Memoria CLI. Use when source files changed, when a README needs review, when `memoria check` fails, or when asked to refresh imported summaries or acknowledge a documentation review.
---

# Memoria skill

Memoria detects which READMEs need attention. You decide whether the prose still matches the code. Use the CLI results. Do not rebuild the dependency system yourself.

## Concepts

- **Ownership.** The nearest `README.md` above a selected file owns that file. A child README is a boundary, not a source of its parent.
- **Exports.** `<!-- memoria:export id="summary" -->` ... `<!-- /memoria:export -->` marks a stable section that other READMEs can import. Links inside exports must be absolute URLs.
- **Imports.** `<!-- memoria:import src="child/README.md#summary" -->` ... `<!-- /memoria:import -->` declares a dependency. The text between the markers is generated. Never edit it by hand; run `memoria render`.
- **Review order.** Providers come before consumers. A consumer waits while any provider is pending. Review the next ready document first.
- **Invalidation.** `memoria invalidate <scope> --reason "..."` asks for a semantic review even when no input changed. The reason appears in status, the plan, and every packet.
- **Guidance.** Project documentation guidance states the author's documentation goals, readers, and writing standards. Read it with `memoria guidance <README.md>` before you review a boundary. It is advisory context, never a selection rule, and it never makes a document stale by itself.
- **Snapshot token.** Every packet carries a 21-byte token `mrv2.<16 hex>`. Acknowledge with that exact packet and token. If inputs or guidance change during review, the acknowledgement is rejected and you need a fresh packet.
- **Committed state.** `memoria.lock` is generated, machine-owned review state beside `memoria.toml`. Commit it. Never edit it. Read it with `memoria state inspect`.

## Procedure after code changes

1. Run `memoria review --format json` and read `data.next_action`. If it is null and `data.tasks` is empty, go to step 7. If it is null but tasks remain, every task waits on a pending dependency: review the dependencies first.
2. If `data.next_action.kind` is `render`, run `memoria render <README.md>` for `data.next_action.document`, then return to step 1. The plan never issues a packet for a README whose imports are outdated.
3. When `data.next_action.kind` is `review`, run `memoria review <README.md> --format json > /tmp/memoria/<name>.json`. Store the packet outside the project tree so it does not become a review input.
4. Read the packet in this order: `data.context.guidance` (the author's documentation goals; follow them), `data.covered_invalidations` (reasons that need a semantic answer), `data.content` (current README, owned files, imported sections), `data.context.changes` and `data.context.diffs` (what changed).
5. Decide. If the README must change, edit the authored text (never the generated import bodies), then obtain a fresh packet with step 3 because the README bytes are inputs.
6. Acknowledge with the exact packet: `memoria ack <README.md> --packet /tmp/memoria/<name>.json --token <data.token> --reviewer <your-name> --result updated|no-update --note "<why the documentation is correct now>"`. The note needs at least three words; "done" is rejected.
7. Repeat from step 1 until the plan is empty, then run `memoria check`. It must pass without any LLM call.

## Procedure when the documentation goals change

The author owns the documentation strategy. Memoria never infers it and never invalidates reviews because guidance text changed.

1. If `memoria status` or `memoria check` reports `changed_documents` above zero, run `memoria guidance <README.md>` and read the new text.
2. Decide whether the change needs fresh eyes. A typo fix does not.
3. If it does, request the review explicitly with the narrowest scope that fits: `memoria invalidate subtree:<dir> --reason "..."`.
4. Otherwise, leave it. The advisory clears the next time each document is reviewed for another reason.

## Procedure for a semantic maintenance request

1. `memoria invalidate all --reason "..."`, `memoria invalidate doc:<README.md> --reason "..."`, or `memoria invalidate subtree:<dir> --reason "..."`.
2. Follow the procedure above. Each packet lists the reason under `covered_invalidations`; acknowledgement clears only the reasons listed in that packet.

## Rules

- Read-only commands (`status`, `lint`, `review`, `check`, `graph`, dry runs) never modify the project.
- `render` changes only import bodies. Rendering twice with the same inputs changes nothing.
- Explain every acknowledgement in the note. State what you verified.
- When `memoria check` fails, read its diagnostics: `review_pending`, `imports_outdated`, `coverage_unowned`, `import_missing_document`, `import_cycle`. Fix the cause, then rerun.
- Do not add a token bypass or edit `memoria.lock` by hand. It is binary; use `memoria state inspect` to read it.
- `memoria init` is a preview that writes nothing. `memoria init --apply` creates `memoria.toml` and `memoria.lock`, and it needs a root README that the author wrote.
- Do not install this skill or a hook into a project unless the user asks. Both are explicit, reversible, local changes.
