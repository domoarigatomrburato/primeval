// The demo app. It uses the package through the import map in index.html, as
// the README's no-bundler path does, and draws each step's shape live.
import { approximate, InternalError, ValidationError } from "@aleburato/primeval";

const $ = (selector) => document.querySelector(selector);

const ui = {
  threads: $("#threads"),
  stage: $("#stage"),
  frame: $("#frame"),
  result: $("#result"),
  original: $("#original"),
  compare: $("#compare"),
  choose: $("#choose"),
  fileInput: $("#file-input"),
  samples: [...document.querySelectorAll(".sample")],
  form: $("#controls"),
  count: $("#count"),
  countRange: $("#count-range"),
  alpha: $("#alpha"),
  alphaValue: $("#alpha-value"),
  seed: $("#seed"),
  newSeed: $("#new-seed"),
  resolution: $("#resolution"),
  run: $("#run"),
  stop: $("#stop"),
  runState: $("#run-state"),
  step: $("#step"),
  progress: $("#progress"),
  score: $("#score"),
  elapsed: $("#elapsed"),
  rate: $("#rate"),
  announcer: $("#announcer"),
  error: $("#error"),
  errorTitle: $("#error-title"),
  errorDetail: $("#error-detail"),
  downloadSvg: $("#download-svg"),
  downloadPng: $("#download-png"),
};

/**
 * A test and debugging seam: `runs` records each finished run (outcome, and
 * the live and final SVG to compare), `step` the current run's last step.
 */
const debug = { ready: false, step: 0, runs: [] };
window.primevalDemo = debug;

const state = {
  /** The loaded image: `{ bytes, url, name, objectUrl }`. */
  image: null,
  /** Bumped by every image load, so a slower earlier load is dropped. */
  loadId: 0,
  /** The current run: `{ controller }`; any other run's results are stale. */
  run: null,
  /** The finished result: `{ svg, width, height, name }`. */
  result: null,
};

// --- Threads badge ---

// The runtime's own rule: threaded when cross-origin isolated, with a thread
// per logical core; one thread otherwise.
function showThreads() {
  const isolated = globalThis.crossOriginIsolated === true;
  const threads = navigator.hardwareConcurrency;
  ui.threads.dataset.isolated = String(isolated);
  ui.threads.textContent = isolated
    ? `${threads} ${threads === 1 ? "thread" : "threads"} · cross-origin isolated`
    : "1 thread";
  ui.threads.title = isolated
    ? "The page is cross-origin isolated, so the render uses a thread per core."
    : "The page is not cross-origin isolated, so the render runs on one thread.";
}

// --- Controls ---

function newSeed() {
  ui.seed.value = String(crypto.getRandomValues(new Uint32Array(1))[0]);
}

/** The seed as typed; anything that is not a plain integer goes to the package to reject. */
function parseSeed(text) {
  const trimmed = text.trim();
  if (/^\d+$/.test(trimmed)) {
    const value = BigInt(trimmed);
    return value <= BigInt(Number.MAX_SAFE_INTEGER) ? Number(value) : value;
  }
  return trimmed === "" ? Number.NaN : Number(trimmed);
}

function readOptions() {
  const fixed = ui.form.elements["alpha-mode"].value === "fixed";
  return {
    shape: ui.form.elements.shape.value,
    count: ui.count.valueAsNumber,
    alpha: fixed ? ui.alpha.valueAsNumber : "auto",
    seed: parseSeed(ui.seed.value),
    resizeInput: Number(ui.resolution.value),
  };
}

function syncAlpha() {
  ui.alpha.disabled = ui.form.elements["alpha-mode"].value !== "fixed";
  ui.alphaValue.textContent = ui.alpha.value;
}

// --- Stage ---

function setView(view) {
  ui.frame.dataset.view = view;
  const comparing = view === "compare";
  ui.compare.hidden = !comparing;
  if (comparing) {
    setSplit(Number(ui.compare.value));
  }
}

function setSplit(percent) {
  ui.frame.style.setProperty("--split", `${percent}%`);
  ui.compare.setAttribute("aria-valuetext", `${percent}% original, ${100 - percent}% shapes`);
}

function setAspect(width, height) {
  if (width > 0 && height > 0) {
    ui.frame.style.setProperty("--aspect", String(width / height));
  }
}

// --- Status ---

const timer = { start: 0, drawStart: 0, frame: 0 };
let progress = { step: 0, total: 0, score: undefined };
let announced = 0;

function formatSeconds(ms) {
  return `${(ms / 1000).toFixed(1)} s`;
}

function setRunState(kind, text) {
  ui.runState.dataset.state = kind;
  ui.runState.textContent = text;
}

function announce(text) {
  ui.announcer.textContent = text;
}

function renderStatus(now = performance.now()) {
  const { step, total, score } = progress;
  ui.step.textContent = `${step} / ${total}`;
  ui.progress.max = Math.max(total, 1);
  ui.progress.value = step;
  ui.score.textContent = score === undefined ? "–" : score.toFixed(4);
  if (timer.start > 0) {
    ui.elapsed.textContent = formatSeconds(now - timer.start);
  }
  const drawing = timer.drawStart > 0 ? (now - timer.drawStart) / 1000 : 0;
  ui.rate.textContent = step > 0 && drawing > 0 ? String(Math.round(step / drawing)) : "–";
  // Screen readers hear every quarter, not every step.
  const quarter = total > 0 ? Math.floor((step / total) * 4) : 0;
  if (quarter > announced && step < total) {
    announced = quarter;
    announce(`${step} of ${total} shapes`);
  }
}

// --- Errors ---

const OPTION_LABELS = {
  alpha: "Opacity",
  count: "Shapes",
  resizeInput: "Working resolution",
  seed: "Seed",
  shape: "Shape",
};

function showError(title, detail) {
  ui.errorTitle.textContent = title;
  ui.errorDetail.textContent = detail;
  ui.error.hidden = false;
}

function clearError() {
  ui.error.hidden = true;
  ui.errorTitle.textContent = "";
  ui.errorDetail.textContent = "";
}

function showRenderError(error) {
  if (error instanceof ValidationError && error.code === "INVALID_IMAGE") {
    showError("This file can't be read as an image", `${error.message}. Try a JPEG, PNG or WebP.`);
  } else if (error instanceof ValidationError && error.option !== undefined) {
    const label = OPTION_LABELS[error.option] ?? error.option;
    showError(`Check “${label}”`, `${error.option} ${error.requirement}`);
  } else if (error instanceof ValidationError) {
    showError("These settings can't be used", error.message);
  } else if (error instanceof InternalError) {
    showError("The renderer failed", `${error.message}. Try again, or reload the page.`);
  } else {
    showError("Something went wrong", error instanceof Error ? error.message : String(error));
  }
}

// --- Live drawing ---

/**
 * The opening `<svg …>` tag and background `<rect>` of an SVG document, and
 * what follows its shapes: the frame of every render with the same options,
 * whatever `count`, since the shapes are the lines in between.
 */
function svgFrame(document) {
  const lines = document.split("\n");
  if (!lines[0]?.startsWith("<svg ") || !lines[1]?.startsWith("<rect ")) {
    throw new InternalError("unexpected SVG layout");
  }
  return {
    head: `${lines[0]}\n${lines[1]}\n`,
    // A `count: 1` document: head, one shape line, then the closing tag.
    tail: lines.slice(3).join("\n"),
  };
}

/** A live `<svg>` in the stage that appends shapes once per animation frame. */
function startLive(frame) {
  ui.result.innerHTML = frame.head + frame.tail;
  const svg = ui.result.querySelector("svg");
  const shapes = [];
  let pending = [];
  return {
    svg,
    push(shape) {
      shapes.push(shape);
      pending.push(shape);
    },
    flush() {
      if (pending.length > 0) {
        svg.insertAdjacentHTML("beforeend", pending.map((shape) => `${shape}\n`).join(""));
        pending = [];
      }
    },
    /** The document the live drawing amounts to. */
    text: () => frame.head + shapes.map((shape) => `${shape}\n`).join("") + frame.tail,
    count: () => svg.querySelectorAll(":scope > :not(rect:first-of-type)").length,
  };
}

// --- Running ---

function setRunning(running) {
  ui.stop.disabled = !running;
  ui.stage.dataset.running = String(running);
}

function setDownloads(enabled) {
  ui.downloadSvg.disabled = !enabled;
  ui.downloadPng.disabled = !enabled;
}

async function run() {
  if (state.image === null) {
    showError("No image yet", "Drop an image, paste one, choose a file or pick a sample.");
    ui.choose.focus();
    return;
  }
  state.run?.controller.abort();
  const current = { controller: new AbortController() };
  state.run = current;
  const isCurrent = () => state.run === current;
  const { signal } = current.controller;
  const image = state.image;
  const options = readOptions();
  const record = { outcome: "", step: 0, total: options.count };
  let live;

  clearError();
  setDownloads(false);
  state.result = null;
  setRunning(true);
  setRunState("busy", "Preparing");
  progress = {
    step: 0,
    total: Number.isFinite(options.count) ? options.count : 0,
    score: undefined,
  };
  announced = 0;
  debug.step = 0;
  timer.start = performance.now();
  timer.drawStart = 0;
  announce(`Rendering ${options.count} shapes`);
  const tick = (now) => {
    if (!isCurrent()) {
      return;
    }
    live?.flush();
    renderStatus(now);
    timer.frame = requestAnimationFrame(tick);
  };
  cancelAnimationFrame(timer.frame);
  timer.frame = requestAnimationFrame(tick);

  try {
    // The frame of the live SVG: the same options give the same canvas and
    // background at any count.
    const first = await approximate({
      input: image.bytes,
      output: "svg",
      render: { ...options, count: 1 },
      execution: { signal },
    });
    if (!isCurrent()) {
      return;
    }
    setAspect(first.width, first.height);
    live = startLive(svgFrame(first.data));
    setView("result");
    setRunState("busy", "Drawing");
    timer.drawStart = performance.now();

    const result = await approximate({
      input: image.bytes,
      output: "svg",
      render: options,
      execution: {
        signal,
        onProgress(info) {
          if (!isCurrent()) {
            return;
          }
          live.push(info.shape);
          progress = info;
          record.step = info.step;
          debug.step = info.step;
        },
      },
    });
    if (!isCurrent()) {
      return;
    }
    live.flush();
    Object.assign(record, {
      liveShapeCount: live.count(),
      liveText: live.text(),
      liveMarkup: live.svg.outerHTML,
    });
    // The final document replaces the live one; they are the same by
    // construction (see the demo tests).
    ui.result.innerHTML = result.data;
    Object.assign(record, {
      outcome: "done",
      finalText: result.data,
      finalMarkup: ui.result.querySelector("svg").outerHTML,
      width: result.width,
      height: result.height,
    });
    state.result = {
      svg: result.data,
      width: result.width,
      height: result.height,
      name: `${image.name}-${options.shape}-${options.count}-${ui.seed.value.trim()}`,
    };
    const took = performance.now() - timer.start;
    setRunState("done", "Done");
    announce(`Done: ${progress.total} shapes in ${formatSeconds(took)}`);
    setDownloads(true);
    setView("compare");
  } catch (error) {
    record.outcome = error?.name === "AbortError" ? "stopped" : "error";
    if (!isCurrent()) {
      return;
    }
    live?.flush();
    if (record.outcome === "stopped") {
      setRunState("stopped", "Stopped");
      announce(`Stopped at ${progress.step} of ${progress.total} shapes`);
      if (live !== undefined) {
        setView("compare");
      }
    } else {
      setRunState("error", "Error");
      showRenderError(error);
      if (live === undefined) {
        setView("original");
      }
    }
  } finally {
    if (record.outcome === "") {
      // Finished just as a newer run replaced it.
      record.outcome = "stopped";
    }
    debug.runs.push(record);
    if (isCurrent()) {
      cancelAnimationFrame(timer.frame);
      renderStatus();
      timer.start = 0;
      setRunning(false);
    }
  }
}

function stop() {
  state.run?.controller.abort();
}

// --- Input ---

function stem(name) {
  const base = name.replace(/\.[^.]*$/, "").replace(/[^\w-]+/g, "-");
  return base.replace(/^-+|-+$/g, "") || "image";
}

/** Loads image bytes from `source` (a File, or a sample's URL) and runs. */
async function loadImage(source) {
  // A run in progress goes on until the new image starts its own run.
  const loadId = ++state.loadId;
  let bytes;
  let url;
  let objectUrl = null;
  let name;
  try {
    if (typeof source === "string") {
      const response = await fetch(source);
      if (!response.ok) {
        throw new Error(`the sample could not be loaded (HTTP ${response.status})`);
      }
      bytes = new Uint8Array(await response.arrayBuffer());
      url = source;
      name = stem(source.split("/").pop());
    } else {
      bytes = new Uint8Array(await source.arrayBuffer());
      objectUrl = URL.createObjectURL(source);
      url = objectUrl;
      name = stem(source.name);
    }
  } catch (error) {
    if (loadId === state.loadId) {
      showError("The image could not be loaded", error instanceof Error ? error.message : "");
    }
    return;
  }
  if (loadId !== state.loadId) {
    if (objectUrl !== null) {
      URL.revokeObjectURL(objectUrl);
    }
    return;
  }
  if (state.image?.objectUrl) {
    URL.revokeObjectURL(state.image.objectUrl);
  }
  state.image = { bytes, url, name, objectUrl };
  for (const sample of ui.samples) {
    sample.setAttribute("aria-pressed", String(url === sampleUrl(sample)));
  }
  ui.result.replaceChildren();
  ui.original.src = url;
  ui.frame.hidden = false;
  ui.stage.dataset.state = "loaded";
  setView("original");
  run();
}

const sampleUrl = (button) => `./samples/${button.dataset.file}`;

function firstFile(list) {
  return list && list.length > 0 ? list[0] : null;
}

function wireInput() {
  ui.choose.addEventListener("click", () => ui.fileInput.click());
  ui.fileInput.addEventListener("change", () => {
    const file = firstFile(ui.fileInput.files);
    ui.fileInput.value = "";
    if (file !== null) {
      loadImage(file);
    }
  });
  for (const sample of ui.samples) {
    sample.addEventListener("click", () => loadImage(sampleUrl(sample)));
  }

  // Dropping a file anywhere else would navigate away from the demo.
  for (const type of ["dragover", "drop"]) {
    window.addEventListener(type, (event) => event.preventDefault());
  }
  let depth = 0;
  ui.stage.addEventListener("dragenter", (event) => {
    event.preventDefault();
    depth += 1;
    ui.stage.dataset.dragging = "true";
  });
  ui.stage.addEventListener("dragover", (event) => {
    event.preventDefault();
    if (event.dataTransfer) {
      event.dataTransfer.dropEffect = "copy";
    }
  });
  ui.stage.addEventListener("dragleave", () => {
    depth = Math.max(0, depth - 1);
    if (depth === 0) {
      delete ui.stage.dataset.dragging;
    }
  });
  ui.stage.addEventListener("drop", (event) => {
    event.preventDefault();
    depth = 0;
    delete ui.stage.dataset.dragging;
    const file = firstFile(event.dataTransfer?.files);
    if (file !== null) {
      loadImage(file);
    }
  });

  document.addEventListener("paste", (event) => {
    const file = firstFile(event.clipboardData?.files);
    if (file !== null) {
      event.preventDefault();
      loadImage(file);
    }
  });

  ui.original.addEventListener("load", () => {
    if (
      ui.frame.style.getPropertyValue("--aspect") === "" ||
      ui.frame.dataset.view === "original"
    ) {
      setAspect(ui.original.naturalWidth, ui.original.naturalHeight);
    }
  });
}

// --- Downloads ---

function save(blob, filename) {
  const url = URL.createObjectURL(blob);
  const link = document.createElement("a");
  link.href = url;
  link.download = filename;
  link.click();
  setTimeout(() => URL.revokeObjectURL(url), 60_000);
}

/** The final SVG rasterized at its own size, 1024 on the long side: no second render. */
async function rasterize({ svg, width, height }) {
  const url = URL.createObjectURL(new Blob([svg], { type: "image/svg+xml" }));
  try {
    const image = new Image(width, height);
    image.src = url;
    await image.decode();
    const canvas = document.createElement("canvas");
    canvas.width = width;
    canvas.height = height;
    canvas.getContext("2d").drawImage(image, 0, 0, width, height);
    const blob = await new Promise((resolve) => canvas.toBlob(resolve, "image/png"));
    if (blob === null) {
      throw new Error("the browser could not encode the PNG");
    }
    return blob;
  } finally {
    URL.revokeObjectURL(url);
  }
}

function wireDownloads() {
  ui.downloadSvg.addEventListener("click", () => {
    const result = state.result;
    if (result !== null) {
      save(new Blob([result.svg], { type: "image/svg+xml" }), `${result.name}.svg`);
    }
  });
  ui.downloadPng.addEventListener("click", async () => {
    const result = state.result;
    if (result === null) {
      return;
    }
    ui.downloadPng.disabled = true;
    ui.downloadPng.setAttribute("aria-busy", "true");
    try {
      save(await rasterize(result), `${result.name}.png`);
    } catch (error) {
      showError("The PNG could not be made", error instanceof Error ? error.message : "");
    } finally {
      ui.downloadPng.removeAttribute("aria-busy");
      // Still the same result: a new run disables the buttons itself.
      ui.downloadPng.disabled = state.result !== result;
    }
  });
}

// --- Wiring ---

function wireControls() {
  ui.form.addEventListener("submit", (event) => {
    event.preventDefault();
    run();
  });
  ui.stop.addEventListener("click", stop);
  document.addEventListener("keydown", (event) => {
    if (event.key === "Escape" && !ui.stop.disabled) {
      stop();
    }
  });

  ui.countRange.addEventListener("input", () => {
    ui.count.value = ui.countRange.value;
  });
  ui.count.addEventListener("input", () => {
    if (Number.isFinite(ui.count.valueAsNumber)) {
      ui.countRange.value = String(ui.count.valueAsNumber);
    }
  });

  for (const radio of ui.form.elements["alpha-mode"]) {
    radio.addEventListener("change", syncAlpha);
  }
  ui.alpha.addEventListener("input", syncAlpha);

  ui.newSeed.addEventListener("click", newSeed);
  ui.compare.addEventListener("input", () => setSplit(Number(ui.compare.value)));
}

showThreads();
newSeed();
syncAlpha();
wireControls();
wireInput();
wireDownloads();
renderStatus();
debug.ready = true;
