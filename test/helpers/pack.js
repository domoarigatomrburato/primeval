import assert from "node:assert/strict";
import { spawnSync } from "node:child_process";
import path from "node:path";

export function npmCommand() {
  return process.platform === "win32" ? "npm.cmd" : "npm";
}

/**
 * Packs the root package in `rootDir` with `npm pack` and returns the path of
 * the tarball, written to `destination` (`rootDir` by default). Lifecycle
 * scripts (`prepack`, which rebuilds `dist/`) run unless `ignoreScripts`.
 */
export function packRootPackage(rootDir, { destination, ignoreScripts = false } = {}) {
  const args = [
    "pack",
    "--json",
    ...(destination === undefined ? [] : ["--pack-destination", destination]),
    ...(ignoreScripts ? ["--ignore-scripts"] : []),
  ];
  const result = spawnSync(npmCommand(), args, { cwd: rootDir, encoding: "utf8" });
  assert.equal(
    result.status,
    0,
    [`command failed: npm ${args.join(" ")}`, result.stdout, result.stderr].join("\n"),
  );
  // Lifecycle scripts print before the JSON summary.
  const jsonStart = result.stdout.lastIndexOf("\n[");
  const summaryText = (
    jsonStart === -1 ? result.stdout : result.stdout.slice(jsonStart + 1)
  ).trim();
  const [summary] = JSON.parse(summaryText);
  return path.join(destination ?? rootDir, summary.filename);
}
