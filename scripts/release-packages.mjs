// Publishes the per-platform packages and the root package, re-runnably.
//
// Usage:
//   node scripts/release-packages.mjs publish <npm dir> <tag>
//   node scripts/release-packages.mjs verify <tag>
//
// `publish` skips every package whose version is already on the registry and
// publishes the root package last, so a failed run can simply be re-run.
// `verify` fails unless every package is on the registry at the tag version.
import { execFileSync, spawnSync } from "node:child_process";
import path from "node:path";
import { setTimeout as delay } from "node:timers/promises";
import { fileURLToPath } from "node:url";

import {
  packageSuffixForTarget,
  readPackageMetadata,
  validatePackageMetadata,
} from "./napi-targets.mjs";

const REPO_ROOT = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..");

/** Every package of a release in publish order: platform packages, then the root package. */
export function releasePackages(pkg, npmDir, rootDir) {
  const { packageName, targets, version } = validatePackageMetadata(pkg);
  return [
    ...targets.map((target) => {
      const suffix = packageSuffixForTarget(target);
      return { name: `${packageName}-${suffix}`, version, dir: path.join(npmDir, suffix) };
    }),
    { name: packageName, version, dir: rootDir },
  ];
}

export function assertTagMatchesVersion(tag, version) {
  if (!tag) {
    throw new Error("a release tag is required");
  }
  if (tag !== `v${version}`) {
    throw new Error(
      `release tag ${tag} does not match package.json version ${version} (v${version})`,
    );
  }
}

/**
 * Interprets `npm view <name>@<version> version --json`: true when the version
 * exists, false when it does not, and an error for any other failure.
 */
export function parseNpmView(version, { status, stdout, stderr }) {
  if (status !== 0) {
    if (/\bE404\b/.test(stderr)) {
      return false;
    }
    throw new Error(`npm view failed with status ${status}: ${stderr.trim()}`);
  }
  const output = stdout.trim();
  if (output === "") {
    return false;
  }
  const published = JSON.parse(output);
  if (published !== version) {
    throw new Error(`npm view returned an unexpected version ${JSON.stringify(published)}`);
  }
  return true;
}

/** Publishes, in order, every package that is not on the registry yet. */
export function publishPackages(packages, { isPublished, publish, log }) {
  const actions = [];
  for (const entry of packages) {
    if (isPublished(entry)) {
      log(`skip ${entry.name}@${entry.version}: already published`);
      actions.push({ ...entry, action: "skip" });
      continue;
    }
    log(`publish ${entry.name}@${entry.version}`);
    publish(entry);
    actions.push({ ...entry, action: "publish" });
  }
  return actions;
}

/** Waits for every package to be visible on the registry; fails with the missing ones. */
export async function verifyPublished(
  packages,
  { isPublished, sleep = delay, attempts = 6, delayMs = 10_000, log = console.log },
) {
  let missing = packages;
  for (let attempt = 1; ; attempt += 1) {
    missing = missing.filter((entry) => !isPublished(entry));
    if (missing.length === 0) {
      for (const entry of packages) {
        log(`verified ${entry.name}@${entry.version}`);
      }
      return;
    }
    if (attempt >= attempts) {
      break;
    }
    await sleep(delayMs);
  }
  throw new Error(
    `not on the registry: ${missing.map(({ name, version }) => `${name}@${version}`).join(", ")}`,
  );
}

function npmCommand() {
  return process.platform === "win32" ? "npm.cmd" : "npm";
}

function npmIsPublished({ name, version }) {
  // --prefer-online: a cached packument from before the publish must not hide it.
  const result = spawnSync(
    npmCommand(),
    ["view", `${name}@${version}`, "version", "--json", "--prefer-online"],
    { encoding: "utf8" },
  );
  if (result.error) {
    throw result.error;
  }
  return parseNpmView(version, result);
}

function npmPublish({ dir }) {
  execFileSync(npmCommand(), ["publish", "--access", "public"], { cwd: dir, stdio: "inherit" });
}

async function main(argv) {
  const [command, firstArg, secondArg] = argv;
  const pkg = readPackageMetadata(path.join(REPO_ROOT, "package.json"));

  switch (command) {
    case "publish": {
      if (!firstArg) {
        throw new Error("publish requires the npm packages directory");
      }
      assertTagMatchesVersion(secondArg, pkg.version);
      const packages = releasePackages(pkg, path.resolve(firstArg), REPO_ROOT);
      publishPackages(packages, {
        isPublished: npmIsPublished,
        publish: npmPublish,
        log: console.log,
      });
      break;
    }
    case "verify": {
      assertTagMatchesVersion(firstArg, pkg.version);
      // Directories are irrelevant for verification.
      await verifyPublished(releasePackages(pkg, REPO_ROOT, REPO_ROOT), {
        isPublished: npmIsPublished,
      });
      break;
    }
    default:
      throw new Error(
        "usage: node scripts/release-packages.mjs publish <npm dir> <tag> | verify <tag>",
      );
  }
}

if (process.argv[1] && path.resolve(process.argv[1]) === fileURLToPath(import.meta.url)) {
  main(process.argv.slice(2)).catch((error) => {
    console.error(error instanceof Error ? error.message : String(error));
    process.exitCode = 1;
  });
}
