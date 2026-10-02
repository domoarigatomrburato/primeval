import assert from "node:assert/strict";
import { spawnSync } from "node:child_process";
import path from "node:path";
import { test } from "node:test";

const repoRoot = process.cwd();

// Needs `binding.d.ts` from `npm run build:node` and `dist/` from `npm run
// build`, so it runs in `npm test`, not in the clean-checkout typecheck.
function typeCheck(tsconfig) {
  return spawnSync(
    process.execPath,
    [
      path.join(repoRoot, "node_modules", "typescript", "bin", "tsc"),
      "-p",
      path.join(repoRoot, "test", "types", tsconfig),
    ],
    { cwd: repoRoot, encoding: "utf8" },
  );
}

test("hand-written native types and public declarations type-check", () => {
  const result = typeCheck("tsconfig.json");
  assert.equal(result.status, 0, `${result.stdout}${result.stderr}`);
});

test("the browser entry's declarations type-check without Node types", () => {
  const result = typeCheck("tsconfig.browser.json");
  assert.equal(result.status, 0, `${result.stdout}${result.stderr}`);
});
