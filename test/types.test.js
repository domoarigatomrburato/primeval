import assert from "node:assert/strict";
import { spawnSync } from "node:child_process";
import path from "node:path";
import { test } from "node:test";

const repoRoot = process.cwd();

// Needs `binding.d.ts` from `npm run build:node` and `dist/` from `npm run
// build`, so it runs in `npm test`, not in the clean-checkout typecheck.
test("hand-written native types and public declarations type-check", () => {
  const result = spawnSync(
    process.execPath,
    [
      path.join(repoRoot, "node_modules", "typescript", "bin", "tsc"),
      "-p",
      path.join(repoRoot, "test", "types", "tsconfig.json"),
    ],
    { cwd: repoRoot, encoding: "utf8" },
  );

  assert.equal(result.status, 0, `${result.stdout}${result.stderr}`);
});
