import assert from "node:assert/strict";
import { test } from "node:test";

import {
  assertTagMatchesVersion,
  parseNpmView,
  publishPackages,
  releasePackages,
  verifyPublished,
} from "../../scripts/release-packages.mjs";

const PKG = {
  name: "@aleburato/primeval",
  version: "1.2.3",
  optionalDependencies: {
    "@aleburato/primeval-darwin-arm64": "1.2.3",
    "@aleburato/primeval-linux-x64-gnu": "1.2.3",
  },
  napi: { targets: ["aarch64-apple-darwin", "x86_64-unknown-linux-gnu"] },
};

const PACKAGES = releasePackages(PKG, "/repo/npm", "/repo");

test("release packages list every platform package, then the root package last", () => {
  assert.deepEqual(PACKAGES, [
    { name: "@aleburato/primeval-darwin-arm64", version: "1.2.3", dir: "/repo/npm/darwin-arm64" },
    {
      name: "@aleburato/primeval-linux-x64-gnu",
      version: "1.2.3",
      dir: "/repo/npm/linux-x64-gnu",
    },
    { name: "@aleburato/primeval", version: "1.2.3", dir: "/repo" },
  ]);
});

test("the release tag must match the package version", () => {
  assert.doesNotThrow(() => assertTagMatchesVersion("v1.2.3", "1.2.3"));
  assert.throws(() => assertTagMatchesVersion("v1.2.4", "1.2.3"), /v1\.2\.4.*1\.2\.3/);
  assert.throws(() => assertTagMatchesVersion("1.2.3", "1.2.3"), /v1\.2\.3/);
  assert.throws(() => assertTagMatchesVersion("", "1.2.3"), /tag/);
});

test("npm view output tells published, unpublished and failed lookups apart", () => {
  assert.equal(parseNpmView("1.2.3", { status: 0, stdout: '"1.2.3"\n', stderr: "" }), true);
  // Older npm versions print nothing and exit 0 for a missing version.
  assert.equal(parseNpmView("1.2.3", { status: 0, stdout: "", stderr: "" }), false);
  // npm 11 reports a missing version or a never-published package as E404.
  assert.equal(
    parseNpmView("1.2.3", { status: 1, stdout: "", stderr: "npm error code E404\n" }),
    false,
  );
  assert.throws(
    () => parseNpmView("1.2.3", { status: 1, stdout: "", stderr: "npm error code ETIMEDOUT\n" }),
    /npm view failed/,
  );
  assert.throws(
    () => parseNpmView("1.2.3", { status: 0, stdout: '"1.2.4"\n', stderr: "" }),
    /unexpected/,
  );
});

test("publishing skips versions that already exist and keeps the root package last", () => {
  const published = new Set(["@aleburato/primeval-darwin-arm64"]);
  const calls = [];
  const actions = publishPackages(PACKAGES, {
    isPublished: ({ name }) => published.has(name),
    publish: ({ name }) => calls.push(name),
    log: () => {},
  });

  assert.deepEqual(calls, ["@aleburato/primeval-linux-x64-gnu", "@aleburato/primeval"]);
  assert.deepEqual(
    actions.map(({ name, action }) => [name, action]),
    [
      ["@aleburato/primeval-darwin-arm64", "skip"],
      ["@aleburato/primeval-linux-x64-gnu", "publish"],
      ["@aleburato/primeval", "publish"],
    ],
  );
});

test("a failed platform publish stops before the root package", () => {
  const calls = [];
  assert.throws(
    () =>
      publishPackages(PACKAGES, {
        isPublished: () => false,
        publish: ({ name }) => {
          calls.push(name);
          if (name.endsWith("linux-x64-gnu")) {
            throw new Error("publish failed");
          }
        },
        log: () => {},
      }),
    /publish failed/,
  );
  assert.deepEqual(calls, [
    "@aleburato/primeval-darwin-arm64",
    "@aleburato/primeval-linux-x64-gnu",
  ]);
});

test("a re-run after a full publish publishes nothing", () => {
  const calls = [];
  publishPackages(PACKAGES, {
    isPublished: () => true,
    publish: ({ name }) => calls.push(name),
    log: () => {},
  });
  assert.deepEqual(calls, []);
});

test("post-publish verification retries until every package is visible", async () => {
  let round = 0;
  const sleeps = [];
  await verifyPublished(PACKAGES, {
    isPublished: ({ name }) => round > 1 || !name.endsWith("primeval"),
    sleep: async (ms) => {
      sleeps.push(ms);
      round += 1;
    },
    attempts: 5,
    delayMs: 10,
    log: () => {},
  });
  assert.deepEqual(sleeps, [10, 10]);
});

test("post-publish verification fails and names every missing package", async () => {
  const sleeps = [];
  await assert.rejects(
    verifyPublished(PACKAGES, {
      isPublished: ({ name }) => name.endsWith("darwin-arm64"),
      sleep: async (ms) => sleeps.push(ms),
      attempts: 3,
      delayMs: 10,
      log: () => {},
    }),
    /not on the registry: @aleburato\/primeval-linux-x64-gnu@1\.2\.3, @aleburato\/primeval@1\.2\.3/,
  );
  assert.deepEqual(sleeps, [10, 10]);
});
