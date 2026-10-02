// A service worker that makes the demo cross-origin isolated on hosts that
// cannot set headers, such as GitHub Pages.
//
// Threads in WebAssembly need SharedArrayBuffer, which browsers only expose to
// cross-origin isolated pages, and isolation needs two response headers:
//
//   Cross-Origin-Opener-Policy: same-origin
//   Cross-Origin-Embedder-Policy: require-corp
//
// Once this worker controls the page, it answers every same-origin request
// (the navigation, scripts, the module workers and their nested workers, the
// .wasm, images) from the network and adds both headers to the response. The
// bootstrap in index.html registers it and reloads the page once, since the
// load that registers a worker is not controlled by it. Every asset of the
// demo is same-origin, so `require-corp` blocks nothing. Cross-origin
// requests (none are made) pass through untouched.

self.addEventListener("install", () => {
  // Take over without waiting for old tabs to close.
  self.skipWaiting();
});

self.addEventListener("activate", (event) => {
  // Control the page that registered the worker; its `controllerchange`
  // event triggers the bootstrap's single reload.
  event.waitUntil(self.clients.claim());
});

/** `response` with the isolation headers, or unchanged when it cannot carry them. */
function isolated(response) {
  // Opaque and redirect responses have no readable headers to extend.
  if (response.status === 0 || response.type === "opaqueredirect") {
    return response;
  }
  const headers = new Headers(response.headers);
  headers.set("Cross-Origin-Opener-Policy", "same-origin");
  headers.set("Cross-Origin-Embedder-Policy", "require-corp");
  return new Response(response.body, {
    status: response.status,
    statusText: response.statusText,
    headers,
  });
}

self.addEventListener("fetch", (event) => {
  const { request } = event;
  if (new URL(request.url).origin !== self.location.origin) {
    return;
  }
  // A Chromium quirk: this cache mode is only valid for same-origin requests.
  if (request.cache === "only-if-cached" && request.mode !== "same-origin") {
    return;
  }
  event.respondWith(fetch(request).then(isolated));
});
