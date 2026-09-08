# Memoria

## Specification status

**Status:** Approved contract for release 0.2.0. The implementation follows it.

**Purpose:** Memoria tracks documentation freshness across machines.

File names, command names, and configuration keys in this document are the released names.
The [command reference](cli.md) gives the exact arguments and diagnostics.

In this document, **must** means required. **Should** means preferred. **May** means optional.

### Portability contract

Two mechanisms decide different questions, and this separation is the core of the release:

| Mechanism | Authority | Host settings |
| --- | --- | --- |
| Actual file eligibility | The Git CLI adapter | Apply normally. |
| Repository policy inventory | Repository `.gitignore` bytes only | Do not read or apply. |

Equal selected inputs and equal repository policy produce equal freshness across hosts.
The guarantee needs equal selected paths, equal bytes, equal document boundaries, and equal repository rules.
It does not normalize case-only paths, Unicode filenames, or line endings.

This table gives the required result for each change:

| Change | Required result |
| --- | --- |
| A host rule changes without changing selected files or bytes | No policy change and no new pending review. |
| A host rule removes an untracked selected file | The owning manifest loses that file, and the owner requires review. |
| A host rule exposes an untracked file | The new owner gains that file and requires review. |
| A host rule matches a tracked file | The file stays selected, and its byte changes still require review. |
| A repository `.gitignore` rule changes | Applicable repository policy changes, even without a selected-file change. |
| A Memoria selection rule changes | Applicable Memoria policy changes. |
| Selected content changes with an identical size or timestamp | Its raw byte hash changes, and the owning document requires review. |

The repository policy inventory uses `gix-ignore` inside the infrastructure boundary.
It starts with an empty search, adds only the supplied repository rule buffers, and uses fixed case-sensitive matching.
It never loads host-local exclusions.
Fixed case behavior makes the inventory portable; actual eligibility still follows the host Git behavior.

## On this page

- [Purpose](#1-purpose)
- [Repository rules](#2-repository-rules)
- [Documentation links](#3-documentation-links)
- [Change detection and review](#4-change-detection-and-review)
- [CLI, agents, and delivery scope](#5-cli-and-agent-support)

## 1. Purpose

### 1.1 The problem

Code changes. Documentation can stop matching the code.

A root README cannot explain every detail of a large project. Local READMEs can explain each part.
But readers also need to understand how those parts fit together.

Memoria must help maintain both views: local detail and the larger project model.

### 1.2 The product

Memoria is a separate, repository-independent command-line tool. It should ship as a small binary.

It must detect changed documentation inputs, identify affected READMEs, and prepare focused reviews.
It must also track the summaries that READMEs use from other READMEs.

```text
Code changes
→ Memoria identifies affected documentation
→ A human or agent reviews it
→ Memoria refreshes imported summaries
→ Memoria records the review
→ A final check confirms that no required work remains
```

Maintenance normally happens after implementation and tests settle.

### 1.3 The boundary

**Memoria detects changed inputs. It does not prove that documentation is correct.**

The tool handles file selection, hashes, dependencies, generated text, and review state.
A human or an LLM decides whether the explanation still matches the implementation.

An LLM review may finish without human approval. Repository policy may add approval, but Memoria must not require it.

### 1.4 The main value

The tool must reduce repeated work for people and agents.

It should make these questions easy to answer:

- Which documentation needs attention, and why?
- What does each README cover?
- How do the documented parts connect?

The graph shows the **documented project model**. It does not claim to discover every dependency in the code.

## 2. Repository rules

### 2.1 Project files

**Layout:**

```text
project/
├── memoria.toml              # Root configuration
├── memoria.lock              # Latest review records, generated
├── .gitattributes            # Contains: /memoria.lock binary
├── README.md                 # Project overview
└── src/
    └── retrieval/
        ├── README.md         # Local explanation
        ├── README.memoria.toml # Optional local settings
        └── engine.py
```

Configuration and review state must be ordinary files that Git can track.
The state file must not require a database or hosted service.

Keep large configuration blocks out of README front matter.
Use an optional sidecar file for local settings. Use short HTML comments for documentation relationships.

### 2.2 Find the project

**Default:** Find the Git root from the current directory. Read the root configuration there.

Commands should work from any directory inside that project.
An explicit root option must be available when discovery is not suitable.

Support for repositories without Git is an open scope decision.

### 2.3 Select the inputs

Memoria must respect repository Git ignore rules, including rules in subdirectories.
Memoria-specific ignores must also support glob patterns.

**Selection rule:** Start with tracked files and untracked files that Git does not ignore.
Then apply Memoria rules.

Under this rule, a tracked file stays eligible even when it matches a Git ignore pattern.
Use a Memoria ignore rule to exclude it.

This matters for committed generated code. Initialization must prompt the user or agent to review generated output, snapshots, and fixtures.

Tests may be excluded. They must not be excluded by default without a project decision.
Some tests or fixtures may be important inputs to a README.

### 2.4 Inherit local rules

Root rules apply across the project. Local rules apply within their scope and pass down to child scopes.

**Precedence:** A more specific local rule overrides an inherited Memoria rule.
A local `include` can restore a file excluded by Memoria.
It does not restore a file excluded by the Git selection step.

Local patterns are relative to the directory that contains the local configuration.
Root patterns are relative to the project root.

The tool must be able to explain why a file was included or excluded.

### 2.5 Assign ownership

**The nearest README above a selected file owns that file.**

A README owns its directory and lower directories until another README creates a boundary.

```text
README.md                       owns app.py and src/common.py
app.py
src/
├── common.py
└── retrieval/
    ├── README.md               owns engine.py and helpers/rank.py
    ├── engine.py
    ├── helpers/
    │   └── rank.py
    └── naive/
        ├── README.md           owns search.py
        └── search.py
```

Ownership requires no file-by-file declarations in the normal case.
A file must have at most one owner.

A child README is a document, not an ordinary source input of its parent.
Changes in its text propagate through declared imports, not through recursive file hashing.

Adding, moving, or deleting a README must recalculate ownership.
Changes to an owner's selected file set must invalidate its previous review.

**Default:** Recognize `README.md`. Report selected files that have no owner as coverage errors.
A root README normally prevents these gaps.
Do not follow paths outside the project or enter nested repositories automatically.

### 2.6 Root configuration

**Example:**

```toml
version = 2
ignore = ["**/generated/**", "**/*.snap"]

[documentation]
guidance = [
    "Use Simplified English.",
    "Keep sections short.",
    "Explain each part before its details.",
]
guidance_files = [
    ".agents/simplified-english.md",
    ".agents/adhd-writing.md",
]

[fingerprints]
default = "raw"
languages = {}
```

An empty `languages` map means that no language-specific filter is enabled.
Instruction paths above are examples. Each project supplies its own files.

**Local exception:**

```toml
# compiler/README.memoria.toml
include = ["fixtures/**"]
```

This restores compiler fixtures only when an inherited Memoria rule excluded them.

## 3. Documentation links

### 3.1 Keep the Markdown useful on its own

READMEs must remain useful in GitHub and ordinary Markdown viewers.
Readers must not need Memoria to read the explanations or copied summaries.

Authors explain each part locally. Parents explain how the parts fit together.
A parent should consume a short child summary, not repeat the child's full explanation.

### 3.2 Export a stable section

An **export** is a marked section that another README can use.
Its identifier must stay separate from its visible heading.

**Syntax in `retrieval/README.md`:**

```markdown
<!-- memoria:export id="summary" -->
## Retrieval

Retrieval selects documents that are relevant to a query.
<!-- /memoria:export -->
```

The heading may change without changing the export identifier.
Identifiers must be unique within a document.

### 3.3 Import that section

An **import** declares a dependency and marks where Memoria writes a copy.

**Syntax in the root README:**

```markdown
<!-- memoria:import src="retrieval/README.md#summary" -->
## Retrieval

Retrieval selects documents that are relevant to a query.
<!-- /memoria:import -->
```

The path is relative to the importing README.
The text between the import markers is generated. The surrounding text belongs to the author.

Humans and agents create the relationships. Memoria validates them and maintains the copied text.
It must not invent architectural claims.

### 3.4 Keep rendering narrow

Rendering must change only declared import blocks. It must preserve all other text.
Running it twice with the same inputs must produce the same file.

Missing exports, duplicate identifiers, and malformed blocks must produce clear errors.
Markers shown inside fenced code examples must not act as declarations.

**Limit:** Blocks cannot overlap or nest.
Parents write their own exported summaries outside imported blocks.

Links and images must remain valid when copied into another directory.
The first release may reject forms that it cannot safely render.
It must not silently produce broken links.

### 3.5 Keep two structures separate

The **ownership tree** comes from folders and README boundaries.

The **import graph** comes from explicit imports. It may include cross-folder dependencies.
It must be a directed acyclic graph: a graph with no dependency loops.

Normal Markdown links may contain loops. Only import loops are errors.
An error must show the complete loop and the import locations.

The default authoring pattern should be simple: parents import child summaries.
Cross-folder imports are available when needed. No automatic cycle repair is required.

### 3.6 Treat normal links as links

A normal Markdown link must not create a review dependency.
It may only mean “read this related page.”

Lint may report a local README link with no matching import as a hint.
The hint must be suppressible and must not fail CI by itself.

No custom categories such as “core,” “useful,” or “optional” are required initially.

### 3.7 Find missing connections

Automatic ownership does not guarantee that a reader can find a README.

**Orphan rule:** A README is disconnected when no path from the root README reaches it through local links or imports.
This is a navigation warning, not an ownership failure.

The graph must still show disconnected READMEs. It must not hide them.
Lint should identify their paths and owned file counts.

## 4. Change detection and review

### 4.1 Use content fingerprints

A **fingerprint** is a hash of the inputs used for a review.
Use XXH3-64 with the default secret and seed zero.
Cryptographic proof is outside the intended threat model.

Include selected file paths and content hashes. This must detect additions, deletions, renames, and content changes.
Use a stable order and an unambiguous record format.

Imported inputs must include the target identity and the actual exported section content.
Unrelated text elsewhere in the exporting README must not invalidate consumers.

A commit identifier or timestamp must not decide review validity.
A rebase with identical inputs must not require another review.

### 4.2 Make formatting filters optional

Raw content hashing is the default.
A project may opt into a language-specific filter that removes known formatting-only changes.

Do not use a generic “remove whitespace, comments, and quotes” rule.
A language-aware implementation can still be wrong. Each filter needs a narrow, stated guarantee and tests.

If a file cannot be parsed safely, fall back to raw hashing and report the fallback.
The filter name, version, and settings must be part of the fingerprint policy.

Python remains raw in the initial configuration. No filter is enabled automatically.
Newline conversion also needs an explicit policy; do not assume all content treats newlines the same way.

The first release may have no filters. Any initial filter must be a separate, optional feature.
A broad AST framework is not required for the core pilot.

### 4.3 Track policy changes

Ownership changes, ignore changes, and filter changes can change what a review covers.
Memoria must detect changes to the effective review inputs and policy.

Project documentation guidance is review context only.
Changing a guidance entry or a guidance file must not automatically make documentation stale.
Memoria cannot deterministically prove that existing prose follows a semantic writing rule.

Do not hash review timestamps, documentation guidance, host ignore rules, or the state file as source inputs.
Otherwise, recording a review or changing review guidance could trigger unrelated source-based reviews.

Each review stores the guidance digest that its reviewer saw.
Status, the review plan, and `check` report a changed digest as an advisory.
The advisory persists until a real review acknowledges the new guidance.
No command manufactures a review record to dismiss it.

The guidance digest uses XXH3-64 with seed 0 and the canonical domain `memoria.guidance.v1`.
The encoding starts with that domain and the entry count, and then each entry's four length-prefixed UTF-8 fields.
It stays outside the policy hash, the input manifest, the review schedule, and the import propagation rules.

### 4.4 Explicitly invalidate documentation

A human or agent may know that documentation needs semantic review even when source fingerprints have not changed.
Memoria must support explicit invalidation for this case.

Examples include:

- Rewrite documentation using Simplified English.
- Replace outdated terminology.
- Review a subsystem after an architecture decision.
- Add failure-mode explanations across a documentation tree.

The command is:

```sh
memoria invalidate <scope> --reason "<reason>"
```

The scope may be one README, a documentation subtree, or the whole project.
The exact scope syntax is proposed, but the capability is required.

Memoria must store the invalidation in its state file.
It must not edit every README just to insert the reason.
The reason must appear in `status`, the review plan, and every affected focused review packet.

An explicitly invalidated README remains pending until it is reviewed against that invalidation.
Acknowledgement clears only the invalidations that were included in the reviewed snapshot.
If another invalidation is added during review, the older acknowledgement must not clear it.

Explicit invalidation is separate from input staleness:

```text
Input staleness
= deterministic change to owned sources, imports, ownership, or fingerprint policy

Explicit invalidation
= semantic review requested by a human or agent with a reason
```

Both kinds of pending work may exist at the same time.
The review packet must present all active reasons together.

### 4.5 Store the latest review

Use one versioned state file: `memoria.lock`, beside `memoria.toml` at the worktree root.
It is generated, machine-owned binary state, at format version 2.
Keep the latest review for each README. Do not append an endless journal to the README.

The artifact must be deterministic, portable, bounded, atomic, corruption-detecting, and safe to commit.
One logical state must produce one byte sequence.
It must hold only the information that the review, acknowledgement, invalidation, Git, and migration contracts need.
Historical event logging is outside this release.

The frame is a magic value, a format version, a codec, a decoded length, a body, and an XXH3-128 checksum.
The payload normalizes records into canonical tables of strings, paths, repeated content descriptors, guidance digests, Git contexts, and integer vectors.
The [state guide](state.md) gives the exact layout, the limits, and the measured sizes.

The release has one clean cutover.
It does not support simultaneous old and new state formats.
The legacy path is `.memoria/state.json` and the current path is `memoria.lock`.
A lone `.memoria/state.json` produces `state_legacy`; both files together produce `state_ambiguous`.
There is no automatic migration and no reserved alias.

Git history can retain older committed records. Uncommitted replacements are not a full audit history.

Each record must identify the document, reviewed inputs, review time, reviewer, result, and short note.
It should also record the available base commit and whether the worktree was dirty.
Those Git fields provide context only.

A result must distinguish between **documentation updated** and **reviewed; no update needed**.
Both outcomes can make a review current.

**State details:** Retain per-file and per-import hashes so the tool can explain changed inputs.
Also record the reviewed README content, excluding tool-owned metadata.
Later document edits can then invalidate that review without affecting unrelated consumers.

### 4.6 Prepare a focused review

The review plan must group work by README, not create one task for every changed file.
A hundred changed files under one owner are one review task, not a hundred separate reviews.

Each task must explain the cause and provide the relevant context:

- Owned files, changed paths, and available diffs.
- Imported sections, exports, and affected consumers.
- The previous review and applicable documentation guidance.

Show input size before asking an agent to consume a large packet.
Prefer a change summary and focused content over dumping the whole repository.
Do not silently omit changed inputs.

A hash alone cannot reproduce old file contents.
When the old content is unavailable, label the diff as unavailable and provide the current inputs.
Do not present a diff from a different snapshot as the reviewed diff.

### 4.7 Review a fixed snapshot

The CLI must identify the exact input snapshot that a reviewer receives.
Acknowledgement must name that snapshot, not whatever happens to be current later.

**Interface:** Return a review token with the packet. Require the same token when recording the result.

Before writing the record, the CLI must check the inputs again.
If they changed during review, it must reject the acknowledgement and explain the difference.

State writes must be atomic. Conflicting updates must not silently replace another review.
An LLM may acknowledge directly, but it must supply a short reason. “Done” is not sufficient.

### 4.8 Propagate only relevant changes

A source change first makes its owning README pending.
It does not immediately require every ancestor to review its prose.

Review dependencies before their consumers. Within the normal hierarchy, this means children before parents.
Cross-folder imports must also respect dependency order.

```text
Implementation changes
→ Local README is reviewed
→ Its exported summary stays the same
→ Stop: no consumer review is required
```

```text
Implementation changes
→ Local README is reviewed
→ Its exported summary changes
→ Refresh the consuming README's import
→ Review that consumer
→ Continue only if its own exported content changes
```

While a dependency is pending, show consumers as waiting for it where necessary.
Do not ask an agent to review them against a summary that may still change.

## 5. CLI and agent support

### 5.1 Command surface

The exact names are proposed. These capabilities are required across the planned product.

| Command | Purpose |
| --- | --- |
| `memoria init` | Create root configuration and guide initial setup. |
| `memoria status` | Show coverage, input size, and review state. |
| `memoria lint` | Check structure, configuration, markers, and link hints. |
| `memoria review` | Show the ordered review plan. A document path selects a focused packet. |
| `memoria render` | Refresh declared import blocks only. |
| `memoria ack` | Record a selected document's review result against its snapshot. |
| `memoria check` | Run read-only validation for CI. |
| `memoria graph` | Show documentation ownership, imports, and current status. |
| `memoria invalidate` | Mark one README, a subtree, or the whole project for semantic review with a reason. |
| `memoria agent install` | Install the managed skill for supported agents. |
| `memoria agent uninstall` | Remove or restore files managed by that installation. |

Review plans and checks must support human-readable and structured JSON output.
Read-only commands must not call an LLM or alter project files.

### 5.2 Show the footprint

Status must make excessive input scope easy to spot.
Show README count, selected file count, input bytes, and review counts.
Show disconnected documentation and files without an owner.
Show active explicit invalidations, their reasons, and how many READMEs remain pending for each one.

**Example output:**

```text
READMEs         12
Selected files  847
Input size      6.2 MiB
Reviews         9 current, 2 pending, 1 never reviewed
Invalidations   1 active, 7 READMEs pending
Navigation      1 README not reachable from the root
```

Explain exclusions when requested.
Do not scan large ignored directory trees just to produce exact ignored-file counts.

### 5.3 Show the documented architecture

The graph must include all discovered READMEs, including disconnected ones.
Distinguish automatic ownership edges from declared import edges.

Overlay review state without implying that the graph is a complete code-dependency map.
A source change must be distinguishable from a change in documentation structure.

Start with a terminal view and a machine-readable export.
A web application or advanced graph editor is not required.

### 5.4 Maintain one canonical skill

The Memoria project must contain the source skill that explains how to use the tool.
Small installation adapters may package it for Codex and Claude.

This is not a runtime plugin framework. It is a managed skill bootstrap.
It must not require Memoria to run its own hosted LLM or maintain a separate review engine for each agent.

The skill must explain ownership, imports, review order, generated regions, acknowledgement, and the final check.
It must teach agents to use CLI results instead of reconstructing the dependency system themselves.

### 5.5 Install and uninstall safely

Installation must detect supported skill locations and allow a custom path.

**Commands:**

```sh
memoria agent install
memoria agent install --target codex --path /custom/skills
memoria agent install --target claude --dry-run
memoria agent uninstall --target codex
```

The adapter must follow the target agent's supported layout.
This specification does not freeze vendor-specific directory paths.

Before writing, display the target agent, destination, and file changes.
Back up existing files that will be replaced. Provide a dry-run mode.

Prefer a dedicated Memoria skill directory. Never replace shared agent configuration wholesale.
Installation must not grant new agent permissions or execute project-supplied code.

Keep an installation record with managed paths, versions, backups, and installed content hashes.
Repeated installation of the same content should make no changes.
A failed installation must not leave a partly installed package.

Uninstall must remove only Memoria-managed content and restore replaced content when safe.
If the user edited an installed file, preserve it and report the conflict.
Never silently overwrite those edits with a backup.

### 5.6 Add project documentation guidance

The root configuration may contain short writing rules and references to guidance files.
Memoria must include the applicable guidance in each focused review packet.
That guidance informs the reviewer. It does not change fingerprints or staleness.
When a project wants existing documentation reviewed against a new rule, it should create an explicit invalidation with a reason.

A missing guidance file must produce a clear error. Do not silently skip it.
The canonical skill must tell the agent to read these project rules.

Simplified English and ADHD-friendly structure are project preferences, not hard-coded rules for every user.
A separate LLM style-linting service is not required.

### 5.7 Finish with a deterministic check

A typical source-change workflow is:

```text
Implement and test
→ memoria review
→ Review or edit the next dependency-ready README
→ Render imports before reviewing their consumers
→ Acknowledge each reviewed snapshot
→ Repeat until the plan is clear
→ memoria check
```

A semantic maintenance workflow may start instead with:

```text
memoria invalidate <scope> --reason "..."
→ memoria review
→ Heal and acknowledge affected READMEs in dependency order
→ memoria check
```

CI must fail for unresolved required reviews, coverage errors, invalid imports, cycles, or outdated generated blocks.
It must provide the affected paths and the reason.

Optional lint hints must remain separate from failures.
CI checks matching inputs and valid structure. It does not certify semantic correctness.

Document pre-commit hook setup. Automatic hook installation is outside the initial scope.
A hook should use the deterministic check, not trigger an unexpected LLM review.

## 6. Delivery scope

### 6.1 Core pilot

Build the review loop before the convenience features.

The pilot includes source selection, nearest-README ownership, raw XXH3-64 fingerprints, explicit section imports, the state file, and explicit semantic invalidation.
It also includes review planning, focused packets, rendering, acknowledgement, and a deterministic final check.

Use a three-level documentation example to test the full workflow.
DIA is the first intended consumer, including corpus types, NaiveRAG, and shared execution documentation.
DIA-specific knowledge belongs in DIA, not in Memoria.

### 6.2 First-release additions

Add the footprint view, graph display, navigation warnings, and missing-import hints.
Add project documentation guidance and the safe Codex and Claude skill installers.

Local override details and any first language filter must be tested before release.
A filter must not delay the core pilot or become a prerequisite for using Memoria.

### 6.3 Not included initially

Do not build repository-wide automatic rewriting, inferred semantic dependencies, or a hosted review service.
Do not add cross-repository imports, a rich link ontology, automatic cycle repair, or an append-only review journal.

No editor integration, documentation hosting, broad AST framework, or automatic hook management is required.
The internal document model may support other Markdown documents later. READMEs are the initial interface.

### 6.4 Acceptance tests

| Scenario | Required result |
| --- | --- |
| A selected source file changes. | Its owner needs review. Unrelated branches do not. |
| A selected file is added, deleted, or renamed. | The affected owner's input set changes. |
| A child README creates a new boundary. | Ownership and affected review inputs are recalculated. |
| An ignored generated file changes. | No review is required for that file. |
| A local include restores a Memoria-excluded fixture. | The fixture appears in its owner's inputs. |
| A tested filter sees a supported formatting-only edit. | The documentation fingerprint stays unchanged. |
| A filter fails or sees unsupported syntax. | Raw hashing is used and the fallback is visible. |
| A reviewer finds no documentation change necessary. | A note records the review without forcing prose edits. |
| A guidance entry changes. | Existing READMEs do not become stale automatically. The new entry appears in future review context, and status reports an advisory. |
| The whole project is explicitly invalidated with a reason. | Every README in scope becomes pending and receives the same semantic review reason. |
| Only one subtree is explicitly invalidated. | READMEs outside that scope remain current unless another rule affects them. |
| A README is reviewed against an explicit invalidation. | Acknowledgement clears that invalidation for that README without requiring a source change. |
| A new invalidation is added after a review packet is created. | The older acknowledgement does not clear the new invalidation. |
| Inputs change after the review packet is created. | Acknowledgement is rejected. |
| A rebase changes commits but not reviewed content. | The review remains current. |
| An exported summary changes. | Only its actual consumers become affected. |
| Unrelated prose changes outside that export. | Consumers of that export stay current. |
| Imports are rendered twice. | The second run changes nothing; authored text is preserved. |
| An import is missing or cyclic. | The tool reports the exact reference or cycle. |
| A README has no navigation path from the root. | It remains visible and receives a warning. |
| A normal link has no explicit import. | It remains a normal link; any hint is non-blocking. |
| An installed skill was edited locally. | Uninstall preserves the edit and reports a conflict. |
| The workflow has no unresolved required work. | The deterministic CI check passes without an LLM. |

### 6.5 Deferred scope

These items are outside this release:

- The README graph explorer and its localhost server.
- Automatic documentation authorship and automatic semantic invalidation from guidance text.
- Historical event logs, trained compression dictionaries, and custom Git merge drivers.
- Automatic migration of version 1 review state and alternate fingerprint algorithms.
- Global hooks, other hook events, other agents, and isolated Claude hooks for linked worktrees.

These platform items are also outside this release:

- Windows support, network filesystem durability, and Unicode or case normalization of paths.
- Automatic client trust, permission changes, and plugin installation.
- Automatic cleanup of persistent installation locks or legacy skill directories.

Keep these choices separate from the main invariant:

> **A README needs review when its declared review inputs change or when it is explicitly invalidated with a reason. Review must not spread to consumers because of unrelated changes upstream.**
