# Competitive landscape for the launch (2026-10-24)

Research for the [launch document](2026-10-launch-strategy.md). Every star count, date and version was read on 2026-10-24 from the GitHub API, npm registry, crates.io or the cited page; "last commit" is the newest commit on the default branch.

## Bottom line

"A Rust port of Primitive that runs in your browser" is not a differentiated pitch. Rust ports exist since 2018 (`primg`, `purr`), browser versions since 2016 (`primitive.js`, the Geometrize web demo), and on 2026-03-03 a Show HN titled "Reconstruct any image using primitive shapes, runs in-browser via WASM" (`primitive-playground`, Go compiled to WebAssembly) got 43 points. Geometric SVG placeholders are not new either: SQIP has wrapped Primitive for that since 2017, and `@nomideusz/svelte-geometrize` (2026) does it with Geometrize at build time. What primeval can claim, within the Primitive family (Primitive, its ports, Geometrize and its ports): it is the only one we found that revises shapes after placing them (refit passes and a joint gradient optimisation of every shape), it is multi-threaded inside the browser (every browser port we found is single-threaded), and its seeded output does not depend on the core count (Go Primitive cannot be seeded; Geometrize's C++ library is reproducible only with the same thread count). Its README benchmark against Go Primitive (lower RMSE in all 18 configurations, 25% to 29% geometric mean, 2.8x to 3.3x faster) is the author's own measurement. What it cannot claim: that joint optimisation of all shapes is new (diffvg 2020, LIVE 2022 and a tiny 2026 Python project do it with gradients on a GPU or in PyTorch; Triangula's genetic algorithm moves every point of its mesh), or that its placeholders are small (30 triangles gzip to about 0.9 KB, against about 0.4 KB for SQIP's default and 20 to 30 characters for BlurHash).

## Direct competitors

| Tool | Language | Runs where | Shapes | SVG | Threads | Deterministic | Refines placed shapes | Licence | Stars | Last commit |
| --- | --- | --- | --- | --- | --- | --- | --- | --- | ---: | --- |
| primeval | Rust, TS | CLI, Node lib, browser (wasm) | 9 modes | yes | yes, also in browser (cross-origin isolated) | seeded, core-count independent | yes: refit passes, joint gradient | MIT | 0 | 2026-10-17 (push) |
| fogleman/primitive | Go | CLI, Go lib | 9 modes | yes | yes (`-j`) | no: clock seed | no | MIT | 13,565 | 2020-05-04 |
| ondras/primitive.js | JS | browser | triangle, rectangle, ellipse, smiley | yes (copy from text area) | no | not stated | no evidence | MIT | 791 | 2020-04-26 |
| transitive-bullshit/primitive | JS | Node, browser | 5 + random | yes | not checked | not checked | no evidence | not detected | 93 | 2026-09-04 |
| taiseiue/primitive-playground | Go to wasm, TS | browser | 9 modes (Go Primitive) | renders SVG | single (inferred) | no (Go Primitive) | no | MIT | 40 | 2026-03-01 |
| chux0519/purr (`purrmitive`) | Rust | CLI, Rust lib | 9 modes | yes | yes (`-j`) | not checked | no evidence | BSD-3 | 25 | 2021-11-18 |
| Geometrize app | C++/Qt | desktop GUI | geometrize-lib set | yes, plus JSON | yes | seed, same thread count only | no | GPL-3.0 | 2,152 | 2022-08-18 |
| geometrize-haxe + web demo | Haxe to JS | lib, browser | 8 kinds incl. lines, beziers | yes, plus JSON | no | not checked | no | MIT | 366 / 135 | 2021-11-21 |
| valeriyvan/swift-geometrize | Swift | lib, CLI, web, Telegram bot | geometrize set + polylines | yes | not checked | not checked | no evidence | MIT | 21 | 2025-11-05 |
| @nomideusz/svelte-geometrize | TS (geometrizejs) | Vite build + Svelte component | triangle, rotated ellipse, others | shape list | no | `seed` option | no | MIT | 0 | 2026-10-21 (push) |
| RH12503/triangula | Go | GUI, CLI, browser | triangulation, polygons (no overlap) | yes | yes | not stated | moves all mesh points (GA) | MIT | 3,877 | 2021-09-02 |

## Tools

**fogleman/primitive (Go).** The original (Show HN 2016-09-20). Nine modes via `-m`: combo, triangle, rect, ellipse, circle, rotated rect, beziers, rotated ellipse, polygon; outputs PNG, JPG, SVG, GIF; `-j` workers, default all cores. Its README states the key limit: a found shape is added to the canvas "where it will remain unchanged". `main.go` seeds from `time.Now()`; PR #117 "support passing a seed in for reproducibility" (2024-10-12) is still open. Last commit 2020-05-04, 55 open issues and PRs: unmaintained. `primitive.lol` (the macOS app) did not respond on 2026-10-24. https://github.com/fogleman/primitive, https://github.com/fogleman/primitive/pull/117

**primitive.js (Ondřej Žára).** Browser re-creation of primitive.lol, canvas rasterisation; the README says Web Workers are "not used at all". Shapes in the UI: triangles, rectangles, ellipses, smileys; vector output by copying SVG from a text area. Demo still loads. An Electron port, `primitive.nextgen`, has 76 stars. https://github.com/ondras/primitive.js, https://ondras.github.io/primitive.js/, https://github.com/security-companion/primitive.nextgen

**Other Primitive ports.** `transitive-bullshit/primitive`: Node and browser port with a web demo and a CLI (`primitive-cli`, last release 2018); its npm name `primitive` now belongs to an unrelated SDK, and the fork `archaic` (2024) has 6 weekly downloads. `primitive-playground`: Go Primitive compiled with `GOOS=js GOARCH=wasm` in a Web Worker; Go's js/wasm port runs goroutines on one thread (golang.org/issue/28631), so we infer it is single-threaded. Rust: `purr`/`purrmitive` (crates.io 0.2.0, 2021-01-07, 9,773 downloads), `larryng/primg` (7 stars, 2018); crates.io also has an unrelated `geometrize` 0.1.0 (26 downloads) and `primitive` 0.1.0 (lines, 2017). The crate name `primeval` is taken by an unrelated prime-number generator. No Python, Dart or Kotlin port with more than a handful of stars turned up. https://github.com/transitive-bullshit/primitive, https://github.com/taiseiue/primitive-playground, https://github.com/chux0519/purr, https://crates.io/crates/primeval

**Geometrize (Sam Twidale).** Primitive-inspired family: Qt desktop app (Windows, Mac, Linux; last release v1.0.1 on 2018-05-30), C++ library `geometrize-lib` (MIT), Haxe library (MIT) with a browser demo that still loads, `geometrizejs` (Haxe-generated JS, 215 weekly downloads). The app exports SVG, PNG, JPG, GIF and JSON shape data, supports batch export and ChaiScript scripting. The Haxe README describes shapes "added one by one" and states the implementation is single-threaded. `geometrize-lib` runs one `std::async` task per thread with a seed per task and notes that identical results require the same `maxThreads`. A Forza livery community uses Geometrize JSON (`forza-painter-geometrize-gpu`, 31 stars, GPU, rotated ellipses only). https://github.com/Tw1ddle/geometrize, https://github.com/Tw1ddle/geometrize-lib/blob/master/geometrize/geometrize/model.cpp, https://github.com/Tw1ddle/geometrize-haxe, https://www.samcodes.co.uk/project/geometrize-haxe-web/

**svelte-geometrize.** The closest match to primeval's LQIP angle: a Vite plugin runs geometrizejs at build time and emits an ordered shape list ("~1-3 KB raw"), and a Svelte 5 component replays the shapes so the placeholder sharpens until the photo loads. Options include shape count, alpha, shape types and seed; the README lists three production sites. 0 stars, 27 weekly downloads, first published 2026-06-13. https://github.com/nomideusz/svelte-geometrize

**Different methods (low-poly and triangulation).** Triangula: a modified genetic algorithm that mutates the points of a triangulation, so earlier geometry keeps moving; shapes do not overlap; GUI, CLI and a browser version (README: desktop is 20 to 50 times faster). esimov/triangle (2,070 stars, last commit 2024-10-24): blur, Sobel edges, Delaunay, done in one pass with no optimisation, SVG export. Trianglify (10,084 stars, GPL-3.0) generates background patterns from palettes, not from input images: not a competitor. DMesh is a closed-source Delaunay app (Mac, Windows, iOS) with manual point editing; its current availability is unclear. tasercake/lowpoly (Rust, 87 stars) is Delaunay low-poly. These compete for the "low-poly look" audience, not for overlapping-shape approximation or placeholders. https://github.com/RH12503/triangula, https://github.com/esimov/triangle, https://github.com/qrohlf/trianglify

**Prior art for refinement (research).** diffvg (1,290 stars, Apache-2.0) is a differentiable vector rasteriser; it ships `painterly_rendering.py` and `refine_svg.py`, which refines an existing SVG against a target image. LIVE (CVPR 2022, 631 stars) adds closed Bezier paths layer by layer "and optimizing all these paths". Stylized Neural Painting (CVPR 2021, 1,607 stars) fits stroke parameters, including a rectangle renderer, by gradient search through a neural renderer. ShapeGrad (0 stars, Python, 2026-10) optimises triangles, rectangles and ellipses with Adam and exports SVG. All are PyTorch, mostly GPU, and none is packaged as a CLI or npm library for web use. https://github.com/BachiLi/diffvg, https://github.com/Picsart-AI-Research/LIVE-Layerwise-Image-Vectorization, https://github.com/jiupinjia/stylized-neural-painting, https://github.com/Mykelxu/shapegrad

## LQIP landscape

| Technique | Typical size | How it is embedded | Notes |
| --- | --- | --- | --- |
| BlurHash | 20 to 30 characters | string in data, decoded by JS (or server) to pixels | 956k npm downloads/week |
| ThumbHash | ~21 bytes binary in the demo example | bytes or base64 in data, `thumbHashToDataURL` in JS | 361k npm downloads/week |
| CSS-only LQIP (Lean Rada, 2025) | one integer, 20 bits | `style="--lqip:567213"`, decoded by CSS, no JS | very blurry by design |
| lqip-modern | 16 px WebP: 80 B raw, 100 B gzip; 32 px: 292 B | base64 data URI, CSS blur | sizes from the SQIP demo |
| plaiceholder | CSS ~600 B, SVG ~1.2 kB, base64 ~300 B (its docs) | CSS, SVG or data URI | maintenance mode; GitHub repo archived |
| SQIP default (Primitive, 8 shapes, combo) | 848 B raw, 390 B gzip (one demo image); README: ~800 to 1000 B | inline SVG data URI with blur | stable 0.3.3 (2018); 1.0.0-beta.2 (2026-03-03); bundles Go Primitive binaries |
| svelte-geometrize | ~1 to 3 KB raw (README) | shape list replayed by a component | build time, Svelte only |
| Next.js `placeholder="blur"` | docs: "10px or less" recommended, no byte figure | `blurDataURL` data URL, auto for static imports | remote images need your own `blurDataURL` |
| Nuxt Image `placeholder` | configurable width, height, quality, blur | small image from the provider | no default size stated |
| Gatsby `gatsby-plugin-image` | not stated | `dominantColor` (default), `blurred`, `none` | `tracedSVG` not listed in current docs |
| Astro, Eleventy Image | no built-in LQIP | community packages (`astro-lqip`, 405/week) | Eleventy only offers a transparent PNG placeholder |
| primeval, triangles, output size 32 | 8: 810 B raw, 391 B gzip; 30: ~0.9 KB; 60: ~1.6 KB; 100: ~2.5 KB (gzip) | inline SVG | 8-triangle figure measured here on `mae-jemison.jpg`, seed 42; not the same image as SQIP's |

## Prior launches

Hacker News (points, comments), from the Algolia HN API:

- 2016-09-20 Show HN: Primitive Pictures (894, 170); 2016-11-15 Show HN: Primitive for macOS (798, 145); 2018-10-31 Primitive, primitive.lol (182, 22).
- 2016-11-23 Primitive.js (15, 1). 2020-12-20 Geometrize (157, 43); 2023-06-14 Geometrize (29, 2); 2017-04-18 Show HN: Geometrize Haxe (4, 1).
- 2021-04-22 Show HN: Triangula (296, 69).
- 2026-03-03 Show HN: Reconstruct any image using primitive shapes, runs in-browser via WASM (43, 8). Top comment asked what the tool is for; another pointed to the Geometrize web demo for SVG export.
- Placeholders: 2017-11-14 Using SVG as image placeholders (983, 134); 2020-02-20 BlurHash (363, 110); 2023-03-22 ThumbHash (768, 117); 2025-03-30 Minimal CSS-only blurry image placeholders (470, 75); 2026-02-28 Show HN: SplatHash (61, 19).
- In the 2020 Geometrize thread, commenters asked "what an optimal minimisation would produce" and whether "gradient descent with some momentum would be faster", and one suggested using such SVGs for lazy loading, answered with a BlurHash link.

Reddit could not be searched (reddit.com and old.reddit.com refused automated requests).

## Positioning implications

Facts:
- Every Primitive-family tool we found adds shapes one at a time and never revisits them; primeval revises them.
- Browser versions exist since 2016 and one was on HN seven months ago; none we found runs on several threads.
- Placeholder developers already have 20-byte hashes, a 20-bit CSS integer, framework built-ins and SQIP.
- At 8 triangles primeval's SVG is about the size of SQIP's default; at 30 or more it is larger.
- No primeval crate is on crates.io, and the name `primeval` is taken there.

Inferences:
- Lead with output quality per shape and the reason for it (shapes are revised), shown side by side with Go Primitive at the same count; "runs in your browser" is a supporting feature, not the headline.
- Say "Primitive-style" rather than "port": the engine differs, and "port" invites "another one" replies.
- Pitch placeholders on look, not size, and compare with SQIP and svelte-geometrize, not with BlurHash.
- Expect "Rust" readers to look for a crate.
- Expect commenters to cite diffvg or LIVE when the joint optimisation is mentioned; acknowledge them as GPU research prior art.

## Unverified

- Whether primitive.lol and the Primitive macOS app are still available, and at what price (site did not respond).
- Prior posts on r/creativecoding, r/webdev and r/generative.
- Whether `primitive-playground` now offers SVG download, and its thread count (inferred single-threaded from the Go js/wasm port).
- Seeding in geometrize-haxe, Triangula, purr and transitive-bullshit/primitive.
- Generated byte sizes for Next.js, Nuxt and Gatsby placeholders.
- DMesh's current availability.
- Whether the Geometrize desktop app is sold or free (an HN commenter wrote "Just purchased").

## Sources

- https://github.com/fogleman/primitive, https://github.com/fogleman/primitive/blob/master/main.go, https://github.com/fogleman/primitive/pull/117
- https://github.com/ondras/primitive.js, https://ondras.github.io/primitive.js/
- https://github.com/transitive-bullshit/primitive, https://www.npmjs.com/package/primitive, https://www.npmjs.com/package/archaic
- https://github.com/taiseiue/primitive-playground, https://primitive-playground.taiseiue.jp/, https://golang.org/issue/28631
- https://github.com/chux0519/purr, https://crates.io/crates/purrmitive, https://github.com/larryng/primg, https://crates.io/crates/primeval
- https://github.com/Tw1ddle/geometrize, https://github.com/Tw1ddle/geometrize-lib, https://github.com/Tw1ddle/geometrize-haxe, https://github.com/Tw1ddle/geometrize-haxe-web, https://www.geometrize.co.uk/, https://www.npmjs.com/package/geometrizejs
- https://github.com/valeriyvan/swift-geometrize, https://github.com/nomideusz/svelte-geometrize, https://github.com/zjl88858/forza-painter-geometrize-gpu
- https://github.com/RH12503/triangula, https://github.com/RH12503/Triangula/wiki/Explanation-of-the-algorithm, https://github.com/esimov/triangle, https://github.com/qrohlf/trianglify, https://github.com/tasercake/lowpoly, https://www.creativeapplications.net/project/dmesh-cinder-mac/
- https://github.com/BachiLi/diffvg, https://github.com/Picsart-AI-Research/LIVE-Layerwise-Image-Vectorization, https://github.com/jiupinjia/stylized-neural-painting, https://github.com/Mykelxu/shapegrad
- https://github.com/axe312ger/sqip, https://sqip.vercel.app/, https://github.com/woltapp/blurhash, https://evanw.github.io/thumbhash/, https://github.com/evanw/thumbhash, https://leanrada.com/notes/css-only-lqip/, https://github.com/transitive-bullshit/lqip-modern, https://plaiceholder.co/docs, https://github.com/joe-bell/plaiceholder
- https://nextjs.org/docs/app/api-reference/components/image, https://image.nuxt.com/usage/nuxt-img, https://www.gatsbyjs.com/docs/reference/built-in-components/gatsby-plugin-image/, https://docs.astro.build/en/guides/images/, https://www.11ty.dev/docs/plugins/image/, https://www.npmjs.com/package/astro-lqip
- HN threads: https://news.ycombinator.com/item?id=12539109, https://news.ycombinator.com/item?id=12958445, https://news.ycombinator.com/item?id=18347708, https://news.ycombinator.com/item?id=13022055, https://news.ycombinator.com/item?id=25485493, https://news.ycombinator.com/item?id=36319665, https://news.ycombinator.com/item?id=14141994, https://news.ycombinator.com/item?id=26899066, https://news.ycombinator.com/item?id=47232339, https://news.ycombinator.com/item?id=15696596, https://news.ycombinator.com/item?id=22374825, https://news.ycombinator.com/item?id=35265752, https://news.ycombinator.com/item?id=43523220, https://news.ycombinator.com/item?id=47193832
