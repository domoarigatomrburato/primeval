// The browser entry (`dist/browser.js`) in headless Chromium, on a page with
// cross-origin isolation (threaded build) and one without (single-threaded
// build). Needs `npm run build`, `npm run build:wasm` and, for the
// native-equality tripwire and the error parity checks, `npm run build:node`.
import assert from "node:assert/strict";
import http from "node:http";
import { after, before, describe, test } from "node:test";
import { chromium } from "playwright";

import * as native from "../../dist/index.js";
import { startServer } from "../../scripts/static-server.mjs";
import { assertPreviewsSvg } from "../helpers/svg.js";

const FIXTURE = "monalisa.jpg";
const SHAPES = [
  "triangle",
  "rectangle",
  "ellipse",
  "circle",
  "rotated-rectangle",
  "rotated-ellipse",
  "quadratic",
  "polygon",
  "any",
];
// Every shape kind, with enough steps that each kind's search runs at least
// one refit pass (the first comes at step 10 or 20) before its final stage.
const TRIPWIRE_RENDERS = [1, 2, 3].flatMap((seed) =>
  SHAPES.map((shape) => ({ count: 20, resizeInput: 64, outputSize: 128, seed, shape })),
);
// The same at a quarter of the pixels, for the PNG writer: the SVG tripwire
// already covers the search at the larger size.
const PNG_TRIPWIRE_RENDERS = TRIPWIRE_RENDERS.map((render) => ({
  ...render,
  resizeInput: 32,
  outputSize: 64,
}));
const SMALL = { count: 4, resizeInput: 16, outputSize: 32, seed: 7 };
const LONG_RENDER = { count: 100000, resizeInput: 16, outputSize: 16, seed: 7 };
// A render that never finishes on its own can only end through a timeout.
const PANIC_TIMEOUT_MS = 10000;

/** Whether `url` is the script of a call's worker or a pool worker, not a module it imports. */
const isWorkerScript = (url) => /\/dist\/worker-(single|threaded)\.js$/.test(new URL(url).pathname);

const delay = (ms) => new Promise((resolve) => setTimeout(resolve, ms));

/**
 * A proxy to `origin` that serves every script of the package (`/dist/` and
 * the wasm-bindgen glue) `delayMs` late and keeps the span of each one, from
 * its request to its response, on one clock. Playwright's routes miss the
 * requests of nested (pool) workers, so the tests delay them here instead.
 */
function startSlowScriptProxy(origin, delayMs) {
  const spans = [];
  const server = http.createServer(async (request, response) => {
    const { pathname } = new URL(request.url, origin);
    const slow = /^\/(dist|wasm)\/.*\.js$/.test(pathname);
    const span = { pathname, start: performance.now(), end: Number.POSITIVE_INFINITY };
    if (slow) {
      spans.push(span);
      await delay(delayMs);
    }
    const upstream = await fetch(`${origin}${request.url}`);
    const body = Buffer.from(await upstream.arrayBuffer());
    span.end = performance.now();
    response.writeHead(upstream.status, Object.fromEntries(upstream.headers)).end(body);
  });
  return new Promise((resolve) => {
    server.listen(0, "127.0.0.1", () => {
      resolve({
        origin: `http://127.0.0.1:${server.address().port}`,
        spans,
        close: () => new Promise((done) => server.close(done)),
      });
    });
  });
}

let browser;

before(async () => {
  browser = await chromium.launch();
});

after(async () => {
  await browser?.close();
});

/** The fields of an error the tests compare, as `window.describeError` reads them. */
function describeError(error) {
  return {
    isError: error instanceof Error,
    isPrimevalError: error instanceof native.PrimevalError,
    constructorName: error?.constructor?.name,
    name: error?.name,
    code: error?.code,
    message: error?.message,
    option: error?.option,
    requirement: error?.requirement,
    hasOption: error instanceof Error && Object.hasOwn(error, "option"),
  };
}

// The native addon's tripwire outputs, rendered once for both builds.
const nativeOutputs = new Map();

function nativeOutput(input, output, render) {
  const key = JSON.stringify([output, render]);
  if (!nativeOutputs.has(key)) {
    nativeOutputs.set(key, native.approximate({ input, output, render }));
  }
  return nativeOutputs.get(key);
}

async function nativeRejection(request) {
  try {
    await native.approximate(request);
  } catch (error) {
    return describeError(error);
  }
  assert.fail("the native render was expected to reject");
}

for (const isolated of [true, false]) {
  const variant = isolated ? "threaded" : "single";
  const other = isolated ? "single" : "threaded";
  const workerCount = (threads) => (isolated ? 1 + threads : 1);

  describe(`browser approximate, ${isolated ? "cross-origin isolated" : "not isolated"} (${variant} build)`, () => {
    let server;
    let shared;
    let fixture;

    // A fresh context per page, so its requests (workers included) and its
    // module cache are its own.
    async function openPage(origin = server.origin) {
      const context = await browser.newContext();
      const requests = [];
      context.on("request", (request) => requests.push(new URL(request.url()).pathname));
      const page = await context.newPage();
      await page.goto(`${origin}/test/browser/page.html`);
      await page.waitForFunction(() => window.ready === true, undefined, { timeout: 10000 });
      await page.evaluate(
        async ({ name, render }) => {
          const input = await window.fixture(name);
          window.smallSvg = (overrides = {}) =>
            window.primeval.approximate({
              input,
              output: "svg",
              render,
              ...overrides,
            });
        },
        { name: FIXTURE, render: SMALL },
      );
      return { context, page, requests };
    }

    const wasmRequests = (requests) => requests.filter((pathname) => pathname.endsWith(".wasm"));

    before(async () => {
      server = await startServer({ isolated });
      shared = await openPage();
      fixture = new Uint8Array(
        await (await fetch(`${server.origin}/docs/readme/originals/${FIXTURE}`)).arrayBuffer(),
      );
    });

    after(async () => {
      await shared?.context.close();
      await server?.close();
    });

    test("the page has the expected isolation", async () => {
      assert.equal(await shared.page.evaluate(() => globalThis.crossOriginIsolated), isolated);
    });

    test("renders an SVG result", async () => {
      const result = await shared.page.evaluate(
        async ({ name, render }) => {
          const svg = await window.primeval.approximate({
            input: await window.fixture(name),
            output: "svg",
            render,
          });
          return { ...svg, dataType: typeof svg.data };
        },
        { name: FIXTURE, render: SMALL },
      );
      const expected = await native.approximate({ input: fixture, output: "svg", render: SMALL });

      assert.equal(result.format, "svg");
      assert.equal(result.mimeType, "image/svg+xml");
      assert.equal(result.dataType, "string");
      assert.match(result.data, /^<svg\b/);
      assert.equal(result.width, expected.width);
      assert.equal(result.height, expected.height);
    });

    test("renders a PNG result as a Uint8Array", async () => {
      const result = await shared.page.evaluate(
        async ({ name, render }) => {
          const png = await window.primeval.approximate({
            input: await window.fixture(name),
            output: "png",
            render,
          });
          return {
            format: png.format,
            mimeType: png.mimeType,
            width: png.width,
            height: png.height,
            isUint8Array: png.data instanceof Uint8Array,
            constructorName: png.data.constructor.name,
            magic: Array.from(png.data.subarray(0, 8)),
            dataUri: window.primeval.toDataUri(png).slice(0, 40),
          };
        },
        { name: FIXTURE, render: SMALL },
      );
      const expected = await native.approximate({ input: fixture, output: "png", render: SMALL });

      assert.equal(result.format, "png");
      assert.equal(result.mimeType, "image/png");
      assert.equal(result.isUint8Array, true);
      assert.equal(result.constructorName, "Uint8Array");
      assert.deepEqual(result.magic, [0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a]);
      assert.equal(result.width, expected.width);
      assert.equal(result.height, expected.height);
      assert.equal(result.dataUri, native.toDataUri(expected).slice(0, 40));
    });

    test("toDataUri encodes an SVG result as Node does", async () => {
      const svg = '<svg xmlns="http://www.w3.org/2000/svg">é ✓</svg>';
      const result = { format: "svg", data: svg, mimeType: "image/svg+xml", width: 1, height: 1 };

      const uri = await shared.page.evaluate((value) => window.primeval.toDataUri(value), result);

      assert.equal(uri, native.toDataUri(result));
    });

    test("calls onProgress count times with increasing steps", async () => {
      const progress = await shared.page.evaluate(
        async ({ name, render }) => {
          const steps = [];
          await window.primeval.approximate({
            input: await window.fixture(name),
            output: "svg",
            render,
            execution: { onProgress: (info) => steps.push(info) },
          });
          return steps;
        },
        { name: FIXTURE, render: { ...SMALL, count: 6 } },
      );

      assert.deepEqual(
        progress.map((info) => info.step),
        [1, 2, 3, 4, 5, 6],
      );
      assert.ok(progress.every((info) => info.total === 6));
      assert.ok(progress.every((info) => typeof info.score === "number"));
    });

    test("onProgress shapes preview the final SVG's shape lines", async () => {
      const { shapes, svg } = await shared.page.evaluate(
        async ({ name, render }) => {
          const shapes = [];
          const result = await window.primeval.approximate({
            input: await window.fixture(name),
            output: "svg",
            render,
            execution: { onProgress: (info) => shapes.push(info.shape) },
          });
          return { shapes, svg: result.data };
        },
        { name: FIXTURE, render: { ...SMALL, count: 6, shape: "quadratic" } },
      );

      assertPreviewsSvg(shapes, svg, 6);
    });

    test("a throwing onProgress rejects with the thrown value and stops the render", async () => {
      const outcome = await shared.page.evaluate(
        async ({ name, render }) => {
          const boom = new Error("boom");
          let calls = 0;
          let rejection;
          try {
            await window.primeval.approximate({
              input: await window.fixture(name),
              output: "svg",
              render,
              execution: {
                onProgress() {
                  calls += 1;
                  throw boom;
                },
              },
            });
          } catch (error) {
            rejection = error;
          }
          await new Promise((resolve) => setTimeout(resolve, 100));
          return { same: rejection === boom, calls };
        },
        { name: FIXTURE, render: LONG_RENDER },
      );

      assert.deepEqual(outcome, { same: true, calls: 1 });
    });

    test("invalid requests reject with the errors Node rejects with", async () => {
      // Each case is built the same way in the page and in Node.
      const cases = [
        { input: "fixture", output: "svg", render: { count: 0 } },
        { input: "fixture", output: "svg", render: { resizeInput: 1 } },
        { input: "fixture", output: "svg", render: { shape: "hexagon" } },
        { input: "fixture", output: "svg", render: { alpha: null } },
        { input: "fixture", output: "svg", render: { count: "4" } },
        { input: "fixture", output: "gif", render: {} },
        { input: "fixture", output: 5, render: {} },
        { input: "bad", output: "svg", render: SMALL },
        { input: "string", output: "svg", render: SMALL },
      ];
      const build = (spec, bytes) => ({
        input:
          spec.input === "fixture"
            ? bytes
            : spec.input === "bad"
              ? new Uint8Array([1, 2, 3, 4])
              : "not bytes",
        output: spec.output,
        render: spec.render,
      });

      const browserErrors = await shared.page.evaluate(
        async ({ name, cases, buildSource }) => {
          const bytes = await window.fixture(name);
          const buildRequest = new Function(`return (${buildSource})`)();
          const errors = [];
          for (const spec of cases) {
            try {
              await window.primeval.approximate(buildRequest(spec, bytes));
              errors.push(null);
            } catch (error) {
              errors.push(window.describeError(error));
            }
          }
          return errors;
        },
        { name: FIXTURE, cases, buildSource: build.toString() },
      );

      const nodeErrors = [];
      for (const spec of cases) {
        nodeErrors.push(await nativeRejection(build(spec, fixture)));
      }
      assert.deepEqual(browserErrors, nodeErrors);
      assert.deepEqual(
        nodeErrors.map((error) => error.code),
        [
          "INVALID_OPTION",
          "INVALID_OPTION",
          "INVALID_OPTION",
          "INVALID_OPTION",
          "INVALID_OPTION",
          "INVALID_OPTION",
          "INVALID_OPTION",
          "INVALID_IMAGE",
          "INVALID_OPTION",
        ],
      );
      assert.equal(nodeErrors[0].option, "count");
      assert.equal(nodeErrors[0].hasOption, true);
      assert.equal(nodeErrors[0].constructorName, "ValidationError");
    });

    test("no seed gives different outputs, the same seed the same output", async () => {
      const svgs = await shared.page.evaluate(
        async ({ name }) => {
          const input = await window.fixture(name);
          const render = { count: 8, resizeInput: 32, outputSize: 32 };
          const run = (extra) =>
            window.primeval
              .approximate({ input, output: "svg", render: { ...render, ...extra } })
              .then((result) => result.data);
          return {
            unseeded: [await run({}), await run({})],
            // A bigint seed crosses postMessage as a bigint.
            seeded: [await run({ seed: 42 }), await run({ seed: 42 }), await run({ seed: 42n })],
          };
        },
        { name: FIXTURE },
      );

      assert.notEqual(svgs.unseeded[0], svgs.unseeded[1]);
      assert.equal(svgs.seeded[0], svgs.seeded[1]);
      assert.equal(svgs.seeded[2], svgs.seeded[0]);
    });

    test("an already-aborted signal rejects without fetching or starting anything", async () => {
      const { context, page, requests } = await openPage();
      try {
        const outcome = await page.evaluate(
          async ({ name }) => {
            const input = await window.fixture(name);
            const controller = new AbortController();
            controller.abort(new Error("stop"));
            let calls = 0;
            try {
              // count: 0 fails Rust validation, so reaching the worker would
              // reject with ValidationError instead.
              await window.primeval.approximate({
                input,
                output: "svg",
                render: { count: 0 },
                execution: { signal: controller.signal, onProgress: () => (calls += 1) },
              });
              return { resolved: true };
            } catch (error) {
              return {
                error: window.describeError(error),
                causeIsReason: error.cause === controller.signal.reason,
                calls,
              };
            }
          },
          { name: FIXTURE },
        );

        assert.equal(outcome.error.constructorName, "AbortError");
        assert.equal(outcome.error.code, "ABORTED");
        assert.equal(outcome.causeIsReason, true);
        assert.equal(outcome.calls, 0);
        assert.deepEqual(
          requests.filter(
            (pathname) => pathname.startsWith("/wasm/") || pathname.startsWith("/dist/worker-"),
          ),
          [],
        );
      } finally {
        await context.close();
      }
    });

    test("aborting mid-render rejects with AbortError whose cause is the reason", async () => {
      const outcome = await shared.page.evaluate(
        async ({ name, render }) => {
          const controller = new AbortController();
          let lastStep = 0;
          let calls = 0;
          let rejection;
          try {
            await window.primeval.approximate({
              input: await window.fixture(name),
              output: "svg",
              render,
              execution: {
                signal: controller.signal,
                onProgress(info) {
                  calls += 1;
                  lastStep = info.step;
                  if (info.step === 1) {
                    controller.abort(new Error("stop"));
                  }
                },
              },
            });
          } catch (error) {
            rejection = error;
          }
          const callsAtRejection = calls;
          await new Promise((resolve) => setTimeout(resolve, 100));
          return {
            error: window.describeError(rejection),
            causeIsReason: rejection?.cause === controller.signal.reason,
            lastStep,
            noProgressAfterRejection: calls === callsAtRejection,
          };
        },
        { name: FIXTURE, render: LONG_RENDER },
      );

      assert.equal(outcome.error.constructorName, "AbortError");
      assert.equal(outcome.error.code, "ABORTED");
      assert.equal(outcome.causeIsReason, true);
      assert.ok(outcome.lastStep < LONG_RENDER.count, `stopped at step ${outcome.lastStep}`);
      assert.equal(outcome.noProgressAfterRejection, true);
    });

    test("an abort before the result is delivered wins even if the render finished", async () => {
      const outcome = await shared.page.evaluate(
        async ({ name, render }) => {
          const controller = new AbortController();
          try {
            await window.primeval.approximate({
              input: await window.fixture(name),
              output: "svg",
              render,
              execution: {
                signal: controller.signal,
                onProgress(info) {
                  if (info.step === info.total) {
                    // The worker finishes and posts its result while the page
                    // is busy, so the result is already queued at the abort.
                    const blockUntil = Date.now() + 300;
                    while (Date.now() < blockUntil) {
                      // Busy-wait.
                    }
                    controller.abort();
                  }
                },
              },
            });
            return { resolved: true };
          } catch (error) {
            return {
              error: window.describeError(error),
              causeIsReason: error.cause === controller.signal.reason,
            };
          }
        },
        { name: FIXTURE, render: { ...SMALL, count: 2 } },
      );

      assert.equal(outcome.error?.constructorName, "AbortError");
      assert.equal(outcome.causeIsReason, true);
    });

    test("aborting after the render settled is a no-op", async () => {
      const format = await shared.page.evaluate(
        async ({ name, render }) => {
          const controller = new AbortController();
          const result = await window.primeval.approximate({
            input: await window.fixture(name),
            output: "svg",
            render,
            execution: { signal: controller.signal },
          });
          controller.abort();
          return result.format;
        },
        { name: FIXTURE, render: SMALL },
      );

      assert.equal(format, "svg");
    });

    test("concurrent calls settle like sequential ones", async () => {
      const outcome = await shared.page.evaluate(
        async ({ name, render }) => {
          const input = await window.fixture(name);
          const seeds = [1, 2, 3, 4];
          const run = (seed) =>
            window.primeval
              .approximate({ input, output: "svg", render: { ...render, seed } })
              .then((result) => result.data);
          const concurrent = await Promise.all(seeds.map(run));
          const sequential = [];
          for (const seed of seeds) {
            sequential.push(await run(seed));
          }
          return { concurrent, sequential };
        },
        { name: FIXTURE, render: { ...SMALL, count: 6 } },
      );

      assert.deepEqual(outcome.concurrent, outcome.sequential);
      assert.equal(new Set(outcome.concurrent).size, 4);
    });

    // WebKit fails module workers started together under service-worker
    // isolation (WebKit bug 261734), so the runtime starts its workers one at
    // a time: a worker's script is only requested once the previous worker,
    // of the same call or of another, has loaded its modules.
    test("workers load one at a time, also across concurrent calls", async () => {
      // Every script is served late. A worker loads once all the scripts it
      // requested arrived, so when a worker's own script is requested, every
      // script requested before it must have been served, and the previous
      // worker's imports took at least one more delay after its own script.
      // (Chromium itself serves concurrent requests for one URL in turn.)
      const delayMs = 30;
      const proxy = await startSlowScriptProxy(server.origin, delayMs);
      const { context, page } = await openPage(proxy.origin);
      try {
        const loadedPage = proxy.spans.length;
        const { formats, threads } = await page.evaluate(
          async ({ name, render }) => {
            const input = await window.fixture(name);
            const run = (seed) =>
              window.primeval
                .approximate({ input, output: "svg", render: { ...render, seed } })
                .then((result) => result.format);
            return {
              formats: await Promise.all([run(1), run(2)]),
              threads: navigator.hardwareConcurrency,
            };
          },
          { name: FIXTURE, render: SMALL },
        );

        assert.deepEqual(formats, ["svg", "svg"]);
        const spans = proxy.spans.slice(loadedPage);
        const workers = spans.filter((span) => isWorkerScript(`${proxy.origin}${span.pathname}`));
        assert.equal(workers.length, 2 * workerCount(threads));
        for (const [k, worker] of workers.entries()) {
          if (k === 0) {
            continue;
          }
          const gap = worker.start - workers[k - 1].end;
          assert.ok(
            gap >= delayMs,
            `worker script ${k} requested ${gap} ms after the previous one`,
          );
          const unserved = spans.filter(
            (span) => span.start < worker.start && span.end > worker.start,
          );
          assert.deepEqual(
            unserved.map((span) => span.pathname),
            [],
            `scripts still loading when a worker script was requested at ${worker.start}`,
          );
        }
      } finally {
        await context.close();
        await proxy.close();
      }
    });

    test("an aborted call waiting to start rejects at once and starts no worker", async () => {
      const { context, page } = await openPage();
      try {
        // The first call's worker script is served late, so the calls after
        // it wait for it to start.
        let routed = 0;
        await context.route(isWorkerScript, async (route) => {
          routed += 1;
          if (routed === 1) {
            await delay(1000);
          }
          await route.continue();
        });
        const workers = [];
        page.on("worker", (worker) => workers.push(worker.url()));
        const firstRequested = context.waitForEvent("request", {
          predicate: (request) => isWorkerScript(request.url()),
        });
        await page.evaluate(
          async ({ name, render }) => {
            window.input = await window.fixture(name);
            const run = () =>
              window.primeval
                .approximate({ input: window.input, output: "svg", render })
                .then((result) => result.format);
            window.first = run();
          },
          { name: FIXTURE, render: SMALL },
        );
        await firstRequested;
        const outcome = await page.evaluate(
          async ({ render }) => {
            const controller = new AbortController();
            const waiting = window.primeval.approximate({
              input: window.input,
              output: "svg",
              render,
              execution: { signal: controller.signal },
            });
            // Behind the aborted call, also waiting.
            const third = window.primeval
              .approximate({ input: window.input, output: "svg", render })
              .then((result) => result.format);
            await new Promise((resolve) => setTimeout(resolve, 200));
            const abortedAt = performance.now();
            controller.abort(new Error("stop"));
            let rejection;
            try {
              await waiting;
            } catch (error) {
              rejection = error;
            }
            const rejectedMs = performance.now() - abortedAt;
            const formats = await Promise.all([window.first, third]);
            return {
              error: window.describeError(rejection),
              causeIsReason: rejection?.cause === controller.signal.reason,
              rejectedMs,
              formats,
              threads: navigator.hardwareConcurrency,
            };
          },
          { render: SMALL },
        );

        assert.equal(outcome.error.constructorName, "AbortError");
        assert.equal(outcome.error.code, "ABORTED");
        assert.equal(outcome.causeIsReason, true);
        assert.ok(outcome.rejectedMs < 100, `rejected after ${outcome.rejectedMs} ms`);
        assert.deepEqual(outcome.formats, ["svg", "svg"]);
        // The first and third calls' workers only.
        assert.equal(workers.length, 2 * workerCount(outcome.threads));
      } finally {
        await context.close();
      }
    });

    // How the first of two concurrent calls ends before it started, what it
    // rejects with, and how its worker script is served.
    for (const [how, firstError, serve, endFirst] of [
      [
        "fails to load",
        "InternalError",
        (route, first) => (first ? route.fulfill({ status: 404, body: "" }) : route.continue()),
        () => undefined,
      ],
      [
        "is aborted while it loads",
        "AbortError",
        async (route, first) => {
          if (first) {
            await delay(1000);
          }
          await route.continue();
        },
        (page) => page.evaluate(() => window.controller.abort()),
      ],
    ]) {
      test(`a call still runs when the call before it ${how}`, async () => {
        const { context, page } = await openPage();
        try {
          let scripts = 0;
          await context.route(isWorkerScript, (route) => {
            scripts += 1;
            return serve(route, scripts === 1);
          });
          const firstRequested = context.waitForEvent("request", {
            predicate: (request) => isWorkerScript(request.url()),
          });
          await page.evaluate(
            async ({ name, render }) => {
              const input = await window.fixture(name);
              window.controller = new AbortController();
              const run = (signal) =>
                window.primeval
                  .approximate({ input, output: "svg", render, execution: { signal } })
                  .then(
                    (result) => result.format,
                    (error) => window.describeError(error).constructorName,
                  );
              window.outcomes = Promise.all([run(window.controller.signal), run(undefined)]);
            },
            { name: FIXTURE, render: SMALL },
          );
          await firstRequested;
          await endFirst(page);
          const outcomes = await page.evaluate(() => window.outcomes);

          assert.deepEqual(outcomes, [firstError, "svg"]);
        } finally {
          await context.close();
        }
      });
    }

    test(`fetches and compiles only the ${variant} build, once`, async () => {
      const { context, page, requests } = await openPage();
      try {
        await page.evaluate(
          async ({ name, render }) => {
            const input = await window.fixture(name);
            await window.primeval.approximate({ input, output: "svg", render });
            await window.primeval.approximate({ input, output: "png", render });
          },
          { name: FIXTURE, render: SMALL },
        );

        assert.deepEqual(wasmRequests(requests), [`/wasm/${variant}/primeval_bg.wasm`]);
        assert.deepEqual(
          requests.filter((pathname) => pathname.startsWith(`/wasm/${other}/`)),
          [],
        );
      } finally {
        await context.close();
      }
    });

    test(`starts only ${variant} workers from the package's own script, never from blob: URLs`, async () => {
      const { context, page } = await openPage();
      try {
        const workers = [];
        page.on("worker", (worker) => workers.push(worker.url()));
        const threads = await page.evaluate(
          async ({ name, render }) => {
            const input = await window.fixture(name);
            await window.primeval.approximate({ input, output: "svg", render });
            return navigator.hardwareConcurrency;
          },
          { name: FIXTURE, render: SMALL },
        );

        // The call's worker, plus one pool worker per thread when threaded.
        assert.deepEqual(
          workers,
          Array.from(
            { length: workerCount(threads) },
            () => `${server.origin}/dist/worker-${variant}.js`,
          ),
        );
      } finally {
        await context.close();
      }
    });

    async function trackTermination(page) {
      await page.evaluate(() => {
        window.terminated = 0;
        const terminate = Worker.prototype.terminate;
        Worker.prototype.terminate = function () {
          window.terminated += 1;
          return terminate.call(this);
        };
      });
    }

    // Chromium reports nested worker closure late (especially after a pool
    // panic). Observe every close event, allowing for that reporting delay.
    function watchWorkers(page) {
      const workers = [];
      const closed = new Set();
      page.on("worker", (worker) => {
        workers.push(worker);
        worker.once("close", () => closed.add(worker));
      });
      return {
        workers,
        closed,
        async waitClosed(expected) {
          const deadline = Date.now() + 15000;
          while (closed.size < expected && Date.now() < deadline) {
            await delay(20);
          }
          assert.equal(closed.size, expected);
        },
      };
    }

    test("worker reuse: mixed requests match fresh outputs and isolate progress", async () => {
      const { context, page } = await openPage();
      const observed = watchWorkers(page);
      try {
        const outcome = await page.evaluate(
          async ({ name, render, shapes }) => {
            const messages = [];
            const postMessage = Worker.prototype.postMessage;
            Worker.prototype.postMessage = function (message, ...args) {
              messages.push({ type: message.type, keys: Object.keys(message).sort() });
              return postMessage.call(this, message, ...args);
            };
            const input = await window.fixture(name);
            const requests = shapes.map((shape, i) => ({
              output: i % 2 ? "png" : "svg",
              render: {
                ...render,
                count: 20,
                shape,
                seed: i + 1,
                alpha: i % 2 ? 128 : "auto",
                outputSize: i % 2 ? 48 : 32,
                background: i % 2 ? "#112233" : "auto",
              },
            }));
            const sequence = async () => {
              const progress = requests.map(() => []);
              const outputs = [];
              for (const [i, request] of requests.entries()) {
                const result = await window.primeval.approximate({
                  input,
                  ...request,
                  execution:
                    i % 2
                      ? undefined
                      : {
                          onProgress: (info) => progress[i].push(info),
                        },
                });
                outputs.push({
                  ...result,
                  data: result.format === "svg" ? result.data : Array.from(result.data),
                });
              }
              return { outputs, progress };
            };
            const reused = await sequence();
            const repeated = await sequence();
            const starts = window.runtime.workersStartedForTests();
            window.runtime.setIdleMsForTests(0);
            const fresh = await sequence();
            return {
              reused,
              repeated,
              fresh,
              starts,
              messages,
              totalStarts: window.runtime.workersStartedForTests(),
              threads: navigator.hardwareConcurrency,
            };
          },
          { name: FIXTURE, render: { ...SMALL, resizeInput: 32 }, shapes: SHAPES },
        );
        assert.equal(outcome.starts, 1);
        assert.equal(outcome.totalStarts, 1 + SHAPES.length);
        assert.equal(observed.workers.length, outcome.totalStarts * workerCount(outcome.threads));
        await observed.waitClosed(observed.workers.length);
        assert.deepEqual(
          outcome.messages.filter((message) => message.type === "init"),
          Array.from({ length: outcome.totalStarts }, () => ({
            type: "init",
            keys: ["channel", "module", "type"],
          })),
        );
        assert.deepEqual(
          outcome.messages.filter((message) => message.type === "run"),
          Array.from({ length: 3 * SHAPES.length }, () => ({
            type: "run",
            keys: ["progress", "task", "type"],
          })),
        );
        assert.deepEqual(outcome.repeated, outcome.reused);
        assert.deepEqual(outcome.reused, outcome.fresh);
        for (const [i, events] of outcome.reused.progress.entries()) {
          assert.equal(events.length, i % 2 ? 0 : 20);
          assert.deepEqual(
            events.map((info) => info.step),
            i % 2 ? [] : Array.from({ length: 20 }, (_, k) => k + 1),
          );
        }
      } finally {
        await context.close();
      }
    });

    const failureErrors = {
      abort: { constructorName: "AbortError", code: "ABORTED" },
      progress: { constructorName: "Error", code: undefined },
      invalid: { constructorName: "ValidationError", code: "INVALID_OPTION" },
      caller: { constructorName: "InternalError", code: "INTERNAL" },
      ...(isolated ? { pool: { constructorName: "InternalError", code: "INTERNAL" } } : {}),
    };
    for (const [failure, expected] of Object.entries(failureErrors)) {
      test(`worker reuse: ${failure} terminates immediately and the next call recovers`, async () => {
        const { context, page } = await openPage();
        const observed = watchWorkers(page);
        await trackTermination(page);
        try {
          const outcome = await page.evaluate(
            async ({ longRender, failure }) => {
              await window.smallSvg();
              let error;
              try {
                if (failure === "caller" || failure === "pool") {
                  await window.runtime.panicForTests(failure);
                } else {
                  const controller = new AbortController();
                  await window.smallSvg({
                    render: failure === "invalid" ? { count: 0 } : longRender,
                    execution: {
                      signal: controller.signal,
                      onProgress() {
                        if (failure === "abort") {
                          controller.abort();
                        } else {
                          throw new Error("reuse progress failure");
                        }
                      },
                    },
                  });
                }
              } catch (caught) {
                error = window.describeError(caught);
              }
              return {
                error,
                starts: window.runtime.workersStartedForTests(),
                terminated: window.terminated,
              };
            },
            { longRender: LONG_RENDER, failure },
          );
          assert.equal(outcome.starts, 1, "the failure ran in the warm worker");
          assert.equal(
            outcome.terminated,
            1,
            "termination happened before rejection was delivered",
          );
          assert.equal(outcome.error?.constructorName, expected.constructorName);
          assert.equal(outcome.error.code, expected.code);
          // Default idle time is 10 s: these closures must happen without
          // waiting for idle expiry (including every pool worker).
          await observed.waitClosed(observed.workers.length);
          const next = await page.evaluate(async () => {
            const result = await window.smallSvg();
            return { format: result.format, starts: window.runtime.workersStartedForTests() };
          });
          assert.deepEqual(next, { format: "svg", starts: 2 });
        } finally {
          await context.close();
        }
      });
    }

    test("worker reuse: concurrent calls retain at most one idle worker", async () => {
      const { context, page } = await openPage();
      const observed = watchWorkers(page);
      try {
        const outcome = await page.evaluate(
          async ({ name, render }) => {
            const input = await window.fixture(name);
            const run = (seed) =>
              window.primeval
                .approximate({ input, output: "svg", render: { ...render, seed } })
                .then((result) => result.data);
            const sequential = [];
            for (const seed of [1, 2, 3]) sequential.push(await run(seed));
            const concurrent = await Promise.all([1, 2, 3].map(run));
            return {
              sequential,
              concurrent,
              starts: window.runtime.workersStartedForTests(),
              threads: navigator.hardwareConcurrency,
            };
          },
          { name: FIXTURE, render: SMALL },
        );
        assert.deepEqual(outcome.concurrent, outcome.sequential);
        assert.equal(outcome.starts, 3);
        const group = workerCount(outcome.threads);
        assert.equal(observed.workers.length, 3 * group);
        await observed.waitClosed(2 * group);
        await page.evaluate(() => window.runtime.setIdleMsForTests(0));
        await observed.waitClosed(3 * group);
      } finally {
        await context.close();
      }
    });

    test("worker reuse: idle expiry closes the worker and its pool", async () => {
      const { context, page } = await openPage();
      const observed = watchWorkers(page);
      await trackTermination(page);
      try {
        const outcome = await page.evaluate(async () => {
          window.runtime.setIdleMsForTests(200);
          await window.smallSvg();
          return { threads: navigator.hardwareConcurrency, terminated: window.terminated };
        });
        const group = workerCount(outcome.threads);
        assert.equal(observed.workers.length, group);
        assert.equal(outcome.terminated, 0, "success keeps the worker idle");
        await observed.waitClosed(group);
        assert.equal(await page.evaluate(() => window.terminated), 1);
        const starts = await page.evaluate(async () => {
          await window.smallSvg();
          return window.runtime.workersStartedForTests();
        });
        assert.equal(starts, 2);
        assert.equal(observed.workers.length, 2 * group);
        await observed.waitClosed(2 * group);
        assert.equal(await page.evaluate(() => window.terminated), 2);
      } finally {
        await context.close();
      }
    });

    test("worker reuse: concurrent failures terminate every worker at settle", async () => {
      const { context, page } = await openPage();
      const observed = watchWorkers(page);
      await trackTermination(page);
      try {
        const terminations = await page.evaluate(
          async ({ name, longRender, pool }) => {
            const input = await window.fixture(name);
            const { approximate } = window.primeval;
            const settle = (promise) => promise.catch(() => undefined);
            const controller = new AbortController();
            await Promise.all([
              settle(approximate({ input, output: "svg", render: { count: 0 } })),
              settle(
                approximate({
                  input,
                  output: "svg",
                  render: longRender,
                  execution: {
                    onProgress() {
                      throw new Error("boom");
                    },
                  },
                }),
              ),
              settle(
                approximate({
                  input,
                  output: "svg",
                  render: longRender,
                  execution: { signal: controller.signal, onProgress: () => controller.abort() },
                }),
              ),
              settle(window.runtime.panicForTests("caller")),
              // A pool panic leaves the calling thread blocked in rayon.
              ...(pool ? [settle(window.runtime.panicForTests("pool"))] : []),
            ]);
            return window.terminated;
          },
          { name: FIXTURE, longRender: LONG_RENDER, pool: isolated },
        );
        assert.equal(terminations, isolated ? 5 : 4);
        const failedWorkers = observed.workers.length;
        await observed.waitClosed(failedWorkers);
      } finally {
        await context.close();
      }
    });

    test("worker reuse: an idle worker bypasses a held fresh-worker start gate", async () => {
      const { context, page } = await openPage();
      let releaseScript;
      const held = new Promise((resolve) => {
        releaseScript = resolve;
      });
      try {
        await page.evaluate(async () => {
          window.run = window.smallSvg;
          await window.run();
        });
        await context.route(isWorkerScript, async (route) => {
          await held;
          await route.continue();
        });
        const requested = context.waitForEvent("request", {
          predicate: (request) => isWorkerScript(request.url()),
        });
        await page.evaluate(async () => {
          const warm = window.run();
          window.fresh = window.run();
          await warm;
        });
        await requested;
        await page.evaluate(() => {
          window.run().then(() => {
            window.reusedComplete = true;
          });
        });
        await page.waitForFunction(() => window.reusedComplete === true, undefined, {
          timeout: 3000,
        });
        releaseScript();
        assert.equal(await page.evaluate(async () => (await window.fresh).format), "svg");
        assert.equal(await page.evaluate(() => window.runtime.workersStartedForTests()), 2);
      } finally {
        releaseScript();
        await context.close();
      }
    });

    for (const failure of ["error", "messageerror", "idle panic"]) {
      test(`worker reuse: worker-lifetime ${failure} closes the channel and recovers`, async () => {
        const { context, page } = await openPage();
        const observed = watchWorkers(page);
        await trackTermination(page);
        try {
          await page.evaluate(async () => {
            const OriginalWorker = Worker;
            window.Worker = class extends OriginalWorker {
              constructor(...args) {
                super(...args);
                window.callingWorker = this;
              }
            };
            const OriginalChannel = BroadcastChannel;
            window.panicChannels = [];
            window.channelsClosed = 0;
            window.BroadcastChannel = class extends OriginalChannel {
              constructor(...args) {
                super(...args);
                window.panicChannels.push(this);
              }
              close() {
                window.channelsClosed += 1;
                super.close();
              }
            };
            window.run = window.smallSvg;
            await window.run();
            await window.run();
          });
          assert.equal(
            await page.evaluate(() => window.panicChannels.length),
            1,
            "reuse retains the same channel",
          );
          if (failure === "idle panic") {
            await page.evaluate(() => {
              const publisher = new BroadcastChannel(window.panicChannels[0].name);
              publisher.postMessage("render panicked: idle report");
              publisher.close();
            });
            await page.waitForFunction(() => window.terminated === 1);
            assert.equal(await page.evaluate(() => window.channelsClosed), 2); // publisher and runtime
          } else {
            const error = await page.evaluate(
              async ({ failure, longRender }) => {
                try {
                  await window.smallSvg({
                    render: longRender,
                    execution: {
                      onProgress() {
                        if (failure === "error") {
                          window.callingWorker.onerror({ preventDefault() {} });
                        } else {
                          window.callingWorker.onmessageerror();
                        }
                      },
                    },
                  });
                } catch (error) {
                  return window.describeError(error);
                }
              },
              { failure, longRender: LONG_RENDER },
            );
            assert.equal(error?.constructorName, "InternalError");
            assert.equal(error.code, "INTERNAL");
            assert.equal(await page.evaluate(() => window.terminated), 1);
            assert.equal(await page.evaluate(() => window.channelsClosed), 1);
          }
          await observed.waitClosed(observed.workers.length);
          assert.equal(await page.evaluate(async () => (await window.run()).format), "svg");
          assert.equal(await page.evaluate(() => window.runtime.workersStartedForTests()), 2);
        } finally {
          await context.close();
        }
      });
    }

    describe("load failures reject with InternalError, and a later call recovers", () => {
      for (const [what, pattern] of [
        ["the .wasm file", `**/wasm/${variant}/primeval_bg.wasm`],
        ["the wasm-bindgen glue", `**/wasm/${variant}/primeval.js`],
        ["the worker script", `**/dist/worker-${variant}.js`],
      ]) {
        test(`when ${what} is missing`, async () => {
          const { context, page } = await openPage();
          try {
            await context.route(pattern, (route) => route.fulfill({ status: 404, body: "" }));
            const failure = await page.evaluate(
              async ({ name, render }) => {
                try {
                  await window.primeval.approximate({
                    input: await window.fixture(name),
                    output: "svg",
                    render,
                  });
                  return null;
                } catch (error) {
                  return window.describeError(error);
                }
              },
              { name: FIXTURE, render: SMALL },
            );
            await context.unroute(pattern);
            const format = await page.evaluate(
              async ({ name, render }) =>
                (
                  await window.primeval.approximate({
                    input: await window.fixture(name),
                    output: "svg",
                    render,
                  })
                ).format,
              { name: FIXTURE, render: SMALL },
            );

            assert.equal(failure?.constructorName, "InternalError");
            assert.equal(failure.code, "INTERNAL");
            assert.equal(failure.isPrimevalError, true);
            assert.equal(format, "svg");
          } finally {
            await context.close();
          }
        });
      }
    });

    describe("a browser without WebAssembly SIMD", () => {
      const SIMD_MESSAGE =
        "this browser does not support WebAssembly SIMD, which primeval needs: " +
        "use Chrome 91, Firefox 89, Safari 16.4 or newer";

      // Fails the page's compile, as a browser without SIMD fails it, with
      // `WebAssembly.validate` answering `validates`; the stubs are undone
      // after the call, so a later call compiles for real.
      async function failedCompile(validates) {
        const { context, page } = await openPage();
        try {
          return await page.evaluate(
            async ({ name, render, validates }) => {
              const { compileStreaming, validate } = WebAssembly;
              const probes = [];
              WebAssembly.compileStreaming = async (source) => {
                await source;
                throw new WebAssembly.CompileError("stub: invalid opcode 0xfd");
              };
              WebAssembly.validate = (bytes) => {
                probes.push(Array.from(bytes));
                return validates;
              };
              const input = await window.fixture(name);
              let failure = null;
              try {
                await window.primeval.approximate({ input, output: "svg", render });
              } catch (error) {
                failure = { ...window.describeError(error), cause: String(error.cause) };
              } finally {
                WebAssembly.compileStreaming = compileStreaming;
                WebAssembly.validate = validate;
              }
              const next = await window.primeval.approximate({ input, output: "svg", render });
              return { failure, probes, nextFormat: next.format };
            },
            { name: FIXTURE, render: SMALL, validates },
          );
        } finally {
          await context.close();
        }
      }

      test("rejects with InternalError naming the browsers that have it", async () => {
        const outcome = await failedCompile(false);

        assert.equal(outcome.failure?.constructorName, "InternalError");
        assert.equal(outcome.failure.code, "INTERNAL");
        assert.equal(outcome.failure.isPrimevalError, true);
        assert.equal(outcome.failure.message, SIMD_MESSAGE);
        assert.equal(outcome.failure.cause, "CompileError: stub: invalid opcode 0xfd");
        // One probe, a module with a SIMD instruction.
        assert.equal(outcome.probes.length, 1);
        assert.ok(outcome.probes[0].includes(0xfd), "the probe has the SIMD opcode prefix");
        assert.equal(outcome.nextFormat, "svg");
      });

      test("is not blamed when the browser has SIMD", async () => {
        const outcome = await failedCompile(true);

        assert.equal(outcome.failure?.constructorName, "InternalError");
        assert.equal(
          outcome.failure.message,
          "could not load the WebAssembly module: stub: invalid opcode 0xfd",
        );
        assert.equal(outcome.nextFormat, "svg");
      });
    });

    const panicSites = isolated
      ? [
          ["on the calling thread", "caller", "on the calling thread"],
          ["in a rayon pool task", "pool", "in a rayon task"],
        ]
      : [["on the calling thread", "caller", "on the calling thread"]];
    for (const [label, site, where] of panicSites) {
      test(`a panic ${label} rejects with InternalError, and a later call works`, async () => {
        const outcome = await shared.page.evaluate(
          async ({ name, render, site, timeoutMs }) => {
            const started = performance.now();
            const timeout = new Promise((resolve) =>
              setTimeout(() => resolve("timeout"), timeoutMs),
            );
            const panic = window.runtime.panicForTests(site).then(
              () => "resolved",
              (error) => window.describeError(error),
            );
            const failure = await Promise.race([panic, timeout]);
            const elapsedMs = performance.now() - started;
            const next = await window.primeval.approximate({
              input: await window.fixture(name),
              output: "svg",
              render,
            });
            return { failure, elapsedMs, nextFormat: next.format };
          },
          { name: FIXTURE, render: SMALL, site, timeoutMs: PANIC_TIMEOUT_MS },
        );

        assert.equal(outcome.failure.constructorName, "InternalError");
        assert.equal(outcome.failure.code, "INTERNAL");
        // The wording of the napi binding's caught panic.
        assert.equal(
          outcome.failure.message,
          `internal render error: render panicked: panic for tests ${where}`,
        );
        assert.equal(outcome.nextFormat, "svg");
      });
    }

    // The engine and the PNG writer call no platform transcendental function
    // on any path they take, and the simd128 scoring kernels equal the scalar
    // and NEON ones bit for bit, so native and wasm output are expected to
    // agree. These tripwires catch a regression (see the WebAssembly plan, W0
    // results).
    for (const [output, renders] of [
      ["svg", TRIPWIRE_RENDERS],
      ["png", PNG_TRIPWIRE_RENDERS],
    ]) {
      test(`${output.toUpperCase()} output equals the native addon's for fixed seeds (${variant} build)`, async () => {
        // PNG bytes cross the page boundary as plain arrays.
        const browserOutputs = await shared.page.evaluate(
          async ({ name, output, renders }) => {
            const input = await window.fixture(name);
            const outputs = [];
            for (const render of renders) {
              const { data } = await window.primeval.approximate({ input, output, render });
              outputs.push(output === "png" ? Array.from(data) : data);
            }
            return outputs;
          },
          { name: FIXTURE, output, renders },
        );

        for (const [index, render] of renders.entries()) {
          const { data } = await nativeOutput(fixture, output, render);
          assert.deepEqual(
            browserOutputs[index],
            output === "png" ? Array.from(data) : data,
            JSON.stringify(render),
          );
        }
      });
    }
  });
}
