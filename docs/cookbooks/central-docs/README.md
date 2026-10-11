# Cookbook: a central docs/ site

A page in `docs/` never sees a code change in `src/`, because a document covers only its own folder and a link to the code carries no freshness. This cookbook shows that pitfall first. Then it shows the pattern that works: a README beside the code exports a short summary of it, and the docs page imports that summary, so a change to the summary reaches the page.

Choose this pattern when readers use one product site and each code area can keep a short, stable summary. The cost is two reviews for one behavior change: the README beside the code first, then the docs page. Every output below comes from a real, tested run of one small project. Read [Memoria concepts](../../concepts.md) first if scopes, handoffs, and imports are new to you, and the [cookbook index](../README.md) to compare the other patterns.

## Contents

1. Set up
   - [The problem](#the-problem)
   - [The project](#the-project)
2. The pattern
   - [The pitfall: a page that only links the code](#the-pitfall-a-page-that-only-links-the-code)
   - [The fix: import a summary from beside the code](#the-pattern-import-a-summary-from-beside-the-code)
3. What happens when
   - [A code change keeps the summary true](#a-code-change-that-keeps-the-summary-true)
   - [A code change changes the summary](#a-code-change-that-changes-the-summary)
   - [An export body has a relative link](#an-export-body-with-a-relative-link)
4. [Tradeoffs](#tradeoffs), including when not to adopt the pattern

## The problem

Many projects keep their product documentation in one `docs/` folder, away from the code.
The pages there describe behavior that lives in `src/`: limits, defaults, commands.
When the code changes, a page can keep the old fact, and nothing tells the team.

A tracked document covers the files in its own folder and below it.
So a page in `docs/` covers `docs/` only. It never covers `src/`, and a code change never makes it pending.
A link from the page to the code does not change this. A link is navigation, and it carries no freshness.

The fix uses the one edge that carries freshness between documents: an import.
A README beside the code covers the code and exports a short summary of it.
The docs page imports that summary. When the summary changes, the docs page becomes pending.

**Limits.** Memoria does not write or judge the prose. A reviewer judges each page.
A passing `memoria check` proves that every page matches its last review and that imports are rendered. It does not prove that a page is correct.

## The project

Keyhole is a small sign-in service. Its user guide lives in `docs/`, and its sign-in limits live in `src/auth/login.rs`:

```text
project/
├── memoria.toml
├── README.md              # links docs/guide.md and src/auth/README.md
├── Cargo.toml
├── docs/
│   └── guide.md           # tracked: the user guide
└── src/
    ├── main.rs
    └── auth/
        ├── README.md      # tracked: exports the summary of the limits
        └── login.rs       # the limits
```

The root README links both pages. Each link hands a subfolder to a tracked document there, so the root covers only `Cargo.toml` and `src/main.rs`:

<!-- cookbook-file: README.md -->
```markdown
# Keyhole

Keyhole is a small sign-in service.

- The [user guide](docs/guide.md) explains how to sign in.
- The [auth module](src/auth/README.md) explains the sign-in code.
```

The limits are two constants:

<!-- cookbook-file: src/auth/login.rs -->
```rust
/// Minutes without activity before a session ends.
pub const SESSION_MINUTES: u32 = 30;

/// Failed sign-in attempts before an account locks.
pub const MAX_ATTEMPTS: u32 = 5;
```

The root `memoria.toml` sets one line of project guidance, and it turns off the hint for links without an import:

<!-- cookbook-file: memoria.toml -->
```toml
version = 3
ignore = []
include = []

[documentation]
guidance = [
    "Write for a user of Keyhole. Use short sentences, and keep commands, paths, and limits exact.",
]
guidance_files = []
section_guidance_files = []

[lint]
# The root README links its pages for navigation, not to import them.
missing_import_hint = false
```

The test fixture is [`tests/fixtures/central-docs`](../../../tests/fixtures/central-docs/), where the README files carry placeholder names.

## The pitfall: a page that only links the code

### The guide

In this first version, the guide repeats the limits in its own words and links the code:

<!-- cookbook-file: docs/guide.md (link only) -->
```markdown
# User guide

<!-- memoria:export id="intro" -->
Keyhole signs you in to the team tools.
<!-- /memoria:export -->

## Sign in

Open the sign-in page and enter your email and password.
A session ends after 30 minutes without activity. After 5 failed sign-in attempts, the account locks.

If your account locks, ask an administrator to unlock it.
The [login code](../src/auth/login.rs) holds these limits.
```

The `intro` export makes the guide a tracked document. Every document is current at the start.

### The code changes

Sessions now end after 15 minutes:

```diff
 /// Minutes without activity before a session ends.
-pub const SESSION_MINUTES: u32 = 30;
+pub const SESSION_MINUTES: u32 = 15;
```

The guide still says 30 minutes. Run `memoria review`:

<!-- cookbook-output: pitfall-plan -->
```text
Review plan: 1 pending document (dependency order)
  3. src/auth/README.md  README · input changed: src/auth/login.rs · ready
Next: memoria review src/auth/README.md
```

Only `src/auth/README.md` is pending. The number `3.` is the position of the README in the dependency order of all three documents.

### Who covers the file

Run `memoria status --explain src/auth/login.rs`. It prints the project status, then the explanation:

<!-- cookbook-output: pitfall-explain -->
```text
Documents       3: 2 READMEs, 1 opted-in document; 2 handoffs; 0 sources covered by more than one document
Selected files  3
Input size      341 B
Reviews         2 current, 1 pending, 0 never reviewed, 0 waiting
Invalidations   0 active, 0 documents pending
Navigation      0 document(s) not reachable from the root
Coverage        0 selected file(s) that no document covers
Excluded        reserved:configuration=1, reserved:document=2, reserved:state=1

Documents:
  current          README.md
  current          docs/guide.md
  pending          src/auth/README.md  [input_changed]

Explain src/auth/login.rs
  outcome  selected
  reason   eligible in Git and matched by no Memoria rule
  covered by src/auth/README.md
  handed off by README.md (link at README.md:6) to src/auth/README.md
```

`src/auth/README.md` covers the file. The guide does not. Run `memoria graph` to see each scope:

<!-- cookbook-output: pitfall-graph -->
```text
Nodes:
  current          README.md (2 file(s))
  current          docs/guide.md (0 file(s))
  pending          src/auth/README.md (1 file(s))
Edges:
  handoff README.md -> docs/guide.md
  handoff README.md -> src/auth/README.md
  link    README.md -> docs/guide.md
  link    README.md -> src/auth/README.md
```

The guide covers 0 files. `docs/` holds no source, and a tracked document is never a source.
The graph shows no edge from the guide to `login.rs`. A link to a source file is navigation only.
The guide drifts, and no review asks anyone to read it again.

### A section cannot reach the code

A section marker points a review at part of a document. It names files with `files`.
Suppose that the guide wraps its `Sign in` section in `<!-- memoria:section id="sign-in" files="../src/auth/login.rs" -->`. Run `memoria lint`:

<!-- cookbook-output: pitfall-section -->
```text
warning [section_mapping_invalid]
  Path: docs/guide.md
  Line: 7
  Column: 1
  section path "../src/auth/login.rs": path must not contain a `.` or `..` component

Documents 3 (2 README(s)): 0 error(s), 1 warning(s), 0 hint(s)
```

A section path is relative to the document's folder, and it cannot leave that folder.
A section names only files in the document's own scope. It never adds a file to the scope.

## The pattern: import a summary from beside the code

The pattern gives a code change one way into `docs/`. The README beside the code covers `login.rs`, so every code edit makes that README pending. The guide imports the README's summary, so only an edit to that summary makes the guide pending. The guide's link to the code stays navigation only.

![A code change reaches the docs page only through the imported summary: any edit to login.rs makes src/auth/README.md pending, and only an edit to its exported summary crosses into docs/ and makes docs/guide.md pending. The guide's link to login.rs carries no change.](scope.svg)

### Export the summary beside the code

`src/auth/README.md` covers `src/auth/`. It explains the code for a contributor, and it exports one short summary for other pages:

<!-- cookbook-file: src/auth/README.md -->
```markdown
# Auth

`login.rs` holds the sign-in limits as constants.
Change a limit there, then update the summary below.

<!-- memoria:export id="summary" -->
A session ends after 30 minutes without activity. After 5 failed sign-in attempts, the account locks.
<!-- /memoria:export -->
```

An export alone does nothing. It becomes an edge only when another document imports it.

### Import the summary in the guide

The guide replaces its own copy of the limits with an import block:

<!-- cookbook-file: docs/guide.md -->
```markdown
# User guide

<!-- memoria:export id="intro" -->
Keyhole signs you in to the team tools.
<!-- /memoria:export -->

## Sign in

Open the sign-in page and enter your email and password.

<!-- memoria:import src="../src/auth/README.md#summary" -->
A session ends after 30 minutes without activity. After 5 failed sign-in attempts, the account locks.
<!-- /memoria:import -->

If your account locks, ask an administrator to unlock it.
The [auth module](../src/auth/README.md) explains the code behind these limits.
```

1. Write the import markers with an empty body. The `src` path is relative to the importing document, so it starts with `../`.
2. Run `memoria render`. It copies the exported text between the markers.
3. Review the guide and acknowledge it.

The text between the import markers belongs to Memoria. Do not edit it by hand. The text around it belongs to you.

### What the import does and does not do

Run `memoria graph`:

<!-- cookbook-output: graph -->
```text
Nodes:
  current          README.md (2 file(s))
  current          docs/guide.md (0 file(s))
  current          src/auth/README.md (1 file(s))
Edges:
  handoff README.md -> docs/guide.md
  handoff README.md -> src/auth/README.md
  link    README.md -> docs/guide.md
  link    README.md -> src/auth/README.md
  import  docs/guide.md -> src/auth/README.md#summary
```

The new `import` edge does two things:

- When the exported text changes, the guide becomes pending.
- While `src/auth/README.md` is pending, the guide waits for it.

The import is sideways: `src/auth/` is not below `docs/`. So it hands off nothing, and the guide still covers 0 files.
`src/auth/README.md` alone covers `login.rs`. Only its review covers the code.

## A code change that keeps the summary true

A new function uses the existing limit:

```diff
 /// Failed sign-in attempts before an account locks.
 pub const MAX_ATTEMPTS: u32 = 5;
+
+/// Whether `attempts` failed sign-ins lock the account.
+pub fn is_locked(attempts: u32) -> bool {
+    attempts >= MAX_ATTEMPTS
+}
```

1. Run `memoria review`:

<!-- cookbook-output: refactor-plan -->
```text
Review plan: 1 pending document (dependency order)
  2. src/auth/README.md  README · input changed: src/auth/login.rs · ready
     current  docs/guide.md  waiting on src/auth/README.md
Next: memoria review src/auth/README.md
```

Only the auth README is pending. The guide stays current. It waits only because its provider is pending, and the summary can still change.

2. Run `memoria review src/auth/README.md`, and read the README against the change. The summary is still true.
3. Save an artifact with `memoria review src/auth/README.md --save "$dir"`.
4. Record the result:

```sh
memoria ack src/auth/README.md --packet "$dir"/memoria-manifest-src_auth_README.md-<id>.json \
  --reviewer docs-agent --result no-update \
  --note "The new is_locked function uses the same limit of 5 attempts, so the summary stays true."
```

<!-- cookbook-output: refactor-ack -->
```text
Recorded src/auth/README.md revision 2 (no-update) by docs-agent
```

5. Run `memoria review` again:

<!-- cookbook-output: refactor-done -->
```text
No document needs review.
```

The guide was never pending. One code edit cost one README review.

## A code change that changes the summary

When the summary changes, both documents need a review, in a fixed order. The README is reviewed first, because the guide waits for it. Then `memoria render` copies the new summary into the guide, and the guide is reviewed last.

![When the summary changes, the README is reviewed first, then the guide is rendered and reviewed: 1, the README edits its summary and the guide becomes pending but waits; 2, the README review makes the README current and unblocks the guide; 3, memoria render copies the summary into the import block; 4, the guide review makes the guide current.](flow.svg)

Sessions now end after 15 minutes:

```diff
-pub const SESSION_MINUTES: u32 = 30;
+pub const SESSION_MINUTES: u32 = 15;
```

### Review the provider first

1. Run `memoria review`:

<!-- cookbook-output: change-plan -->
```text
Review plan: 1 pending document (dependency order)
  2. src/auth/README.md  README · input changed: src/auth/login.rs · ready
     current  docs/guide.md  waiting on src/auth/README.md
Next: memoria review src/auth/README.md
```

2. Read the README against the change. The summary now states a wrong limit. Edit the export:

```diff
 <!-- memoria:export id="summary" -->
-A session ends after 30 minutes without activity. After 5 failed sign-in attempts, the account locks.
+A session ends after 15 minutes without activity. After 5 failed sign-in attempts, the account locks.
 <!-- /memoria:export -->
```

3. Run `memoria review` again:

<!-- cookbook-output: summary-plan -->
```text
warning [imports_outdated]
  Path: docs/guide.md
  Line: 11
  Column: 1
  import of src/auth/README.md#summary differs from the current export; run `memoria render`

Review plan: 2 pending documents (dependency order)
  2. src/auth/README.md  README · input changed: src/auth/login.rs · own text changed · ready
  3. docs/guide.md  opted-in document · input changed: src/auth/README.md#summary · waiting on src/auth/README.md  (run `memoria render` first)
Next: memoria review src/auth/README.md
```

The guide is pending with `input changed: src/auth/README.md#summary`. It waits for the auth README.
If you run `memoria review docs/guide.md` at this point, Memoria refuses:

<!-- cookbook-output: guide-waits -->
```text
error [dependencies_pending]
  Path: docs/guide.md
  docs/guide.md waits for: src/auth/README.md
  waiting_on:
    -
      src/auth/README.md

memoria review failed with exit status 1
```

A reviewer never reads the guide against a summary that can still change.

4. Save a fresh artifact for the auth README. Run `memoria review src/auth/README.md --save "$dir"`:

<!-- cookbook-output: save-auth -->
```text
warning [imports_outdated]
  Path: docs/guide.md
  Line: 11
  Column: 1
  import of src/auth/README.md#summary differs from the current export; run `memoria render`

Review src/auth/README.md — README, pending since revision 2
Scope: 1 source in src/auth/ and below
Handed src/auth/ by README.md (link, line 6)
Baseline: revision 2 by docs-agent (no-update)
What changed since that review:
  changed  this document's own text
      @@ -4,5 +4,5 @@
       Change a limit there, then update the summary below.

       <!-- memoria:export id="summary" -->
      -A session ends after 30 minutes without activity. After 5 failed sign-in attempts, the account locks.
      +A session ends after 15 minutes without activity. After 5 failed sign-in attempts, the account locks.
       <!-- /memoria:export -->
  changed  src/auth/login.rs · scope source · no section of this document describes it
      @@ -1,4 +1,4 @@
       /// Minutes without activity before a session ends.
      -pub const SESSION_MINUTES: u32 = 30;
      +pub const SESSION_MINUTES: u32 = 15;

       /// Failed sign-in attempts before an account locks.
Downstream:
  export summary → docs/guide.md (waits for this review)
How to read:
  Mode: full baseline. Review the whole document against its complete current scope, not only the changes.
  unmapped_change [src/auth/login.rs]: no valid section describes this changed source, so the review cannot narrow to a part of the document
  Read first:
    src/auth/README.md (whole_document)
    src/auth/login.rs (changed_source)
  Rest of the scope: none. The list above is the complete scope.
  Whole document pass: required
  Guidance: memoria guidance src/auth/README.md
Next:
  1. Read the whole document and the listed inputs; edit src/auth/README.md if it is wrong.
  2. After any edit, save a fresh artifact: dir=$(mktemp -d); memoria review src/auth/README.md --save "$dir"
  3. Record the result: memoria ack src/auth/README.md --packet <saved file> --reviewer <you> --result <updated|no-update> --note "<why>"
Saved: $dir/memoria-manifest-src_auth_README.md-<id>.json
Acknowledge after your review: memoria ack src/auth/README.md --packet $dir/memoria-manifest-src_auth_README.md-<id>.json --reviewer <REVIEWER> --result <updated|no-update> --note <NOTE>
```

The `Downstream:` line names the guide. The README review decides what the guide receives.

5. Record the result:

```sh
memoria ack src/auth/README.md --packet "$dir"/memoria-manifest-src_auth_README.md-<id>.json \
  --reviewer docs-agent --result updated \
  --note "Sessions now end after 15 minutes, and the summary states the new limit."
```

<!-- cookbook-output: ack-auth -->
```text
warning [imports_outdated]
  Path: docs/guide.md
  Line: 11
  Column: 1
  import of src/auth/README.md#summary differs from the current export; run `memoria render`

Recorded src/auth/README.md revision 3 (updated) by docs-agent
```

### Render, then review the guide

1. Run `memoria review`. The plan asks for a render:

<!-- cookbook-output: render-plan -->
```text
warning [imports_outdated]
  Path: docs/guide.md
  Line: 11
  Column: 1
  import of src/auth/README.md#summary differs from the current export; run `memoria render`

Review plan: 1 pending document (dependency order)
  3. docs/guide.md  opted-in document · input changed: src/auth/README.md#summary · ready  (run `memoria render` first)
Next: memoria render docs/guide.md
```

2. Run `memoria render`. It copies the new summary into the guide, and it changes no other text:

<!-- cookbook-output: render -->
```text
warning [imports_outdated]
  Path: docs/guide.md
  Line: 11
  Column: 1
  import of src/auth/README.md#summary differs from the current export; run `memoria render`

updated  docs/guide.md
    line 11: src/auth/README.md#summary (102 -> 102 bytes)
```

The warning comes from the check before the render. The render clears it.

3. Run `memoria review docs/guide.md --save "$dir"`:

<!-- cookbook-output: save-guide -->
```text
Review docs/guide.md — opted-in document, pending since revision 1
Scope: 0 sources in docs/ and below
Handed docs/ by README.md (link, line 5)
Baseline: revision 1 by fixture (no-update)
What changed since that review:
  changed  this document's own text
      @@ -9,7 +9,7 @@
       Open the sign-in page and enter your email and password.

       <!-- memoria:import src="../src/auth/README.md#summary" -->
      -A session ends after 30 minutes without activity. After 5 failed sign-in attempts, the account locks.
      +A session ends after 15 minutes without activity. After 5 failed sign-in attempts, the account locks.
       <!-- /memoria:import -->

       If your account locks, ask an administrator to unlock it.
  changed  import of src/auth/README.md#summary
      @@ -1,1 +1,1 @@
      -A session ends after 30 minutes without activity. After 5 failed sign-in attempts, the account locks.
      +A session ends after 15 minutes without activity. After 5 failed sign-in attempts, the account locks.
How to read:
  Mode: full baseline. Review the whole document against its complete current scope, not only the changes.
  imports_changed [src/auth/README.md#summary]: an imported contract changed; review the provider first, render, then review this document against its current scope
  Read first:
    docs/guide.md (whole_document)
    src/auth/README.md#summary (current_import)
  Rest of the scope: none. The list above is the complete scope.
  Whole document pass: required
  Guidance: memoria guidance docs/guide.md
Next:
  1. Read the whole document and the listed inputs; edit docs/guide.md if it is wrong.
  2. After any edit, save a fresh artifact: dir=$(mktemp -d); memoria review docs/guide.md --save "$dir"
  3. Record the result: memoria ack docs/guide.md --packet <saved file> --reviewer <you> --result <updated|no-update> --note "<why>"
Saved: $dir/memoria-manifest-docs_guide.md-<id>.json
Acknowledge after your review: memoria ack docs/guide.md --packet $dir/memoria-manifest-docs_guide.md-<id>.json --reviewer <REVIEWER> --result <updated|no-update> --note <NOTE>
```

The mode is `full baseline` with `imports_changed`. The reviewer reads the whole guide, not only the new summary.
Here the prose around the import names no time limit, so it stays correct.

4. Record the result:

```sh
memoria ack docs/guide.md --packet "$dir"/memoria-manifest-docs_guide.md-<id>.json \
  --reviewer docs-agent --result no-update \
  --note "The imported summary states 15 minutes, and the prose around it names no time limit."
```

<!-- cookbook-output: ack-guide -->
```text
Recorded docs/guide.md revision 2 (no-update) by docs-agent
```

The result is `no-update`, because the reviewer did not edit the prose. `memoria render` wrote the new summary.

5. Run `memoria check`:

<!-- cookbook-output: check -->
```text
OK: 3 document(s) current, imports rendered, no coverage or structure errors.
```

## An export body with a relative link

The summary travels into another folder, so a relative link in it can break.
An export body accepts absolute `https://`, `http://`, and `mailto:` links only. Raw HTML and reference-style links are invalid too.

Suppose that the summary ends with `See [login.rs](login.rs).` Run `memoria lint`:

<!-- cookbook-output: export-link -->
```text
warning [imports_outdated]
  Path: docs/guide.md
  Line: 11
  Column: 1
  import of src/auth/README.md#summary differs from the current export; run `memoria render`

error [export_invalid]
  Path: src/auth/README.md
  Line: 6
  Column: 1
  export "summary": link destination Borrowed("login.rs") must be an absolute
  https://, http://, or mailto: URL

memoria lint failed with exit status 1
```

1. Move the link out of the export, into the README text around it.
2. Put a link to the code in the docs page, outside the import block. The guide in this project does that.

## Tradeoffs

### What this setup costs

- **A README for each code area.** Each area that a docs page describes needs a README beside the code, with an export.
- **Two reviews for one behavior change.** The README is reviewed first, then the docs page. Each review reads its whole document.
- **A summary to maintain.** The README author keeps the summary short and true. A docs page cannot ask for more detail than the export holds.

### Limits

- **Only the exported text travels.** The prose around the import block is the docs page's own text. A reviewer judges it when the page is pending, and a code change does not make it pending.
- **A summary must be short and stable.** Each edit to the export makes every page that imports it pending. A summary that changes often sends the same review to every consumer.
- **A docs page covers its own folder.** A section, a link, or an import never adds `src/` files to its scope.
- **Export bodies are plain.** Absolute web and email links only. No relative links, raw HTML, or reference-style links.

### How this differs from the other patterns

- [Agent instruction files](../agent-instructions/README.md) live at the root, so they cover the code directly. They need no import to see a change.
- In [a README tree](../readme-tree/README.md), a parent imports summaries from its subfolders, and each import also hands off the subfolder.
- [Documents beside the code](../beside-code/README.md) share their folder with the README there, so a code change reaches them directly.
- A central `docs/` site is the only pattern where every edge to the code is a sideways import. It hands off nothing and covers nothing in `src/`.

### When not to adopt

- The docs pages describe code in detail. A short summary cannot carry that detail. Keep those pages beside the code instead.
- The project has no README beside its code areas, and the team does not want to keep one there.
- The docs site is generated from the code, for example API reference pages. A generator already keeps them current.

Next: run `memoria graph` in your project and look for a docs page that covers 0 files.
