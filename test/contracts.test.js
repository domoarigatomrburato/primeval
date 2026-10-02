import assert from "node:assert/strict";
import fs from "node:fs";
import path from "node:path";
import { test } from "node:test";

const repoRoot = process.cwd();

function readRepoFile(...segments) {
  return fs.readFileSync(path.join(repoRoot, ...segments), "utf8");
}

function parseStringArray(source, label) {
  const match = source.match(new RegExp(`const ${label}:[^=]+= \\[(.*?)\\];`, "s"));
  assert.ok(match, `missing ${label}`);
  return [...match[1].matchAll(/"([^"]+)"/g)].map((entry) => entry[1]);
}

function parseRustVariants(source, enumName) {
  const match = source.match(
    new RegExp(
      `impl ${enumName} \\{\\s+#[^\\n]+\\s+pub const fn variants\\(\\) -> &'static \\[&'static str\\] \\{\\s+&\\[(.*?)\\]\\s+\\}`,
      "s",
    ),
  );
  assert.ok(match, `missing ${enumName}::variants`);
  return [...match[1].matchAll(/"([^"]+)"/g)].map((entry) => entry[1]);
}

function parseRustRenderDefaults(source) {
  const block = source.match(/impl Default for RenderOptions \{.*?Self \{(.*?)\n\s*}\n\s*}/s);
  assert.ok(block, "missing RenderOptions::default");

  const patterns = {
    count: /count:\s*(\d+),/,
    shape: /shape:\s*ShapeKind::([A-Za-z]+),/,
    background: /background:\s*BackgroundOption::([A-Za-z]+),/,
    resizeInput: /resize_input:\s*(\d+),/,
    outputSize: /output_size:\s*(\d+),/,
  };

  const values = Object.fromEntries(
    Object.entries(patterns).map(([key, pattern]) => {
      const match = block[1].match(pattern);
      assert.ok(match, `missing Rust default for ${key}`);
      return [key, match[1]];
    }),
  );

  return {
    count: Number(values.count),
    shape: values.shape.toLowerCase(),
    alpha: (() => {
      const alphaMatch = block[1].match(/alpha:\s*Alpha::Auto,/);
      assert.ok(alphaMatch, "missing Rust default for alpha");
      return "auto";
    })(),
    background: values.background.toLowerCase(),
    resizeInput: Number(values.resizeInput),
    outputSize: Number(values.outputSize),
  };
}

function parseAlphaMessage(source, fileLabel) {
  const match = source.match(/alpha must be[^"\n]*/);
  assert.ok(match, `missing alpha validation message in ${fileLabel}`);
  return match[0];
}

test("wrapper shape vocabulary mirrors Rust", () => {
  const tsSource = readRepoFile("src", "index.ts");
  const rustSource = readRepoFile("crates", "primeval-core", "src", "shapes.rs");

  assert.deepEqual(
    parseStringArray(tsSource, "VALID_SHAPES"),
    parseRustVariants(rustSource, "ShapeKind"),
  );
});

test("wrapper output vocabulary mirrors Rust", () => {
  const tsSource = readRepoFile("src", "index.ts");
  const rustSource = readRepoFile("crates", "primeval-render", "src", "output.rs");

  assert.deepEqual(
    parseStringArray(tsSource, "VALID_OUTPUTS"),
    parseRustVariants(rustSource, "OutputFormat"),
  );
});

test("wrapper leaves render defaults to Rust", () => {
  const tsSource = readRepoFile("src", "index.ts");
  const rustSource = readRepoFile("crates", "primeval-render", "src", "lib.rs");

  const rustDefaults = parseRustRenderDefaults(rustSource);

  for (const value of Object.values(rustDefaults)) {
    assert.doesNotMatch(
      tsSource,
      new RegExp(`\\?\\? ${typeof value === "number" ? value : `"${value}"`}`),
    );
  }
});

test("alpha validation message is aligned across surfaces", () => {
  const messages = [
    parseAlphaMessage(readRepoFile("src", "index.ts"), "src/index.ts"),
    parseAlphaMessage(readRepoFile("src", "cli.ts"), "src/cli.ts"),
    parseAlphaMessage(
      readRepoFile("crates", "primeval-core", "src", "alpha.rs"),
      "crates/primeval-core/src/alpha.rs",
    ),
  ];

  assert.deepEqual(
    messages,
    new Array(messages.length).fill("alpha must be auto or an integer 1..255"),
  );
});

test("binding uses shared Rust option parsers and render defaults", () => {
  const bindingSource = readRepoFile("binding", "src", "binding.rs");

  assert.match(bindingSource, /parse::<OutputFormat>/);
  assert.match(bindingSource, /parse::<ShapeKind>/);
  assert.match(bindingSource, /parse::<Alpha>/);
  assert.match(bindingSource, /set_number\(RenderOption::Alpha,/);
  assert.match(bindingSource, /set_number\(RenderOption::Count,/);
  assert.match(bindingSource, /set_number\(RenderOption::Seed,/);
  assert.match(bindingSource, /set_number\(RenderOption::ResizeInput,/);
  assert.match(bindingSource, /set_number\(RenderOption::OutputSize,/);
  assert.match(bindingSource, /parse::<BackgroundOption>/);
  assert.match(bindingSource, /RenderOptions::default\(\)\.merge\(/);
  assert.doesNotMatch(bindingSource, /unwrap_or\(defaults/);
});

test("binding depends on primeval-render only", () => {
  const manifest = readRepoFile("binding", "Cargo.toml");

  assert.match(manifest, /^primeval-render = /m);
  assert.doesNotMatch(manifest, /primeval-core/);
});

test("obsolete rust cli tree is absent", () => {
  assert.equal(fs.existsSync(path.join(repoRoot, "crates", "primeval-cli")), false);
});

test("package test script exercises the native package path", () => {
  const pkg = JSON.parse(readRepoFile("package.json"));

  assert.match(pkg.scripts.test, /test\/native\.test\.js/);
});
