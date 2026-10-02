import assert from "node:assert/strict";
import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import { test } from "node:test";
import { pathToFileURL } from "node:url";
import {
  AbortError,
  approximate,
  InternalError,
  PrimevalError,
  toDataUri,
  ValidationError,
} from "@aleburato/primeval";

import { mapNativeError } from "../dist/errors.js";

test("package root import resolves", async () => {
  const mod = await import("@aleburato/primeval");
  assert.equal(typeof mod.approximate, "function");
  assert.equal(typeof mod.toDataUri, "function");
  assert.equal(typeof mod.ValidationError, "function");
  assert.equal(mod.NotFoundError, undefined);
  assert.equal(typeof mod.AbortError, "function");
  assert.equal(typeof mod.InternalError, "function");
  assert.equal(typeof mod.PrimevalError, "function");
});

function nativeError(code, message) {
  return Object.assign(new Error(message), { code });
}

test("native errors map on code to PrimevalError subclasses with the native cause", () => {
  const cases = [
    ["INVALID_OPTION", ValidationError, "ValidationError"],
    ["INVALID_IMAGE", ValidationError, "ValidationError"],
    ["ABORTED", AbortError, "AbortError"],
    ["INTERNAL", InternalError, "InternalError"],
  ];
  for (const [code, ErrorClass, name] of cases) {
    const cause = nativeError(code, "first line\nsecond line");

    const mapped = mapNativeError(cause);

    assert.ok(mapped instanceof ErrorClass, code);
    assert.ok(mapped instanceof PrimevalError, code);
    assert.equal(mapped.name, name);
    assert.equal(mapped.code, code);
    assert.equal(mapped.message, "first line\nsecond line");
    assert.equal(mapped.cause, cause);
  }
});

test("native errors map on code, never on message text", () => {
  const unknown = nativeError("InvalidArg", "[ValidationError] looks like the old prefix");
  assert.equal(mapNativeError(unknown), unknown);

  const plain = new Error("INVALID_OPTION");
  assert.equal(mapNativeError(plain), plain);
});

test("wrapper validation errors carry the INVALID_OPTION code", () => {
  assert.throws(
    () => toDataUri(/** @type {any} */ (null)),
    (err) => err instanceof ValidationError && err.code === "INVALID_OPTION",
  );
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

test("approximate rejects non-function progress callbacks with ValidationError", async () => {
  await assert.rejects(
    approximate(
      /** @type {any} */ ({
        input: Buffer.from([0]),
        output: "svg",
        render: { count: 2, resizeInput: 8, outputSize: 16, seed: 7 },
        execution: { onProgress: 123 },
      }),
    ),
    (err) =>
      err instanceof ValidationError &&
      err.option === undefined &&
      err.message === "execution.onProgress must be a function",
  );
});

test("approximate rejects invalid abort signals with ValidationError", async () => {
  await assert.rejects(
    approximate(
      /** @type {any} */ ({
        input: Buffer.from([0]),
        output: "svg",
        render: { count: 1, resizeInput: 8, outputSize: 16, seed: 7 },
        execution: { signal: {} },
      }),
    ),
    (err) =>
      err instanceof ValidationError && err.message === "execution.signal must be an AbortSignal",
  );
});

test("approximate rejects a missing request with ValidationError", async () => {
  for (const request of [undefined, null, "photo.jpg"]) {
    await assert.rejects(
      approximate(/** @type {any} */ (request)),
      (err) => err instanceof ValidationError && err.message === "request must be an object",
    );
  }
});

test("approximate rejects alpha outside auto and 1..255 with ValidationError", async () => {
  for (const alpha of [0, 256, -1, 1.5, 2 ** 32 + 128, Number.NaN, true]) {
    await assert.rejects(
      approximate(
        /** @type {any} */ ({
          input: Buffer.from([0]),
          output: "svg",
          render: { alpha },
        }),
      ),
      (err) =>
        err instanceof ValidationError &&
        err.option === "alpha" &&
        err.message === "alpha must be auto or an integer 1..255",
      String(alpha),
    );
  }
});

test("native invalid-option errors carry the option and requirement", () => {
  const cause = Object.assign(nativeError("INVALID_OPTION", "resizeInput must be 2"), {
    option: "resizeInput",
    requirement: "must be 2",
  });

  const mapped = mapNativeError(cause);

  assert.ok(mapped instanceof ValidationError);
  assert.equal(mapped.option, "resizeInput");
  assert.equal(mapped.requirement, "must be 2");
});

test("a native load failure rejects with InternalError", async (t) => {
  const tmp = fs.mkdtempSync(path.join(os.tmpdir(), "primeval-load-failure-"));
  t.after(() => fs.rmSync(tmp, { recursive: true, force: true }));
  fs.cpSync(path.join(process.cwd(), "dist"), path.join(tmp, "dist"), { recursive: true });
  fs.writeFileSync(path.join(tmp, "package.json"), '{ "type": "commonjs" }\n');
  fs.writeFileSync(path.join(tmp, "dist", "package.json"), '{ "type": "module" }\n');
  fs.writeFileSync(path.join(tmp, "binding.js"), 'throw new Error("no native addon");\n');
  const isolated = await import(pathToFileURL(path.join(tmp, "dist", "index.js")).href);

  let promise;
  assert.doesNotThrow(() => {
    promise = isolated.approximate({ input: Buffer.from([0]), output: "svg" });
  });
  await assert.rejects(
    promise,
    (err) =>
      err instanceof isolated.InternalError &&
      err.code === "INTERNAL" &&
      err.message === "no native addon" &&
      err.cause instanceof Error,
  );
});
