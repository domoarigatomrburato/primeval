// The worker of the bootstrap's isolation probe (see index.html). The
// bootstrap starts several at once; each statically imports the same module
// and reports once loaded. WebKit refuses all but the first of such
// concurrently started module workers when the page's isolation comes from a
// service worker (WebKit bug 261734), which would make the package's threaded
// runtime fail; the bootstrap then falls back to one thread.
import { loaded } from "./coi-probe-module.js";

self.postMessage(loaded);
