# Contributing

Thanks for considering a contribution to `primeval`.

## Scope

The repository has two public faces that must stay aligned:

- the Rust engine and render layer
- the ESM-only Node package and CLI

When behavior changes, keep `crates/primeval-render`, `binding`, `src/index.ts`, and `src/cli.ts` consistent on defaults, accepted values, and error behavior.

## Local Setup

Prerequisites:

- Node 20+
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

## Pull Requests

- Keep changes narrowly scoped.
- Include tests for behavioral changes.
- Update docs in the same change when public behavior changes.
- Prefer fixes at the root cause rather than package-layer workarounds.

## Release Notes

For release steps and workflow details, see [RELEASING.md](RELEASING.md).
For support and versioning expectations, see [SUPPORT.md](SUPPORT.md).
For vulnerability reporting, see [SECURITY.md](SECURITY.md).