# Audit And Refactoring Plan (October 2026): open items

The audit of `3e872be` (2026-10-01) and the algorithm work that followed it are done, except the items below; the full plan, with its findings, decisions and measurements, is in the git history (last complete at `dcc0ab4`). The engine work's decisions are in [`../adr/`](../adr/README.md), its measured defaults in [`../tuning.md`](../tuning.md) and the experiments it did not adopt in [`../experiments.md`](../experiments.md); [`../architecture.md`](../architecture.md) describes the result. Delete this file when its last item lands or is dropped.

## Release and tooling

- **REL-7: musl targets.** Decided: add `x86_64-unknown-linux-musl` and `aarch64-unknown-linux-musl` to `napi.targets` as a change of its own, validated with a `workflow_dispatch` run (an Alpine smoke test in a container). Status: open. Releases 0.2.0 and 0.2.1 ran green end to end, which was the precondition; the loader already detects musl and rejects it with a clear message.
- **The `$/...` self-repository syntax.** `napi-prebuilds.yml` calls the quality workflow with `uses: ./.github/workflows/quality.yml` and `# zizmor: ignore[self-repository]`, because actionlint 1.7.12 rejects the newer syntax zizmor prefers. Status: open, waiting for an actionlint release that accepts it; the workflow's comment says so.

## Engine

- **PERF-3: NEON `line_after` reduces horizontally every 8 pixels.** `neon::line_after` in `crates/primeval-core/src/score.rs` sums each 8-pixel chunk's squares with `vaddvq_u32`; keeping vector accumulators across the line and reducing once would save those reductions (`line_sums` and `difference_bytes` already accumulate). Status: not scheduled: the maintainer closed the speed work on 2026-10-13.
- **Experiment 6: scale guard.** Measure peak memory and time at `resizeInput` 1024 and 2048 with 500 and 2000 shapes, and set a threshold above which the final stage falls back from the joint optimisation to refit passes. Status: open. The joint optimisation caps its checkpoints (64 MiB) and its coverage cache (32 MiB), but not the buffers that hold the canvas under each layer of a segment; the README's "Memory" section states what a render takes.
- **Joint optimisation for rotated ellipses.** Rotated ellipses end with one refit pass: the joint optimisation gained them only 2–3%, and the guard rejected it on most images, on thin ellipses, where the locally straight coverage errs most ([`../experiments.md`](../experiments.md#2026-10-08-rotated-ellipses-in-the-joint-optimisation)). Status: deferred until a coverage exact at high curvature exists.
- **A stroke width that varies along one quadratic curve (4c).** It needs the curve exported as a filled outline, about three times the bytes ([`../experiments.md`](../experiments.md#2026-10-09-a-stroke-width-that-varies-along-one-curve-4c)). Status: parked.

## Demo

- **Manual check of "Refining".** Check by hand that the demo's "Refining" state stays visible during a long single-threaded final stage, and that Stop during it keeps the preview. Status: open. `test/browser/demo.test.js` checks that "Refining" precedes "Done" and that Stop works mid-render, but not Stop during the final stage.
