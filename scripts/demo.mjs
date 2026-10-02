// Assembles the demo (demo/) into a static site and serves it locally.
//
//   node scripts/demo.mjs build              writes site/
//   node scripts/demo.mjs serve [--isolated] builds, then serves site/
//
// The site is the demo's files plus the package's browser files at the same
// relative layout as in the package (`dist/`, `wasm/single/`,
// `wasm/threaded/`), so the demo imports the package by a relative URL, and
// the sample images from docs/readme/originals/. `serve` sends no COOP/COEP
// headers by default, so the demo's service worker (demo/coi-sw.js) provides
// the isolation, as on GitHub Pages; `--isolated` sends them from the server.
import fs from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";

const REPO_ROOT = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..");

/** The port `serve` listens on, on 127.0.0.1. */
export const DEMO_PORT = 8417;

const BROWSER_ENTRY = "dist/browser.js";
const WASM_FILES = ["primeval.js", "primeval_bg.wasm"];
const VARIANTS = ["single", "threaded"];
const SAMPLES_DIR = "docs/readme/originals";

/**
 * The `dist/` files the browser entry loads: its static imports and the
 * workers it starts, followed transitively. Paths outside `dist/` (the wasm
 * glue) are copied with `wasm/`.
 */
function browserDistFiles(repoRoot) {
  const distDir = path.join(repoRoot, "dist");
  const found = new Set();
  const pending = [path.basename(BROWSER_ENTRY)];
  while (pending.length > 0) {
    const name = pending.pop();
    if (found.has(name)) {
      continue;
    }
    found.add(name);
    const source = fs.readFileSync(path.join(distDir, name), "utf8");
    const specifiers = source.matchAll(/(?:\bfrom\s+|new URL\(\s*)"\.\/([\w.-]+\.js)"/g);
    for (const [, specifier] of specifiers) {
      pending.push(specifier);
    }
  }
  return [...found].sort();
}

function copyTree(from, to) {
  fs.cpSync(from, to, { recursive: true });
}

/**
 * Writes the site to `siteDir` (default `site/`), replacing it. Throws, before
 * writing anything, if the package's browser files are not built.
 */
export function buildSite({ repoRoot = REPO_ROOT, siteDir = path.join(repoRoot, "site") } = {}) {
  const required = [
    BROWSER_ENTRY,
    ...VARIANTS.flatMap((variant) => WASM_FILES.map((file) => `wasm/${variant}/${file}`)),
  ];
  const missing = required.filter((file) => !fs.existsSync(path.join(repoRoot, file)));
  if (missing.length > 0) {
    throw new Error(
      `demo:build needs the package's browser files, missing: ${missing.join(", ")}. ` +
        "Run `npm run build` and `npm run build:wasm` first.",
    );
  }

  fs.rmSync(siteDir, { recursive: true, force: true });
  copyTree(path.join(repoRoot, "demo"), siteDir);

  fs.mkdirSync(path.join(siteDir, "dist"), { recursive: true });
  for (const file of browserDistFiles(repoRoot)) {
    fs.copyFileSync(path.join(repoRoot, "dist", file), path.join(siteDir, "dist", file));
  }

  for (const variant of VARIANTS) {
    const from = path.join(repoRoot, "wasm", variant);
    const to = path.join(siteDir, "wasm", variant);
    fs.mkdirSync(to, { recursive: true });
    for (const file of WASM_FILES) {
      fs.copyFileSync(path.join(from, file), path.join(to, file));
    }
    if (fs.existsSync(path.join(from, "snippets"))) {
      copyTree(path.join(from, "snippets"), path.join(to, "snippets"));
    }
  }

  const samples = path.join(siteDir, "samples");
  fs.mkdirSync(samples, { recursive: true });
  for (const file of fs.readdirSync(path.join(repoRoot, SAMPLES_DIR))) {
    if (file.endsWith(".jpg")) {
      fs.copyFileSync(path.join(repoRoot, SAMPLES_DIR, file), path.join(samples, file));
    }
  }
  return siteDir;
}

async function main(args) {
  const [command, ...flags] = args;
  const unknown = flags.filter((flag) => flag !== "--isolated");
  if ((command !== "build" && command !== "serve") || unknown.length > 0) {
    console.error("usage: node scripts/demo.mjs build | serve [--isolated]");
    process.exitCode = 2;
    return;
  }
  const siteDir = buildSite();
  console.log(`demo: wrote ${path.relative(REPO_ROOT, siteDir)}/`);
  if (command === "serve") {
    // The test server, reused: same MIME types and header switch.
    const { startServer } = await import("../test/browser/server.js");
    const isolated = flags.includes("--isolated");
    const { origin } = await startServer({ isolated, root: siteDir, port: DEMO_PORT });
    console.log(
      `demo: serving on ${origin}/ ${isolated ? "with COOP/COEP headers" : "without COOP/COEP headers (the service worker isolates the page)"}; Ctrl-C stops`,
    );
  }
}

if (
  process.argv[1] !== undefined &&
  path.resolve(process.argv[1]) === fileURLToPath(import.meta.url)
) {
  main(process.argv.slice(2)).catch((error) => {
    console.error(error instanceof Error ? error.message : error);
    process.exitCode = 1;
  });
}
