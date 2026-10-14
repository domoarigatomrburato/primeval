# Architectural decision records

Each record states one decision that shapes the code, the forces behind it, and what follows from it. [`../architecture.md`](../architecture.md) says how the system works today; the records say why it is built that way. The numbers behind the engine's defaults are in [`../tuning.md`](../tuning.md), and the experiments that were tried and not adopted, or parked, in [`../experiments.md`](../experiments.md).

| Record | Decision | Date |
| --- | --- | --- |
| [0001](0001-rust-engine-behind-two-bindings.md) | A Rust engine behind two bindings, with Rust owning the contract | 2026-10-13 |
| [0002](0002-same-output-on-every-thread-count-and-platform.md) | The same seed gives the same output at any thread count and on every target | 2026-10-12 |
| [0003](0003-optimise-the-composite-the-export-draws.md) | The engine optimises the composite the export draws, and the export guards the final stage | 2026-10-12 |
| [0004](0004-a-pipeline-per-shape-kind.md) | A pipeline per shape kind: passes during the search, then a final stage | 2026-10-12 |
| [0005](0005-shapes-read-as-their-kind.md) | Every shape reads as its kind, in every optimiser | 2026-10-09 |
| [0006](0006-how-engine-changes-are-judged.md) | Engine changes are judged on a fixed corpus, with time weighed against visible quality | 2026-10-12 |
| [0007](0007-simd-kernels-equal-the-scalar-ones.md) | SIMD kernels equal the scalar ones bit for bit, and WebAssembly requires SIMD | 2026-10-12 |
| [0008](0008-the-refit-sums-in-a-fixed-four-slot-order.md) | The refit pass sums its pixels in a fixed four-slot order | 2026-10-13 |
| [0009](0009-browser-calls-run-in-web-workers.md) | Browser calls run in Web Workers, started one at a time, with one idle worker reused | 2026-10-13 |
| [0010](0010-release-from-napi-targets-with-trusted-publishing.md) | Releases derive from `napi.targets` and publish through trusted publishing only | 2026-10-14 |

## Rules

- A record is numbered in order, `NNNN-<kebab-slug>.md`, and has, in this order: the title as a short statement of the decision; `Status:` and `Date:` lines; `## Context`, the forces and what was measured or observed; `## Decision`, stated so that a reader can check the code against it; `## Consequences`, good and bad, with the tests and pins that enforce the decision and what would have to change to revisit it; and optionally `## Evidence`, the commits and the `tuning.md` or `experiments.md` entries that hold the numbers.
- The date is the day the decision was made or last confirmed by a measurement or a change, from the history, not the day the record was written.
- An accepted record is never edited, except to set its status to `Superseded by` and a link to the record that replaces it. A changed decision gets a new record that supersedes the old one and says what changed.
- A record cites commits, files in the repository and tests, never files outside it.

## History

These records, [`../tuning.md`](../tuning.md) and [`../experiments.md`](../experiments.md) replace the chronological review log of the engine work from 2026-10-07 to 2026-10-13, which stays in the history: `git log --follow -- docs/algorithm-leap-review-2026-10-07.md`.
