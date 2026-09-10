# Review context: method and results

Memoria 0.4.0 separates short reading views from complete acknowledgement packets.
Full review remains the default. Experimental P1 has no completed model-quality evaluation.

## Reading path

1. [Method](#method)
2. [Results](#results)
3. [Review limits](#review-limits)

## Method

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

## Results

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

Unchanged inputs can still determine whether a changed claim is correct.
P1 requires explicit owner approval, a trusted prior review, coverage rationale, relevant context retrieval, and full-review fallback.
Model-quality evaluation remains unrun. The [shipped skill](../../skills/memoria/SKILL.md) defines the gate before default activation.
Canonical packets remain complete, and reduced views cannot acknowledge reviews.
Historical evidence requires exact length and fingerprint correspondence. Dirty acknowledgements remain valid without complete Git coverage.

The product retains automated regression coverage in [review_context.rs](../../tests/review_context.rs).
Study-only scripts, tables, and raw captures remain outside the product tree in the release archive.
The release handoff records their archive locations and checksums.
The preserved v0.3.0 study is historical evidence, not a claim about every repository or every future version.

Next: follow the [review workflow](../workflow.md) with a saved canonical packet.
