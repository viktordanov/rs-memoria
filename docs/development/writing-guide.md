# Writing Memoria documentation

Shape each page around its reader before you write a sentence. Decide what the reader must leave with, then how the page must look, then the words. This guide adapts the reader-shaped-writing approach to Memoria. The project guidance in `memoria.toml` keeps the facts, the terms, and the safety rules. This guide decides the shape of a page and how its explanation connects.

## Decide the reader first

Answer four questions before you edit a page, and keep the answers in your review note when they changed:

1. **Reader:** who reads the page, and what do they already know about Memoria?
2. **Arrival:** how do they get here: the root README, a link from `memoria review`, an error, or a search? What are they trying to do at that moment?
3. **The one sentence:** what must they know or be able to do if they read nothing else?
4. **Mode:** how do they read it? The mode decides the first screen and the skeleton.

| Mode | The reader… | First screen | Memoria pages |
| --- | --- | --- | --- |
| Decide | must choose or approve something | The choice, the recommendation, and its cost | A proposal or a decision record; the repository has none yet |
| Learn | reads once, top to bottom, to understand | The problem and the outcome in two or three sentences | `docs/concepts.md`, the cookbooks |
| Do | is in the middle of a task | The goal, what they need first, and step 1 | `docs/workflow.md`, the skill stages, a cookbook step |
| Look up | arrives for one fact, finds it, and leaves | What the page covers, and where each kind of question goes | The root README, `docs/cli.md`, `docs/state.md`, nested READMEs |

A page has one mode. A "start from your situation" list belongs only to a look-up page, and only when at least three reasons to arrive lead to different places. A how-to, a cookbook, or a concept guide never gets one.

Do not force one template on every page. The mode gives the skeleton. The subject decides which parts it needs. If an outline would fit a different kind of page just as well, it came from habit, so rebuild it from the reader's questions.

## Give every fact one home

Condensing means finding the shape of the subject, not shortening sentences. Write the reader's top questions in their words, and group the material into two to five sections that do not overlap. Then give each fact one home:

- **Here:** a question on this page needs it.
- **Elsewhere:** another page owns it. Give one line and a link. The three structures and the review cycle live in `docs/concepts.md`. The command contract lives in `docs/cli.md`.
- **Cut:** nobody on this page needs it, or the code says it plainly. A fact is not kept because it is true.

A nested README describes one boundary. Import a short summary from it instead of repeating its sections elsewhere.

## Connect the explanation

The goal is to lower what the reader must hold in mind, not what the reader can learn. A page is heavy when facts arrive without the links between them, or when one idea is explained in five places. A page is not heavy because it explains why.

- Open each paragraph with the idea that connects its facts, not with the first fact.
- Join related sentences with the word that states their relation: because, so, unless, instead of, as a result.
- Never split a sentence if the split deletes its "because" or "unless". A longer sentence with one clear relation is better than two short sentences that lose it. This rule takes priority over a sentence-length target.
- Give every rule that the reader must obey a one-sentence reason, or a link to one. Never invent a reason. If the sources give none, report the gap.
- Show each core distinction side by side: two states or two outputs that differ in one thing. For example, a focused candidate next to a full baseline.
- Give a small real case before the general rule: a real file tree, a real command, and its real output.
- Explain the central model once. Elsewhere, give one line and a link.

## Choose the form by the content

Choose the form of each block from the shape of its content. Variety is never a reason.

| If the content is… | Use |
| --- | --- |
| A claim with its reason, a tradeoff, or a consequence | Prose, two to five sentences |
| Steps in order | A numbered list, one action for each step, with its result |
| Parallel, independent items | A bulleted list of about seven items at most, or groups |
| Items compared on two or more attributes | A table with two named dimensions |
| Folders, ownership, and files | A text tree with notes |
| Relations between several parts | A diagram with labelled arrows |
| An exact command, configuration, marker, or output | A code block, copied from a real run |
| A rule that the reader must not miss | Prose at the step, with the condition first |
| Detail that only some readers need | A late section, a collapsed block, or a link |

Bullets that contain "because" or "so" hide reasoning, so write them as prose. A table with one meaningful column is a list.

## Diagrams

Draw a diagram only when you can finish the sentence "a list or a table cannot show this because…". Write its one-sentence claim first. Name every box by its role, label every arrow in its direction, and place the diagram directly after the paragraph that sets it up. One diagram makes one claim.

Use GitHub-compatible Mermaid inside Markdown everywhere in this repository, as the project guidance says. The one exception is `docs/cookbooks/`. Cookbook pages are where the project tries out its visual style, so they can use SVG diagrams drawn with `scripts/ascii-diagram.py`, in the style that [the cookbook index](../cookbooks/README.md#diagram-style) defines. Do not carry that SVG style to other pages unless the project guidance changes.

## Check before you acknowledge

After an edit, check the page as a stranger would:

1. Read only the headings and the first sentence under each. They must give a correct summary that answers the reader's top three questions.
2. Read only the first screen. It must match the mode: a do page opens with the goal and step 1, a look-up page with what it covers and where to go, a learn page with the problem and the outcome.
3. Check that each enforced rule has a reason, each term is defined at or before its first use, and each relative link points to a file that exists.
4. Compare every command, marker, path, identifier, number, and quoted output with its source. Cookbook outputs come from their tests.

Then capture a fresh artifact and read the changed prose against the project guidance before you acknowledge.
