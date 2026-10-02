import assert from "node:assert/strict";
import fs from "node:fs";
import path from "node:path";
import { test } from "node:test";
import zlib from "node:zlib";
import { AbortError, approximate, PrimevalError, ValidationError } from "@aleburato/primeval";

const LONG_RENDER = { count: 100000, resizeInput: 16, outputSize: 16, seed: 7 };

const FIXTURE_IMAGE = fs.readFileSync(
  path.join(process.cwd(), "docs", "readme", "originals", "monalisa.jpg"),
);

function render(overrides = {}) {
  return {
    count: 4,
    shape: "any",
    alpha: 128,
    seed: 7,
    background: "auto",
    resizeInput: 8,
    outputSize: 16,
    ...overrides,
  };
}

test("native approximate renders bytes to svg", async () => {
  const result = await approximate({
    input: FIXTURE_IMAGE,
    output: "svg",
    render: render(),
  });

  assert.equal(result.format, "svg");
  assert.match(result.data, /^<svg\b/);
  assert.equal(result.mimeType, "image/svg+xml");
  assert.ok(result.width > 0);
  assert.ok(result.height > 0);
});

test("native approximate renders bytes to png", async () => {
  const result = await approximate({
    input: FIXTURE_IMAGE,
    output: "png",
    render: render(),
  });

  assert.equal(result.format, "png");
  assert.equal(result.mimeType, "image/png");
  assert.equal(result.data[0], 0x89);
  assert.equal(result.data[1], 0x50);
  assert.ok(result.width > 0);
  assert.ok(result.height > 0);
});

test("native approximate accepts omitted seed", async () => {
  const result = await approximate({
    input: FIXTURE_IMAGE,
    output: "svg",
    render: {
      count: 4,
      shape: "any",
      alpha: 128,
      background: "auto",
      resizeInput: 8,
      outputSize: 16,
    },
  });

  assert.equal(result.format, "svg");
  assert.match(result.data, /^<svg\b/);
});

test("native approximate accepts alpha auto", async () => {
  const result = await approximate({
    input: FIXTURE_IMAGE,
    output: "svg",
    render: render({ alpha: "auto" }),
  });

  assert.equal(result.format, "svg");
  assert.match(result.data, /^<svg\b/);
});

test("native approximate rejects an unknown alpha string with ValidationError", async () => {
  await assert.rejects(
    approximate({
      input: FIXTURE_IMAGE,
      output: "svg",
      render: render({ alpha: /** @type {any} */ ("half") }),
    }),
    (error) =>
      error instanceof ValidationError &&
      error.message === "alpha must be auto or an integer 1..255",
  );
});

test("approximate rejects non-Uint8Array input with ValidationError", async () => {
  const inputs = [
    undefined,
    null,
    "photo.jpg",
    [0, 1, 2, 3],
    FIXTURE_IMAGE.buffer,
    { kind: "path", path: "photo.jpg" },
    { kind: "bytes", data: FIXTURE_IMAGE },
  ];

  for (const input of inputs) {
    await assert.rejects(
      approximate(/** @type {any} */ ({ input, output: "svg", render: render() })),
      (error) => error instanceof ValidationError && error.message === "input must be a Uint8Array",
    );
  }
});

test("native approximate accepts a plain Uint8Array input", async () => {
  const result = await approximate({
    input: new Uint8Array(FIXTURE_IMAGE),
    output: "svg",
    render: render(),
  });

  assert.equal(result.format, "svg");
  assert.match(result.data, /^<svg\b/);
});

test("approximate rejects non-opaque and non-ASCII backgrounds with ValidationError", async () => {
  for (const background of ["a€bc", "#a€bc", "€", "#1234", "#11223344", "11223344"]) {
    await assert.rejects(
      approximate({ input: FIXTURE_IMAGE, output: "svg", render: render({ background }) }),
      (error) =>
        error instanceof ValidationError &&
        error.message === "background must be auto or an opaque hex color (RGB or RRGGBB)",
      background,
    );
  }
});

test("native approximate accepts RGB and RRGGBB backgrounds", async () => {
  for (const background of ["#abc", "abc", "#336699", "336699"]) {
    const result = await approximate({
      input: FIXTURE_IMAGE,
      output: "svg",
      render: render({ background }),
    });
    assert.equal(result.format, "svg");
  }
});

test("native approximate maps invalid bytes to ValidationError", async () => {
  await assert.rejects(
    approximate({
      input: Buffer.from([0, 1, 2, 3]),
      output: "svg",
      render: render(),
    }),
    (error) =>
      error instanceof ValidationError &&
      error.code === "INVALID_IMAGE" &&
      error.message.startsWith("invalid image data: ") &&
      error.cause instanceof Error &&
      error.cause.code === "INVALID_IMAGE",
  );
});

// A minimal 8-bit RGB PNG, so tests can build odd sizes without fixtures.
function solidPng(width, height) {
  const crcTable = Array.from({ length: 256 }, (_, n) => {
    let c = n;
    for (let k = 0; k < 8; k++) c = c & 1 ? 0xedb88320 ^ (c >>> 1) : c >>> 1;
    return c >>> 0;
  });
  const crc = (bytes) => {
    let c = 0xffffffff;
    for (const byte of bytes) c = crcTable[(c ^ byte) & 0xff] ^ (c >>> 8);
    return (c ^ 0xffffffff) >>> 0;
  };
  const chunk = (type, data) => {
    const body = Buffer.concat([Buffer.from(type, "ascii"), data]);
    const out = Buffer.alloc(body.length + 8);
    out.writeUInt32BE(data.length, 0);
    body.copy(out, 4);
    out.writeUInt32BE(crc(body), body.length + 4);
    return out;
  };
  const header = Buffer.alloc(13);
  header.writeUInt32BE(width, 0);
  header.writeUInt32BE(height, 4);
  header[8] = 8;
  header[9] = 2;
  const row = Buffer.concat([Buffer.from([0]), Buffer.alloc(width * 3, 0x80)]);
  const raw = Buffer.concat(Array.from({ length: height }, () => row));
  return Buffer.concat([
    Buffer.from([0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a]),
    chunk("IHDR", header),
    chunk("IDAT", zlib.deflateSync(raw)),
    chunk("IEND", Buffer.alloc(0)),
  ]);
}

test("native approximate rejects a 1000x1 image as INVALID_IMAGE", async () => {
  await assert.rejects(
    approximate({ input: solidPng(1000, 1), output: "svg", render: render() }),
    (error) =>
      error instanceof ValidationError &&
      error.code === "INVALID_IMAGE" &&
      error.message ===
        "invalid image data: the image is 1000x1 pixels; both sides must be at least 2",
  );
});

test("native approximate renders a 2000x5 banner on a 256x2 canvas", async () => {
  const result = await approximate({
    input: solidPng(2000, 5),
    output: "svg",
    render: render({ count: 2, resizeInput: 256 }),
  });

  assert.match(result.data, /viewBox="0 0 256 2"/);
});

test("numeric options are range-checked in Rust without wrapping", async () => {
  const cases = [
    ["count", "must be an integer from 1 to 100000"],
    ["resizeInput", "must be an integer from 2 to 2048"],
    ["outputSize", "must be an integer from 2 to 8192"],
  ];
  for (const [name, requirement] of cases) {
    for (const value of [2 ** 32 + 1, 2 ** 32 + 16, 1e20, -1, 0, 1.5, Number.NaN, Infinity]) {
      await assert.rejects(
        approximate({ input: FIXTURE_IMAGE, output: "svg", render: render({ [name]: value }) }),
        (error) =>
          error instanceof ValidationError &&
          error.code === "INVALID_OPTION" &&
          error.option === name &&
          error.requirement === requirement &&
          error.message === `${name} ${requirement}`,
        `${name} = ${value}`,
      );
    }
  }
});

test("seeds accept safe integers and bigints up to 2^64 - 1", async () => {
  const seedMessage = /^seed must be an integer from 0 to 2\^64 - 1/;
  for (const seed of [2 ** 53, 1e20, -1, 1.5, -1n, 2n ** 64n]) {
    await assert.rejects(
      approximate({ input: FIXTURE_IMAGE, output: "svg", render: render({ seed }) }),
      (error) =>
        error instanceof ValidationError &&
        error.code === "INVALID_OPTION" &&
        error.option === "seed" &&
        seedMessage.test(error.message),
      String(seed),
    );
  }
  await assert.rejects(
    approximate({ input: FIXTURE_IMAGE, output: "svg", render: render({ seed: "7" }) }),
    (error) =>
      error instanceof ValidationError &&
      error.option === "seed" &&
      error.message === "seed must be a number or a bigint",
  );

  const asNumber = await approximate({ input: FIXTURE_IMAGE, output: "svg", render: render() });
  const asBigint = await approximate({
    input: FIXTURE_IMAGE,
    output: "svg",
    render: render({ seed: 7n }),
  });
  assert.equal(asBigint.data, asNumber.data);

  const maxSeed = await approximate({
    input: FIXTURE_IMAGE,
    output: "svg",
    render: render({ seed: 2n ** 64n - 1n }),
  });
  assert.equal(maxSeed.format, "svg");
});

test("every failure is a rejection, never a synchronous throw", async () => {
  const cases = [
    [{ input: FIXTURE_IMAGE, output: "svg", render: { count: 0 } }, "count"],
    [{ input: FIXTURE_IMAGE, output: "svg", render: { shape: "hexagon" } }, "shape"],
    [{ input: FIXTURE_IMAGE, output: "svg", render: { background: "#1234" } }, "background"],
    [{ input: FIXTURE_IMAGE, output: "gif" }, "output"],
  ];
  for (const [request, option] of cases) {
    let promise;
    assert.doesNotThrow(() => {
      promise = approximate(/** @type {any} */ (request));
    }, option);
    await assert.rejects(
      promise,
      (error) =>
        error instanceof ValidationError &&
        error.code === "INVALID_OPTION" &&
        error.option === option,
      option,
    );
  }
});

test("request fields of the wrong type reject with ValidationError, not napi errors", async () => {
  const cases = [
    [{ output: 1 }, "output", "output must be a string"],
    [{ render: { background: 123 } }, "background", "background must be a string"],
    [{ render: { shape: 1 } }, "shape", "shape must be a string"],
    [{ render: { count: "4" } }, "count", "count must be a number"],
    [{ render: { resizeInput: 8n } }, "resizeInput", "resizeInput must be a number"],
    [{ render: { outputSize: {} } }, "outputSize", "outputSize must be a number"],
  ];
  for (const [overrides, option, message] of cases) {
    const request = { input: FIXTURE_IMAGE, output: "svg", ...overrides };
    await assert.rejects(
      approximate(/** @type {any} */ (request)),
      (error) =>
        error instanceof ValidationError &&
        error.code === "INVALID_OPTION" &&
        error.option === option &&
        error.message === message,
      message,
    );
  }
});

test("null is rejected like any other wrong type for every optional field", async () => {
  const fields = ["count", "shape", "alpha", "seed", "background", "resizeInput", "outputSize"];
  for (const field of fields) {
    await assert.rejects(
      approximate(
        /** @type {any} */ ({ input: FIXTURE_IMAGE, output: "svg", render: { [field]: null } }),
      ),
      (error) =>
        error instanceof ValidationError &&
        error.code === "INVALID_OPTION" &&
        error.option === field,
      field,
    );
  }
  for (const request of [
    { render: null },
    { execution: null },
    { execution: { onProgress: null } },
    { execution: { signal: null } },
  ]) {
    await assert.rejects(
      approximate(/** @type {any} */ ({ input: FIXTURE_IMAGE, output: "svg", ...request })),
      (error) => error instanceof ValidationError && error.code === "INVALID_OPTION",
      JSON.stringify(request),
    );
  }
});

test("approximate rejects removed output formats with ValidationError", async () => {
  for (const output of ["jpg", "jpeg", "gif"]) {
    await assert.rejects(
      approximate({
        input: FIXTURE_IMAGE,
        output,
        render: render(),
      }),
      (error) =>
        error instanceof ValidationError &&
        error.code === "INVALID_OPTION" &&
        error.message === "output must be one of: svg, png",
    );
  }
});

function isAbortFrom(signal) {
  return (error) =>
    error instanceof AbortError &&
    error instanceof PrimevalError &&
    error.code === "ABORTED" &&
    error.cause === signal.reason;
}

test("an already-aborted signal rejects without starting native work", async () => {
  const controller = new AbortController();
  controller.abort();
  let progressCalls = 0;

  // count: 0 fails Rust validation, so reaching native code would reject
  // with ValidationError instead.
  await assert.rejects(
    approximate({
      input: FIXTURE_IMAGE,
      output: "svg",
      render: render({ count: 0 }),
      execution: {
        signal: controller.signal,
        onProgress() {
          progressCalls += 1;
        },
      },
    }),
    isAbortFrom(controller.signal),
  );
  assert.equal(progressCalls, 0);
});

test("aborting after the first progress event rejects a long render with AbortError", async () => {
  const controller = new AbortController();
  let lastStep = 0;

  await assert.rejects(
    approximate({
      input: FIXTURE_IMAGE,
      output: "svg",
      render: render(LONG_RENDER),
      execution: {
        signal: controller.signal,
        onProgress(info) {
          lastStep = info.step;
          if (info.step === 1) {
            controller.abort(new Error("stop"));
          }
        },
      },
    }),
    isAbortFrom(controller.signal),
  );
  assert.ok(lastStep < LONG_RENDER.count, `stopped at step ${lastStep}`);
});

test("an abort before the native promise settles wins even if the render finished", async () => {
  const controller = new AbortController();
  const promise = approximate({
    input: FIXTURE_IMAGE,
    output: "svg",
    render: render({ count: 2 }),
    execution: { signal: controller.signal },
  });

  // The render finishes on the blocking pool while the main thread is busy,
  // so its result is already queued when the signal fires.
  const blockUntil = Date.now() + 200;
  while (Date.now() < blockUntil) {
    // Busy-wait.
  }
  controller.abort();

  await assert.rejects(promise, isAbortFrom(controller.signal));
});

test("a throwing onProgress rejects with its error and stops the render", async () => {
  const boom = new Error("boom");
  let calls = 0;

  await assert.rejects(
    approximate({
      input: FIXTURE_IMAGE,
      output: "svg",
      render: render(LONG_RENDER),
      execution: {
        onProgress() {
          calls += 1;
          throw boom;
        },
      },
    }),
    (error) => error === boom,
  );
  await new Promise((resolve) => setTimeout(resolve, 50));
  assert.equal(calls, 1);
});

test("native approximate emits monotonic progress exactly count times", async () => {
  const progress = [];

  const result = await approximate({
    input: FIXTURE_IMAGE,
    output: "svg",
    render: render({ count: 6 }),
    execution: {
      onProgress(info) {
        progress.push(info);
      },
    },
  });

  assert.equal(result.format, "svg");
  assert.equal(progress.length, 6);
  assert.deepEqual(
    progress.map((info) => info.step),
    [1, 2, 3, 4, 5, 6],
  );
  assert.ok(progress.every((info) => info.total === 6));
  assert.ok(progress.every((info, index) => index === 0 || info.step > progress[index - 1].step));
});

test("aborting after the render settled is a no-op", async () => {
  const controller = new AbortController();

  const result = await approximate({
    input: FIXTURE_IMAGE,
    output: "svg",
    render: render({ count: 2 }),
    execution: { signal: controller.signal },
  });
  controller.abort();

  assert.equal(result.format, "svg");
});
