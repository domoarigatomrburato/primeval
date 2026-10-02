import assert from "node:assert/strict";
import { spawnSync } from "node:child_process";
import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import { test } from "node:test";

import { bindingMetadataForPackage } from "../../scripts/generate-binding.mjs";

const repoRoot = process.cwd();
const packageJson = JSON.parse(fs.readFileSync(path.join(repoRoot, "package.json"), "utf8"));
const currentTarget = findCurrentTarget();
const currentBinaryPath = currentTarget
  ? path.join(repoRoot, currentTarget.localFile.replace(/^\.\//, ""))
  : null;

function npmCommand() {
  return process.platform === "win32" ? "npm.cmd" : "npm";
}

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

function packRootPackage() {
  const result = run(npmCommand(), ["pack", "--json"], { cwd: repoRoot });
  const jsonStart = result.stdout.lastIndexOf("\n[");
  const summaryText = (
    jsonStart === -1 ? result.stdout : result.stdout.slice(jsonStart + 1)
  ).trim();
  const [summary] = JSON.parse(summaryText);
  return path.join(repoRoot, summary.filename);
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
    tarballPath = packRootPackage();

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

    fs.mkdirSync(platformPackageDir, { recursive: true });
    fs.copyFileSync(
      currentBinaryPath,
      path.join(platformPackageDir, path.basename(currentBinaryPath)),
    );
    writeFile(
      path.join(platformPackageDir, "package.json"),
      `${JSON.stringify(
        {
          name: currentTarget.packageName,
          version: packageJson.version,
          main: "index.js",
        },
        null,
        2,
      )}\n`,
    );
    writeFile(
      path.join(platformPackageDir, "index.js"),
      `module.exports = require("./${path.basename(currentBinaryPath)}");\n`,
    );

    run(npmCommand(), ["install", "./local-platform"], { cwd: tempDir });
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

    const fixturePath = path.join(repoRoot, "docs", "readme", "originals", "monalisa.jpg");
    const smokeScript = [
      'import { readFile } from "node:fs/promises";',
      'import { approximate } from "@aleburato/primeval";',
      `const input = await readFile(${JSON.stringify(fixturePath)});`,
      "const result = await approximate({",
      "  input,",
      '  output: "svg",',
      "  render: { count: 4, resizeInput: 8, outputSize: 16, seed: 7 },",
      "});",
      'if (result.format !== "svg" || !result.data.startsWith("<svg")) {',
      '  throw new Error("unexpected packed-install result");',
      "}",
      'process.stdout.write("ok\\n");',
    ].join("\n");

    const smokeResult = run(process.execPath, ["--input-type=module", "-e", smokeScript], {
      cwd: tempDir,
    });
    assert.match(smokeResult.stdout, /^ok$/m);
  } finally {
    if (tarballPath) {
      fs.rmSync(tarballPath, { force: true });
    }
    fs.rmSync(tempDir, { recursive: true, force: true });
  }
});
