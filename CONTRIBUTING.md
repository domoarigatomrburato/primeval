# Contributing

Thanks for considering a contribution to `primeval`.

## Scope

The repository has two public faces that must stay aligned:

- the Rust engine and render layer
- the ESM-only Node package and CLI

When behavior changes, keep `crates/primeval-render`, `binding`, `src/index.ts`, and `src/cli.ts` consistent on defaults, accepted values, and error behavior.

## Local Setup

Prerequisites:

- Node 22.13+ for development (`@napi-rs/cli` requires it; the published package supports Node 22.12+)
- Rust stable `1.99.0` to match CI

Initial setup from the repository root:

```bash
npm ci
npm run build
npm run build:node
```

## Project Rules

- Treat Rust as the source of truth for render defaults and validation semantics.
- Do not duplicate Rust defaults in TypeScript or the napi binding shim.
- Keep the TypeScript wrapper thin; move canonical runtime behavior into Rust unless there is a strong package-layer reason not to.
- Add or update a failing test first, then make it pass with the smallest useful change.
- Do not edit `dist/` by hand.
- Keep the README user-facing. Contributor and maintainer process belongs in dedicated repo docs.

## Verification

Run these checks from the repository root before opening a pull request:

```bash
npm ci
npm run verify
```

`npm run verify` includes `npm run lint`, which runs Biome over `src/`, `scripts/`, and `test/` to check formatting, import order, and lint rules. Run `npm run format` to apply formatting, import order, and safe lint fixes.

## Benchmarks

Engine changes that affect speed or output quality should include before and after numbers from both tools below, run on the same machine with nothing else heavy running. Neither is part of `npm run verify`; the gate only compiles and lints them.

Micro-benchmarks use [Divan](https://docs.rs/divan). They live inside each crate behind its `bench` feature, so they can reach crate-private kernels without widening the public API:

```bash
cargo bench -p primeval-core --features bench --bench core      # rasterizers, colour and energy kernels, Model::step
cargo bench -p primeval-render --features bench --bench render  # SVG and PNG writers at 1024 px
```

Append a filter to run a subset, for example `-- rasterize` or `-- model_step`.

The end-to-end runner records wall time and quality (the engine's score and the RMSE of the PNG at output size) for every image, shape kind and step count, with seed 42 and default options, and prints a sorted Markdown table with the commit and machine in its header:

```bash
cargo run --release -p primeval-render --example quality -- --quick > quick.md  # seconds
cargo run --release -p primeval-render --example quality > baseline.md          # several minutes
```

Its corpus is the public-domain images in `docs/readme/originals/` plus generated images. Use `--image PATH` (repeatable), `--no-synthetic`, `--shapes LIST` and `--steps LIST` to change it; the doc comment in `crates/primeval-render/examples/quality.rs` defines the metrics. Diff two tables to compare runs. Quality numbers are reproducible for the same commit on the same platform whatever the thread count; set `RAYON_NUM_THREADS` to measure scaling, which changes only the times.

The README's comparison with the Go [`primitive`](https://github.com/fogleman/primitive) CLI comes from `cargo run --release -p primeval-render --example versus_go > versus-go.md` (about 45 minutes with the defaults on an Apple M3). It needs the Go tool (`go install github.com/fogleman/primitive@latest`), is not part of any gate, and its doc comment in `crates/primeval-render/examples/versus_go.rs` defines the settings and metrics.

## Gallery Images

The SVGs and thumbnails in `docs/images/`, `docs/gallery.md` and the README's alpha comparison images in `docs/readme/comparisons/` are generated; do not edit them by hand. Regenerate them after an engine change that alters output:

```bash
cargo run --release -p primeval-render --example gallery  # several minutes
```

To add a photograph, drop a JPEG, PNG or WebP file you may redistribute into `docs/readme/originals/` and run the same command. Name it with dashes (`the-kiss.jpg` becomes "The Kiss" in the gallery) or add its title to `TITLES` in `crates/primeval-render/examples/gallery.rs`. The doc comment there lists the step counts, seed and output sizes.

## Profiling

Builds target each architecture's portable baseline; the repository sets no `target-cpu`, and the release workflow rejects artifacts that use AVX-512 or SVE. To profile with every instruction your own CPU supports, opt in for that build only and keep the output in a separate target directory:

```bash
RUSTFLAGS="-C target-cpu=native" cargo build --profile profiling --target-dir target/native
```

Never commit a `target-cpu` setting (for example in `.cargo/config.toml`): a tooling test fails on it, because binaries built that way crash on CPUs that lack the build machine's features.

## Pull Requests

- Keep changes narrowly scoped.
- Include tests for behavioral changes.
- Update docs in the same change when public behavior changes.
- Prefer fixes at the root cause rather than package-layer workarounds.

## Release Notes

For release steps and workflow details, see [RELEASING.md](RELEASING.md).
For support and versioning expectations, see [SUPPORT.md](SUPPORT.md).
For vulnerability reporting, see [SECURITY.md](SECURITY.md).