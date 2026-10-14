import { test, expect } from "@playwright/test";
import { readFileSync } from "node:fs";
import { createRequire } from "node:module";
import { resolve } from "node:path";
import { runInNewContext } from "node:vm";

// Exercise the shipped, patched entry point rather than a reimplementation or
// TypeScript-only fixture. Expose webpack's module loader only inside this VM.
function bundledXterm() {
  const require = createRequire(resolve("apps/web/package.json"));
  const source = readFileSync(
    process.env.ERGENT_XTERM_BUNDLE || require.resolve("@xterm/xterm"),
    "utf8",
  );
  const entry = "var s={};return(()=>{";
  expect(source.split(entry)).toHaveLength(2);
  const timers: (() => void)[] = [];
  const module = { exports: {} as any };
  runInNewContext(
    source.replace(entry, "var s={};s.__modules=i;return(()=>{"),
    {
      module,
      exports: module.exports,
      setTimeout: (fn: () => void) => timers.push(fn),
      clearTimeout: () => {},
      console,
      process,
      navigator: { userAgent: "Node", platform: "Linux", language: "en-US" },
    },
  );
  const drain = () => {
    let remaining = 100;
    while (timers.length) {
      expect(remaining--).toBeGreaterThan(0);
      timers.shift()!();
    }
  };
  return { ...module.exports, timers, drain };
}
function composition() {
  const bundle = bundledXterm();
  const term = new bundle.Terminal();
  const sent: string[] = [];
  term.onData((data: string) => sent.push(data));
  const textarea = { value: "", style: {} };
  const view = {
    classList: { add() {}, remove() {} },
    textContent: "",
    style: {},
    getBoundingClientRect: () => ({ width: 1, height: 1 }),
  };
  const { CompositionHelper } = bundle.__modules(2950);
  const helper = new CompositionHelper(
    textarea,
    view,
    term._core._bufferService,
    term._core.optionsService,
    term._core.coreService,
    { dimensions: { css: { cell: { width: 1, height: 1 } } } },
  );
  return { ...bundle, term, sent, textarea, helper };
}
for (const boundary of [
  "disabled at start, enabled before end",
  "disabled at start, enabled before timer",
  "disabled during composition",
  "disabled after end, before timer",
  "reset before timer",
  "dispose before timer",
]) {
  test(`bundled xterm discards IME input: ${boundary}`, () => {
    const h = composition();
    try {
      if (boundary.startsWith("disabled at start"))
        h.term.options.disableStdin = true;
      h.helper.compositionstart();
      h.textarea.value = "不应重发";
      if (boundary === "disabled at start, enabled before end")
        h.term.options.disableStdin = false;
      if (boundary === "disabled during composition") {
        h.term.options.disableStdin = true;
        h.term.options.disableStdin = false;
      }
      h.helper.compositionend();
      if (boundary === "disabled at start, enabled before timer")
        h.term.options.disableStdin = false;
      if (boundary === "disabled after end, before timer") {
        h.term.options.disableStdin = true;
        h.term.options.disableStdin = false;
      }
      if (boundary === "reset before timer") h.term.reset();
      if (boundary === "dispose before timer") h.term.dispose();
      h.drain();
      expect(h.sent).toEqual([]);
      if (boundary !== "dispose before timer") {
        h.textarea.value = "";
        h.helper.compositionstart();
        h.textarea.value = "恢复输入";
        h.helper.compositionend();
        h.drain();
        expect(h.sent).toEqual(["恢复输入"]);
      }
    } finally {
      h.term.dispose();
    }
  });
}

test("bundled xterm does not replay deferred keyCode229 textarea changes", () => {
  const h = composition();
  try {
    for (const initiallyDisabled of [true, false]) {
      h.textarea.value = "";
      h.term.options.disableStdin = initiallyDisabled;
      h.helper.keydown({ keyCode: 229 });
      h.textarea.value = "1";
      if (!initiallyDisabled) h.term.options.disableStdin = true;
      h.term.options.disableStdin = false;
      h.drain();
      expect(h.sent).toEqual([]);
    }
    h.textarea.value = "";
    h.helper.keydown({ keyCode: 229 });
    h.textarea.value = "2";
    h.drain();
    expect(h.sent).toEqual(["2"]);
  } finally {
    h.term.dispose();
  }
});

test("bundled xterm rejects interrupted IME finalized immediately by Enter", () => {
  const h = composition();
  try {
    h.helper.compositionstart();
    h.textarea.value = "不应重发";
    h.helper.compositionupdate({ data: "不应重发" });
    h.drain();
    h.term.options.disableStdin = true;
    h.term.options.disableStdin = false;
    h.helper.keydown({ keyCode: 13 });
    h.drain();
    expect(h.sent).toEqual([]);
    h.textarea.value = "";
    h.helper.compositionstart();
    h.textarea.value = "恢复输入";
    h.helper.compositionend();
    h.drain();
    expect(h.sent).toEqual(["恢复输入"]);
  } finally {
    h.term.dispose();
  }
});

test("bundled xterm ignores queued viewport reset callbacks after disposal", () => {
  const bundle = bundledXterm();
  const { Viewport } = bundle.__modules(1680);
  const subscription = () => ({ dispose() {} });
  let disposed = false;
  const dimensions = {
    css: { canvas: { height: 24 } },
    device: { cell: { height: 1 } },
  };
  const renderer = {
    get dimensions() {
      if (disposed) throw Error("disposed renderer dimensions");
      return dimensions;
    },
    onDimensionsChange: subscription,
  };
  const viewport = new Viewport(
    {
      offsetWidth: 80,
      offsetHeight: 24,
      scrollTop: 0,
      style: {},
      addEventListener() {},
      removeEventListener() {},
    },
    { offsetWidth: 80, style: {} },
    {
      buffer: { lines: { length: 24 }, ydisp: 0 },
      buffers: { onBufferActivate: subscription },
    },
    { onSpecificOptionChange: subscription },
    { height: 1 },
    renderer,
    {
      dpr: 1,
      window: {
        requestAnimationFrame: (fn: () => void) => bundle.timers.push(fn),
        cancelAnimationFrame() {},
      },
    },
    { colors: { background: { css: "black" } }, onChangeColors: subscription },
  );
  viewport.reset();
  viewport.syncScrollArea(true);
  viewport.dispose();
  disposed = true;
  expect(bundle.timers.length).toBeGreaterThan(0);
  expect(() => bundle.drain()).not.toThrow();
});
