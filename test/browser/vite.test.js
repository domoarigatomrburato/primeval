// A Vite build of a minimal app (test/browser/fixtures/vite/) that imports the
// packed package, in headless Chromium on a page with cross-origin isolation
// (threaded build) and one without (single-threaded build). The app is built
// with no Vite config: a user needs none. Needs `npm run build`,
// `npm run build:wasm` and, for the native-equality check, `npm run build:node`.
import assert from "node:assert/strict";
import { spawnSync } from "node:child_process";
import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import { after, before, describe, test } from "node:test";
import { chromium } from "playwright";
import { build } from "vite";

import * as native from "../../dist/index.js";
import { startServer } from "../../scripts/static-server.mjs";
import { packRootPackage } from "../helpers/pack.js";

const repoRoot = process.cwd();
const FIXTURE = "monalisa.jpg";
// The request the fixture app makes.
const SMALL = { count: 4, resizeInput: 16, outputSize: 32, seed: 7 };

let browser;
let tempDir;
let outDir;
/** Per variant, the emitted worker script and the `.wasm` it loads, as URL paths. */
const assets = {};

/** The emitted per-call worker of `variant` and the `.wasm` asset it references. */
function variantAssets(variant) {
  const names = fs.readdirSync(path.join(outDir, "assets"));
  const workers = names.filter((name) => name.startsWith(`worker-${variant}-`));
  assert.equal(workers.length, 1, `one worker-${variant} chunk: ${workers.join(", ")}`);
  const source = fs.readFileSync(path.join(outDir, "assets", workers[0]), "utf8");
  const wasm = new Set(source.match(/primeval_bg-[\w-]+\.wasm/g));
  assert.equal(wasm.size, 1, `worker-${variant} references one .wasm: ${[...wasm].join(", ")}`);
  return { worker: `/assets/${workers[0]}`, wasm: `/assets/${[...wasm][0]}` };
}

before(async () => {
  tempDir = fs.mkdtempSync(path.join(os.tmpdir(), "primeval-vite-"));
  const app = path.join(tempDir, "app");
  const installed = path.join(app, "node_modules", "@aleburato", "primeval");
  fs.mkdirSync(installed, { recursive: true });

  // The tarball's contents are what an install unpacks; extracting it avoids
  // the registry lookups `npm install` makes for the optional native packages.
  // Lifecycle scripts are skipped: `prepack` would rebuild `dist/` while the
  // other browser test serves it.
  const tarball = packRootPackage(repoRoot, { destination: tempDir, ignoreScripts: true });
  const tar = spawnSync("tar", ["-xzf", tarball, "-C", installed, "--strip-components=1"], {
    encoding: "utf8",
  });
  assert.equal(tar.status, 0, tar.stderr);

  fs.cpSync(path.join(repoRoot, "test", "browser", "fixtures", "vite"), app, { recursive: true });
  fs.copyFileSync(
    path.join(repoRoot, "docs", "readme", "originals", FIXTURE),
    path.join(app, FIXTURE),
  );
  outDir = path.join(app, "dist");
  await build({ root: app, configFile: false, logLevel: "warn" });

  for (const variant of ["single", "threaded"]) {
    assets[variant] = variantAssets(variant);
  }
  browser = await chromium.launch();
});

after(async () => {
  await browser?.close();
  if (tempDir) {
    fs.rmSync(tempDir, { recursive: true, force: true });
  }
});

test("the build emits a separate worker and .wasm per variant", () => {
  assert.notEqual(assets.single.worker, assets.threaded.worker);
  assert.notEqual(assets.single.wasm, assets.threaded.wasm);
});

for (const isolated of [true, false]) {
  const variant = isolated ? "threaded" : "single";

  describe(`Vite build, ${isolated ? "cross-origin isolated" : "not isolated"} (${variant} build)`, () => {
    let server;
    let context;
    let outcome;
    let threads;
    const requests = [];
    const workers = [];

    before(async () => {
      server = await startServer({ isolated, root: outDir });
      context = await browser.newContext();
      context.on("request", (request) => requests.push(new URL(request.url()).pathname));
      const page = await context.newPage();
      page.on("worker", (worker) => workers.push(new URL(worker.url()).pathname));
      await page.goto(`${server.origin}/`);
      await page.waitForFunction(() => window.outcome !== undefined, undefined, {
        timeout: 20000,
      });
      outcome = await page.evaluate(() => window.outcome);
      threads = await page.evaluate(() => navigator.hardwareConcurrency);
    });

    after(async () => {
      await context?.close();
      await server?.close();
    });

    test("the page has the expected isolation", () => {
      assert.equal(outcome.crossOriginIsolated, isolated);
    });

    test("renders the SVG the native addon renders", async () => {
      assert.equal(outcome.error, undefined);
      const input = fs.readFileSync(path.join(repoRoot, "docs", "readme", "originals", FIXTURE));
      const expected = await native.approximate({ input, output: "svg", render: SMALL });
      assert.deepEqual(outcome.result, expected);
    });

    test(`runs the ${variant} build's workers only`, () => {
      // The call's worker, plus one pool worker per thread when threaded.
      assert.deepEqual(
        workers,
        Array.from({ length: isolated ? 1 + threads : 1 }, () => assets[variant].worker),
      );
    });

    test(`fetches only the ${variant} build's .wasm, once`, () => {
      assert.deepEqual(
        requests.filter((pathname) => pathname.endsWith(".wasm")),
        [assets[variant].wasm],
      );
    });
  });
}
