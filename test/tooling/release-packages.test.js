import assert from "node:assert/strict";
import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import { test } from "node:test";

import { OUTPUT_FILES, VARIANTS } from "../../scripts/build-wasm.mjs";
import {
  assertTagMatchesVersion,
  glueSnippets,
  NPM_PUBLISH_ARGS,
  packedRootManifest,
  parseNpmView,
  parseNpmViewDependencies,
  publishPackages,
  publishRelease,
  readRootPackageInputs,
  releasePackages,
  requiredRootFiles,
  verifyPlatformDependencies,
  verifyPublished,
  verifyRelease,
  verifyRootPackage,
  withPlatformDependencies,
} from "../../scripts/release-packages.mjs";

const repoRoot = process.cwd();

const PKG = {
  name: "@aleburato/primeval",
  version: "1.2.3",
  napi: { targets: ["aarch64-apple-darwin", "x86_64-unknown-linux-gnu"] },
};

const PLATFORM_DEPENDENCIES = {
  "@aleburato/primeval-darwin-arm64": "1.2.3",
  "@aleburato/primeval-linux-x64-gnu": "1.2.3",
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

// --- Platform packages as optionalDependencies, added at publish time ---

test("withPlatformDependencies adds every platform package at the version, in napi.targets order", () => {
  const before = structuredClone(PKG);
  const published = withPlatformDependencies(PKG);
  assert.deepEqual(published, { ...PKG, optionalDependencies: PLATFORM_DEPENDENCIES });
  assert.deepEqual(Object.keys(published.optionalDependencies), [
    "@aleburato/primeval-darwin-arm64",
    "@aleburato/primeval-linux-x64-gnu",
  ]);
  assert.deepEqual(PKG, before, "the input is not modified");
});

test("withPlatformDependencies refuses a package.json that already lists them", () => {
  assert.throws(
    () => withPlatformDependencies({ ...PKG, optionalDependencies: PLATFORM_DEPENDENCIES }),
    /added at publish time/,
  );
});

test("platform dependency verification wants exactly the expected map", () => {
  assert.doesNotThrow(() =>
    verifyPlatformDependencies("the packed root package", PLATFORM_DEPENDENCIES, {
      "@aleburato/primeval-linux-x64-gnu": "1.2.3",
      "@aleburato/primeval-darwin-arm64": "1.2.3",
    }),
  );
  for (const actual of [
    undefined,
    {},
    { "@aleburato/primeval-darwin-arm64": "1.2.3" },
    { ...PLATFORM_DEPENDENCIES, "@aleburato/primeval-darwin-arm64": "1.2.2" },
    { ...PLATFORM_DEPENDENCIES, extra: "1.0.0" },
  ]) {
    assert.throws(
      () => verifyPlatformDependencies("the packed root package", PLATFORM_DEPENDENCIES, actual),
      /^Error: the packed root package has optionalDependencies .*, expected /,
      JSON.stringify(actual),
    );
  }
});

test("npm view of optionalDependencies reads a map, nothing, or a failure", () => {
  assert.deepEqual(
    parseNpmViewDependencies({ status: 0, stdout: `${JSON.stringify(PLATFORM_DEPENDENCIES)}\n` }),
    PLATFORM_DEPENDENCIES,
  );
  // npm prints nothing for a field the version does not have.
  assert.deepEqual(parseNpmViewDependencies({ status: 0, stdout: "\n", stderr: "" }), {});
  assert.throws(
    () => parseNpmViewDependencies({ status: 1, stdout: "", stderr: "npm error code E404\n" }),
    /npm view failed with status 1/,
  );
});

test("the root package is published without lifecycle scripts", () => {
  // prepack would rebuild dist/ and binding.js between the root package
  // check and the publish, from a package.json that lists the platform
  // packages, which the binding generator refuses.
  assert.deepEqual(NPM_PUBLISH_ARGS, ["publish", "--access", "public", "--ignore-scripts"]);
});

function releaseFixture(pkg = PKG) {
  const rootDir = fs.mkdtempSync(path.join(os.tmpdir(), "primeval-release-"));
  const source = `${JSON.stringify(pkg, null, 2)}\n`;
  fs.writeFileSync(path.join(rootDir, "package.json"), source);
  const readPackage = () => JSON.parse(fs.readFileSync(path.join(rootDir, "package.json"), "utf8"));
  const events = [];
  const tools = {
    verifyRoot: (dir) => {
      assert.equal(dir, rootDir);
      events.push(["verifyRoot", readPackage().optionalDependencies]);
    },
    packedManifest: (dir) => {
      assert.equal(dir, rootDir);
      events.push(["packedManifest"]);
      return readPackage();
    },
    isPublished: () => false,
    publish: ({ name, dir }) => {
      const onDisk = dir === rootDir ? readPackage().optionalDependencies : undefined;
      events.push(["publish", name, onDisk]);
    },
    log: () => {},
  };
  return { rootDir, source, events, tools };
}

test("publish adds the platform packages before it checks and publishes anything", () => {
  const { rootDir, source, events, tools } = releaseFixture();
  try {
    const actions = publishRelease({ rootDir, npmDir: "/repo/npm", tag: "v1.2.3" }, tools);
    assert.deepEqual(events, [
      ["verifyRoot", PLATFORM_DEPENDENCIES],
      ["packedManifest"],
      ["publish", "@aleburato/primeval-darwin-arm64", undefined],
      ["publish", "@aleburato/primeval-linux-x64-gnu", undefined],
      ["publish", "@aleburato/primeval", PLATFORM_DEPENDENCIES],
    ]);
    assert.deepEqual(
      actions.map(({ name, dir }) => [name, dir]),
      [
        ["@aleburato/primeval-darwin-arm64", "/repo/npm/darwin-arm64"],
        ["@aleburato/primeval-linux-x64-gnu", "/repo/npm/linux-x64-gnu"],
        ["@aleburato/primeval", rootDir],
      ],
    );
    // The committed package.json is back, so later steps validate it as committed.
    assert.equal(fs.readFileSync(path.join(rootDir, "package.json"), "utf8"), source);
  } finally {
    fs.rmSync(rootDir, { recursive: true, force: true });
  }
});

test("publish fails before publishing anything when the packed root misses them", () => {
  const { rootDir, source, events, tools } = releaseFixture();
  try {
    assert.throws(
      () =>
        publishRelease(
          { rootDir, npmDir: "/repo/npm", tag: "v1.2.3" },
          { ...tools, packedManifest: () => PKG },
        ),
      /the packed root package has optionalDependencies \(none\), expected /,
    );
    assert.deepEqual(
      events.filter(([event]) => event === "publish"),
      [],
    );
    assert.equal(fs.readFileSync(path.join(rootDir, "package.json"), "utf8"), source);
  } finally {
    fs.rmSync(rootDir, { recursive: true, force: true });
  }
});

test("publish fails before publishing anything when the root package check fails", () => {
  const { rootDir, source, events, tools } = releaseFixture();
  try {
    assert.throws(
      () =>
        publishRelease(
          { rootDir, npmDir: "/repo/npm", tag: "v1.2.3" },
          {
            ...tools,
            verifyRoot: () => {
              throw new Error("the root package is missing dist/index.js");
            },
          },
        ),
      /missing dist\/index\.js/,
    );
    assert.deepEqual(events, []);
    assert.equal(fs.readFileSync(path.join(rootDir, "package.json"), "utf8"), source);
  } finally {
    fs.rmSync(rootDir, { recursive: true, force: true });
  }
});

test("publish refuses a mismatched tag or committed platform packages, leaving package.json alone", () => {
  for (const [pkg, tag, error] of [
    [PKG, "v1.2.4", /v1\.2\.4/],
    [{ ...PKG, optionalDependencies: PLATFORM_DEPENDENCIES }, "v1.2.3", /added at publish time/],
  ]) {
    const { rootDir, source, events, tools } = releaseFixture(pkg);
    try {
      assert.throws(() => publishRelease({ rootDir, npmDir: "/repo/npm", tag }, tools), error);
      assert.deepEqual(events, []);
      assert.equal(fs.readFileSync(path.join(rootDir, "package.json"), "utf8"), source);
    } finally {
      fs.rmSync(rootDir, { recursive: true, force: true });
    }
  }
});

test("release verification also checks the root package's published optionalDependencies", async () => {
  const views = [];
  const tools = {
    isPublished: () => true,
    rootDependencies: ({ name, version }) => {
      views.push(`${name}@${version}`);
      return PLATFORM_DEPENDENCIES;
    },
    sleep: async () => {},
    log: () => {},
  };
  await verifyRelease(PKG, "v1.2.3", tools);
  assert.deepEqual(views, ["@aleburato/primeval@1.2.3"]);

  await assert.rejects(
    verifyRelease(PKG, "v1.2.3", { ...tools, rootDependencies: () => ({}) }),
    /the published root package has optionalDependencies \(none\), expected /,
  );
  await assert.rejects(verifyRelease(PKG, "v1.2.4", tools), /v1\.2\.4/);
});

test("the packed root manifest is read from a real tarball", () => {
  const rootDir = fs.mkdtempSync(path.join(os.tmpdir(), "primeval-packed-manifest-"));
  try {
    const pkg = {
      ...withPlatformDependencies(PKG),
      files: ["index.js"],
      scripts: { prepack: "exit 1" },
    };
    fs.writeFileSync(path.join(rootDir, "package.json"), `${JSON.stringify(pkg, null, 2)}\n`);
    fs.writeFileSync(path.join(rootDir, "index.js"), "export {};\n");
    const manifest = packedRootManifest(rootDir);
    assert.equal(manifest.name, PKG.name);
    assert.deepEqual(manifest.optionalDependencies, PLATFORM_DEPENDENCIES);
    // No tarball is left behind.
    assert.deepEqual(fs.readdirSync(rootDir).sort(), ["index.js", "package.json"]);
  } finally {
    fs.rmSync(rootDir, { recursive: true, force: true });
  }
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
