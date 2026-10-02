// The demo's site assembly (scripts/demo.mjs) and its GitHub Pages workflow.
// The demo itself runs in test/browser/demo.test.js, which needs the wasm
// builds; these checks do not.
import assert from "node:assert/strict";
import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import { test } from "node:test";

import { buildSite } from "../../scripts/demo.mjs";

const repoRoot = process.cwd();
const readRepoFile = (...parts) => fs.readFileSync(path.join(repoRoot, ...parts), "utf8");

test("demo:build fails clearly when dist/ or wasm/ is missing", () => {
  const emptyRoot = fs.mkdtempSync(path.join(os.tmpdir(), "primeval-demo-root-"));
  const siteDir = path.join(emptyRoot, "site");
  try {
    assert.throws(
      () => buildSite({ repoRoot: emptyRoot, siteDir }),
      (error) => {
        assert.match(error.message, /dist\/browser\.js/);
        assert.match(error.message, /wasm\/single\/primeval_bg\.wasm/);
        assert.match(error.message, /wasm\/threaded\/primeval_bg\.wasm/);
        assert.match(error.message, /npm run build\b/);
        assert.match(error.message, /npm run build:wasm/);
        return true;
      },
    );
    assert.equal(fs.existsSync(siteDir), false, "no partial site is written");
  } finally {
    fs.rmSync(emptyRoot, { recursive: true, force: true });
  }
});

test("the demo scripts and the generated site are wired up", () => {
  const { scripts } = JSON.parse(readRepoFile("package.json"));
  assert.equal(scripts["demo:build"], "node scripts/demo.mjs build");
  assert.equal(scripts.demo, "node scripts/demo.mjs serve");
  assert.match(readRepoFile(".gitignore"), /^\/site\/$/m);
  assert.match(readRepoFile("AGENTS.md"), /`site\/`/);
});

test("the Pages workflow builds the site without caches and deploys it", () => {
  const workflow = readRepoFile(".github", "workflows", "pages.yml");

  assert.match(workflow, /^on:\n {2}push:\n {4}branches: \[main\]\n {2}workflow_dispatch:\n/m);
  assert.match(workflow, /^concurrency:\n {2}group: pages\n {2}cancel-in-progress: false\n/m);
  assert.match(workflow, /^permissions:\n {2}contents: read\n/m);
  assert.doesNotMatch(workflow, /cache: npm|rust-cache|actions\/cache/);
  assert.match(workflow, /package-manager-cache: false/);
  assert.match(workflow, /persist-credentials: false/);

  const build = workflow.slice(workflow.indexOf("  build:"), workflow.indexOf("  deploy:"));
  const order = [
    "- run: npm ci\n",
    "- run: rustup toolchain install\n",
    "- run: node scripts/build-wasm.mjs install-nightly\n",
    '- run: cargo install wasm-bindgen-cli --locked --version "$(node scripts/build-wasm.mjs wasm-bindgen-version)"\n',
    "- run: npm run build\n",
    "- run: npm run build:wasm\n",
    "- run: npm run demo:build\n",
    "uses: actions/upload-pages-artifact@",
  ];
  let last = -1;
  for (const step of order) {
    const index = build.indexOf(step);
    assert.ok(index > last, `build job: ${step.trim()} missing or out of order`);
    last = index;
  }
  assert.match(build, /path: site\//);

  const deploy = workflow.slice(workflow.indexOf("  deploy:"));
  assert.match(deploy, /needs: build/);
  assert.match(deploy, /permissions:\n {6}pages: write\n {6}id-token: write\n/);
  assert.match(deploy, /environment:\n {6}name: github-pages\n/);
  assert.match(deploy, /uses: actions\/deploy-pages@/);
});
