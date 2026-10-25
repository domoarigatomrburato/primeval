# Launch plan (October 2026)

A checklist for making primeval known as an open-source project, written on 2026-10-24 and revised the same day after the [competitive research](2026-10-competitors.md). `main` is at `a0f0c0a`, which rewrote the [README](../../../README.md) for this launch. The Nudaluce discreet mode, once section 5 of this document, is a feature of that site and now lives in its repository (`photo-website`, `docs/plans/2026-10-discreet-mode.md`); it is not a prerequisite for anything here.

## What exists

MIT licence and public sources; a [browser demo](https://domoarigatomrburato.github.io/primeval/) with no installation or sign-up, where the image never leaves the page; SVG and PNG export; an npm package with a Node API and a CLI; the Rust engine; a [gallery](../../gallery.md) of public-domain examples; a README with a side-by-side comparison against Go `primitive`, honest benchmarks and a placeholder section with measured sizes.

## Positioning

The research found that "a Rust port of Primitive that runs in your browser" is not a differentiator: Rust ports exist since 2018, browser versions since 2016, and a Go-to-WebAssembly version was on Show HN on 2026-03-03 (43 points). What no other tool in the Primitive family does is revise shapes after placing them, which is why primeval gets a lower error with the same number of shapes (in every one of the 18 benchmark configurations, 25% to 29% in geometric mean). Everything else supports that: multi-threaded in the browser (the other browser versions are single-threaded), seeded output independent of the core count (Go cannot be seeded; Geometrize needs the same thread count), about three times faster than Go.

Lead with the picture: the same image, the same number of shapes, Primitive and primeval side by side. Then the reason. Do not lead with speed, with "browser" or with "Rust".

Wording:

- "Primitive-style", "independent implementation", never "port". The README says so; the posts must match it.
- Credit Michael Fogleman's Primitive in the first sentence of every post.
- Acknowledge diffvg (2020) and LIVE (2022) as research prior art for optimising all shapes at once with gradients, before a commenter does. primeval does it on the CPU, deterministically, in a package.
- No benchmark headlines from the best shape kind; the honest figure is "about three times faster" overall, and quadratics are even.

One-liner: "Primitive-style image approximation that keeps refining its shapes. Rust engine, Node API and CLI, and the same engine in the browser; your image stays on your device."

Show HN title: "Show HN: Primeval – Primitive-style geometric image approximation that refines its shapes".

## The placeholder angle

For web developers and portfolio authors: geometric previews as an alternative to blurred placeholders, chosen for how they look. The README section gives the build script and the sizes. Say the numbers before anyone else does: 8 triangles are about 0.4 KB gzipped, 30 about 0.9 KB, against about 30 bytes for BlurHash and ThumbHash and a single integer for the CSS-only technique of 2025. The comparable tools are SQIP (Go Primitive, 8 shapes, since 2017) and svelte-geometrize (2026), not the hashes. A copyable integration (a build script for Astro or Eleventy, which have no built-in placeholder) would convert better than any post; it is optional for the first wave.

Nudaluce is the production example: its previews are generated with primeval, so the README credit is true and a "generated with primeval" credit can go on the site at any time. Link it from posts only with an NSFW flag, and keep it out of r/webdev and r/creativecoding posts; the primeval demo and gallery carry the launch with images suitable for every audience.

## Channels

Web developers and portfolio authors first, creative communities for visual feedback, Rust communities last and only with substance of their own.

| Channel | Form | Note |
| --- | --- | --- |
| [r/creativecoding](https://www.reddit.com/r/creativecoding/) | Short video: original, shapes appearing, result; how the code makes the image | Rules ask for context on the role of the code |
| [r/generative](https://www.reddit.com/r/generative/) | Visual examples and the approximation process | Relevant self-promotion is allowed, in moderation |
| [Show HN](https://news.ycombinator.com/showhn.html) | Demo link, the portfolio problem, Primitive, what primeval changes | Needs the author available for the thread; no vote requests. Primitive made 894 points in 2016, Geometrize 157 in 2020, Triangula 296 in 2021 |
| [r/webdev](https://www.reddit.com/r/webdev/) | Showoff Saturday: blur versus geometric, the build script, API and browser | Saturday only, correct flair, no LLM-written text |
| Bluesky, Mastodon | Short clips and comparisons, one idea and a demo link each | |
| [r/rust](https://www.reddit.com/r/rust/) | The weekly "What's everyone working on" thread; a personal write-up only if there is one worth writing | Plain repository links are restricted; the crate name `primeval` is taken on crates.io and no primeval crate is published, which Rust readers will notice |
| [This Week in Rust](https://github.com/rust-lang/this-week-in-rust/blob/main/README.md) | Only with substantial Rust content | No longer takes Project/Tooling Updates PRs; excludes LLM-written articles; not needed for the launch |

Reddit's rules were read on 2026-10-24; its archives could not be searched for earlier posts about Primitive or Geometrize. Re-read every community's rules right before posting, and write every post yourself. Product Hunt, paid promotion and directory submissions have low priority against demonstrations, discussions and external uses.

## Before the first post

Done:

- [x] README rewritten (`a0f0c0a`): the difference from Primitive first, side-by-side comparison, benefits-first highlights, prior art, placeholder section.
- [x] GitHub showcase (2026-10-25): Website set to the demo, description rewritten around the positioning, topics `webassembly` and `creative-coding` added.
- [x] Open Graph and Twitter card metadata on the demo page, with `demo/og.jpg` (primeval's own render of the Grand Prismatic Spring, 1200 × 630).

To do:

- [ ] A 10 to 20 second clip: original, shapes appearing, result (needs ffmpeg or a screen recording; not installed).
- [ ] Three static comparisons on different subjects, one of them blur versus geometric placeholder, with images suited to each channel.
- [ ] Let 3 to 5 people try the demo unguided: do they find how to start, get a result, download it?
- [ ] Decide whether to publish the crates before any r/rust mention; `primeval-core`, `primeval-render` and `primeval-node` were free on crates.io on 2026-10-25. Without them, skip r/rust.
- [ ] Put the "generated with primeval" credit on nudaluce.com, with a link.

## Sequence and goals

| Phase | What |
| --- | --- |
| First days | Visual material, GitHub showcase, unguided tests. |
| First post | One creative community, with a specific question about the results or the demo. |
| After the first feedback | Fix what it showed; Show HN when the author can follow the thread for a day. |
| First useful Saturday | r/webdev, on placeholders and integration. |
| Second week | New examples, the portfolio story if it fits, external uses shown. |

First month: ten concrete pieces of feedback and two or three external uses, however small. Stars and downloads are secondary to people integrating the tool and showing what they get.
