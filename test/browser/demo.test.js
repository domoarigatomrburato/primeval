// The demo app (demo/), assembled by scripts/demo.mjs into a temporary site
// and driven in headless Chromium. Needs `npm run build` and
// `npm run build:wasm`. Counts and working resolutions stay small.
import assert from "node:assert/strict";
import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import { after, before, describe, test } from "node:test";
import { chromium } from "playwright";

import { buildSite } from "../../scripts/demo.mjs";
import { startServer } from "../../scripts/static-server.mjs";
import { pngSize } from "../helpers/png.js";

const SAMPLE = /Mona Lisa/;

let browser;
let siteDir;

before(async () => {
  siteDir = fs.mkdtempSync(path.join(os.tmpdir(), "primeval-demo-site-"));
  buildSite({ siteDir });
  browser = await chromium.launch();
});

after(async () => {
  await browser?.close();
  if (siteDir !== undefined) {
    fs.rmSync(siteDir, { recursive: true, force: true });
  }
});

/**
 * A fresh context and page on `origin`; `serviceWorkers` is Playwright's
 * option, and `initScript`, if given, runs in the page before the demo.
 */
async function openDemo(origin, { serviceWorkers = "block", initScript } = {}) {
  const context = await browser.newContext({ serviceWorkers, acceptDownloads: true });
  if (initScript !== undefined) {
    await context.addInitScript(initScript);
  }
  const requests = [];
  context.on("request", (request) => requests.push(new URL(request.url()).pathname));
  const page = await context.newPage();
  const errors = [];
  page.on("pageerror", (error) => errors.push(error));
  await page.goto(`${origin}/`);
  await page.waitForFunction(() => window.primevalDemo?.ready === true, undefined, {
    timeout: 10000,
  });
  await collectRuns(page);
  return { context, page, requests, errors };
}

/**
 * Keeps, in the page, a copy of each run's record as the run finishes
 * (`primevalDemo.onRun`): the demo keeps only the last one, and the copy
 * reads its markup at once, before a later run changes the stage.
 */
async function collectRuns(page) {
  await page.evaluate(() => {
    window.finishedRuns = [];
    window.primevalDemo.onRun = (run) => window.finishedRuns.push({ ...run });
  });
}

async function setControls(page, { count, resolution }) {
  await page.getByRole("spinbutton", { name: "Shapes" }).fill(String(count));
  await page.getByRole("spinbutton", { name: "Shapes" }).press("Tab");
  const advanced = page.locator("#advanced");
  if (!(await advanced.evaluate((details) => details.open))) {
    await advanced.locator("summary").click();
  }
  await page.getByLabel("Working resolution").selectOption(String(resolution));
}

const autoRun = (page) => page.getByRole("switch", { name: "Auto-run" });

/** Turns auto-run on or off. */
async function setAutoRun(page, on) {
  await autoRun(page).setChecked(on);
}

/** Picks a shape by its label: the radio itself is visually hidden. */
async function pickShape(page, value) {
  await page.locator(`label:has(> input[name="shape"][value="${value}"])`).click();
}

/** Comfortably longer than auto-run's debounce: a scheduled run would have started. */
const SETTLE_MS = 1000;

/** The live SVG's shape elements, the background `<rect>` aside. */
const shapeTags = (markup) =>
  [...markup.matchAll(/<(\w+)[\s>]/g)].map((match) => match[1]).slice(2);

/** Waits for the run that ends after `previous` finished runs, and returns it. */
async function waitForRun(page, previous) {
  await page.waitForFunction((count) => window.finishedRuns.length > count, previous, {
    timeout: 20000,
  });
  return page.evaluate((index) => window.finishedRuns[index], previous);
}

describe("demo, cross-origin isolated by headers", () => {
  let server;

  before(async () => {
    server = await startServer({ isolated: true, root: siteDir });
  });

  after(async () => {
    await server?.close();
  });

  test("a sample draws live, ends on the identical final SVG, and downloads", async () => {
    const { context, page, errors } = await openDemo(server.origin);
    try {
      await setControls(page, { count: 12, resolution: 128 });
      await page.getByRole("button", { name: SAMPLE }).click();
      const run = await waitForRun(page, 0);

      assert.equal(run.outcome, "done");
      assert.equal(run.total, 12);
      // Every step's shape reached the live SVG, which the stage keeps: it is
      // the DOM the final SVG parses to.
      assert.equal(run.liveShapeCount, 12);
      assert.equal(run.liveMarkup, run.finalMarkup);
      assert.equal(
        await page.locator("#result > svg").evaluate((svg) => svg.outerHTML),
        run.finalMarkup,
        "the stage shows the final SVG",
      );
      assert.match(await page.locator("#step").textContent(), /12\s*\/\s*12/);

      const svgButton = page.getByRole("button", { name: "Download SVG" });
      const pngButton = page.getByRole("button", { name: "Download PNG" });
      assert.equal(await svgButton.isEnabled(), true);
      assert.equal(await pngButton.isEnabled(), true);

      const [svgDownload] = await Promise.all([page.waitForEvent("download"), svgButton.click()]);
      assert.match(svgDownload.suggestedFilename(), /\.svg$/);
      assert.equal(fs.readFileSync(await svgDownload.path(), "utf8"), run.finalText);

      const [pngDownload] = await Promise.all([page.waitForEvent("download"), pngButton.click()]);
      assert.match(pngDownload.suggestedFilename(), /\.png$/);
      const size = pngSize(fs.readFileSync(await pngDownload.path()));
      assert.deepEqual(size, { width: run.width, height: run.height });
      assert.equal(Math.max(size.width, size.height), 1024);

      // The seed is shown, so Run with the same controls reproduces the result.
      await page.getByRole("button", { name: "Run", exact: true }).click();
      const again = await waitForRun(page, 1);
      assert.equal(again.finalText, run.finalText);
      // The demo itself keeps only the last run.
      const kept = await page.evaluate(() => ({
        runCount: window.primevalDemo.runCount,
        lastRun: window.primevalDemo.lastRun,
        keys: Object.keys(window.primevalDemo),
      }));
      assert.equal(kept.runCount, 2);
      assert.equal(kept.lastRun.finalText, again.finalText);
      assert.equal(kept.lastRun.liveMarkup, kept.lastRun.finalMarkup);
      assert.deepEqual(kept.keys.sort(), ["lastRun", "onRun", "ready", "runCount", "step"]);

      // A new seed gives another result.
      const seed = await page.getByRole("textbox", { name: "Seed" }).inputValue();
      await page.getByRole("button", { name: "New seed" }).click();
      assert.notEqual(await page.getByRole("textbox", { name: "Seed" }).inputValue(), seed);
      assert.deepEqual(errors, []);
    } finally {
      await context.close();
    }
  });

  test("Stop mid-render shows Stopped and a new run works", async () => {
    const { context, page, errors } = await openDemo(server.origin);
    try {
      // The Run button's own behavior: no setting change runs.
      await setAutoRun(page, false);
      await setControls(page, { count: 2000, resolution: 256 });
      await page.getByRole("button", { name: SAMPLE }).click();
      await page.waitForFunction(() => window.primevalDemo.step >= 1, undefined, {
        timeout: 20000,
      });
      await page.getByRole("button", { name: "Stop" }).click();
      const stopped = await waitForRun(page, 0);

      assert.equal(stopped.outcome, "stopped");
      assert.ok(stopped.step < 2000, "stopped before the end");
      assert.match(await page.locator("#run-state").textContent(), /Stopped/);
      assert.equal(await page.getByRole("alert").isVisible(), false, "no error shown");
      assert.equal(await page.getByRole("button", { name: "Stop" }).isEnabled(), false);
      assert.equal(await page.getByRole("button", { name: "Download SVG" }).isEnabled(), false);

      await setControls(page, { count: 10, resolution: 128 });
      await page.getByRole("button", { name: "Run", exact: true }).click();
      const done = await waitForRun(page, 1);
      assert.equal(done.outcome, "done");
      assert.equal(done.liveShapeCount, 10);
      assert.match(await page.locator("#run-state").textContent(), /Done/);
      assert.deepEqual(errors, []);
    } finally {
      await context.close();
    }
  });

  test("a new Run aborts the previous one, whose results are dropped", async () => {
    const { context, page } = await openDemo(server.origin);
    try {
      await setAutoRun(page, false);
      await setControls(page, { count: 2000, resolution: 256 });
      await page.getByRole("button", { name: SAMPLE }).click();
      await page.waitForFunction(() => window.primevalDemo.step >= 1, undefined, {
        timeout: 20000,
      });
      await setControls(page, { count: 10, resolution: 128 });
      await page.getByRole("button", { name: "Run", exact: true }).click();
      const first = await waitForRun(page, 0);
      assert.equal(first.outcome, "stopped");
      const second = await waitForRun(page, 1);
      assert.equal(second.outcome, "done");
      assert.equal(second.liveShapeCount, 10);
      assert.equal(second.liveMarkup, second.finalMarkup);
    } finally {
      await context.close();
    }
  });

  test("an unreadable picked or dropped file shows the validation error", async () => {
    const { context, page } = await openDemo(server.origin);
    try {
      await page.locator("#file-input").setInputFiles({
        name: "notes.txt",
        mimeType: "text/plain",
        buffer: Buffer.from("not an image"),
      });
      const picked = await waitForRun(page, 0);
      assert.equal(picked.outcome, "error");
      const alert = page.getByRole("alert");
      assert.equal(await alert.isVisible(), true);
      assert.match(await alert.textContent(), /invalid image data/);

      await page.evaluate(() => {
        const data = new DataTransfer();
        data.items.add(new File(["still not an image"], "notes.txt", { type: "text/plain" }));
        const stage = document.querySelector("#stage");
        for (const type of ["dragenter", "dragover", "drop"]) {
          stage.dispatchEvent(
            new DragEvent(type, { dataTransfer: data, bubbles: true, cancelable: true }),
          );
        }
      });
      const dropped = await waitForRun(page, 1);
      assert.equal(dropped.outcome, "error");
      assert.match(await alert.textContent(), /invalid image data/);
    } finally {
      await context.close();
    }
  });

  test("an invalid seed shows the option and its requirement", async () => {
    const { context, page } = await openDemo(server.origin);
    try {
      await page.getByRole("textbox", { name: "Seed" }).fill("-3");
      await page.getByRole("button", { name: SAMPLE }).click();
      const run = await waitForRun(page, 0);
      assert.equal(run.outcome, "error");
      const text = await page.getByRole("alert").textContent();
      assert.match(text, /seed/);
      assert.match(text, /must be an integer from 0 to 2\^64 - 1/);

      // Auto-run tries another invalid seed once, and does not loop.
      await page.getByRole("textbox", { name: "Seed" }).fill("-4");
      const again = await waitForRun(page, 1);
      assert.equal(again.outcome, "error");
      assert.match(await page.getByRole("alert").textContent(), /seed/);
      await page.waitForTimeout(SETTLE_MS);
      assert.equal(await page.evaluate(() => window.finishedRuns.length), 2);
    } finally {
      await context.close();
    }
  });

  test("auto-run is on by default, and a setting change aborts the run for a new one", async () => {
    const { context, page, errors } = await openDemo(server.origin);
    try {
      assert.equal(await autoRun(page).isChecked(), true);
      await setControls(page, { count: 2000, resolution: 256 });
      await page.getByRole("button", { name: SAMPLE }).click();
      await page.waitForFunction(() => window.primevalDemo.step >= 1, undefined, {
        timeout: 20000,
      });

      await pickShape(page, "circle");
      const first = await waitForRun(page, 0);
      assert.equal(first.outcome, "stopped");
      assert.ok(first.step < 2000, "aborted before the end");
      // The replaced run says nothing: the new one owns the status.
      assert.doesNotMatch(await page.locator("#run-state").textContent(), /Stopped|Error/);
      assert.equal(await page.getByRole("alert").isVisible(), false, "no error shown");
      await page.waitForFunction(() => window.primevalDemo.step >= 1, undefined, {
        timeout: 20000,
      });
      assert.match(await page.locator("#caption-meta").textContent(), /2000 circle shapes/);

      // Stop stops the run, and auto-run stays on.
      await page.getByRole("button", { name: "Stop" }).click();
      const stopped = await waitForRun(page, 1);
      assert.equal(stopped.outcome, "stopped");
      assert.match(await page.locator("#run-state").textContent(), /Stopped/);
      assert.equal(await autoRun(page).isChecked(), true);

      // The next change runs again, with every setting as shown.
      await page.getByRole("spinbutton", { name: "Shapes" }).fill("6");
      const done = await waitForRun(page, 2);
      assert.equal(done.outcome, "done");
      assert.equal(done.total, 6);
      assert.deepEqual(shapeTags(done.liveMarkup), Array(6).fill("circle"));
      assert.match(await page.locator("#run-state").textContent(), /Done/);
      assert.deepEqual(errors, []);
    } finally {
      await context.close();
    }
  });

  test("auto-run starts one run for several rapid changes", async () => {
    const { context, page } = await openDemo(server.origin);
    try {
      await setControls(page, { count: 4, resolution: 128 });
      await page.getByRole("button", { name: SAMPLE }).click();
      assert.equal((await waitForRun(page, 0)).outcome, "done");

      const shapes = page.getByRole("spinbutton", { name: "Shapes" });
      for (const count of ["5", "6", "7"]) {
        await shapes.fill(count);
      }
      const run = await waitForRun(page, 1);
      assert.equal(run.outcome, "done");
      assert.equal(run.total, 7);
      await page.waitForTimeout(SETTLE_MS);
      assert.equal(await page.evaluate(() => window.finishedRuns.length), 2);
    } finally {
      await context.close();
    }
  });

  test("with auto-run off, a setting change starts nothing", async () => {
    const { context, page } = await openDemo(server.origin);
    try {
      await setControls(page, { count: 4, resolution: 128 });
      await page.getByRole("button", { name: SAMPLE }).click();
      assert.equal((await waitForRun(page, 0)).outcome, "done");

      await setAutoRun(page, false);
      await pickShape(page, "circle");
      await page.getByRole("spinbutton", { name: "Shapes" }).fill("5");
      await page.getByRole("button", { name: "New seed" }).click();
      // Turning auto-run off also drops a run already scheduled.
      await setAutoRun(page, true);
      await page.getByRole("spinbutton", { name: "Shapes" }).fill("6");
      await setAutoRun(page, false);
      await page.waitForTimeout(SETTLE_MS);
      assert.equal(await page.evaluate(() => window.finishedRuns.length), 1);
      assert.equal(await page.getByRole("button", { name: "Stop" }).isEnabled(), false);
      assert.match(await page.locator("#run-state").textContent(), /Done/);

      // Run still works.
      await page.getByRole("button", { name: "Run", exact: true }).click();
      const run = await waitForRun(page, 1);
      assert.equal(run.outcome, "done");
      assert.deepEqual(shapeTags(run.liveMarkup), Array(6).fill("circle"));
    } finally {
      await context.close();
    }
  });

  test("the auto-run choice survives a reload, and storage failures are harmless", async () => {
    const { context, page } = await openDemo(server.origin);
    try {
      await setAutoRun(page, false);
      await page.reload();
      await page.waitForFunction(() => window.primevalDemo?.ready === true);
      assert.equal(await autoRun(page).isChecked(), false);
      await setAutoRun(page, true);
      await page.reload();
      await page.waitForFunction(() => window.primevalDemo?.ready === true);
      assert.equal(await autoRun(page).isChecked(), true);
    } finally {
      await context.close();
    }

    const blocked = await openDemo(server.origin, {
      initScript: () => {
        Object.defineProperty(window, "localStorage", {
          get() {
            throw new DOMException("storage is blocked", "SecurityError");
          },
        });
      },
    });
    try {
      assert.equal(await autoRun(blocked.page).isChecked(), true);
      await setAutoRun(blocked.page, false);
      assert.equal(await autoRun(blocked.page).isChecked(), false);
      assert.deepEqual(blocked.errors, []);
    } finally {
      await blocked.context.close();
    }
  });

  test("before an image is loaded, a setting change starts nothing", async () => {
    const { context, page } = await openDemo(server.origin);
    try {
      await pickShape(page, "circle");
      await page.getByRole("spinbutton", { name: "Shapes" }).fill("5");
      await page.getByRole("button", { name: "New seed" }).click();
      await page.waitForTimeout(SETTLE_MS);
      assert.equal(await page.evaluate(() => window.finishedRuns.length), 0);
      assert.equal(await page.getByRole("alert").isVisible(), false, "no error shown");
      assert.match(await page.locator("#run-state").textContent(), /Ready/);
    } finally {
      await context.close();
    }
  });

  test("every control has an accessible name", async () => {
    const { context, page } = await openDemo(server.origin);
    try {
      const unnamed = await page.evaluate(() => {
        const text = (ids) =>
          ids
            .split(/\s+/)
            .map((id) => document.getElementById(id)?.textContent ?? "")
            .join(" ");
        return [...document.querySelectorAll("input, select, textarea, button, summary")]
          .filter((element) => {
            const name =
              element.getAttribute("aria-label") ??
              (element.hasAttribute("aria-labelledby")
                ? text(element.getAttribute("aria-labelledby"))
                : null) ??
              ([...(element.labels ?? [])].map((label) => label.textContent).join(" ") ||
                (element.matches("button, summary") ? element.textContent : ""));
            return name.trim() === "";
          })
          .map((element) => element.outerHTML.slice(0, 80));
      });
      assert.deepEqual(unnamed, []);

      // The names Chromium's accessibility tree computes for the main controls.
      for (const shape of [
        "Any",
        "Triangle",
        "Rectangle",
        "Ellipse",
        "Circle",
        "Rotated rectangle",
        "Quadratic",
        "Rotated ellipse",
        "Polygon",
      ]) {
        assert.equal(await page.getByRole("radio", { name: shape, exact: true }).count(), 1, shape);
      }
      assert.equal(await page.getByRole("slider", { name: "Shapes" }).count(), 1);
      assert.equal(await page.getByRole("slider", { name: "Opacity" }).count(), 1);
      assert.equal(await page.getByRole("textbox", { name: "Seed" }).count(), 1);
      assert.equal(await page.getByRole("progressbar", { name: "Progress" }).count(), 1);
      assert.equal(await page.getByRole("button", { name: "Choose image" }).count(), 1);
      assert.equal(await page.getByRole("switch", { name: "Auto-run" }).count(), 1);
    } finally {
      await context.close();
    }
  });
});

describe("demo, served without isolation headers", () => {
  let server;

  before(async () => {
    server = await startServer({ isolated: false, root: siteDir });
  });

  after(async () => {
    await server?.close();
  });

  test("the service worker isolates the page after a single reload", async () => {
    const context = await browser.newContext({ serviceWorkers: "allow" });
    try {
      const requests = [];
      context.on("request", (request) => requests.push(new URL(request.url()).pathname));
      const page = await context.newPage();
      // Main-frame navigations commit before the new document runs.
      let loads = 0;
      page.on("framenavigated", (frame) => {
        if (frame === page.mainFrame()) {
          loads += 1;
        }
      });
      await page.goto(`${server.origin}/`);
      await page.waitForFunction(() => window.crossOriginIsolated === true, undefined, {
        timeout: 15000,
      });
      await page.waitForFunction(() => window.primevalDemo?.ready === true);
      await collectRuns(page);
      assert.equal(loads, 2, "the first load and the bootstrap's one reload");

      const threads = await page.evaluate(() => navigator.hardwareConcurrency);
      assert.equal(
        (await page.locator("#threads").textContent()).trim(),
        `${threads} ${threads === 1 ? "thread" : "threads"} · cross-origin isolated`,
      );

      await setControls(page, { count: 4, resolution: 128 });
      await page.getByRole("button", { name: SAMPLE }).click();
      const run = await waitForRun(page, 0);
      assert.equal(run.outcome, "done");
      const wasm = requests.filter((pathname) => pathname.endsWith(".wasm"));
      assert.ok(wasm.includes("/wasm/threaded/primeval_bg.wasm"), wasm.join(", "));
      assert.ok(!wasm.includes("/wasm/single/primeval_bg.wasm"), wasm.join(", "));

      // Once controlled, a reload is isolated at once: no second reload.
      await page.reload();
      await page.waitForFunction(() => window.primevalDemo?.ready === true);
      assert.equal(await page.evaluate(() => window.crossOriginIsolated), true);
      assert.equal(loads, 3);
    } finally {
      await context.close();
    }
  });

  test("without a service worker the demo runs on one thread and says so", async () => {
    const { context, page, requests } = await openDemo(server.origin, { serviceWorkers: "block" });
    try {
      assert.equal(await page.evaluate(() => window.crossOriginIsolated), false);
      assert.equal((await page.locator("#threads").textContent()).trim(), "1 thread");
      await setControls(page, { count: 4, resolution: 128 });
      await page.getByRole("button", { name: SAMPLE }).click();
      const run = await waitForRun(page, 0);
      assert.equal(run.outcome, "done");
      assert.equal(run.liveMarkup, run.finalMarkup);
      const wasm = requests.filter((pathname) => pathname.endsWith(".wasm"));
      assert.deepEqual(wasm, ["/wasm/single/primeval_bg.wasm"]);
    } finally {
      await context.close();
    }
  });
});
