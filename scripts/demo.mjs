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

import { OUTPUT_FILES, VARIANTS } from "./build-wasm.mjs";

const REPO_ROOT = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..");

/** The port `serve` listens on, on 127.0.0.1. */
const DEMO_PORT = 8417;

const BROWSER_ENTRY = "dist/browser.js";
const SAMPLES_DIR = "docs/readme/originals";

/**
 * Writes the site to `siteDir` (default `site/`), replacing it. Throws, before
 * writing anything, if the package's browser files are not built.
 */
export function buildSite({ repoRoot = REPO_ROOT, siteDir = path.join(repoRoot, "site") } = {}) {
  const required = [
    BROWSER_ENTRY,
    ...Object.values(VARIANTS).flatMap(({ outDir }) =>
      Object.values(OUTPUT_FILES).map((file) => `${outDir}/${file}`),
    ),
  ];
  const missing = required.filter((file) => !fs.existsSync(path.join(repoRoot, file)));
  if (missing.length > 0) {
    throw new Error(
      `demo:build needs the package's browser files, missing: ${missing.join(", ")}. ` +
        "Run `npm run build` and `npm run build:wasm` first.",
    );
  }

  fs.rmSync(siteDir, { recursive: true, force: true });
  fs.cpSync(path.join(repoRoot, "demo"), siteDir, { recursive: true });

  // Every module the package ships (`dist/*.js`); the page never loads the
  // Node-only ones.
  fs.mkdirSync(path.join(siteDir, "dist"), { recursive: true });
  for (const file of fs.readdirSync(path.join(repoRoot, "dist"))) {
    if (file.endsWith(".js")) {
      fs.copyFileSync(path.join(repoRoot, "dist", file), path.join(siteDir, "dist", file));
    }
  }

  for (const { outDir } of Object.values(VARIANTS)) {
    const from = path.join(repoRoot, outDir);
    const to = path.join(siteDir, outDir);
    fs.mkdirSync(to, { recursive: true });
    for (const file of Object.values(OUTPUT_FILES)) {
      fs.copyFileSync(path.join(from, file), path.join(to, file));
    }
    if (fs.existsSync(path.join(from, "snippets"))) {
      fs.cpSync(path.join(from, "snippets"), path.join(to, "snippets"), { recursive: true });
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
    // The browser tests' server: same MIME types and header switch.
    const { startServer } = await import("./static-server.mjs");
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
