// Release smoke test: load the freshly built native addon through the
// package loader (dist/index.js -> binding.js -> ./primeval-node.<suffix>.node)
// and render a tiny in-memory image to SVG and PNG.
//
// Usage: node scripts/smoke-test-addon.mjs [napi target]
//
// Needs `npm run prepare:package` (dist/ and binding.js) and the addon in the
// repository root. Without a target, the target is derived from this machine.
import fs from "node:fs";
import path from "node:path";
import { fileURLToPath, pathToFileURL } from "node:url";
import zlib from "node:zlib";

import { readPackageMetadata, runtimeTargetForTarget } from "./napi-targets.mjs";

const REPO_ROOT = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..");
const PNG_SIGNATURE = Buffer.from([0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a]);

function pngChunk(type, data) {
  const typeAndData = Buffer.concat([Buffer.from(type, "latin1"), data]);
  const length = Buffer.alloc(4);
  length.writeUInt32BE(data.length);
  const crc = Buffer.alloc(4);
  crc.writeUInt32BE(zlib.crc32(typeAndData));
  return Buffer.concat([length, typeAndData, crc]);
}

/** Encodes a small 8-bit RGB gradient as PNG, so the smoke test needs no fixture file. */
export function createFixturePng(width, height) {
  const header = Buffer.alloc(13);
  header.writeUInt32BE(width, 0);
  header.writeUInt32BE(height, 4);
  header[8] = 8; // bit depth
  header[9] = 2; // color type: RGB
  // compression, filter and interlace methods stay 0

  const rowLength = 1 + width * 3;
  const pixels = Buffer.alloc(rowLength * height);
  for (let y = 0; y < height; y += 1) {
    pixels[y * rowLength] = 0; // filter: none
    for (let x = 0; x < width; x += 1) {
      const offset = y * rowLength + 1 + x * 3;
      pixels[offset] = Math.round((255 * x) / Math.max(1, width - 1));
      pixels[offset + 1] = Math.round((255 * y) / Math.max(1, height - 1));
      pixels[offset + 2] = (x + y) % 2 === 0 ? 200 : 40;
    }
  }

  return Buffer.concat([
    PNG_SIGNATURE,
    pngChunk("IHDR", header),
    pngChunk("IDAT", zlib.deflateSync(pixels)),
    pngChunk("IEND", Buffer.alloc(0)),
  ]);
}

/** Throws unless `result` is a non-empty render in the requested format. */
export function validateSmokeResult(format, result) {
  if (result?.format !== format) {
    throw new Error(`expected format ${format}, got ${String(result?.format)}`);
  }
  if (!(Number.isInteger(result.width) && result.width > 0)) {
    throw new Error(`${format} result has invalid dimensions`);
  }
  if (!(Number.isInteger(result.height) && result.height > 0)) {
    throw new Error(`${format} result has invalid dimensions`);
  }
  if (!result.data || result.data.length === 0) {
    throw new Error(`${format} result is empty`);
  }
  if (format === "svg" && !String(result.data).includes("<svg")) {
    throw new Error("svg result is not an SVG document");
  }
  if (
    format === "png" &&
    !Buffer.from(result.data).subarray(0, PNG_SIGNATURE.length).equals(PNG_SIGNATURE)
  ) {
    throw new Error("png result is not a PNG image");
  }
}

/**
 * Returns the napi target to smoke test on this runtime. An explicit target
 * must match the running platform and architecture, so that, for example, an
 * x64 addon is never "tested" on an arm64 host that cannot run it.
 */
export function smokeTargetForRuntime(targets, runtime, explicitTarget) {
  const matches = (target) => {
    const { platform, arch, abi } = runtimeTargetForTarget(target);
    // Linux runners are glibc, so a musl target never matches.
    return platform === runtime.platform && arch === runtime.arch && abi !== "musl";
  };

  if (explicitTarget) {
    if (!matches(explicitTarget)) {
      throw new Error(
        `cannot smoke test ${explicitTarget} on ${runtime.platform}-${runtime.arch}; run it on a runner of that platform`,
      );
    }
    return explicitTarget;
  }

  const target = targets.find(matches);
  if (!target) {
    throw new Error(`no napi target for ${runtime.platform}-${runtime.arch}`);
  }
  return target;
}

async function main(argv) {
  const pkg = readPackageMetadata(path.join(REPO_ROOT, "package.json"));
  const target = smokeTargetForRuntime(pkg.napi.targets, process, argv[0]);
  const { suffix } = runtimeTargetForTarget(target);
  const addon = path.join(REPO_ROOT, `${pkg.napi.binaryName}.${suffix}.node`);
  if (!fs.existsSync(addon)) {
    // The loader would fall back to an installed platform package, which is
    // not the artifact under test.
    throw new Error(`missing native addon for ${target}: ${path.basename(addon)}`);
  }

  const { approximate } = await import(pathToFileURL(path.join(REPO_ROOT, "dist", "index.js")));
  const input = createFixturePng(16, 12);
  for (const format of ["svg", "png"]) {
    const result = await approximate({ input, output: format, render: { count: 8 } });
    validateSmokeResult(format, result);
    console.log(
      `${target} ${format}: ${result.data.length} bytes, ${result.width}x${result.height}`,
    );
  }
}

if (process.argv[1] && path.resolve(process.argv[1]) === fileURLToPath(import.meta.url)) {
  main(process.argv.slice(2)).catch((error) => {
    console.error(error instanceof Error ? (error.stack ?? error.message) : String(error));
    process.exitCode = 1;
  });
}
