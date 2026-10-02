# Releasing

This repository publishes one root npm package plus per-platform native packages.

Release automation is defined in [.github/workflows/quality.yml](.github/workflows/quality.yml) and [.github/workflows/napi-prebuilds.yml](.github/workflows/napi-prebuilds.yml).

## Versioning

The npm package, the platform packages, and the Cargo crates share one version. `package.json` is the source; the Cargo workspace carries it in `[workspace.package]`, and every crate inherits it with `version.workspace = true`. `npm run check:napi-targets` (part of `npm run verify`) fails if they drift.

Versioning restarts at the first real release. A `v0.1.1` tag existed but nothing was ever published from it, and it has been deleted; give the first release a version that was never tagged (for example `0.1.2`).

## Before Tagging

Run from the repository root on a clean checkout:

```bash
npm ci
npm run verify
```

## Version Bump

Use the repository helper, which keeps `package.json`, `package-lock.json`, the platform package versions, `Cargo.toml`, and `Cargo.lock` aligned with `napi.targets`:

```bash
npm run bump:version -- 0.1.2
```

It restores every file if a step fails. Review the resulting changes and rerun the checks above.

## Release Flow

1. Commit the version bump.
2. Create an annotated tag in the form `v<version>`; it must equal the `package.json` version.
3. Push the commit and tag.

Example:

```bash
git commit -am "Release 0.1.2"
git tag -a v0.1.2 -m "v0.1.2"
git push origin main
git push origin v0.1.2
```

## What The Tag Workflow Does

When a `v*` tag is pushed, [.github/workflows/napi-prebuilds.yml](.github/workflows/napi-prebuilds.yml) will:

1. Re-run the shared quality workflow, including the Rust tests on arm64 macOS.
2. Derive the native target matrix from `package.json` `napi.targets`.
3. Build each native addon on a runner of its own platform. Linux GNU targets link against glibc 2.17 through `napi build --use-napi-cross`.
4. Check each artifact with `scripts/check-artifact.mjs`: no AVX-512 (x86_64) or SVE (aarch64) instructions, and no glibc symbol newer than 2.17 on Linux.
5. Smoke test each artifact with `scripts/smoke-test-addon.mjs`: load it through the package loader and render a small image to SVG and PNG.
6. Build both WebAssembly builds for the browser entry with `npm run build:wasm`, on the pinned stable toolchain, the dated nightly, and the matching wasm-bindgen-cli (all pinned through `scripts/build-wasm.mjs`). The build fails if the threaded `.wasm` does not import a shared memory, the single-threaded one does, or either is over the size budget in that script.
7. Assemble per-platform npm packages and verify that every expected package contains the correct `.node` payload.
8. Put the wasm build in `wasm/`, build the root package, and check with `scripts/release-packages.mjs verify-root` (run by `publish` before it publishes anything, and by `npm run verify:wasm` on every pull request) that its tarball has every `dist/` module and every wasm file both builds need. A missing or incomplete wasm build fails the release here.
9. Publish with `scripts/release-packages.mjs publish`: every platform package, then the root package last, with provenance. A package whose version is already on the registry is skipped.
10. Verify with `npm view` that every package is on the registry at the tag version, and fail otherwise.
11. Create the GitHub Release with generated notes, unless it already exists.

## First Release

npm trusted publishing (OIDC) can only be set up for a package that already exists, so the first release of every package publishes with a token:

1. On npmjs.com, create a granular access token with read and write access to the `@aleburato` scope and a short expiry, for example 7 days.
2. Add it to the repository as the Actions secret `NPM_TOKEN`. The publish step passes it to npm as `NODE_AUTH_TOKEN`.
3. Release as described above.
4. On npmjs.com, add a trusted publisher to each of the six packages (`@aleburato/primeval` and the five platform packages in `package.json` `optionalDependencies`): repository `domoarigatomrburato/primeval`, workflow `napi-prebuilds.yml`.
5. Delete the `NPM_TOKEN` secret and revoke the token. Later releases publish through OIDC, which needs npm 11.5.1 or newer on the runner.

## Re-running A Failed Release

Re-run the failed workflow run for the same tag. Packages already published are skipped, the root package is published only after every platform package, and an existing GitHub Release is left as is. Never move a tag after any package of that version was published; bump to a new version instead.

## Dry Runs

To test the build side of the release workflow without publishing, use the workflow's `workflow_dispatch` trigger from GitHub Actions.

`workflow_dispatch` runs the build matrix, the artifact checks, the smoke tests, and the wasm build, but the publish and GitHub Release jobs only run for tagged refs.

## After Publish

The workflow already verifies that every package exists on the registry. Also check:

- install and runtime behavior on at least one supported machine
- the packaged CLI and one programmatic API path
