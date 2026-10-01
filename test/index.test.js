import assert from "node:assert/strict";
import { test } from "node:test";
import { approximate, toDataUri, ValidationError } from "@aleburato/primeval";

test("package root import resolves", async () => {
  const mod = await import("@aleburato/primeval");
  assert.equal(typeof mod.approximate, "function");
  assert.equal(typeof mod.toDataUri, "function");
  assert.equal(typeof mod.ValidationError, "function");
  assert.equal(typeof mod.NotFoundError, "function");
  assert.equal(typeof mod.AbortError, "function");
});

test("toDataUri encodes svg results", () => {
  const uri = toDataUri({
    format: "svg",
    data: '<svg xmlns="http://www.w3.org/2000/svg"></svg>',
    mimeType: "image/svg+xml",
    width: 1,
    height: 1,
  });

  assert.match(uri, /^data:image\/svg\+xml;base64,/);
  assert.equal(
    Buffer.from(uri.split(",")[1], "base64").toString("utf8"),
    '<svg xmlns="http://www.w3.org/2000/svg"></svg>',
  );
});

test("toDataUri encodes raster results", () => {
  const uri = toDataUri({
    format: "png",
    data: Buffer.from([0x89, 0x50, 0x4e, 0x47]),
    mimeType: "image/png",
    width: 1,
    height: 1,
  });

  assert.match(uri, /^data:image\/png;base64,/);
  assert.equal(Buffer.from(uri.split(",")[1], "base64")[0], 0x89);
});

test("validation rejects missing input before native loading", () => {
  assert.throws(
    () => {
      toDataUri(/** @type {any} */ (null));
    },
    (err) => err instanceof ValidationError,
  );
});

test("approximate rejects non-function progress callbacks with ValidationError", () => {
  assert.throws(
    () =>
      approximate(
        /** @type {any} */ ({
          input: { kind: "bytes", data: Buffer.from([0]) },
          output: "svg",
          render: { count: 2, resizeInput: 8, outputSize: 16, seed: 7 },
          execution: { onProgress: 123 },
        }),
      ),
    (err) =>
      err instanceof ValidationError && err.message === "execution.onProgress must be a function",
  );
});

test("approximate rejects invalid abort signals with ValidationError", () => {
  assert.throws(
    () =>
      approximate(
        /** @type {any} */ ({
          input: { kind: "bytes", data: Buffer.from([0]) },
          output: "svg",
          render: { count: 1, resizeInput: 8, outputSize: 16, seed: 7 },
          execution: { signal: {} },
        }),
      ),
    (err) =>
      err instanceof ValidationError && err.message === "execution.signal must be an AbortSignal",
  );
});
