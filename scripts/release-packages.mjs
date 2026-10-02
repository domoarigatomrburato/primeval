// Publishes the per-platform packages and the root package, re-runnably.
//
// Usage:
//   node scripts/release-packages.mjs publish <npm dir> <tag>
//   node scripts/release-packages.mjs verify <tag>
//   node scripts/release-packages.mjs verify-root
//
// `publish` first checks that the root package is complete (`verify-root`),
// then skips every package whose version is already on the registry and
// publishes the root package last, so a failed run can simply be re-run.
// `verify` fails unless every package is on the registry at the tag version.
// `verify-root` fails unless the root tarball (`npm pack --dry-run`) has
// every file both entries need at runtime, the wasm builds included; it
// expects `npm run prepare:package` and `npm run build:wasm` to have run.
import { execFileSync, spawnSync } from "node:child_process";
import fs from "node:fs";
import path from "node:path";
import { setTimeout as delay } from "node:timers/promises";
import { fileURLToPath } from "node:url";

import { OUTPUT_FILES, VARIANTS } from "./build-wasm.mjs";
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

/** The `./snippets/...` files a wasm-bindgen glue imports, relative to its directory. */
export function glueSnippets(glueSource) {
  return [...glueSource.matchAll(/^import\b[^'"]*['"]\.\/(snippets\/[^'"]+)['"]/gm)].map(
    ([, file]) => file,
  );
}

/**
 * The files the root tarball must contain: `package.json`, every literal
 * (non-glob) `files` entry (each wasm build's glue and `.wasm` among them),
 * the compiled module of every `src/*.ts` (`sourceModules`, without
 * extension), and the snippets each wasm build's glue imports
 * (`glueSources[variant]`, the glue's source when it is built).
 */
export function requiredRootFiles({ pkg, sourceModules, glueSources }) {
  const files = new Set(["package.json"]);
  for (const entry of pkg.files ?? []) {
    if (!/[*?[{]/.test(entry)) {
      files.add(entry);
    }
  }
  for (const module of sourceModules) {
    files.add(`dist/${module}.js`);
  }
  for (const [variant, { outDir }] of Object.entries(VARIANTS)) {
    const glue = glueSources[variant];
    for (const snippet of glue === undefined ? [] : glueSnippets(glue)) {
      files.add(`${outDir}/${snippet}`);
    }
  }
  return [...files];
}

/** The inputs of `requiredRootFiles` for the package in `rootDir`. */
export function readRootPackageInputs(rootDir) {
  const pkg = readPackageMetadata(path.join(rootDir, "package.json"));
  const sourceModules = fs
    .readdirSync(path.join(rootDir, "src"))
    .filter((file) => file.endsWith(".ts") && !file.endsWith(".d.ts"))
    .map((file) => file.slice(0, -".ts".length));
  const glueSources = {};
  for (const [variant, { outDir }] of Object.entries(VARIANTS)) {
    const glue = path.join(rootDir, outDir, OUTPUT_FILES.glue);
    if (fs.existsSync(glue)) {
      glueSources[variant] = fs.readFileSync(glue, "utf8");
    }
  }
  return { pkg, sourceModules, glueSources };
}

/** Fails unless `packedFiles` (tarball paths) contains every `required` file. */
export function verifyRootPackage(required, packedFiles) {
  const packed = new Set(packedFiles);
  const missing = required.filter((file) => !packed.has(file));
  if (missing.length > 0) {
    throw new Error(`the root package is missing ${missing.join(", ")}`);
  }
}

/** The paths in the root tarball, from `npm pack --dry-run --json`, without lifecycle scripts. */
function packedRootFiles(rootDir) {
  const output = execFileSync(npmCommand(), ["pack", "--dry-run", "--json", "--ignore-scripts"], {
    cwd: rootDir,
    encoding: "utf8",
  });
  const [summary] = JSON.parse(output);
  return summary.files.map((file) => file.path);
}

function verifyRepositoryRootPackage() {
  verifyRootPackage(
    requiredRootFiles(readRootPackageInputs(REPO_ROOT)),
    packedRootFiles(REPO_ROOT),
  );
  console.log("verified the root package contents");
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
      // Before anything is published: an incomplete root package fails the release.
      verifyRepositoryRootPackage();
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
    case "verify-root":
      verifyRepositoryRootPackage();
      break;
    default:
      throw new Error(
        "usage: node scripts/release-packages.mjs publish <npm dir> <tag> | verify <tag> | verify-root",
      );
  }
}

if (process.argv[1] && path.resolve(process.argv[1]) === fileURLToPath(import.meta.url)) {
  main(process.argv.slice(2)).catch((error) => {
    console.error(error instanceof Error ? error.message : String(error));
    process.exitCode = 1;
  });
}
