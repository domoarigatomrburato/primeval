import assert from "node:assert/strict";
import fs from "node:fs";
import path from "node:path";
import { test } from "node:test";

import { AbortError, approximate, ValidationError } from "@aleburato/primeval";

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
    async () =>
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
      async () => approximate(/** @type {any} */ ({ input, output: "svg", render: render() })),
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
      async () =>
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
    (error) => error instanceof ValidationError,
  );
});

test("approximate rejects removed output formats with ValidationError", async () => {
  for (const output of ["jpg", "jpeg", "gif"]) {
    await assert.rejects(
      async () =>
        approximate({
          input: FIXTURE_IMAGE,
          output,
          render: render(),
        }),
      (error) =>
        error instanceof ValidationError && error.message === `unknown output format: ${output}`,
    );
  }
});

test("native approximate maps abort signals to AbortError", async () => {
  const controller = new AbortController();

  await assert.rejects(
    approximate({
      input: FIXTURE_IMAGE,
      output: "svg",
      render: render({ count: 32 }),
      execution: {
        signal: controller.signal,
        onProgress(info) {
          if (info.step === 1) {
            controller.abort();
          }
        },
      },
    }),
    (error) => error instanceof AbortError,
  );
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
