// A static server for the browser tests. It serves the repository root, with
// or without the cross-origin isolation headers (COOP/COEP) that threads need.
import fs from "node:fs";
import http from "node:http";
import path from "node:path";

const repoRoot = process.cwd();

const TYPES = {
  ".html": "text/html; charset=utf-8",
  ".jpg": "image/jpeg",
  ".js": "text/javascript; charset=utf-8",
  ".png": "image/png",
  ".wasm": "application/wasm",
};

/** Starts a server on a free local port; resolves with `{ origin, close() }`. */
export function startServer({ isolated }) {
  const server = http.createServer((request, response) => {
    const pathname = decodeURIComponent(new URL(request.url, "http://localhost").pathname);
    const file = path.join(repoRoot, path.normalize(pathname));
    if (
      !file.startsWith(`${repoRoot}${path.sep}`) ||
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
  return new Promise((resolve) => {
    server.listen(0, "127.0.0.1", () => {
      resolve({
        origin: `http://127.0.0.1:${server.address().port}`,
        close: () => new Promise((done) => server.close(done)),
      });
    });
  });
}
