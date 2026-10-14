# 0010. Releases derive from `napi.targets` and publish through trusted publishing only

Status: Accepted

Date: 2026-10-14

## Context

The Node package needs a native addon per platform (macOS arm64 and x64, Linux GNU arm64 and x64, Windows x64), each in its own npm package, `@domoarigatomrburato/primeval-<suffix>`, which the root package lists as optional dependencies. The same list of targets is needed by the generated loader, the release matrix, the platform packages, the artifact checks and the root package's dependencies; spelled more than once, the copies would drift.

npm's trusted publishing (OIDC) publishes from a GitHub workflow without a stored token, but cannot create a package, and completes a publish asynchronously: a new version usually becomes visible a minute or two after `npm publish` returns. For 0.2.2 the Windows package took about 9 minutes; the release's wait, then 30 checks 10 s apart, ran out, and the job failed before publishing the root package, which was the intended behaviour, and passed on a re-run.

## Decision

- `package.json` `napi.targets` is the one list of native targets. The loader ([`scripts/generate-binding.mjs`](../../../scripts/generate-binding.mjs)), the release matrix, the platform packages and the artifact checks ([`scripts/napi-targets.mjs`](../../../scripts/napi-targets.mjs), [`scripts/check-artifact.mjs`](../../../scripts/check-artifact.mjs)) derive from it.
- The committed `package.json` and lockfile carry no platform packages. `scripts/release-packages.mjs publish` ([`release-packages.mjs`](../../../scripts/release-packages.mjs)) adds them as `optionalDependencies` at publish time, checks that the packed root lists exactly them, publishes every platform package, waits until each is visible on the registry, then publishes the root package last, with provenance and without lifecycle scripts, and restores `package.json`. Versions already on the registry are skipped, so a failed release is re-run, never patched by hand.
- The wait is 90 checks, 10 s apart, about 15 minutes.
- Releases publish only from [`napi-prebuilds.yml`](../../../.github/workflows/napi-prebuilds.yml) on a `v*` tag, through trusted publishing with provenance. All six packages require two-factor authentication and disallow tokens for publishing, and no npm token exists; a token is used only once, to create a new package before its trusted publisher can be set ([`RELEASING.md`](../../../RELEASING.md)).
- A missing native artefact fails the release, even where a packaging command would only warn. Every addon targets its architecture's baseline instruction set, checked per artefact (no AVX-512 on x86_64 outside dispatched dependency functions, no SVE on aarch64, a glibc 2.17 floor on Linux).

## Consequences

- Adding a platform is one entry in `napi.targets`, plus its runner; the open musl targets (REL-7 in the [audit plan](../plans/2026-10-audit-and-refactor-plan.md)) are such a change.
- A release cannot be published from a workstation or with a leaked token. Creating a new package needs the token bootstrap that `RELEASING.md` describes.
- A version stuck in npm staging needs `npm stage approve` and a re-run; a wait longer than 15 minutes needs a re-run. The root package is never published before its platform packages are visible, so users never install a root whose optional dependencies do not exist.
- Tests in [`test/tooling/napi-targets.test.js`](../../../test/tooling/napi-targets.test.js) ("optional dependencies are derived from napi targets", "release matrix runners are derived from napi targets", "the repository package.json and lockfile carry no platform packages", "binding loader only references the platform packages of napi targets") and [`test/tooling/release-packages.test.js`](../../../test/tooling/release-packages.test.js) ("a platform package that never becomes visible stops before the root package", "the default wait is 90 checks, 10 seconds apart (about 15 minutes)", "publish restores package.json when a platform package never becomes visible").

## Evidence

- `3e9a8f8` (2026-10-02): portable, smoke-tested prebuilds and re-runnable releases; `1655d66` (2026-10-13): platform packages added at publish time; `4ac61d0` (2026-10-13): the root package only after every platform package is visible.
- `df7e7ba8809f16d70599a961ecaf0c7a7cddd5d2` (2026-10-13): a five-minute wait; `191a930` (2026-10-14): fifteen minutes, after 0.2.2.
- [Runtime and release](../tuning.md#runtime-and-release) in `tuning.md`.
