# AGENTS.md

## Repo Shape

`primeval` is a Rust workspace plus a small Node package surface.

Main components:

- `crates/primeval-core`: optimization engine, rasterization, scoring, and export
- `crates/primeval-render`: high-level decode -> render -> encode facade shared by binding and package surfaces
- `crates/primeval-js`: the JavaScript-facing request layer shared by both bindings: option parsing onto Rust defaults, and the JavaScript spelling of option names and error messages
- `binding`: napi-rs crate named `primeval-node`
- `binding-wasm`: wasm-bindgen crate named `primeval-wasm`, the browser binding; `scripts/build-wasm.mjs` builds its single-threaded and threaded variants
- `src/index.ts`: single-source TypeScript wrapper for the npm package; `src/errors.ts` holds its error classes and the mapping from native error codes; `src/native-binding.ts` is the native boundary shim
- `src/browser.ts`: browser entry (the `browser` condition of `exports`), run by `src/browser-runtime.ts` in one Web Worker per call (`src/worker-single.ts` or `src/worker-threaded.ts`, which also runs the rayon pool workers; shared logic in `src/worker-common.ts`); `src/types.ts`, `src/request.ts` and `src/data-uri.ts` are shared with `src/index.ts`, and `tsconfig.browser.json` builds the browser files without Node types
- `src/cli.ts`: Node CLI entrypoint distributed via npm package `bin`
- `scripts/`: binding-loader generation, `napi.targets` tooling, the wasm build (`build-wasm.mjs`, the single source of the nightly pin, threaded flags, output layout and the gzip size budget per `.wasm`), and the demo site (`demo.mjs`: `npm run demo:build` assembles `site/`, `npm run demo` also serves it)
- `test/`: Node/package tests; `test/tooling/`: packaging, loader, and release-metadata tests; `test/browser/`: the browser entry in headless Chromium (Playwright), and the demo
- `demo/`: the browser demo, plain HTML, CSS and ES modules with no build step, deployed to GitHub Pages by `.github/workflows/pages.yml`; `demo/coi-sw.js` is its cross-origin isolation service worker
- `docs/readme/`: source images (`originals/`, also the test fixtures) and alpha comparisons used by the README
- `docs/images/` and `docs/gallery.md`: progression gallery and thumbnails
- `docs/plans/`: active planning docs; keep this directory small and current. Plans can be large: read the contents and the sections for the item you are working on, not the whole file.

Generated output (gitignored; never edit by hand):

- `dist/`, produced by `npm run build`
- root `binding.js`, `binding.d.ts`, and `primeval-node.*.node`, produced by `npm run build:node`
- `wasm/` (`wasm/single/`, `wasm/threaded/`), produced by `npm run build:wasm`
- `site/`, the demo site, produced by `npm run demo:build`

## Repo Direction

- The npm package is ESM-only and targets Node 22.12+.
- Browsers are supported through WebAssembly (`binding-wasm`), following `docs/plans/2026-10-wasm-browser-plan.md`: a single-threaded build on the pinned stable toolchain and a threaded build that needs the dated nightly pinned in `scripts/build-wasm.mjs`. Only the threaded build uses the nightly; clippy for wasm32 runs on the stable toolchain.
- CommonJS support is explicitly out of scope.
- Rust is the runtime source of truth for accepted vocabularies, defaults, and validation semantics.
- The final Rust API is the only source of truth for render-option defaults. If a value is omitted, `undefined`, or intentionally left unset, wrapper and binding layers should pass that absence through so Rust can decide the default.
- TypeScript mirrors the Rust contract; do not introduce cross-language code generation or shared schema systems.
- Do not add new public render options unless required to make existing behavior consistent across render, binding, and TypeScript.

## Implementation Rules

- Keep `primeval-render`, `primeval-js`, `binding`, `binding-wasm`, `src/index.ts`, and `src/cli.ts` aligned on defaults, accepted values, and error behavior. Request parsing and JavaScript spellings shared by both bindings belong in `primeval-js`, not in either binding.
- Do not reinvent Rust defaults in TypeScript or napi request shims. Validate explicit user input there, but let Rust merge omitted fields onto its own defaults.
- Prefer shared Rust parsers/helpers over duplicated string tables in binding.
- Keep the TypeScript wrapper thin. Canonical runtime behavior should live in Rust unless there is a strong package-layer reason not to.
- Keep `npm run build`, `npm run typecheck`, and `npm pack --dry-run` viable from a clean checkout before any local native build step. If runtime-generated native files are unavoidable, isolate them behind a small shim and cover the clean-checkout path with a tooling test.
- Red-green-refactor TDD is a hard rule for code changes: add or update a failing test first, make it pass with the smallest useful change, then clean up. Red and green use the targeted test only (see Verification).
- Treat `package-lock.json` as npm-managed output: do not hand-edit it, regenerate it with npm when package metadata changes, and verify the result with `npm ci`.
- If public package behavior changes, update `README.md` in the same change.
- If a plan is implemented or superseded, prune or reduce the corresponding file in `docs/plans/`.

## CI And Release Rules

Apply these when editing `.github/workflows/`, `scripts/`, or package metadata.

- Avoid duplicated CI/release verification logic. The single quality path is the `verify*` scripts in `package.json`; workflows call them.
- Treat missing native artifacts as release failures even if packaging commands only warn.
- Pin CI tooling versions. Do not rely on floating-branch downloads or unpinned CLI installs in workflows.
- For native package targets, keep one canonical source of truth in `package.json` `napi.targets`; derive or validate release metadata and artifact checks from it.

## Verification

Run commands from the repository root. Each check is fast on a warm cache, but the full gate prints several KB of output, so repeating it in a loop wastes time and tokens. Match the check to the change, and run the full gate once.

Test prerequisites:

- `npm test` imports the package through `dist/` and loads the local native addon. After editing `src/`, run `npm run build` first. After editing Rust used by the binding, run `npm run build:node` first (a release build with fat LTO, the slowest step); otherwise the tests run against a stale addon.
- `npm run test:tooling` packs the package and installs it into a temp project. It is only relevant when touching `scripts/`, `package.json`, binding loading, or packaging.
- `npm run build:wasm` and `npm run verify:wasm` (so also `npm run verify`) need two extra tools; nothing else uses them:
  - the dated nightly with `rust-src`: `node scripts/build-wasm.mjs install-nightly`;
  - wasm-bindgen-cli at the version of the `wasm-bindgen` crate in `Cargo.lock` (the build checks it): `cargo install wasm-bindgen-cli --locked --version "$(node scripts/build-wasm.mjs wasm-bindgen-version)"`.
- `npm run test:browser` runs `test/browser/` in Playwright's Chromium headless shell, with and without cross-origin isolation. It needs `npm run build`, `npm run build:wasm` and `npm run build:node` first, and the browser once per machine (see `CONTRIBUTING.md`). Run it when touching the browser entry, `binding-wasm`, or the shared `src/` modules.
- The first build in a fresh checkout or worktree is cold and much slower. Do it once; never wipe `target/`, `node_modules/`, or `dist/` to start fresh.

While working:

- Run only the test you are writing or fixing: `cargo test -q -p <crate> <name_filter>` or `node --test --test-name-pattern "<name>" test/<file>.test.js`.
- Widen to the suite you touched when the change is done: `cargo test -q -p <crate>` for Rust, `npm run typecheck && npm run build && npm test` for TypeScript.
- Fix formatting with `cargo fmt` (and `cargo clippy --fix --allow-dirty --all-targets` when useful) instead of repeating check-only commands.
- Prefer quiet output (`cargo test -q`, `node --test --test-reporter=dot`) and read the failure, not the whole log.
- Changes limited to docs or comments need no gates. The exception is `CONTRIBUTING.md` and `RELEASING.md`: `npm run test:tooling` checks them for the toolchain pin and the `verify` command.

Final gate, once, before handing off or committing:

- `npm run verify` runs exactly what CI runs: `verify:rust` (fmt check, clippy, rustdoc, cargo test), `verify:node` (napi targets check, typecheck, build, native build, tests, tooling tests), `verify:pack`, and `verify:wasm` (wasm32 clippy on the stable toolchain, both builds with their shared-memory checks, the TypeScript and native builds, then the browser tests). Run the full gate after any Rust change, since `test/contracts.test.js` checks the Node layer against Rust sources. If no `.rs` file, `Cargo.toml`, `Cargo.lock`, or `rust-toolchain.toml` changed, `npm run verify:node && npm run verify:pack` is enough, plus `npm run verify:wasm` if `scripts/build-wasm.mjs`, the browser entry or the `src/` modules it shares changed.
- If a step fails, fix it and re-run only that step. Re-run the full gate only if the fix could affect other steps.
- Do not re-run a gate that passed if nothing has changed since.

Do not:

- run `npm ci` as a check; run it only when `node_modules` is missing or `package.json`/`package-lock.json` changed.
- simulate a clean checkout by deleting build dirs; `test/tooling/` covers the clean-checkout and packed-install paths.
- run `cargo build --release` as verification; `npm run build:node` already builds the release addon.
- install `typos`, `cargo-deny`, `actionlint` or `zizmor` just to check locally; CI's hygiene job runs them. Run them locally only if they are already installed.
- repeat the gate locally after pushing to double-check, or poll CI; CI re-runs the same scripts.
