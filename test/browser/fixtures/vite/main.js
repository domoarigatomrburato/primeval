// A minimal app built with Vite against the packed package (see
// test/browser/vite.test.js, which copies the fixture image next to it). It
// renders once on load and exposes the outcome as `window.outcome`.
import { approximate } from "@aleburato/primeval";

async function render() {
  const response = await fetch(new URL("./monalisa.jpg", import.meta.url));
  const input = new Uint8Array(await response.arrayBuffer());
  return approximate({
    input,
    output: "svg",
    render: { count: 4, resizeInput: 16, outputSize: 32, seed: 7 },
  });
}

render().then(
  (result) => {
    window.outcome = { crossOriginIsolated: globalThis.crossOriginIsolated, result };
  },
  (error) => {
    window.outcome = {
      crossOriginIsolated: globalThis.crossOriginIsolated,
      error: { name: error?.name, message: error?.message },
    };
  },
);
