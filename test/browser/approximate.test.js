// The browser entry (`dist/browser.js`) in headless Chromium, on a page with
// cross-origin isolation (threaded build) and one without (single-threaded
// build). Needs `npm run build`, `npm run build:wasm` and, for the
// native-equality tripwire and the error parity checks, `npm run build:node`.
import assert from "node:assert/strict";
import { after, before, describe, test } from "node:test";
import { chromium } from "playwright";

import * as native from "../../dist/index.js";
import { startServer } from "../../scripts/static-server.mjs";
import { assertPreviewsSvg } from "../helpers/svg.js";

const FIXTURE = "monalisa.jpg";
const SMALL = { count: 4, resizeInput: 16, outputSize: 32, seed: 7 };
const LONG_RENDER = { count: 100000, resizeInput: 16, outputSize: 16, seed: 7 };
// A render that never finishes on its own can only end through a timeout.
const PANIC_TIMEOUT_MS = 10000;

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

  describe(`browser approximate, ${isolated ? "cross-origin isolated" : "not isolated"} (${variant} build)`, () => {
    let server;
    let shared;
    let fixture;

    // A fresh context per page, so its requests (workers included) and its
    // module cache are its own.
    async function openPage() {
      const context = await browser.newContext();
      const requests = [];
      context.on("request", (request) => requests.push(new URL(request.url()).pathname));
      const page = await context.newPage();
      await page.goto(`${server.origin}/test/browser/page.html`);
      await page.waitForFunction(() => window.ready === true, undefined, { timeout: 10000 });
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
            { length: isolated ? 1 + threads : 1 },
            () => `${server.origin}/dist/worker-${variant}.js`,
          ),
        );
      } finally {
        await context.close();
      }
    });

    test("terminates each call's worker when the call settles, whatever the outcome", async () => {
      const { context, page } = await openPage();
      try {
        const closed = [];
        page.on("worker", (worker) => {
          closed.push(new Promise((resolve) => worker.once("close", resolve)));
        });
        await page.evaluate(
          async ({ name, render, longRender, pool }) => {
            const input = await window.fixture(name);
            const { approximate } = window.primeval;
            const settle = (promise) =>
              promise.then(
                () => undefined,
                () => undefined,
              );
            const controller = new AbortController();
            await Promise.all([
              settle(approximate({ input, output: "svg", render })),
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
                  execution: {
                    signal: controller.signal,
                    onProgress: () => controller.abort(),
                  },
                }),
              ),
              settle(window.runtime.panicForTests("caller")),
              // A pool panic leaves the calling thread blocked in rayon.
              ...(pool ? [settle(window.runtime.panicForTests("pool"))] : []),
            ]);
          },
          { name: FIXTURE, render: SMALL, longRender: LONG_RENDER, pool: isolated },
        );

        let timer;
        const timeout = new Promise((resolve) => {
          // Generous: on isolated pages Chromium reports the pool workers'
          // close events about 2 s after termination, 4 s after a pool panic.
          timer = setTimeout(() => resolve("timeout"), 15000);
        });
        const outcome = await Promise.race([Promise.all(closed).then(() => "closed"), timeout]);
        clearTimeout(timer);

        assert.ok(closed.length >= (isolated ? 6 : 5), `saw ${closed.length} workers`);
        assert.equal(outcome, "closed");
      } finally {
        await context.close();
      }
    });

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

    // A tripwire, not a guarantee: native and wasm output were observed to be
    // identical for fixed seeds, but libm differences between platforms are
    // only absorbed by rounding today (see the WebAssembly plan, W0 results).
    test(`SVG output equals the native addon's for fixed seeds (${variant} build)`, async () => {
      const renders = [];
      for (const seed of [1, 2, 3]) {
        for (const shape of ["triangle", "rotated-ellipse", "quadratic", "any"]) {
          renders.push({ count: 12, resizeInput: 64, outputSize: 128, seed, shape });
        }
      }

      const browserSvgs = await shared.page.evaluate(
        async ({ name, renders }) => {
          const input = await window.fixture(name);
          const svgs = [];
          for (const render of renders) {
            svgs.push((await window.primeval.approximate({ input, output: "svg", render })).data);
          }
          return svgs;
        },
        { name: FIXTURE, renders },
      );

      for (const [index, render] of renders.entries()) {
        const expected = await native.approximate({ input: fixture, output: "svg", render });
        assert.equal(browserSvgs[index], expected.data, JSON.stringify(render));
      }
    });
  });
}
