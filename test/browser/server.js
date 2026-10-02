// A static server for the browser tests and the demo (scripts/demo.mjs). It
// serves a directory, the repository root by default, with or without the
// cross-origin isolation headers (COOP/COEP) that threads need. A directory
// URL serves its `index.html`.
import fs from "node:fs";
import http from "node:http";
import path from "node:path";

const repoRoot = process.cwd();

const TYPES = {
  ".css": "text/css; charset=utf-8",
  ".html": "text/html; charset=utf-8",
  ".jpg": "image/jpeg",
  ".js": "text/javascript; charset=utf-8",
  ".png": "image/png",
  ".svg": "image/svg+xml",
  ".wasm": "application/wasm",
};

/**
 * Starts a server on `port` (a free one by default) serving `root`; resolves
 * with `{ origin, close() }`.
 */
export function startServer({ isolated, root = repoRoot, port = 0 }) {
  const server = http.createServer((request, response) => {
    const pathname = decodeURIComponent(new URL(request.url, "http://localhost").pathname);
    let file = path.join(root, path.normalize(pathname));
    if (fs.statSync(file, { throwIfNoEntry: false })?.isDirectory()) {
      file = path.join(file, "index.html");
    }
    if (
      !file.startsWith(`${root}${path.sep}`) ||
      !fs.statSync(file, { throwIfNoEntry: false })?.isFile()
    ) {
      response.writeHead(404).end();
      return;
    }
    const headers = {
      "content-type": TYPES[path.extname(file)] ?? "application/octet-stream",
      "cache-control": "no-store",
    };
    if (isolated) {
      headers["cross-origin-opener-policy"] = "same-origin";
      headers["cross-origin-embedder-policy"] = "require-corp";
    }
    response.writeHead(200, headers);
    fs.createReadStream(file).pipe(response);
  });
  return new Promise((resolve, reject) => {
    server.once("error", reject);
    server.listen(port, "127.0.0.1", () => {
      resolve({
        origin: `http://127.0.0.1:${server.address().port}`,
        close: () => new Promise((done) => server.close(done)),
      });
    });
  });
}
