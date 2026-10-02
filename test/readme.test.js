import assert from "node:assert/strict";
import { spawn } from "node:child_process";
import fs from "node:fs";
import path from "node:path";
import { test } from "node:test";

const repoRoot = process.cwd();
const fixture = "docs/readme/originals/monalisa.jpg";

// The ```js blocks of the README, with the placeholder path replaced by the
// fixture and step counts capped at 20 to keep the run short (the abort
// example stops at step 10). A block without imports continues an earlier
// one, so it gets the imports that block had.
function readmeExamples() {
  const readme = fs.readFileSync(path.join(repoRoot, "README.md"), "utf8");
  return [...readme.matchAll(/^```js\n(.*?)^```$/gms)].map(([, code]) => {
    const source = code
      .replaceAll('"photo.jpg"', JSON.stringify(fixture))
      .replace(/\bcount: (\d+)/g, (_, count) => `count: ${Math.min(Number(count), 20)}`);
    return /^import /m.test(source)
      ? source
      : [
          'import { approximate } from "@aleburato/primeval";',
          'import { readFile } from "node:fs/promises";',
          source,
        ].join("\n");
  });
}

function runModule(source) {
  return new Promise((resolve, reject) => {
    const child = spawn(process.execPath, ["--input-type=module", "-e", source], {
      cwd: repoRoot,
      stdio: ["ignore", "pipe", "pipe"],
    });
    let stdout = "";
    let stderr = "";
    child.stdout.setEncoding("utf8").on("data", (chunk) => {
      stdout += chunk;
    });
    child.stderr.setEncoding("utf8").on("data", (chunk) => {
      stderr += chunk;
    });
    child.on("error", reject);
    child.on("close", (status) => resolve({ status, stdout, stderr }));
  });
}

test("the README API examples run against the built package", async () => {
  const examples = readmeExamples();
  assert.ok(examples.length >= 5, `found ${examples.length} examples`);

  const results = await Promise.all(examples.map(runModule));

  results.forEach(({ status, stderr }, index) => {
    assert.equal(status, 0, `example ${index + 1}:\n${examples[index]}\n${stderr}`);
    assert.equal(stderr, "", `example ${index + 1}`);
  });
  const output = (marker) => results[examples.findIndex((code) => code.includes(marker))].stdout;
  assert.match(output('output: "svg"'), /^svg \d+ \d+\n<svg/);
  assert.match(output("toDataUri(result)"), /^data:image\/png;base64,/);
  assert.equal(output("controller.abort()"), "render aborted\n");
});
