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

test("the README API examples read photo.jpg, not repository paths", () => {
  const readme = fs.readFileSync(path.join(repoRoot, "README.md"), "utf8");
  const blocks = [...readme.matchAll(/^```js\n(.*?)^```$/gms)].map(([, code]) => code);
  const reads = blocks.flatMap((code) =>
    [...code.matchAll(/readFile\(([^)]*)\)/g)].map(([, arg]) => arg),
  );

  assert.ok(reads.length >= 5, `found ${reads.length} readFile calls`);
  assert.deepEqual(new Set(reads), new Set(['"photo.jpg"']));
});

// Local `src` and `href` targets of a Markdown file, resolved from the
// repository root. External links and in-page anchors are skipped.
function localLinks(file) {
  const source = fs.readFileSync(path.join(repoRoot, file), "utf8");
  const targets = [
    ...[...source.matchAll(/\b(?:src|href)="([^"]+)"/g)].map(([, target]) => target),
    ...[...source.matchAll(/\]\(([^)\s]+)\)/g)].map(([, target]) => target),
  ];
  return targets
    .filter((target) => !/^(?:[a-z]+:|#)/.test(target))
    .map((target) => path.normalize(path.join(path.dirname(file), target.split("#")[0])));
}

function filesUnder(dir) {
  return fs
    .readdirSync(path.join(repoRoot, dir), { recursive: true, withFileTypes: true })
    .filter((entry) => entry.isFile() && !entry.name.startsWith("."))
    .map((entry) => path.relative(repoRoot, path.join(entry.parentPath, entry.name)));
}

test("the README and the gallery link only to files in the repository", () => {
  for (const file of ["README.md", "docs/gallery.md"]) {
    const links = localLinks(file);
    assert.ok(links.length > 0, `${file} has no local links`);
    for (const link of links) {
      assert.ok(fs.existsSync(path.join(repoRoot, link)), `${file} links to missing ${link}`);
    }
  }
});

test("the gallery shows every original and every generated image", () => {
  const linked = new Set([...localLinks("docs/gallery.md"), ...localLinks("README.md")]);
  const generated = [
    ...filesUnder("docs/readme/originals"),
    ...filesUnder("docs/images"),
    ...filesUnder("docs/readme/comparisons"),
  ];
  for (const file of generated) {
    assert.ok(linked.has(file), `${file} is not linked from the gallery or the README`);
  }
});

test("the Benchmarks section cites the runner and commit of each table", () => {
  const readme = fs.readFileSync(path.join(repoRoot, "README.md"), "utf8");
  const section = readme.split("\n## Benchmarks\n")[1].split("\n## ")[0];
  const [runner, versusGo] = section.split("\n### Compared with Go primitive\n");
  assert.match(runner, /cargo run --release -p primeval-render --example quality/);
  assert.match(runner, /commit `[0-9a-f]{7,}`/);
  assert.doesNotMatch(runner, /Go CLI|Go time|primitive/);
  assert.ok(versusGo, "the Go comparison is missing");
  assert.match(versusGo, /cargo run --release -p primeval-render --example versus_go/);
  assert.match(versusGo, /commit `[0-9a-f]{7,}`/);
});
