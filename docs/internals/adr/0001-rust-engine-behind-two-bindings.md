# 0001. A Rust engine behind two bindings, with Rust owning the contract

Status: Accepted

Date: 2026-10-13

## Context

`primeval` ships one npm package, `@domoarigatomrburato/primeval`, to three audiences: Node programs, browsers and a command line. The engine is CPU-bound and long-running: a default render runs 100 greedy steps of 16 search rounds each, plus refit passes and a joint optimisation, so it must run in native code on Node and in WebAssembly in browsers, off the caller's main thread in both.

Every surface accepts the same options (`count`, `shape`, `alpha`, `seed`, `background`, `resizeInput`, `outputSize`) and must give the same defaults, the same accepted values, the same errors and the same output bytes. The audit of October 2026 found that the contract tests of the time, regex checks over the sources, had missed real drift between the layers.

## Decision

- The engine is Rust: `primeval-core` (search, scoring, refit passes, joint optimisation), with no knowledge of files, formats or JavaScript.
- `primeval-render` is the facade every surface calls: options, decoding, the per-kind pipeline, SVG and PNG encoding, and the error codes. `RenderOptions::default` is the only place a render default lives; `PartialRenderOptions` carries what a caller set, so every layer passes an absent option through and Rust picks the default.
- `primeval-js` is the request layer both bindings share: JavaScript values onto `RenderOptions::default`, and the JavaScript spelling of option names and error messages (`js_option_name`, `js_error_fields`).
- Two thin bindings sit on it: `binding` (napi-rs, `primeval-node`) for Node and `binding-wasm` (wasm-bindgen, `primeval-wasm`) for browsers, in a single-threaded and a threaded build.
- TypeScript (`src/index.ts`, `src/browser.ts`, `src/cli.ts`) mirrors the Rust contract by hand: it checks types and passes absent fields through. There is no cross-language code generation or shared schema.
- The package is ESM-only and targets Node 22.12 or newer; browsers are supported through WebAssembly only.
- Non-goals, from `AGENTS.md` "Repo Direction": CommonJS; new public render options beyond what consistency needs; a CLI framework (`src/cli.ts` uses `node:util`'s `parseArgs`); test runners other than `node:test`; supply-chain tooling beyond Dependabot, `cargo-deny` and workflow linting (actionlint, zizmor); GPU work.

## Consequences

- One change to a default or a vocabulary is one change in Rust; the Node API, the browser entry and the CLI follow without edits.
- The TypeScript types and the README must still be updated by hand when the contract changes. `test/contracts.test.js` holds them to the Rust sources: "the contract table holds through the Node API" and "through the CLI", "README and JSDoc defaults match the Rust render defaults", and "an omitted option renders like its explicit Rust default" through the API and the CLI.
- The package carries two native artefacts per release: platform addons for Node ([0010](0010-release-from-napi-targets-with-trusted-publishing.md)) and two `.wasm` builds for browsers ([0009](0009-browser-calls-run-in-web-workers.md)). The threaded build needs a dated nightly toolchain; everything else builds on the pinned stable one.
- Output equality across surfaces is a property of the engine ([0002](0002-same-output-on-every-thread-count-and-platform.md)), not of the bindings: they add no arithmetic of their own.
- Without code generation, a field added in Rust and forgotten in TypeScript is caught only by the contract tests. Revisiting this means adding a schema or generator, which `AGENTS.md` rules out today.

## Evidence

- The audit and refactor of October 2026 (`8ef226b`) moved defaults and validation into `primeval-render`; the browser binding (`1b25053`, 2026-10-02) added `binding-wasm` and the `primeval-js` request layer.
- `15dffb0` (2026-10-13) added the unused-code checks (`cargo machete`, knip) that keep the crate boundaries clean.
- [`../architecture.md`](../architecture.md), sections 2, 5, 6 and 7, describe the components.
