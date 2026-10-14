# 0006. Engine changes are judged on a fixed corpus, with time weighed against visible quality

Status: Accepted

Date: 2026-10-12

## Context

Almost every engine default trades time for quality, and both are noisy to read: a change can help the paintings and hurt a synthetic image, a median can hide a per-image regression, and a 2% timing difference is within the noise of a busy machine. The algorithm work of October 2026 fixed one method and applied it to every stage.

The purpose sets the exchange rate. `primeval` draws a recognisable shape version of a picture ([0005](0005-shapes-read-as-their-kind.md)), so an error change of a few tenths of a level out of 255 is invisible. A 2% lower RMSE is about 0.15 of a level on average, and roughly 5% more shapes buy the same for about 5% more time, a knob the user already holds. On 2026-10-03 the maintainer accepted large time costs for large gains, such as the joint optimisation's 8–14%; on 2026-10-12 the maintainer rejected four seeds per search round, about 2% for 1.3–1.6 times the time.

## Decision

- **Corpus and runner.** Engine changes are measured with the engine runner (`cargo run --release -p primeval-render --features lab --example engine`; [`CONTRIBUTING.md`](../../../CONTRIBUTING.md#benchmarks)) on its default corpus: *American Gothic*, *Mona Lisa* and three synthetic 512 × 512 images (gradient, shapes, texture), every shape kind, seed 42, checkpoints at 50, 100, 200 and 500 shapes, all cores. Lab hooks behind the `lab` feature set the variant; the defaults do not change until the measurement is decided.
- **Quality** is `rmse256`, the RMSE of the exported PNG at the working size against the working target, on 0–1. A stage or a budget is judged by the mean of the 100- and 200-shape medians over the images. A change wins a kind when it lowers that mean by at least 0.5% without raising the mean of the per-image changes at those counts, at no more than 1.1 times the time; or when, at quality within ±0.5%, it takes at most 0.9 times the time.
- **Time against quality.** Large visible gains are worth time; marginal ones are not: about 2% of RMSE at 1.3 times the time is rejected. A speed change that loses quality reports its worst row as the largest RMSE increase on one image, in levels out of 255, and is taken when that loss is invisible.
- **Timings** are read on one quiet machine, base and change interleaved, with two base runs to measure the noise; times are wall time of the search and its stages, decode and encode excluded. x86_64 is timed on GitHub-hosted runners through the manual workflow [`engine-bench.yml`](../../../.github/workflows/engine-bench.yml), which builds two refs and runs them interleaved; timings under Rosetta mean nothing.
- **Speed-only changes are exact.** A change that claims no output change must keep every pin and every non-time column of the corpus run unchanged; the replaced computation stays as a test reference.

## Consequences

- Every default in [`../tuning.md`](../tuning.md) cites a measurement taken this way, and every rejected variant in [`../experiments.md`](../experiments.md) the numbers that rejected it.
- Quality numbers are reproducible: seeded output does not depend on timing or threads ([0002](0002-same-output-on-every-thread-count-and-platform.md)), so two runs of the same commit agree to the last digit and only times vary (within about 4% between a run and its repeat after a reboot).
- synthetic-shapes, the hard-edged image, swings by 20% or more between checkpoints on changes that move the paintings by 1–2%. The per-image condition and a reading of the paintings keep it from deciding alone, in either direction.
- The method favours the stated use case, a fixed shape count. At equal time the pipeline can lose to a plain greedy search with more shapes; that view is reported (the equal-time ratios in `tuning.md`) but does not decide.
- Revisiting the exchange rate, for example if render time at any count became the target, would reopen most entries of `tuning.md`.

## Evidence

- `16877bb` and `d85a5b1` (2026-10-07): the rule as first applied to the pipeline, and the retune that weighed time.
- `e649332` (2026-10-11): the greedy-proposal experiment judged by the full rule; `c1786c4` (2026-10-12): the budgets judged by the worst row in levels.
- `33d90d1` (2026-10-12): the x86 timing workflow.
