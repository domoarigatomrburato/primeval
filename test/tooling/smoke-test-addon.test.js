import assert from "node:assert/strict";
import { spawnSync } from "node:child_process";
import fs from "node:fs";
import path from "node:path";
import { test } from "node:test";
import zlib from "node:zlib";

import {
  createFixturePng,
  smokeTargetForRuntime,
  validateSmokeResult,
} from "../../scripts/smoke-test-addon.mjs";

const repoRoot = process.cwd();
const TARGETS = JSON.parse(fs.readFileSync(path.join(repoRoot, "package.json"), "utf8")).napi
  .targets;

function pngChunks(png) {
  const chunks = [];
  let offset = 8;
  while (offset < png.length) {
    const length = png.readUInt32BE(offset);
    const type = png.subarray(offset + 4, offset + 8).toString("latin1");
    const data = png.subarray(offset + 8, offset + 8 + length);
    const crc = png.readUInt32BE(offset + 8 + length);
    chunks.push({ type, data, crc, crcInput: png.subarray(offset + 4, offset + 8 + length) });
    offset += 12 + length;
  }
  return chunks;
}

test("fixture PNG is a valid RGB image of the requested size", () => {
  const png = createFixturePng(12, 8);
  assert.deepEqual([...png.subarray(0, 8)], [0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a]);

  const chunks = pngChunks(png);
  assert.deepEqual(
    chunks.map(({ type }) => type),
    ["IHDR", "IDAT", "IEND"],
  );
  for (const chunk of chunks) {
    assert.equal(chunk.crc, zlib.crc32(chunk.crcInput), `${chunk.type} CRC`);
  }

  const header = chunks[0].data;
  assert.equal(header.readUInt32BE(0), 12);
  assert.equal(header.readUInt32BE(4), 8);
  assert.equal(header[8], 8, "bit depth");
  assert.equal(header[9], 2, "RGB color type");

  const pixels = zlib.inflateSync(chunks[1].data);
  assert.equal(pixels.length, 8 * (1 + 12 * 3));
  assert.ok(new Set(pixels).size > 2, "fixture must not be a flat color");
});

test("smoke results must be non-empty and of the requested format", () => {
  const svg = { format: "svg", data: "<svg></svg>", width: 4, height: 4 };
  const png = {
    format: "png",
    data: Buffer.from([0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a, 0]),
    width: 4,
    height: 4,
  };
  assert.doesNotThrow(() => validateSmokeResult("svg", svg));
  assert.doesNotThrow(() => validateSmokeResult("png", png));

  assert.throws(() => validateSmokeResult("svg", { ...svg, data: "" }), /empty/);
  assert.throws(() => validateSmokeResult("svg", { ...svg, data: "<html>" }), /SVG/);
  assert.throws(() => validateSmokeResult("png", { ...png, data: Buffer.alloc(0) }), /empty/);
  assert.throws(() => validateSmokeResult("png", { ...png, data: Buffer.from("PNG!") }), /PNG/);
  assert.throws(() => validateSmokeResult("png", svg), /format/);
  assert.throws(() => validateSmokeResult("svg", { ...svg, width: 0 }), /dimensions/);
});

test("smoke target must match the running platform", () => {
  assert.equal(
    smokeTargetForRuntime(TARGETS, { platform: "linux", arch: "arm64" }),
    "aarch64-unknown-linux-gnu",
  );
  assert.equal(
    smokeTargetForRuntime(TARGETS, { platform: "darwin", arch: "x64" }),
    "x86_64-apple-darwin",
  );
  assert.equal(
    smokeTargetForRuntime(TARGETS, { platform: "win32", arch: "x64" }, "x86_64-pc-windows-msvc"),
    "x86_64-pc-windows-msvc",
  );
  // An arm64 Node process cannot load the x64 addon, so it must not claim to have tested it.
  assert.throws(
    () =>
      smokeTargetForRuntime(TARGETS, { platform: "darwin", arch: "arm64" }, "x86_64-apple-darwin"),
    /x86_64-apple-darwin.*darwin-arm64/,
  );
  assert.throws(
    () => smokeTargetForRuntime(TARGETS, { platform: "freebsd", arch: "x64" }),
    /no napi target/,
  );
});

test("smoke test renders through the package loader on this machine", () => {
  const result = spawnSync(process.execPath, ["scripts/smoke-test-addon.mjs"], {
    cwd: repoRoot,
    encoding: "utf8",
  });
  assert.equal(result.status, 0, `${result.stdout}\n${result.stderr}`);
  assert.match(result.stdout, /svg: \d+ bytes/);
  assert.match(result.stdout, /png: \d+ bytes/);
});
