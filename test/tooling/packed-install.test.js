import assert from "node:assert/strict";
import { spawnSync } from "node:child_process";
import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import { test } from "node:test";
import { fileURLToPath } from "node:url";

import { bindingMetadataForPackage } from "../../scripts/generate-binding.mjs";
import { readRootPackageInputs, requiredRootFiles } from "../../scripts/release-packages.mjs";
import { npmCommand, packRootPackage } from "../helpers/pack.js";

const repoRoot = process.cwd();
const packageJson = JSON.parse(fs.readFileSync(path.join(repoRoot, "package.json"), "utf8"));
const currentTarget = findCurrentTarget();
const currentBinaryPath = currentTarget
  ? path.join(repoRoot, currentTarget.localFile.replace(/^\.\//, ""))
  : null;

function detectCurrentLinuxLibc() {
  if (process.platform !== "linux") {
    return null;
  }

  const report =
    typeof process.report?.getReport === "function" ? process.report.getReport() : null;
  const header = report?.header;
  if (
    header &&
    typeof header.glibcVersionRuntime === "string" &&
    header.glibcVersionRuntime.length > 0
  ) {
    return "gnu";
  }

  const sharedObjects = report?.sharedObjects;
  if (
    Array.isArray(sharedObjects) &&
    sharedObjects.some(
      (entry) =>
        typeof entry === "string" && (entry.includes("ld-musl-") || entry.includes("libc.musl-")),
    )
  ) {
    return "musl";
  }

  return null;
}

function findCurrentTarget() {
  const runtimeAbi = detectCurrentLinuxLibc();
  return (
    bindingMetadataForPackage(packageJson).targets.find(
      (target) =>
        target.platform === process.platform &&
        target.arch === process.arch &&
        (target.abi === null || target.abi === runtimeAbi),
    ) ?? null
  );
}

function run(command, args, options) {
  const result = spawnSync(command, args, {
    encoding: "utf8",
    ...options,
  });

  assert.equal(
    result.status,
    0,
    [`command failed: ${command} ${args.join(" ")}`, result.stdout, result.stderr].join("\n"),
  );

  return result;
}

function writeFile(filePath, content) {
  fs.mkdirSync(path.dirname(filePath), { recursive: true });
  fs.writeFileSync(filePath, content);
}

test("packed package can be installed and render in a consumer project", {
  skip:
    currentTarget === null
      ? `unsupported local runtime: ${process.platform}-${process.arch}`
      : !fs.existsSync(currentBinaryPath)
        ? `missing local native binary: ${currentBinaryPath}`
        : false,
}, () => {
  const tempDir = fs.mkdtempSync(path.join(os.tmpdir(), "primeval-packed-install-"));
  const platformPackageDir = path.join(tempDir, "local-platform");
  let tarballPath;

  try {
    tarballPath = packRootPackage(repoRoot);

    writeFile(
      path.join(tempDir, "package.json"),
      `${JSON.stringify(
        {
          name: "primeval-consumer-smoke",
          private: true,
          type: "module",
        },
        null,
        2,
      )}\n`,
    );

    // The shape `napi create-npm-dirs` generates for npm/<target>/package.json:
    // the `.node` file is the package's `main` and its only file.
    const binaryFile = path.basename(currentBinaryPath);
    fs.mkdirSync(platformPackageDir, { recursive: true });
    fs.copyFileSync(currentBinaryPath, path.join(platformPackageDir, binaryFile));
    writeFile(
      path.join(platformPackageDir, "package.json"),
      `${JSON.stringify(
        {
          name: currentTarget.packageName,
          version: packageJson.version,
          cpu: [currentTarget.arch],
          main: binaryFile,
          files: [binaryFile],
          license: packageJson.license,
          engines: packageJson.engines,
          os: [currentTarget.platform],
          ...(currentTarget.abi === "gnu" ? { libc: ["glibc"] } : {}),
          ...(currentTarget.abi === "musl" ? { libc: ["musl"] } : {}),
        },
        null,
        2,
      )}\n`,
    );
    // Install the packed tarball, so `files` decides what ships.
    const platformTarball = run(npmCommand(), ["pack", "--json", "--pack-destination", tempDir], {
      cwd: platformPackageDir,
    });
    const [{ filename: platformFilename }] = JSON.parse(platformTarball.stdout);
    run(npmCommand(), ["install", path.join(tempDir, platformFilename)], { cwd: tempDir });
    run(npmCommand(), ["install", tarballPath], { cwd: tempDir });

    const installedDist = path.join(tempDir, "node_modules", "@aleburato", "primeval", "dist");
    const shipped = fs.readdirSync(installedDist);
    assert.ok(!shipped.includes("cli.d.ts"), "dist/cli.d.ts must not ship");
    assert.ok(!shipped.includes("native-binding.d.ts"), "dist/native-binding.d.ts must not ship");
    for (const file of shipped.filter((name) => name.endsWith(".d.ts"))) {
      const source = fs.readFileSync(path.join(installedDist, file), "utf8");
      for (const [, specifier] of source.matchAll(/from "(\.\/[^"]+)\.js"/g)) {
        assert.ok(
          shipped.includes(`${specifier.slice(2)}.d.ts`),
          `${file} imports ${specifier}.js, whose declarations are not shipped`,
        );
      }
    }

    // The browser entry under the `browser` condition, the Node entry without
    // it, and nothing internal (the browser runtime's test-only export) under
    // either.
    const installedRoot = path.join(tempDir, "node_modules", "@aleburato", "primeval");
    const resolve = (specifier, conditions) => {
      const result = spawnSync(
        process.execPath,
        [
          ...conditions.map((condition) => `--conditions=${condition}`),
          "--input-type=module",
          "-e",
          `process.stdout.write(import.meta.resolve(${JSON.stringify(specifier)}));`,
        ],
        { cwd: tempDir, encoding: "utf8" },
      );
      return result.status === 0 ? fileURLToPath(result.stdout) : { stderr: result.stderr };
    };
    assert.equal(
      resolve("@aleburato/primeval", ["browser"]),
      fs.realpathSync(path.join(installedRoot, "dist", "browser.js")),
    );
    assert.equal(
      resolve("@aleburato/primeval", []),
      fs.realpathSync(path.join(installedRoot, "dist", "index.js")),
    );
    for (const conditions of [["browser"], []]) {
      assert.match(
        resolve("@aleburato/primeval/dist/browser-runtime.js", conditions).stderr ?? "",
        /ERR_PACKAGE_PATH_NOT_EXPORTED/,
      );
    }
    for (const file of ["browser.js", "browser.d.ts", "worker-single.js", "worker-threaded.js"]) {
      assert.ok(shipped.includes(file), `dist/${file} must ship`);
    }

    // The wasm files ship when they are built (a clean checkout has none); the
    // release requires all of them (scripts/release-packages.mjs verify-root).
    const wasmFiles = requiredRootFiles(readRootPackageInputs(repoRoot)).filter((file) =>
      file.startsWith("wasm/"),
    );
    for (const file of wasmFiles.filter((file) => fs.existsSync(path.join(repoRoot, file)))) {
      assert.ok(fs.existsSync(path.join(installedRoot, file)), `${file} must ship`);
    }
    for (const variant of ["single", "threaded"]) {
      const dir = path.join(installedRoot, "wasm", variant);
      const declarations = fs.existsSync(dir)
        ? fs.readdirSync(dir, { recursive: true }).filter((file) => file.endsWith(".d.ts"))
        : [];
      assert.deepEqual(declarations, [], `wasm/${variant} must not ship declarations`);
    }

    const fixturePath = path.join(repoRoot, "docs", "readme", "originals", "monalisa.jpg");
    const smokeScript = [
      'import { readFile } from "node:fs/promises";',
      'import { approximate } from "@aleburato/primeval";',
      `const input = await readFile(${JSON.stringify(fixturePath)});`,
      "const render = { count: 4, resizeInput: 8, outputSize: 16, seed: 7 };",
      'const svg = await approximate({ input, output: "svg", render });',
      'if (svg.format !== "svg" || !svg.data.startsWith("<svg")) {',
      '  throw new Error("unexpected packed-install SVG result");',
      "}",
      'const png = await approximate({ input, output: "png", render });',
      'if (png.format !== "png" || png.data.readUInt32BE(0) !== 0x89504e47) {',
      '  throw new Error("unexpected packed-install PNG result");',
      "}",
      'process.stdout.write("ok\\n");',
    ].join("\n");

    const smokeResult = run(process.execPath, ["--input-type=module", "-e", smokeScript], {
      cwd: tempDir,
    });
    assert.match(smokeResult.stdout, /^ok$/m);

    // The installed `bin` shim, as a user runs it.
    const bin = path.join(tempDir, "node_modules", ".bin", "primeval");
    const runBin = (args) =>
      process.platform === "win32"
        ? run(`${bin}.cmd`, args, { cwd: tempDir, shell: true })
        : run(bin, args, { cwd: tempDir });
    assert.equal(runBin(["--version"]).stdout, `${packageJson.version}\n`);
    for (const [output, isFormat] of [
      ["out.svg", (bytes) => bytes.toString("utf8").startsWith("<svg")],
      ["out.png", (bytes) => bytes.readUInt32BE(0) === 0x89504e47],
    ]) {
      const cli = runBin([fixturePath, "-o", output, "--count", "4", "--resize-input", "8"]);
      assert.equal(cli.stderr, "");
      assert.ok(isFormat(fs.readFileSync(path.join(tempDir, output))), output);
    }
  } finally {
    if (tarballPath) {
      fs.rmSync(tarballPath, { force: true });
    }
    fs.rmSync(tempDir, { recursive: true, force: true });
  }
});
