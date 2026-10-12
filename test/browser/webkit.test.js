// The demo in Playwright's WebKit, served without isolation headers, so that
// its service worker (demo/coi-sw.js) isolates the page, as on GitHub Pages.
// Under that isolation WebKit fails module workers started together (WebKit
// bug 261734); the browser runtime starts its workers one at a time, so the
// threaded build works there too. Needs `npm run build`, `npm run build:wasm`
// and Playwright's WebKit (`npx playwright install webkit`); without WebKit
// the tests are skipped. CI does not install it.
import assert from "node:assert/strict";
import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import { after, before, describe, test } from "node:test";
import { webkit } from "playwright";

import { buildSite } from "../../scripts/demo.mjs";
import { startServer } from "../../scripts/static-server.mjs";

function webkitInstalled() {
  try {
    return fs.existsSync(webkit.executablePath());
  } catch {
    return false;
  }
}

const skip = webkitInstalled()
  ? false
  : "Playwright's WebKit is not installed; install it with `npx playwright install webkit`";

describe("demo in WebKit, cross-origin isolated by its service worker", { skip }, () => {
  let siteDir;
  let server;
  let browser;
  let context;
  let page;

  before(async () => {
    siteDir = fs.mkdtempSync(path.join(os.tmpdir(), "primeval-webkit-site-"));
    buildSite({ siteDir });
    server = await startServer({ isolated: false, root: siteDir });
    browser = await webkit.launch();
    // A fresh context: the first visit registers the service worker.
    context = await browser.newContext({ serviceWorkers: "allow" });
    page = await context.newPage();
    await page.goto(`${server.origin}/`);
    await page.waitForFunction(
      () => window.crossOriginIsolated === true && navigator.serviceWorker.controller !== null,
      undefined,
      { timeout: 15000 },
    );
    await page.waitForFunction(() => window.primevalDemo?.ready === true);
  });

  after(async () => {
    await context?.close();
    await browser?.close();
    await server?.close();
    if (siteDir !== undefined) {
      fs.rmSync(siteDir, { recursive: true, force: true });
    }
  });

  test("a first visit ends isolated, with a thread per core, and runs a sample", async () => {
    // Long enough for anything the bootstrap might still do (a reload).
    await page.waitForTimeout(1000);
    assert.equal(await page.evaluate(() => window.crossOriginIsolated), true);
    const threads = await page.evaluate(() => navigator.hardwareConcurrency);
    assert.ok(threads > 1, `${threads} logical cores`);
    assert.equal((await page.locator("#threads").textContent()).trim(), `${threads} threads`);

    await page.evaluate(() => {
      window.finishedRuns = [];
      window.primevalDemo.onRun = (run) => window.finishedRuns.push({ outcome: run.outcome });
    });
    await page.getByRole("spinbutton", { name: "Shapes" }).fill("8");
    await page.getByRole("spinbutton", { name: "Shapes" }).press("Tab");
    await page.getByRole("button", { name: /Mona Lisa/ }).click();
    await page.waitForFunction(() => window.finishedRuns.length > 0, undefined, {
      timeout: 30000,
    });
    const [run] = await page.evaluate(() => window.finishedRuns);
    assert.equal(run.outcome, "done", (await page.locator("#error").textContent()).trim());
  });

  test("concurrent calls through the package both resolve", async () => {
    const outcomes = await page.evaluate(async () => {
      const { approximate } = await import("@aleburato/primeval");
      const input = new Uint8Array(await (await fetch("./samples/monalisa.jpg")).arrayBuffer());
      const run = (seed) =>
        approximate({
          input,
          output: "svg",
          render: { count: 4, resizeInput: 32, outputSize: 32, seed },
        }).then(
          (result) => result.format,
          (error) => `${error?.constructor?.name}: ${error?.message}`,
        );
      return Promise.all([run(1), run(2)]);
    });

    assert.deepEqual(outcomes, ["svg", "svg"]);
  });
});
