// Publishes the per-platform packages and the root package, re-runnably.
//
// Usage:
//   node scripts/release-packages.mjs publish <npm dir> <tag>
//   node scripts/release-packages.mjs verify <tag>
//   node scripts/release-packages.mjs verify-root
//
// `publish` first adds the platform packages to the root package.json as
// `optionalDependencies` (the committed file must not list them: npm cannot
// lock a version that is not on the registry yet), checks that the root
// package is complete (`verify-root`) and that its tarball lists them, then
// skips every package whose version is already on the registry, publishes the
// platform packages, waits until each of them is visible on the registry (npm
// can leave a new version in staging while `npm publish` exits 0), and only
// then publishes the root package, so a failed run can simply be re-run. It restores the
// committed package.json when it is done, and publishes without lifecycle
// scripts: `npm run prepare:package` must have run.
// `verify` fails unless every package is on the registry at the tag version
// and the published root package lists every platform package.
// `verify-root` fails unless the root tarball (`npm pack --dry-run`) has
// every file both entries need at runtime, the wasm builds included; it
// expects `npm run prepare:package` and `npm run build:wasm` to have run.
import { execFileSync, spawnSync } from "node:child_process";
import fs from "node:fs";
import os from "node:os";
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

/**
 * The root package.json as published: `pkg` (the committed file, which must
 * not list them) with every platform package of `napi.targets`, in that
 * order, at the package version as `optionalDependencies`.
 */
export function withPlatformDependencies(pkg) {
  const { expectedOptionalDependencies, version } = validatePackageMetadata(pkg);
  return {
    ...pkg,
    optionalDependencies: Object.fromEntries(
      expectedOptionalDependencies.map((name) => [name, version]),
    ),
  };
}

/** Fails unless `actual` has exactly the `expected` dependencies, in any order. */
export function verifyPlatformDependencies(label, expected, actual) {
  const normalize = (dependencies) =>
    JSON.stringify(Object.entries(dependencies ?? {}).sort(([a], [b]) => a.localeCompare(b)));
  if (normalize(actual) !== normalize(expected)) {
    const show = (dependencies) =>
      Object.keys(dependencies ?? {}).length === 0 ? "(none)" : JSON.stringify(dependencies);
    throw new Error(
      `${label} has optionalDependencies ${show(actual)}, expected ${show(expected)}`,
    );
  }
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

function verifyRepositoryRootPackage(rootDir = REPO_ROOT) {
  verifyRootPackage(requiredRootFiles(readRootPackageInputs(rootDir)), packedRootFiles(rootDir));
  console.log("verified the root package contents");
}

/**
 * The package.json inside the root tarball, as `npm pack` (without lifecycle
 * scripts) writes it; the tarball goes to a temporary directory and is removed.
 */
export function packedRootManifest(rootDir) {
  const destination = fs.mkdtempSync(path.join(os.tmpdir(), "primeval-root-pack-"));
  try {
    const output = execFileSync(
      npmCommand(),
      ["pack", "--json", "--ignore-scripts", "--pack-destination", destination],
      { cwd: rootDir, encoding: "utf8" },
    );
    const [{ filename }] = JSON.parse(output);
    return JSON.parse(
      execFileSync("tar", ["-xzOf", path.join(destination, filename), "package/package.json"], {
        encoding: "utf8",
      }),
    );
  } finally {
    fs.rmSync(destination, { recursive: true, force: true });
  }
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

/**
 * Publishes, in order, every package that is not on the registry yet: the
 * platform packages, then, once each of them is visible on the registry
 * (`verifyPublished`, with the same `tools`), the root package last. npm can
 * leave a new version in staging while `npm publish` exits 0; the root
 * package, which pins every platform package, must never go out without them.
 */
export async function publishPackages(packages, tools) {
  const { isPublished, publish, log } = tools;
  const platforms = packages.slice(0, -1);
  const root = packages.at(-1);
  const actions = [];
  const publishMissing = (entry) => {
    if (isPublished(entry)) {
      log(`skip ${entry.name}@${entry.version}: already published`);
      actions.push({ ...entry, action: "skip" });
      return;
    }
    log(`publish ${entry.name}@${entry.version}`);
    publish(entry);
    actions.push({ ...entry, action: "publish" });
  };
  for (const entry of platforms) {
    publishMissing(entry);
  }
  try {
    await verifyPublished(platforms, tools);
  } catch (error) {
    throw new Error(
      `${error.message}; the root package ${root.name}@${root.version} was not published. ` +
        "A version left in npm staging must be approved (`npm stage approve`) " +
        "and this command re-run, or the release bumped to a new version.",
    );
  }
  publishMissing(root);
  return actions;
}

/**
 * Publishes a release from the root package in `rootDir`: writes the
 * package.json with the platform packages (`withPlatformDependencies`), then,
 * before publishing anything, checks the root package (`verifyRoot`) and that
 * its tarball (`packedManifest`) lists exactly them; then `publishPackages`,
 * which publishes the root package only once every platform package is
 * visible on the registry. The committed package.json is restored in every
 * case.
 */
export async function publishRelease({ rootDir, npmDir, tag }, tools) {
  const packageJsonPath = path.join(rootDir, "package.json");
  const committedSource = fs.readFileSync(packageJsonPath, "utf8");
  const pkg = JSON.parse(committedSource);
  assertTagMatchesVersion(tag, pkg.version);
  const packages = releasePackages(pkg, npmDir, rootDir);
  const published = withPlatformDependencies(pkg);

  fs.writeFileSync(packageJsonPath, `${JSON.stringify(published, null, 2)}\n`);
  try {
    tools.verifyRoot(rootDir);
    verifyPlatformDependencies(
      "the packed root package",
      published.optionalDependencies,
      tools.packedManifest(rootDir).optionalDependencies,
    );
    return await publishPackages(packages, tools);
  } finally {
    fs.writeFileSync(packageJsonPath, committedSource);
  }
}

/**
 * Waits for every package to be visible on the registry; fails with the missing ones.
 * The default wait is about 15 minutes: npm completes trusted-publishing (OIDC)
 * publishes asynchronously. It usually shows a version about 2 minutes after
 * `npm publish` returns, but once (0.2.2, the Windows package) it took about 9.
 */
export async function verifyPublished(
  packages,
  { isPublished, sleep = delay, attempts = 90, delayMs = 10_000, log = console.log },
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

/**
 * Waits for every package of the release of `pkg` (the committed package.json)
 * to be on the registry (`verifyPublished`), then checks that the published
 * root package lists every platform package (`rootDependencies`).
 */
export async function verifyRelease(pkg, tag, { rootDependencies, ...tools }) {
  assertTagMatchesVersion(tag, pkg.version);
  // Directories are irrelevant for verification.
  const packages = releasePackages(pkg, "", "");
  await verifyPublished(packages, tools);
  verifyPlatformDependencies(
    "the published root package",
    withPlatformDependencies(pkg).optionalDependencies,
    rootDependencies(packages.at(-1)),
  );
  (tools.log ?? console.log)(`verified the optionalDependencies of ${pkg.name}@${pkg.version}`);
}

/**
 * Interprets `npm view <name>@<version> optionalDependencies --json`: the map,
 * or `{}` when the version has none. Any failure is an error: the caller has
 * already seen the version on the registry.
 */
export function parseNpmViewDependencies({ status, stdout, stderr = "" }) {
  if (status !== 0) {
    throw new Error(`npm view failed with status ${status}: ${stderr.trim()}`);
  }
  const output = stdout.trim();
  return output === "" ? {} : JSON.parse(output);
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

function npmRootDependencies({ name, version }) {
  const result = spawnSync(
    npmCommand(),
    ["view", `${name}@${version}`, "optionalDependencies", "--json", "--prefer-online"],
    { encoding: "utf8" },
  );
  if (result.error) {
    throw result.error;
  }
  return parseNpmViewDependencies(result);
}

// Without lifecycle scripts: the root package's `prepack` would rebuild dist/
// and binding.js after the checks, and the binding generator refuses a
// package.json that lists the platform packages.
export const NPM_PUBLISH_ARGS = ["publish", "--access", "public", "--ignore-scripts"];

function npmPublish({ dir }) {
  execFileSync(npmCommand(), NPM_PUBLISH_ARGS, { cwd: dir, stdio: "inherit" });
}

async function main(argv) {
  const [command, firstArg, secondArg] = argv;
  const pkg = readPackageMetadata(path.join(REPO_ROOT, "package.json"));

  switch (command) {
    case "publish": {
      if (!firstArg) {
        throw new Error("publish requires the npm packages directory");
      }
      await publishRelease(
        { rootDir: REPO_ROOT, npmDir: path.resolve(firstArg), tag: secondArg },
        {
          verifyRoot: verifyRepositoryRootPackage,
          packedManifest: packedRootManifest,
          isPublished: npmIsPublished,
          publish: npmPublish,
          log: console.log,
        },
      );
      break;
    }
    case "verify": {
      await verifyRelease(pkg, firstArg, {
        isPublished: npmIsPublished,
        rootDependencies: npmRootDependencies,
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
