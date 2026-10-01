import assert from "node:assert/strict";
import { spawnSync } from "node:child_process";
import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import { test } from "node:test";

import {
  optionalDependencyNamesForTargets,
  releaseMatrixForTargets,
  updatePackageVersion,
  validatePackageLock,
  validatePackageMetadata,
  verifyArtifacts,
} from "../../scripts/napi-targets.mjs";

function parseRustToolchainVersion(source) {
  const match = source.match(/^channel = "([^"]+)"$/m);
  assert.ok(match, "missing rust toolchain channel");
  return match[1];
}

function readWorkflows() {
  const dir = path.join(process.cwd(), ".github", "workflows");
  return fs
    .readdirSync(dir)
    .filter((name) => name.endsWith(".yml"))
    .sort()
    .map((name) => ({
      label: `.github/workflows/${name}`,
      source: fs.readFileSync(path.join(dir, name), "utf8"),
    }));
}

test("optional dependencies are derived from napi targets", () => {
  assert.deepEqual(
    optionalDependencyNamesForTargets("@aleburato/primeval", [
      "aarch64-apple-darwin",
      "x86_64-apple-darwin",
      "aarch64-unknown-linux-gnu",
      "x86_64-unknown-linux-gnu",
      "x86_64-pc-windows-msvc",
    ]),
    [
      "@aleburato/primeval-darwin-arm64",
      "@aleburato/primeval-darwin-x64",
      "@aleburato/primeval-linux-arm64-gnu",
      "@aleburato/primeval-linux-x64-gnu",
      "@aleburato/primeval-win32-x64-msvc",
    ],
  );
});

test("release matrix runners are derived from napi targets", () => {
  assert.deepEqual(releaseMatrixForTargets(["x86_64-unknown-linux-gnu"]), {
    include: [
      {
        runner: "ubuntu-latest",
        target: "x86_64-unknown-linux-gnu",
      },
    ],
  });
});

test("package metadata validation rejects drift between targets and optional dependencies", () => {
  assert.throws(
    () =>
      validatePackageMetadata({
        name: "@aleburato/primeval",
        version: "0.1.0",
        optionalDependencies: {
          "@aleburato/primeval-linux-x64-gnu": "0.1.0",
        },
        napi: {
          targets: [
            "aarch64-apple-darwin",
            "x86_64-apple-darwin",
            "aarch64-unknown-linux-gnu",
            "x86_64-unknown-linux-gnu",
            "x86_64-pc-windows-msvc",
          ],
        },
      }),
    /optionalDependencies/i,
  );
});

test("package-lock validation rejects stale optional dependency versions", () => {
  const pkg = {
    name: "@aleburato/primeval",
    version: "0.1.1",
    optionalDependencies: {
      "@aleburato/primeval-darwin-arm64": "0.1.1",
      "@aleburato/primeval-darwin-x64": "0.1.1",
    },
    napi: {
      targets: ["aarch64-apple-darwin", "x86_64-apple-darwin"],
    },
  };

  assert.throws(
    () =>
      validatePackageLock(pkg, {
        name: "@aleburato/primeval",
        version: "0.1.1",
        packages: {
          "": {
            name: "@aleburato/primeval",
            version: "0.1.1",
            optionalDependencies: {
              "@aleburato/primeval-darwin-arm64": "0.1.0",
              "@aleburato/primeval-darwin-x64": "0.1.0",
            },
          },
          "node_modules/@aleburato/primeval-darwin-arm64": {
            version: "0.1.0",
          },
          "node_modules/@aleburato/primeval-darwin-x64": {
            version: "0.1.0",
          },
        },
      }),
    /package-lock/i,
  );
});

test("package version updates rewrite optional dependency versions from napi targets", () => {
  const pkg = updatePackageVersion(
    {
      name: "@aleburato/primeval",
      version: "0.1.0",
      optionalDependencies: {
        "@aleburato/primeval-linux-x64-gnu": "0.1.0",
      },
      napi: {
        targets: [
          "aarch64-apple-darwin",
          "x86_64-apple-darwin",
          "aarch64-unknown-linux-gnu",
          "x86_64-unknown-linux-gnu",
          "x86_64-pc-windows-msvc",
        ],
      },
      devDependencies: {
        typescript: "^5.5.4",
      },
    },
    "0.1.1",
  );

  assert.equal(pkg.version, "0.1.1");
  assert.deepEqual(pkg.optionalDependencies, {
    "@aleburato/primeval-darwin-arm64": "0.1.1",
    "@aleburato/primeval-darwin-x64": "0.1.1",
    "@aleburato/primeval-linux-arm64-gnu": "0.1.1",
    "@aleburato/primeval-linux-x64-gnu": "0.1.1",
    "@aleburato/primeval-win32-x64-msvc": "0.1.1",
  });
  assert.deepEqual(pkg.devDependencies, {
    typescript: "^5.5.4",
  });
});

test("binding loader only references declared optional dependency packages", () => {
  const pkg = JSON.parse(fs.readFileSync(path.join(process.cwd(), "package.json"), "utf8"));
  const bindingSource = fs.readFileSync(path.join(process.cwd(), "binding.js"), "utf8");
  const referencedPackages = [
    ...new Set(
      [...bindingSource.matchAll(/require\('(@aleburato\/primeval[^']+)'\)/g)].map(
        ([, packageName]) => packageName,
      ),
    ),
  ].sort();

  assert.deepEqual(referencedPackages, Object.keys(pkg.optionalDependencies).sort());
});

test("package scripts regenerate the binding loader before packing", () => {
  const pkg = JSON.parse(fs.readFileSync(path.join(process.cwd(), "package.json"), "utf8"));

  assert.match(pkg.scripts.prepack, /prepare:package/);
  assert.match(pkg.scripts["prepare:package"], /generate:binding/);
  assert.match(pkg.scripts["build:node"], /generate:binding/);
});

test("rust-toolchain.toml is the only rust toolchain pin", () => {
  const rustToolchain = fs.readFileSync(path.join(process.cwd(), "rust-toolchain.toml"), "utf8");
  const contributing = fs.readFileSync(path.join(process.cwd(), "CONTRIBUTING.md"), "utf8");
  const workflows = readWorkflows();
  assert.ok(workflows.length > 0, "missing workflows");

  for (const { label, source } of workflows) {
    assert.doesNotMatch(source, /dtolnay\/rust-toolchain/, `${label} uses dtolnay/rust-toolchain`);
    assert.doesNotMatch(source, /^\s*toolchain:/m, `${label} pins a toolchain`);
  }

  const version = parseRustToolchainVersion(rustToolchain);
  assert.match(contributing, new RegExp(`Rust stable \`${version.replace(/\./g, "\\.")}\``));
});

test("workflow actions are pinned by full sha or digest with a version comment", () => {
  const workflows = readWorkflows();
  assert.ok(workflows.length > 0, "missing workflows");

  const pinned = [
    /^\.\/\S+$/,
    /^[\w.-]+\/[\w./-]+@[0-9a-f]{40} # v\d+\.\d+\.\d+$/,
    /^docker:\/\/\S+@sha256:[0-9a-f]{64} # \S+$/,
  ];
  for (const { label, source } of workflows) {
    const uses = [...source.matchAll(/^\s*(?:-\s+)?uses:\s*(.+?)\s*$/gm)].map(([, ref]) => ref);
    for (const ref of uses) {
      assert.ok(
        pinned.some((pattern) => pattern.test(ref)),
        `${label}: unpinned uses: ${ref}`,
      );
    }
  }
});

test("package exposes canonical verify scripts and CI uses the split workflow", () => {
  const pkg = JSON.parse(fs.readFileSync(path.join(process.cwd(), "package.json"), "utf8"));
  const qualityWorkflow = fs.readFileSync(
    path.join(process.cwd(), ".github", "workflows", "quality.yml"),
    "utf8",
  );
  const contributing = fs.readFileSync(path.join(process.cwd(), "CONTRIBUTING.md"), "utf8");
  const releasing = fs.readFileSync(path.join(process.cwd(), "RELEASING.md"), "utf8");

  assert.equal(typeof pkg.scripts.verify, "string");
  assert.equal(typeof pkg.scripts["verify:rust"], "string");
  assert.equal(typeof pkg.scripts["verify:node"], "string");
  assert.equal(typeof pkg.scripts["verify:pack"], "string");

  assert.match(pkg.scripts["verify:rust"], /cargo fmt --check/);
  assert.match(pkg.scripts["verify:rust"], /cargo clippy --all-targets -- -D warnings/);
  assert.match(pkg.scripts["verify:rust"], /cargo test/);

  assert.equal(pkg.scripts.lint, "biome check --error-on-warnings");
  assert.match(pkg.scripts["verify:node"], /^npm run lint && /);
  assert.match(pkg.scripts["verify:node"], /npm run check:napi-targets/);
  assert.match(pkg.scripts["verify:node"], /npm run typecheck/);
  assert.match(pkg.scripts["verify:node"], /npm run build/);
  assert.match(pkg.scripts["verify:node"], /npm run build:node/);
  assert.match(pkg.scripts["verify:node"], /npm test/);
  assert.match(pkg.scripts["verify:node"], /npm run test:tooling/);

  assert.match(pkg.scripts["verify:pack"], /npm run prepare:package/);
  assert.match(pkg.scripts["verify:pack"], /npm pack --dry-run --ignore-scripts/);

  assert.match(pkg.scripts.verify, /npm run verify:rust/);
  assert.match(pkg.scripts.verify, /npm run verify:node/);
  assert.match(pkg.scripts.verify, /npm run verify:pack/);

  assert.match(qualityWorkflow, /npm run verify:rust/);
  assert.match(qualityWorkflow, /npm run verify:node/);
  assert.match(qualityWorkflow, /npm run verify:pack/);
  assert.match(contributing, /npm run verify/);
  assert.match(releasing, /npm run verify/);
});

test("quality workflow runs once per pull request push", () => {
  const qualityWorkflow = fs.readFileSync(
    path.join(process.cwd(), ".github", "workflows", "quality.yml"),
    "utf8",
  );
  const triggers = qualityWorkflow.match(/^on:\n((?:[ \t]+.*\n)+)/m);
  assert.ok(triggers, "missing on: block in .github/workflows/quality.yml");

  assert.match(triggers[1], /^ {2}pull_request:\s*$/m);
  assert.match(triggers[1], /^ {2}workflow_call:\s*$/m);
  assert.match(
    triggers[1],
    /^ {2}push:\n {4}branches: \[main\]\n/m,
    "push must be limited to main, or PR branches and release tags run the workflow twice",
  );
});

test("node support window matches engines and the CI matrix", () => {
  const pkg = JSON.parse(fs.readFileSync(path.join(process.cwd(), "package.json"), "utf8"));
  assert.equal(pkg.engines?.node, ">=22.12", "require(esm) in the loader needs Node 22.12+");

  const qualityWorkflow = fs.readFileSync(
    path.join(process.cwd(), ".github", "workflows", "quality.yml"),
    "utf8",
  );
  const packageChecks = qualityWorkflow.match(/^ {2}package-checks:\n((?:(?: {4,}.*)?\n)+)/m);
  assert.ok(packageChecks, "missing package-checks job in .github/workflows/quality.yml");
  const matrix = packageChecks[1].match(/^ {8}node-version: \[([^\]]*)\]\s*$/m);
  assert.ok(matrix, "missing node-version matrix in package-checks job");
  assert.deepEqual(
    matrix[1].split(",").map((version) => version.trim()),
    ["22", "24", "26"],
  );
});

test("typescript build succeeds without generated binding files", () => {
  const fixtureDir = fs.mkdtempSync(path.join(os.tmpdir(), "primeval-tsc-"));

  try {
    for (const file of ["package.json", "tsconfig.json"]) {
      fs.copyFileSync(path.join(process.cwd(), file), path.join(fixtureDir, file));
    }

    const fixtureSrcDir = path.join(fixtureDir, "src");
    fs.mkdirSync(fixtureSrcDir, { recursive: true });
    for (const file of fs.readdirSync(path.join(process.cwd(), "src"))) {
      if (file.endsWith(".ts")) {
        fs.copyFileSync(path.join(process.cwd(), "src", file), path.join(fixtureSrcDir, file));
      }
    }

    fs.symlinkSync(path.join(process.cwd(), "node_modules"), path.join(fixtureDir, "node_modules"));

    const result = spawnSync(
      process.execPath,
      [path.join(process.cwd(), "node_modules", "typescript", "bin", "tsc"), "-p", fixtureDir],
      { encoding: "utf8" },
    );

    assert.equal(
      result.status,
      0,
      `tsc failed without generated bindings:\n${result.stdout}\n${result.stderr}`,
    );
    assert.ok(fs.existsSync(path.join(fixtureDir, "dist", "index.js")));
  } finally {
    fs.rmSync(fixtureDir, { recursive: true, force: true });
  }
});

test("artifact verification accepts matching native payloads", () => {
  const artifactsDir = fs.mkdtempSync(path.join(os.tmpdir(), "primeval-artifacts-"));
  try {
    fs.writeFileSync(
      path.join(artifactsDir, "aleburato-primeval-linux-x64-gnu.primeval-node.linux-x64-gnu.node"),
      Buffer.alloc(0),
    );

    verifyArtifacts(artifactsDir, {
      name: "@aleburato/primeval",
      version: "0.1.0",
      optionalDependencies: {
        "@aleburato/primeval-linux-x64-gnu": "0.1.0",
      },
      napi: {
        targets: ["x86_64-unknown-linux-gnu"],
      },
    });
  } finally {
    fs.rmSync(artifactsDir, { recursive: true, force: true });
  }
});

test("artifact verification rejects missing native payloads", () => {
  const artifactsDir = fs.mkdtempSync(path.join(os.tmpdir(), "primeval-artifacts-"));
  try {
    fs.writeFileSync(
      path.join(artifactsDir, "aleburato-primeval-linux-x64-gnu.txt"),
      "placeholder",
    );

    assert.throws(
      () =>
        verifyArtifacts(artifactsDir, {
          name: "@aleburato/primeval",
          version: "0.1.0",
          optionalDependencies: {
            "@aleburato/primeval-linux-x64-gnu": "0.1.0",
          },
          napi: {
            targets: ["x86_64-unknown-linux-gnu"],
          },
        }),
      /missing \.node payloads/i,
    );
  } finally {
    fs.rmSync(artifactsDir, { recursive: true, force: true });
  }
});

test("artifact verification accepts napi package directories", () => {
  const artifactsDir = fs.mkdtempSync(path.join(os.tmpdir(), "primeval-artifacts-"));
  const packageDir = path.join(artifactsDir, "npm", "linux-x64-gnu");

  try {
    fs.mkdirSync(packageDir, { recursive: true });
    fs.writeFileSync(
      path.join(packageDir, "package.json"),
      JSON.stringify({
        name: "@aleburato/primeval-linux-x64-gnu",
        version: "0.1.0",
      }),
    );
    fs.writeFileSync(path.join(packageDir, "primeval-node.linux-x64-gnu.node"), Buffer.alloc(0));

    verifyArtifacts(artifactsDir, {
      name: "@aleburato/primeval",
      version: "0.1.0",
      optionalDependencies: {
        "@aleburato/primeval-linux-x64-gnu": "0.1.0",
      },
      napi: {
        targets: ["x86_64-unknown-linux-gnu"],
      },
    });
  } finally {
    fs.rmSync(artifactsDir, { recursive: true, force: true });
  }
});

test("artifact verification rejects wrong target payloads", () => {
  const artifactsDir = fs.mkdtempSync(path.join(os.tmpdir(), "primeval-artifacts-"));
  const packageDir = path.join(artifactsDir, "npm", "linux-x64-gnu");

  try {
    fs.mkdirSync(packageDir, { recursive: true });
    fs.writeFileSync(
      path.join(packageDir, "package.json"),
      JSON.stringify({
        name: "@aleburato/primeval-linux-x64-gnu",
        version: "0.1.0",
      }),
    );
    fs.writeFileSync(path.join(packageDir, "primeval-node.win32-x64-msvc.node"), Buffer.alloc(0));

    assert.throws(
      () =>
        verifyArtifacts(artifactsDir, {
          name: "@aleburato/primeval",
          version: "0.1.0",
          optionalDependencies: {
            "@aleburato/primeval-linux-x64-gnu": "0.1.0",
          },
          napi: {
            targets: ["x86_64-unknown-linux-gnu"],
          },
        }),
      /missing \.node payloads/i,
    );
  } finally {
    fs.rmSync(artifactsDir, { recursive: true, force: true });
  }
});
