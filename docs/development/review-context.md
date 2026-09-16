# Review context: method and results

Memoria 0.6.0 makes a small review manifest the default result of
`memoria review <README.md>`. The full content export moved behind `--full`.

This page records what was measured, and what was not. Reading cost is an
observation with stated limits. It is never a release promise.

## Reading path

1. [What Memoria does not claim](#what-memoria-does-not-claim)
2. [0.6.0 artifact sizes](#060-artifact-sizes)
3. [0.6.0 practical comparison](#060-practical-comparison-limited-and-incomplete)
4. [0.4.0 presentation study](#040-presentation-study)
5. [Review limits](#review-limits)

## What Memoria does not claim

Memoria states no target for token savings. It states no bound on missed
documentation changes. It makes no claim of statistical equivalence with full
review, and no claim about a population of repositories.

Earlier drafts proposed a one-percentage-point degradation ceiling at 95%
confidence and a 50% median token reduction on eligible small edits. Those
targets are withdrawn. No completed evaluation supports them.

The automated snapshot-safety rules are the guarantee of this release. No
reading-cost result may weaken a snapshot rule, a fallback reason, or the
whole-README pass.

## 0.6.0 artifact sizes

The numbers that follow are serialized byte counts of the two artifacts for
the four pending boundaries of this repository. The measurement invoked the
built executable and counted stdout bytes.

| Boundary | Manifest JSON | Full export JSON | Fewer bytes | Manifest human | Full human |
| --- | ---: | ---: | ---: | ---: | ---: |
| `crates/memoria-domain` | 4,741 | 251,294 | 98.1% | 1,866 | 210,234 |
| `crates/memoria-application` | 10,406 | 680,060 | 98.5% | 4,246 | 464,524 |
| `crates/memoria-infrastructure` | 5,053 | 816,185 | 99.4% | 2,067 | 690,227 |
| `src` | 4,750 | 226,941 | 97.9% | 1,714 | 138,015 |

Read these numbers narrowly. They measure transported bytes on one repository
at one moment. They are not tokens. They are not a reading cost, because the
reviewer still reads the listed paths from disk. They say nothing about
review quality.

A manifest stays proportional to the number of changes. A full export stays
proportional to the size of the boundary. The difference in this table follows
from that shape, not from any measured reviewer behavior. The 0.6.0 practical
comparison reports model-token observations. Those observations measure review
cost more directly than bytes, but they are limited and incomplete.

## 0.6.0 practical comparison: limited and incomplete

The approved practical comparison of full review against focused review did
run, on 2026-09-16. It used five paired cases on this repository, with one
full arm and one focused arm for each case.

A human closed the experiment before it completed. The result is limited,
incomplete evidence. It is not a passing cross-section test.

### What the comparison does not establish

| Question | Status |
| --- | --- |
| Does focused review catch a consequence outside the mapped section? | UNPROVEN. The one case built to test this question is void. |
| Does focused review prevent false edits on a no-change case? | INCONCLUSIVE. That case was never a clean control. |
| Does focused review cost fewer model tokens? | No combined claim. Three per-case observations are available. |
| Are fewer transported bytes a token saving? | No. Bytes are not a measure of review cost. |

### The cross-section case is void

Case c2 tested whether focused review finds a consequence outside the mapped
section. The surviving focused run made zero edits of its own. It inherited
its corrections from an earlier run that a harness error destroyed.

An earlier report stated that this run corrected both claims at 20% of full
cost. That claim is withdrawn. The combined savings ratio that was built on it
is withdrawn with it. Cross-section effectiveness stays unproven.

### The control case is inconclusive

Case c5 required no documentation change. The README of that case contained
the v3 token error that this release corrects, so the full arm made real
corrections. The case was never a clean control. Case c5 gives no
false-positive rate and no equivalence claim, in either direction.

### Per-case token observations

These three pairs have verifiable provenance. Cite them only per case. No
combined ratio is available, because the void c2 pair contaminates every
aggregate.

| Case | Full arm tokens | Focused arm tokens |
| --- | ---: | ---: |
| c1 ordinary mapped change | 424,280 | 248,054 |
| c3 unmapped new file, full fallback | 525,835 | 471,650 |
| c4 invalid mapping, full fallback | 527,324 | 393,888 |

On the two fallback cases the focused arm is only a little cheaper. That is
the expected result when the fallback correctly refuses to narrow the scope.
A `full_baseline` scope in the result does not establish that the reviewer
inspected all the required content.

### Process failures of the exercise

The failures that follow belong to the evaluation harness, not to Memoria:

- The approved bound was ten review runs. At least fifteen were launched. The
  exact count is unrecoverable, because overlapping attempts destroyed their
  own evidence.
- Independent verification refused three harness revisions.
- Both authorized recovery launches stayed unused, and the human closed the
  experiment as incomplete.

The one product error that this exercise found is the v3 token description in
the domain README. This release corrects that description.

## 0.4.0 presentation study

The 0.4.0 study measured human stdout bytes after a newline change. It
belongs to the 0.4.0 interface, where JSON always carried complete content.
It is preserved as historical evidence.

The study used exact v0.3.0 commit `45379560c1805a9751ec1a94468a806553ca8724`.
The comparison used the development implementation before the 0.4.0 version bump.
Each fixture had an inspected, CLI-acknowledged baseline and committed source evidence.
Each comparison added one newline byte after the first line of one owned input.
Direct subprocess pipes captured stdout and stderr outside the fixtures without terminal filtering.
The comparison covered small, large, deeply nested, shared-export, and ordinary Markdown projects.

| Fixture | Definition | Old human stdout bytes | New human stdout bytes |
| --- | --- | ---: | ---: |
| Small | Four Rust inputs, 562 bytes each | 3,700 | 793 |
| Large | 256 Rust inputs, 2,098 bytes each | 550,393 | 802 |
| Deep | Eight child levels; three Rust inputs per owner | 3,260 | 937 |
| Shared export | One provider and eight consumers | 4,509 | 847 |
| Markdown | 64 root notes and four consumers | 45,407 | 769 |

The large case reduced initial human stdout by 99.85% and retained its verified hunk.
Its complete JSON packet remained 666,643 bytes on disk.
An experimental P1 selection used 105,718 bytes before further retrieval.
That 84.14% byte reduction does not establish model-token savings or review quality.
The Markdown fallback selection grew from 74,344 to 80,662 bytes because it included complete content and coverage records.

No tokenizer, model evaluation, or network-transfer measurement established these results.
Earlier token estimates used bytes divided by four, which is only a rough estimate.
Deep, shared-export, and Markdown human commands also emitted unchanged navigation diagnostics on stderr.
The general plan was already small. The large case used 294 human stdout bytes.

## Review limits

Unchanged inputs can still determine whether a changed claim is correct. A
small reading list never narrows the complete input state that
acknowledgement validates.

`focused_candidate` is technical eligibility. It does not certify the previous
review. Without explicit reviewer trust in that review, the workflow uses the
full baseline.

Legacy P1 requires explicit owner approval, a trusted prior review, a coverage
rationale, relevant context retrieval, and full-review fallback. It never
reduces the scope that the manifest requires.

Full exports remain complete, and a reduced view cannot acknowledge a review.
Historical evidence requires exact length and fingerprint correspondence.
Dirty acknowledgements remain valid without complete Git coverage.

The product retains automated regression coverage in
[review_context.rs](../../tests/review_context.rs) and
[sections.rs](../../tests/sections.rs).
Study-only scripts, tables, and raw captures remain outside the product tree in the release archive.
The release handoff records their archive locations and checksums.
The preserved v0.3.0 study is historical evidence, not a claim about every repository or every future version.

Next: follow the [review workflow](../workflow.md) with a saved manifest.
