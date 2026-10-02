import assert from "node:assert/strict";
import fs from "node:fs";
import path from "node:path";
import { test } from "node:test";
import vm from "node:vm";

import { renderBindingLoader } from "../../scripts/generate-binding.mjs";
import { runtimeTargetForTarget } from "../../scripts/napi-targets.mjs";

const packageJson = JSON.parse(fs.readFileSync(path.join(process.cwd(), "package.json"), "utf8"));

function runGeneratedBindingLoader({ processMock, requireImpl }) {
  const source = renderBindingLoader(packageJson)
    .replace(
      "import { createRequire } from 'node:module'\n\nconst require = createRequire(import.meta.url)\n",
      "const require = globalThis.__require\n",
    )
    .replace(
      "export const { NativeTask, startApproximate } = loadNativeBinding()\n",
      "globalThis.__bindingExports = loadNativeBinding()\n",
    );

  const context = vm.createContext({
    Error,
    process: processMock,
  });
  context.globalThis = context;
  context.__require = requireImpl;

  return () => vm.runInContext(source, context, { filename: "binding.js" });
}

function glibcProcess(arch = "x64") {
  return {
    platform: "linux",
    arch,
    report: {
      getReport() {
        return {
          header: { glibcVersionRuntime: "2.39" },
          sharedObjects: ["/lib64/libc.so.6"],
        };
      },
    },
  };
}

function muslProcess(arch = "x64") {
  return {
    platform: "linux",
    arch,
    report: {
      getReport() {
        return {
          header: {},
          sharedObjects: ["/lib/ld-musl-x86_64.so.1"],
        };
      },
    },
  };
}

test("binding loader loads the local artifact of every declared target on its own platform", () => {
  for (const target of packageJson.napi.targets) {
    const { platform, arch, suffix } = runtimeTargetForTarget(target);
    const requireCalls = [];
    const run = runGeneratedBindingLoader({
      processMock: platform === "linux" ? glibcProcess(arch) : { platform, arch },
      requireImpl(specifier) {
        requireCalls.push(specifier);
        return { NativeTask: class {}, startApproximate() {} };
      },
    });

    assert.doesNotThrow(run, `${target} should load`);
    assert.deepEqual(requireCalls, [`./primeval-node.${suffix}.node`], target);
  }
});

test("binding loader rejects linux musl before attempting gnu artifacts", () => {
  const requireCalls = [];
  const run = runGeneratedBindingLoader({
    processMock: muslProcess(),
    requireImpl(specifier) {
      requireCalls.push(specifier);
      throw new Error(`unexpected require: ${specifier}`);
    },
  });

  assert.throws(run, (error) => {
    assert.match(error.message, /Unsupported Linux runtime/i);
    assert.match(error.message, /musl/i);
    assert.match(error.message, /GNU libc/i);
    return true;
  });

  assert.deepEqual(requireCalls, []);
});

test("binding loader reports both local and package load failures with install guidance", () => {
  const requireCalls = [];
  const run = runGeneratedBindingLoader({
    processMock: glibcProcess(),
    requireImpl(specifier) {
      requireCalls.push(specifier);
      throw new Error(`cannot load ${specifier}`);
    },
  });

  assert.throws(run, (error) => {
    assert.match(error.message, /Failed to load native binding/i);
    assert.match(error.message, /optional dependencies/i);
    assert.match(error.message, /--omit=optional/i);
    assert.match(error.message, /Node 22\.12\+/);
    assert.match(error.message, /Local file error:/i);
    assert.match(error.message, /Package error:/i);
    return true;
  });

  assert.deepEqual(requireCalls, [
    "./primeval-node.linux-x64-gnu.node",
    "@aleburato/primeval-linux-x64-gnu",
  ]);
});

test("binding loader excludes network data while reading the linux process report, then restores the setting", () => {
  const processMock = glibcProcess();
  processMock.report.excludeNetwork = false;
  const excludeNetworkAtReport = [];
  const getReport = processMock.report.getReport;
  processMock.report.getReport = () => {
    excludeNetworkAtReport.push(processMock.report.excludeNetwork);
    return getReport();
  };
  const run = runGeneratedBindingLoader({
    processMock,
    requireImpl: () => ({ NativeTask: class {}, startApproximate() {} }),
  });

  run();

  assert.deepEqual(excludeNetworkAtReport, [true]);
  assert.equal(processMock.report.excludeNetwork, false);
});

test("binding loader restores the report network setting when reading the report throws", () => {
  const processMock = glibcProcess();
  processMock.report.excludeNetwork = false;
  processMock.report.getReport = () => {
    throw new Error("report failed");
  };
  const run = runGeneratedBindingLoader({
    processMock,
    requireImpl: () => ({ NativeTask: class {}, startApproximate() {} }),
  });

  assert.throws(run, /report failed/);
  assert.equal(processMock.report.excludeNetwork, false);
});
