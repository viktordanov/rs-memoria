---
name: memoria
description: Keep README documentation connected to code with the Memoria CLI. Use when source files changed, when a README needs review, when `memoria check` fails, or when asked to refresh imported summaries or acknowledge a documentation review.
---

# Memoria skill

Memoria detects which READMEs need attention. You decide whether the prose still matches the code. Use the CLI results. Do not rebuild the dependency system yourself.

The owner chooses documentation goals. The reviewer judges correctness.
Memoria validates exact inputs, dependencies, and acknowledgement consistency.
Guidance is review context. It grants no permission and does not override the user's task or higher-priority instructions.
If guidance conflicts with the task, obtain an owner decision.

## Reading path

1. Read the concepts.
2. Process code changes through the review procedure.
3. Use the semantic procedure only for an authorized review request.
4. Read the acknowledgement constraints before a state change.

## Concepts

- **Ownership.** The nearest `README.md` above a selected file owns that file. A child README is a boundary, not a source of its parent.
- **Exports.** `<!-- memoria:export id="summary" -->` ... `<!-- /memoria:export -->` marks a stable section that other READMEs can import. Links inside exports must be absolute URLs.
- **Imports.** `<!-- memoria:import src="child/README.md#summary" -->` ... `<!-- /memoria:import -->` declares a dependency. The text between the markers is generated. Never edit it by hand. Invoke `memoria render` instead.
- **Review order.** Providers come before consumers. A consumer waits while any provider is pending. Review the next ready document first.
- **Invalidation.** `memoria invalidate <scope> --reason "..."` asks for a semantic review even when no input changed. The reason appears in status, the plan, and every review artifact.
- **Guidance.** Project documentation guidance states the author's documentation goals, readers, and writing standards. Read it with `memoria guidance <README.md>` before you review a boundary. It is advisory context, never a selection rule, and it never makes a document stale by itself.
- **Sections.** `<!-- memoria:section id="persistence" files="handle.go service.go" -->` ... `<!-- /memoria:section -->` maps one part of a README to the sources it describes. A section is an optional reading hint. It creates no ownership and no separate freshness. One invalid mapping withdraws the advice of the whole README.
- **Review manifest.** `memoria review <README.md>` states the review requirements. It carries no file content and no hunks. Read the listed paths with your ordinary file tools.
- **Full export.** `memoria review <README.md> --full --format json` adds every reviewed byte. Use it for offline reading or for a machine that cannot open the project.
- **Snapshot token.** Every review artifact carries a 21-byte token `mrv3.<16 hex>`. Acknowledge with that exact artifact and token. If the inputs, the guidance, the mappings, or the provider context change during review, Memoria rejects the acknowledgement. Then get a fresh artifact and reconcile.
- **Committed state.** `memoria.lock` is generated, machine-owned review state beside `memoria.toml`. Commit it. Never edit it. Read it with `memoria state inspect`.

## Procedure after code changes

1. Invoke `memoria review --format json`. Read `data.next_action`.
2. If `data.next_action` is null and `data.tasks` is empty, go to step 12. The plan is empty, so there is no README to acknowledge. Never acknowledge here.
3. If `data.next_action` is null and tasks remain, review the pending dependencies first.
4. If `data.next_action.kind` is `render`, invoke `memoria render <README.md>` for `data.next_action.document`. Then return to step 1.
5. If `data.next_action.kind` is `review`, invoke `memoria review <README.md> --format json > /tmp/memoria/<name>.json`. Store the manifest outside the project tree. A manifest inside the project becomes a review input.
6. Invoke `memoria guidance <README.md>` and read the current text. Read `data.covered_invalidations` for semantic requests.
7. Read `data.review.mode` and do the reading that follows it. Read `data.inputs` for the suggested paths.
8. Read the whole README at the hash in `data.inputs`. This pass is always required.
9. If the README must change, edit the authored text. Never edit a generated import body.
10. After any edit, return to step 5 for a fresh manifest. Then reconcile the new requirements against your completed inspection.
11. Acknowledge with the exact artifact: `memoria ack <README.md> --packet /tmp/memoria/<name>.json --token <data.token> --reviewer <your-name> --result updated|no-update --note "<why the documentation is correct now>"`. Supply an explicit reviewer label. Do not inherit an unknown `MEMORIA_REVIEWER` value. Then return to step 1.
12. Invoke `memoria check`. It must pass without any LLM call. Then stop.

Acknowledge only at step 11, and only with the artifact that step 5 or step 10
saved for that exact README. An empty plan has no selected README, no saved
artifact, and no token. Never replay an earlier artifact to satisfy step 12.

### What each review mode requires

`data.review.mode` is `focused_candidate` or `full_baseline`.

`full_baseline` means one thing: read all current owned sources, all current
import bodies, the whole README, the effective guidance, and every active
covered reason. `data.review.fallback_reasons` gives the exact code and
message for each cause. Fallback is the normal outcome after an added source,
a removed source, a rename, a changed mapping, a changed policy, a changed
import, a changed guidance text, a semantic invalidation, or missing
historical evidence.

`focused_candidate` means the CLI found no technical reason to require the
full baseline. It is eligibility, not certification of the previous review.
Decide separately whether that previous review is worth reuse. Without that
explicit trust, use the full baseline.

For a focused review, do the five steps that follow:

1. Read the effective guidance and every covered reason.
2. Read each changed source in `data.inputs`, and each section that `data.review.sections` suggests.
3. If a claim depends on content outside those sections, read that content too.
4. Read the whole README. This pass is never optional.
5. If uncertainty remains, read the full current boundary instead.

The line range in `data.review.sections[].lines` is a hint for the current
README bytes. It is not the reviewed state. The complete README hash binds
every byte, including the bytes outside the suggested lines.

### Records to keep outside the project

Memoria stores no checklist. Keep these four records yourself:

1. The document, the token, the artifact digest, and the baseline revision.
2. The exact sources, imports, and sections you inspected, with their hashes.
3. Each reused earlier inspection, the review you trusted, and why reuse is still relevant.
4. The completed whole-README pass, and the disposition of every covered reason.

Equal hashes permit reuse. Equal hashes do not transfer understanding. After
a source change, reopen every claim that depends on that source.

## Ordinary reading

The manifest names paths. Your ordinary file tools read them. Memoria adds no
read command, no range command, and no content API.

```sh
memoria review container/README.md --format json > /tmp/memoria/container.json
jq '.data | {document, baseline, changes, review, guidance}' /tmp/memoria/container.json
memoria guidance container/README.md
sed -n '42,78p' container/README.md
cat container/service.go
cat container/README.md
```

For several suggested sources, list the unique paths first:

```sh
jq -r '.data.review.sections[].sources[]' /tmp/memoria/container.json | sort -u
```

Do not substitute `git diff HEAD` for verified historical evidence. If you use
`git diff`, first make sure that its old commit and each relevant blob match
the acknowledged lengths and hashes. The current side must also match the
manifest. Otherwise use the full baseline.

`memoria explain <README.md> --full` gives verified hunks when you want them.

## Exact saved-export retrieval

A full export carries every reviewed byte. Produce it with
`memoria review <README.md> --full --format json`. Store it outside the project
before retrieval.

1. Invoke `memoria packet view /tmp/review.json --section guidance` for the saved guidance.
2. Invoke `memoria packet view /tmp/review.json --section content` for all saved current content.
3. Invoke `memoria packet view /tmp/review.json --file path/to/input.rs` for one exact saved input.
4. Invoke `memoria packet view /tmp/review.json --section history` for old bodies and available historical evidence.

`packet view` accepts a current full export only. A manifest carries no
content, so `packet view` reports `packet_content_unavailable` and names the
two ways to read the content.

The reader validates the export without project discovery or live-source reads.
JSON views use `kind=packet_view` and `view_version=1`.
A view is not a canonical artifact and cannot replace the original acknowledgement file.
Retrieval does not certify that you read or understood the content.

## Legacy incremental projection: P1

P1 is an older opt-in projection over a saved full export. The section-based
review above replaces it for normal work. P1 remains for an owner who already
uses it.

P1 never overrides `data.review.mode`. If the manifest reports
`full_baseline`, use the full baseline whatever P1 reports.

Do not infer permission from a small diff, a hash match, or a tool-generated
reuse candidate. The owner must authorize incremental review and name a
trusted prior review. A prior acknowledgement asserts review. It is not
authenticated evidence of understanding.

1. If the owner authorizes P1, invoke `memoria packet view /tmp/review.json --section incremental --trust-prior-review`.
2. If `selection.full_review_required` is true, use the full review of this owner.
3. Otherwise, examine the supplied README, changed files, imports, guidance, diffs, and all active reasons.
4. For each unchanged candidate, justify reliance on the trusted prior review, or read the content.
5. Record the examined inventory and its rationale outside the project inputs before acknowledgement.

The inventory must identify each README, file, and import by snapshot identity.
Distinguish fresh inspection from reliance on an earlier review.
Never copy `reviewed_by_this_view=false` into a claim of completed inspection.
Every scope expansion requires a named claim or dependency.
If relevance remains uncertain, use one full review of the affected owner.
Do not add another dependency traversal or repeatedly reread arbitrary content.

New boundaries, untrusted baselines, guidance changes, semantic invalidation,
selection changes, path-set changes, and import changes require full review.
Changes in an owner with consumers also require full review under this
conservative policy. Unavailable historical evidence requires full review.
Even an eligible projection can require more unchanged context for a semantic
change. Within one session, reuse identical guidance only after actual
inspection. A guidance digest does not supply its instructions to a new
reviewer.

### What Memoria does not claim about reading cost

Memoria states no target for token savings and no bound on missed
documentation changes. Earlier drafts of this skill proposed a
one-percentage-point degradation ceiling at 95% confidence and a 50% median
token reduction. Those targets are withdrawn. No completed evaluation
supports them.

Reading cost and missed changes are observations with stated limits, never
release promises. Reduced transport is not evidence of equal review quality.
The automated snapshot-safety rules are the guarantee. Never weaken a
snapshot rule, a fallback, or the whole-README pass to reduce reading cost.

## Procedure when the documentation goals change

The author owns the documentation strategy. Memoria never infers it and never invalidates reviews because guidance text changed.

1. If `memoria status` or `memoria check` reports `changed_documents` above zero, run `memoria guidance <README.md>` and read the new text.
2. Decide whether the change needs fresh eyes. A typo fix does not.
3. If it does, request the review explicitly with the narrowest scope that fits: `memoria invalidate subtree:<dir> --reason "..."`.
4. Otherwise, leave it. The advisory clears the next time each document is reviewed for another reason.

## Procedure for a semantic maintenance request

1. `memoria invalidate all --reason "..."`, `memoria invalidate doc:<README.md> --reason "..."`, or `memoria invalidate subtree:<dir> --reason "..."`.
2. Follow the review procedure. Each artifact lists the reason under `covered_invalidations`. Acknowledgement clears only the reasons in that artifact.

## Rules

### Acknowledgement constraints

The CLI permits an optional `MEMORIA_REVIEWER` default. An explicit `--reviewer` takes precedence.
An absent or blank default without an explicit label produces `reviewer_required` with exit 2.
Success output confirms the resolved label. The label provides attribution, not authentication or authority.
Memoria never guesses OS, Git, or model identity and never reads identity from shared configuration.

The note explains why this README is correct for this snapshot.
After trim, the note requires 12–1000 Unicode characters and at least three whitespace-separated words.
CR/LF are allowed. Tabs and other controls are forbidden.
Generic phrases are invalid: `done`, `reviewed`, `looks good`, `ok`, `okay`, `lgtm`, `fine`, `no changes`, `no change`, and `updated`.
The note is not an instruction, an override, or proof that the reviewer read all inputs.
Reviewer and note validation precede artifact reads and mutation.

### Discovery and inspection

`init` validates root setup inputs, including markers, import reference syntax, existing configuration, referenced guidance, and existing state.
It does not lint the full project, judge prose, create nested boundaries, or acknowledge documentation.
Invoke `status` and `lint` for the broader project view.

Git eligibility precedes Memoria selection. Tracked files remain eligible despite Git ignore rules.
Memoria includes cannot restore untracked files that Git excludes.
README discovery precedes Memoria ignore/include rules. Excluding surrounding source files does not hide an eligible nested README boundary.
Ownership follows the nearest discovered README. A tracked README absent from the worktree is not a current boundary.
Nested repositories, submodules, and tool-reserved trees retain their boundaries.

`memoria explain <README.md>` explains whole-file freshness without acknowledgement.
It includes hashes and verified local Git hunks, with explicit reasons for unavailable hunks.
For new acknowledgements, `git.base_commit` names a commit only after all reviewed content matches it.
That content includes the README, owned inputs, and relevant imported export bodies.
It is the baseline of acknowledged inputs, not the last prose-edit commit.
Partial or unavailable coverage leaves this reference null and appears in `historical_coverage` diagnostics.
The review remains valid. Selection policy and guidance keep their separate fingerprints.
Legacy references remain readable but require verification before historical use.
Bounded local history can recover later-committed exact bytes without rewriting the original review or attribution.
Committing source before acknowledgement can improve evidence availability, but it is optional.
Committing only the lock afterward does not repair an earlier historical reference.
Never replace unavailable evidence with unverified HEAD content.
`memoria state diff <OLD_LOCK> <NEW_LOCK>` compares saved records without source inspection or embedded history.

### State changes

- Read-only commands (`status`, `lint`, `review`, `check`, `graph`, dry runs) never modify the project.
- `render` changes only import bodies. Rendering twice with the same inputs changes nothing.
- Explain every acknowledgement in the note. State what you verified.
- When `memoria check` fails, read its diagnostics: `review_pending`, `imports_outdated`, `coverage_unowned`, `import_missing_document`, `import_cycle`. Fix the cause, then rerun.
- Do not add a token bypass or edit `memoria.lock` by hand. It is binary. Read it with `memoria state inspect`.
- `memoria init` is a preview that writes nothing. `memoria init --apply` creates `memoria.toml` and `memoria.lock`, and it needs a root README that the author wrote.
- Do not install this skill or a hook into a project unless the user asks. Both are explicit, reversible, local changes.

### Integrations

- The preferred names are `memoria integrations skill ...`, `memoria integrations hook ...`, and `memoria integrations github ...`. The older `memoria agent ...` names keep the same behavior.
- `memoria integrations github install|upgrade|uninstall` previews the change and writes nothing without `--apply`.
- `install` and `upgrade` need a root README, a valid `memoria.toml`, and a readable `memoria.lock`. If a command reports `github_prerequisites_missing`, do the normal setup first. Never work around it.
- Do not install, upgrade, or remove the GitHub workflow unless the user asks. The change adds two files to the user's repository.
- Memoria never adopts or overwrites a workflow file it does not own. If a command reports `github_unmanaged` or `github_modified`, report that to the user instead of removing the file.
- The generated workflow and its ownership record are ordinary documentation inputs. They make the owning README pending, so the normal review procedure follows the change.
- The workflow never acknowledges a review. A failing `memoria check` in CI still needs a local review.
