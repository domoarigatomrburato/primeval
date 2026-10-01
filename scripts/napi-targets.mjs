import { execFileSync } from "node:child_process";
import fs from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";

const TARGET_SUFFIXES = {
  "aarch64-apple-darwin": "darwin-arm64",
  "x86_64-apple-darwin": "darwin-x64",
  "aarch64-unknown-linux-gnu": "linux-arm64-gnu",
  "x86_64-unknown-linux-gnu": "linux-x64-gnu",
  "x86_64-pc-windows-msvc": "win32-x64-msvc",
};

const TARGET_RUNNERS = {
  "aarch64-apple-darwin": "macos-14",
  "x86_64-apple-darwin": "macos-14",
  "aarch64-unknown-linux-gnu": "ubuntu-24.04-arm",
  "x86_64-unknown-linux-gnu": "ubuntu-latest",
  "x86_64-pc-windows-msvc": "windows-latest",
};

const RUNTIME_TARGETS = {
  "aarch64-apple-darwin": { platform: "darwin", arch: "arm64", abi: null },
  "x86_64-apple-darwin": { platform: "darwin", arch: "x64", abi: null },
  "aarch64-unknown-linux-gnu": { platform: "linux", arch: "arm64", abi: "gnu" },
  "x86_64-unknown-linux-gnu": { platform: "linux", arch: "x64", abi: "gnu" },
  "x86_64-pc-windows-msvc": { platform: "win32", arch: "x64", abi: "msvc" },
};

const DEFAULT_PACKAGE_JSON = path.resolve(
  path.dirname(fileURLToPath(import.meta.url)),
  "..",
  "package.json",
);
const DEFAULT_PACKAGE_LOCK = path.resolve(
  path.dirname(fileURLToPath(import.meta.url)),
  "..",
  "package-lock.json",
);

export function optionalDependencyNamesForTargets(packageName, targets) {
  return targets.map((target) => `${packageName}-${packageSuffixForTarget(target)}`);
}

export function runtimeTargetForTarget(target) {
  const runtime = RUNTIME_TARGETS[target];
  if (!runtime) {
    throw new Error(`unsupported napi target: ${target}`);
  }

  return {
    target,
    suffix: packageSuffixForTarget(target),
    ...runtime,
  };
}

export function releaseMatrixForTargets(targets) {
  return {
    include: targets.map((target) => ({
      runner: runnerForTarget(target),
      target,
    })),
  };
}

export function readPackageMetadata(packageJsonPath = DEFAULT_PACKAGE_JSON) {
  return JSON.parse(fs.readFileSync(packageJsonPath, "utf8"));
}

export function readPackageLock(packageLockPath = DEFAULT_PACKAGE_LOCK) {
  return JSON.parse(fs.readFileSync(packageLockPath, "utf8"));
}

export function validatePackageMetadata(pkg) {
  const packageName = requiredString(pkg.name, "package.json name");
  const version = requiredString(pkg.version, "package.json version");
  const targets = requiredTargets(pkg.napi?.targets);
  const expectedOptionalDependencies = optionalDependencyNamesForTargets(packageName, targets);
  const actualOptionalDependencies = Object.keys(pkg.optionalDependencies ?? {}).sort();

  if (
    JSON.stringify(actualOptionalDependencies) !==
    JSON.stringify([...expectedOptionalDependencies].sort())
  ) {
    throw new Error(
      `optionalDependencies drift from napi.targets; expected ${expectedOptionalDependencies.join(
        ", ",
      )}, got ${actualOptionalDependencies.join(", ") || "(none)"}`,
    );
  }

  for (const dependencyName of expectedOptionalDependencies) {
    if (pkg.optionalDependencies[dependencyName] !== version) {
      throw new Error(
        `optional dependency ${dependencyName} must match package version ${version}`,
      );
    }
  }

  return {
    packageName,
    targets,
    version,
    expectedOptionalDependencies,
  };
}

export function validatePackageLock(pkg, packageLock) {
  const { packageName, version, expectedOptionalDependencies } = validatePackageMetadata(pkg);
  const lockName = requiredString(packageLock.name, "package-lock.json name");
  const lockVersion = requiredString(packageLock.version, "package-lock.json version");
  const rootPackage = requiredObject(packageLock.packages?.[""], 'package-lock.json packages[""]');

  if (lockName !== packageName) {
    throw new Error(
      `package-lock.json name ${lockName} must match package.json name ${packageName}`,
    );
  }

  if (lockVersion !== version) {
    throw new Error(
      `package-lock.json version ${lockVersion} must match package.json version ${version}`,
    );
  }

  if (rootPackage.name !== packageName) {
    throw new Error(
      `package-lock.json root package name ${rootPackage.name} must match ${packageName}`,
    );
  }

  if (rootPackage.version !== version) {
    throw new Error(
      `package-lock.json root package version ${rootPackage.version} must match ${version}`,
    );
  }

  const expectedOptionalDependencyVersions = optionalDependencyVersionMap(
    expectedOptionalDependencies,
    version,
  );
  const rootOptionalDependencies = normalizeDependencyMap(rootPackage.optionalDependencies);

  if (
    JSON.stringify(rootOptionalDependencies) !==
    JSON.stringify(normalizeDependencyMap(expectedOptionalDependencyVersions))
  ) {
    throw new Error(
      `package-lock.json root optionalDependencies must match package.json for version ${version}`,
    );
  }

  for (const dependencyName of expectedOptionalDependencies) {
    const dependencyPackage = requiredObject(
      packageLock.packages?.[`node_modules/${dependencyName}`],
      `package-lock.json entry for ${dependencyName}`,
    );

    if (dependencyPackage.version !== version) {
      throw new Error(`package-lock.json entry ${dependencyName} must use version ${version}`);
    }
  }

  return {
    packageName,
    version,
    expectedOptionalDependencies,
  };
}

export function updatePackageVersion(pkg, nextVersion) {
  const packageName = requiredString(pkg.name, "package.json name");
  const version = requiredString(nextVersion, "next version");
  const targets = requiredTargets(pkg.napi?.targets);
  const expectedOptionalDependencies = optionalDependencyNamesForTargets(packageName, targets);

  return {
    ...pkg,
    version,
    optionalDependencies: optionalDependencyVersionMap(expectedOptionalDependencies, version),
  };
}

export function bumpPackageVersion(
  nextVersion,
  packageJsonPath = DEFAULT_PACKAGE_JSON,
  packageLockPath = DEFAULT_PACKAGE_LOCK,
) {
  const resolvedPackageJsonPath = path.resolve(packageJsonPath);
  const resolvedPackageLockPath = path.resolve(packageLockPath);
  const packageDir = path.dirname(resolvedPackageJsonPath);
  const existingPackageJson = fs.readFileSync(resolvedPackageJsonPath, "utf8");
  const existingPackageLock = fs.existsSync(resolvedPackageLockPath)
    ? fs.readFileSync(resolvedPackageLockPath, "utf8")
    : null;

  try {
    const updatedPackage = updatePackageVersion(
      readPackageMetadata(resolvedPackageJsonPath),
      nextVersion,
    );
    writeJsonFile(resolvedPackageJsonPath, updatedPackage);
    execFileSync(npmCommand(), ["install", "--package-lock-only", "--ignore-scripts"], {
      cwd: packageDir,
      stdio: "inherit",
    });
    validatePackageLock(updatedPackage, readPackageLock(resolvedPackageLockPath));
    return updatedPackage;
  } catch (error) {
    fs.writeFileSync(resolvedPackageJsonPath, existingPackageJson);
    if (existingPackageLock === null) {
      fs.rmSync(resolvedPackageLockPath, { force: true });
    } else {
      fs.writeFileSync(resolvedPackageLockPath, existingPackageLock);
    }
    throw error;
  }
}

export function verifyArtifacts(artifactsDir, pkg) {
  const { expectedOptionalDependencies, targets } = validatePackageMetadata(pkg);
  const absoluteArtifactsDir = path.resolve(artifactsDir);
  if (!fs.existsSync(absoluteArtifactsDir)) {
    throw new Error(`artifact directory does not exist: ${absoluteArtifactsDir}`);
  }

  const missingPackages = [];
  const missingPayloads = [];

  for (const [index, dependencyName] of expectedOptionalDependencies.entries()) {
    const target = targets[index];
    const candidates = findArtifactCandidates(absoluteArtifactsDir, dependencyName);
    if (candidates.length === 0) {
      missingPackages.push(dependencyName);
      continue;
    }

    const hasNodePayload = candidates.some((candidate) =>
      candidateContainsNodePayload(candidate, expectedNodePayloadName(target)),
    );
    if (!hasNodePayload) {
      missingPayloads.push(dependencyName);
    }
  }

  if (missingPackages.length > 0 || missingPayloads.length > 0) {
    const parts = [];
    if (missingPackages.length > 0) {
      parts.push(`missing package artifacts: ${missingPackages.join(", ")}`);
    }
    if (missingPayloads.length > 0) {
      parts.push(`missing .node payloads: ${missingPayloads.join(", ")}`);
    }
    throw new Error(parts.join("; "));
  }
}

export function requiredString(value, label) {
  if (typeof value !== "string" || value.length === 0) {
    throw new Error(`${label} is required`);
  }
  return value;
}

function requiredObject(value, label) {
  if (!value || typeof value !== "object" || Array.isArray(value)) {
    throw new Error(`${label} is required`);
  }
  return value;
}

function requiredTargets(targets) {
  if (!Array.isArray(targets) || targets.length === 0) {
    throw new Error("package.json napi.targets must be a non-empty array");
  }
  return targets.map((target) => requiredString(target, "napi.targets entry"));
}

function normalizeDependencyMap(dependencies) {
  return Object.fromEntries(
    Object.entries(requiredObject(dependencies ?? {}, "dependency map")).sort(([left], [right]) =>
      left.localeCompare(right),
    ),
  );
}

function optionalDependencyVersionMap(optionalDependencies, version) {
  return Object.fromEntries(
    optionalDependencies.map((dependencyName) => [dependencyName, version]),
  );
}

function writeJsonFile(filePath, value) {
  fs.writeFileSync(filePath, `${JSON.stringify(value, null, 2)}\n`);
}

function npmCommand() {
  return process.platform === "win32" ? "npm.cmd" : "npm";
}

export function packageSuffixForTarget(target) {
  const suffix = TARGET_SUFFIXES[target];
  if (!suffix) {
    throw new Error(`unsupported napi target: ${target}`);
  }
  return suffix;
}

function runnerForTarget(target) {
  const runner = TARGET_RUNNERS[target];
  if (!runner) {
    throw new Error(`no GitHub runner mapping for napi target: ${target}`);
  }
  return runner;
}

function findArtifactCandidates(rootDir, dependencyName) {
  const normalized = dependencyName.replace(/^@/, "").replace("/", "-");
  const packageDirs = [];
  const matchingFiles = [];
  const queue = [rootDir];

  while (queue.length > 0) {
    const current = queue.pop();
    const packageJsonPath = path.join(current, "package.json");
    if (fs.existsSync(packageJsonPath)) {
      try {
        const pkg = JSON.parse(fs.readFileSync(packageJsonPath, "utf8"));
        if (pkg.name === dependencyName) {
          packageDirs.push(current);
          continue;
        }
      } catch {}
    }

    for (const entry of fs.readdirSync(current, { withFileTypes: true })) {
      const resolved = path.join(current, entry.name);
      if (entry.isDirectory()) {
        queue.push(resolved);
      } else if (entry.isFile()) {
        const basename = path.basename(resolved);
        if (
          basename.includes(normalized) ||
          resolved.includes(`${path.sep}${normalized}${path.sep}`)
        ) {
          matchingFiles.push(resolved);
        }
      }
    }
  }

  return [...new Set([...packageDirs, ...matchingFiles])];
}

function walkFiles(rootDir) {
  const files = [];
  const stack = [rootDir];
  while (stack.length > 0) {
    const current = stack.pop();
    for (const entry of fs.readdirSync(current, { withFileTypes: true })) {
      const resolved = path.join(current, entry.name);
      if (entry.isDirectory()) {
        stack.push(resolved);
      } else if (entry.isFile()) {
        files.push(resolved);
      }
    }
  }
  return files;
}

function candidateContainsNodePayload(candidate, expectedPayloadName) {
  if (fs.existsSync(candidate) && fs.statSync(candidate).isDirectory()) {
    return walkFiles(candidate).some((file) => path.basename(file) === expectedPayloadName);
  }
  if (candidate.endsWith(".node")) {
    return path.basename(candidate).includes(expectedPayloadName);
  }
  if (candidate.endsWith(".tgz")) {
    return tarballContainsNodePayload(candidate, expectedPayloadName);
  }
  return false;
}

function tarballContainsNodePayload(candidate, expectedPayloadName) {
  const listing = execFileSync("tar", ["-tf", candidate], {
    encoding: "utf8",
  });
  return listing
    .split("\n")
    .some((line) => line.endsWith(expectedPayloadName) || line.endsWith(`/${expectedPayloadName}`));
}

function expectedNodePayloadName(target) {
  return `primeval-node.${packageSuffixForTarget(target)}.node`;
}

function usage() {
  console.error(
    [
      "Usage:",
      "  node scripts/napi-targets.mjs check-package [package.json path]",
      "  node scripts/napi-targets.mjs release-matrix [package.json path]",
      "  node scripts/napi-targets.mjs expected-packages [package.json path]",
      "  node scripts/napi-targets.mjs bump-version <version> [package.json path] [package-lock.json path]",
      "  node scripts/napi-targets.mjs verify-artifacts <artifacts dir> [package.json path]",
    ].join("\n"),
  );
}

function main(argv) {
  const [command, firstArg, secondArg, thirdArg] = argv;
  if (!command) {
    usage();
    process.exitCode = 1;
    return;
  }

  try {
    switch (command) {
      case "check-package": {
        const packageJsonPath = firstArg ? path.resolve(firstArg) : DEFAULT_PACKAGE_JSON;
        const pkg = readPackageMetadata(packageJsonPath);
        const result = validatePackageMetadata(pkg);
        const packageLockPath = path.resolve(path.dirname(packageJsonPath), "package-lock.json");
        if (!fs.existsSync(packageLockPath)) {
          throw new Error(`package-lock.json is required next to ${packageJsonPath}`);
        }
        validatePackageLock(pkg, readPackageLock(packageLockPath));
        console.log(JSON.stringify(result));
        break;
      }
      case "release-matrix": {
        const pkg = readPackageMetadata(firstArg);
        const { targets } = validatePackageMetadata(pkg);
        console.log(JSON.stringify(releaseMatrixForTargets(targets)));
        break;
      }
      case "expected-packages": {
        const result = validatePackageMetadata(readPackageMetadata(firstArg));
        console.log(JSON.stringify(result.expectedOptionalDependencies));
        break;
      }
      case "bump-version": {
        if (!firstArg) {
          throw new Error("bump-version requires a version");
        }
        const result = bumpPackageVersion(
          firstArg,
          secondArg ?? DEFAULT_PACKAGE_JSON,
          thirdArg ?? DEFAULT_PACKAGE_LOCK,
        );
        console.log(JSON.stringify(result));
        break;
      }
      case "verify-artifacts": {
        if (!firstArg) {
          throw new Error("verify-artifacts requires an artifacts directory");
        }
        const pkg = readPackageMetadata(secondArg);
        verifyArtifacts(firstArg, pkg);
        console.log(`verified artifacts for ${pkg.name}`);
        break;
      }
      default:
        throw new Error(`unknown command: ${command}`);
    }
  } catch (error) {
    console.error(error instanceof Error ? error.message : String(error));
    process.exitCode = 1;
  }
}

if (process.argv[1] && path.resolve(process.argv[1]) === fileURLToPath(import.meta.url)) {
  main(process.argv.slice(2));
}
