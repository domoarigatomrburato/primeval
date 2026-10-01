import assert from "node:assert/strict";
import fs from "node:fs";
import path from "node:path";
import { test } from "node:test";

import { AbortError, approximate, NotFoundError, ValidationError } from "@aleburato/primeval";

const FIXTURE_IMAGE = fs.readFileSync(
  path.join(process.cwd(), "docs", "readme", "originals", "monalisa.jpg"),
);

function render(overrides = {}) {
  return {
    count: 4,
    shape: "any",
    alpha: 128,
    repeat: 0,
    seed: 7,
    background: "auto",
    resizeInput: 8,
    outputSize: 16,
    ...overrides,
  };
}

test("native approximate renders bytes to svg", async () => {
  const result = await approximate({
    input: { kind: "bytes", data: FIXTURE_IMAGE },
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
    input: { kind: "bytes", data: FIXTURE_IMAGE },
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
    input: { kind: "bytes", data: FIXTURE_IMAGE },
    output: "svg",
    render: {
      count: 4,
      shape: "any",
      alpha: 128,
      repeat: 0,
      background: "auto",
      resizeInput: 8,
      outputSize: 16,
    },
  });

  assert.equal(result.format, "svg");
  assert.match(result.data, /^<svg\b/);
});

test("native approximate treats alpha 0 as auto", async () => {
  const result = await approximate({
    input: { kind: "bytes", data: FIXTURE_IMAGE },
    output: "svg",
    render: render({ alpha: 0 }),
  });

  assert.equal(result.format, "svg");
  assert.match(result.data, /^<svg\b/);
});

test("native approximate maps missing files to NotFoundError", async () => {
  await assert.rejects(
    approximate({
      input: {
        kind: "path",
        path: path.join(process.cwd(), "does-not-exist.png"),
      },
      output: "svg",
      render: render(),
    }),
    (error) => error instanceof NotFoundError,
  );
});

test("native approximate maps invalid bytes to ValidationError", async () => {
  await assert.rejects(
    approximate({
      input: { kind: "bytes", data: Buffer.from([0, 1, 2, 3]) },
      output: "svg",
      render: render(),
    }),
    (error) => error instanceof ValidationError,
  );
});

test("native approximate maps abort signals to AbortError", async () => {
  const controller = new AbortController();

  await assert.rejects(
    approximate({
      input: { kind: "bytes", data: FIXTURE_IMAGE },
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
    input: { kind: "bytes", data: FIXTURE_IMAGE },
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
