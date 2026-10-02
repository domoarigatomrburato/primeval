import assert from "node:assert/strict";
import fs from "node:fs";
import path from "node:path";
import { test } from "node:test";

import { OUTPUT_FILES, VARIANTS } from "../../scripts/build-wasm.mjs";
import {
  assertTagMatchesVersion,
  glueSnippets,
  parseNpmView,
  publishPackages,
  readRootPackageInputs,
  releasePackages,
  requiredRootFiles,
  verifyPublished,
  verifyRootPackage,
} from "../../scripts/release-packages.mjs";

const repoRoot = process.cwd();

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

// --- Root package contents ---

const GLUE = [
  "import { broadcast } from './snippets/primeval-wasm-0123/inline0.js';",
  "import { startWorkers } from './snippets/other-4567/src/helpers.js';",
  "export function approximate() {}",
].join("\n");

const ROOT_INPUTS = {
  pkg: {
    files: [
      "dist/*.js",
      "dist/index.d.ts",
      "wasm/single/primeval.js",
      "wasm/single/primeval_bg.wasm",
      "wasm/single/snippets/**",
      "wasm/threaded/primeval.js",
      "wasm/threaded/primeval_bg.wasm",
      "README.md",
    ],
  },
  sourceModules: ["index", "browser"],
  glueSources: { single: GLUE, threaded: GLUE },
};

const COMPLETE_ROOT = [
  "package.json",
  "README.md",
  "dist/index.js",
  "dist/browser.js",
  "dist/index.d.ts",
  "wasm/single/primeval.js",
  "wasm/single/primeval_bg.wasm",
  "wasm/single/snippets/primeval-wasm-0123/inline0.js",
  "wasm/single/snippets/other-4567/src/helpers.js",
  "wasm/threaded/primeval.js",
  "wasm/threaded/primeval_bg.wasm",
  "wasm/threaded/snippets/primeval-wasm-0123/inline0.js",
  "wasm/threaded/snippets/other-4567/src/helpers.js",
];

test("glueSnippets lists the snippets a wasm-bindgen glue imports", () => {
  assert.deepEqual(glueSnippets(GLUE), [
    "snippets/primeval-wasm-0123/inline0.js",
    "snippets/other-4567/src/helpers.js",
  ]);
  assert.deepEqual(glueSnippets("export function f() {}"), []);
});

test("the root package needs every dist module, literal file and both wasm builds", () => {
  assert.deepEqual(requiredRootFiles(ROOT_INPUTS).sort(), [...COMPLETE_ROOT].sort());
});

test("without a built glue, the root package still needs its glue and .wasm", () => {
  const required = requiredRootFiles({ ...ROOT_INPUTS, glueSources: {} });
  for (const variant of ["single", "threaded"]) {
    assert.ok(required.includes(`wasm/${variant}/primeval.js`), variant);
    assert.ok(required.includes(`wasm/${variant}/primeval_bg.wasm`), variant);
    assert.ok(!required.some((file) => file.startsWith(`wasm/${variant}/snippets/`)), variant);
  }
});

test("package.json files list every wasm build's glue and .wasm by name", () => {
  // requiredRootFiles takes them from the literal `files` entries.
  const { files } = JSON.parse(fs.readFileSync(path.join(repoRoot, "package.json"), "utf8"));
  for (const { outDir } of Object.values(VARIANTS)) {
    for (const file of Object.values(OUTPUT_FILES)) {
      assert.ok(files.includes(`${outDir}/${file}`), `${outDir}/${file}`);
    }
  }
});

test("a complete root package passes", () => {
  verifyRootPackage(requiredRootFiles(ROOT_INPUTS), COMPLETE_ROOT);
});

test("a root package without a wasm build fails and names every missing file", () => {
  const required = requiredRootFiles(ROOT_INPUTS);
  const packed = COMPLETE_ROOT.filter(
    (file) => file !== "dist/browser.js" && !file.startsWith("wasm/threaded/"),
  );
  assert.throws(
    () => verifyRootPackage(required, packed),
    (error) => {
      assert.match(error.message, /^the root package is missing /);
      for (const file of [
        "dist/browser.js",
        "wasm/threaded/primeval.js",
        "wasm/threaded/primeval_bg.wasm",
        "wasm/threaded/snippets/primeval-wasm-0123/inline0.js",
      ]) {
        assert.ok(error.message.includes(file), file);
      }
      assert.ok(!error.message.includes("wasm/single/"));
      return true;
    },
  );
});

test("the repository's root package needs every exports and bin target", () => {
  const pkg = JSON.parse(fs.readFileSync(path.join(repoRoot, "package.json"), "utf8"));
  const required = requiredRootFiles(readRootPackageInputs(repoRoot));
  const targets = [...JSON.stringify(pkg.exports).matchAll(/"\.\/((?:dist|wasm)\/[^"]+)"/g)].map(
    ([, file]) => file,
  );
  assert.ok(targets.includes("dist/browser.js"));
  for (const file of [...targets, ...Object.values(pkg.bin), "binding.js"]) {
    assert.ok(required.includes(file), file);
  }
  for (const worker of ["dist/worker-single.js", "dist/worker-threaded.js"]) {
    assert.ok(required.includes(worker), worker);
  }
});

test("the release publishes the wasm build of its own workflow run", () => {
  const workflow = fs.readFileSync(
    path.join(repoRoot, ".github", "workflows", "napi-prebuilds.yml"),
    "utf8",
  );
  const job = (name) => {
    const start = workflow.indexOf(`\n  ${name}:\n`);
    assert.notEqual(start, -1, `no ${name} job`);
    const end = workflow.slice(start + 1).search(/\n {2}[a-z-]+:\n/);
    return end === -1 ? workflow.slice(start) : workflow.slice(start, start + 1 + end);
  };

  const wasm = job("wasm");
  assert.match(wasm, /- run: rustup toolchain install\n/);
  assert.match(wasm, /- run: node scripts\/build-wasm\.mjs install-tools\n/);
  assert.match(wasm, /- run: npm run build:wasm\n/);
  assert.match(wasm, /uses: actions\/upload-artifact@[0-9a-f]{40} /);
  assert.match(wasm, /name: wasm\n\s+path: wasm\/\n\s+if-no-files-found: error\n/);
  // No caches in the release workflow (zizmor cache-poisoning).
  assert.doesNotMatch(wasm, /rust-cache|cache: npm/);
  assert.match(wasm, /package-manager-cache: false/);

  const publish = job("publish");
  assert.match(publish, /needs: \[build, wasm\]/);
  assert.match(
    publish,
    /uses: actions\/download-artifact@[0-9a-f]{40} [^\n]*\n\s+with:\n\s+name: wasm\n\s+path: wasm\n/,
  );
  assert.ok(
    publish.indexOf("name: wasm") < publish.indexOf("release-packages.mjs publish"),
    "the wasm build must be in place before publishing",
  );
});
