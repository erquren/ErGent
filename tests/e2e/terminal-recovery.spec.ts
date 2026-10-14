import { test, expect } from "@playwright/test";
import {
  TerminalConnection,
  type TerminalSurface,
} from "../../apps/web/src/terminal-connection";
import type { BrowserCommand, Event } from "../../packages/protocol-ts";

class Socket extends EventTarget {
  readyState = 1;
  bufferedAmount = 0;
  sent: BrowserCommand[] = [];
  send(data: string) {
    this.sent.push(JSON.parse(data));
  }
  close() {
    this.readyState = 3;
    this.dispatchEvent(new globalThis.Event("close"));
  }
  emit(event: Event) {
    this.dispatchEvent(
      new MessageEvent("message", { data: JSON.stringify(event) }),
    );
  }
  commands(type: BrowserCommand["type"]) {
    return this.sent.filter((command) => command.type === type);
  }
  get websocket() {
    return this as unknown as WebSocket;
  }
}
class Surface implements TerminalSurface {
  cols = 80;
  rows = 24;
  resets = 0;
  sizes: [number, number][] = [];
  text = "";
  writes: { bytes: Uint8Array; done: () => void }[] = [];
  reset() {
    this.resets++;
    this.text = "";
  }
  resize(cols: number, rows: number) {
    this.sizes.push([cols, rows]);
    this.cols = cols;
    this.rows = rows;
  }
  write(bytes: Uint8Array, done: () => void) {
    this.writes.push({ bytes, done });
  }
  finish() {
    const write = this.writes.shift();
    expect(write).toBeTruthy();
    this.text += new TextDecoder().decode(write!.bytes);
    write!.done();
  }
}
const tick = () => new Promise<void>((resolve) => setImmediate(resolve));
function harness() {
  const term = new Surface(),
    notices: string[] = [],
    writable: boolean[] = [],
    losses: boolean[] = [];
  const connection = new TerminalConnection(
    "s",
    term,
    (value) => notices.push(value),
    (value) => writable.push(value),
    () => {},
    () => losses.push(true),
  );
  return { term, notices, writable, losses, connection };
}
async function sync(
  socket: Socket,
  reset: boolean,
  seq = "0",
  streamId = "stream",
  epoch = "epoch",
  size = { cols: 80, rows: 24 },
) {
  await tick();
  const attach = socket.commands("terminal.attach").at(-1)!;
  expect(attach?.type).toBe("terminal.attach");
  if (attach.type !== "terminal.attach") throw Error("missing attach");
  socket.emit({
    type: "terminal.sync",
    payload: {
      sessionId: "s",
      requestId: attach.payload.requestId,
      streamId,
      epoch,
      seq,
      head: seq,
      revision: "0",
      ...size,
      reset,
    },
  });
}
function snapshot(
  socket: Socket,
  text: string,
  last = true,
  index = 0,
  streamId = "stream",
) {
  socket.emit({
    type: "terminal.snapshot",
    payload: {
      sessionId: "s",
      streamId,
      epoch: "epoch",
      seq: "0",
      index,
      first: index === 0,
      last,
      dataBase64: btoa(text),
    },
  });
}
function output(
  socket: Socket,
  from: number,
  text: string,
  streamId = "stream",
  seq = from + text.length,
) {
  socket.emit({
    type: "terminal.output",
    payload: {
      sessionId: "s",
      streamId,
      epoch: "epoch",
      from: String(from),
      seq: String(seq),
      dataBase64: btoa(text),
    },
  });
}
function lease(socket: Socket, streamId = "stream") {
  socket.emit({
    type: "terminal.lease.changed",
    payload: { sessionId: "s", streamId, leaseId: "lease", writable: true },
  });
}
async function ready(h: ReturnType<typeof harness>, socket: Socket) {
  h.connection.connect(socket.websocket);
  await sync(socket, true);
  snapshot(socket, "screen");
  h.term.finish();
  output(socket, 0, "");
  lease(socket);
}

test("reconnect drains the last accepted write, preserves the surface and resumes its applied byte cursor", async () => {
  const h = harness(),
    first = new Socket(),
    second = new Socket();
  try {
    await ready(h, first);
    // Next output arrives only after the preceding write callback requested it.
    await new Promise((r) => setTimeout(r, 60));
    output(first, 0, "line one\r\n");
    expect(h.term.writes).toHaveLength(1);
    first.close();
    h.connection.connect(second.websocket);
    await tick();
    expect(second.commands("terminal.attach")).toHaveLength(0);
    h.term.finish();
    await tick();
    expect(second.commands("terminal.attach").at(-1)?.payload).toMatchObject({
      seq: "10",
      epoch: "epoch",
    });
    await sync(second, false, "10", "new-stream");
    expect(h.term.resets).toBe(1);
    output(second, 10, "line two\r\n", "new-stream");
    h.term.finish();
    expect(h.term.text).toBe("screenline one\r\nline two\r\n");
    expect(first.commands("terminal.pull")).toHaveLength(3);
    expect(h.losses).toEqual([]);
  } finally {
    h.connection.dispose();
  }
});

test("gap and explicit overflow resync never apply missing or duplicated bytes", async () => {
  const h = harness(),
    socket = new Socket();
  try {
    await ready(h, socket);
    await new Promise((r) => setTimeout(r, 60));
    output(socket, 7, "bad");
    expect(h.term.writes).toHaveLength(0);
    await sync(socket, false, "0", "retry");
    socket.emit({
      type: "terminal.resync",
      payload: { sessionId: "s", streamId: "retry", reason: "replay_expired" },
    });
    await new Promise((r) => setTimeout(r, 550));
    await sync(socket, true, "0", "snapshot");
    snapshot(socket, "authoritative", true, 0, "snapshot");
    h.term.finish();
    expect(h.term.text).toBe("authoritative");
    expect(h.term.resets).toBe(2);
    expect(h.losses).toEqual([true]);
    output(socket, 0, "stale", "stream");
    expect(h.term.writes).toHaveLength(0);
  } finally {
    h.connection.dispose();
  }
});

test("one chunk in flight bounds xterm's queue even when the renderer is stalled", async () => {
  const h = harness(),
    socket = new Socket();
  try {
    h.connection.connect(socket.websocket);
    await sync(socket, true);
    snapshot(socket, "x".repeat(16 * 1024), false);
    expect(socket.commands("terminal.pull")).toHaveLength(1);
    expect(h.term.writes).toHaveLength(1);
    // A faulty peer cannot flood the parser with unsolicited additional chunks.
    snapshot(socket, "next", true, 1);
    expect(socket.readyState).toBe(3);
    expect(h.term.writes).toHaveLength(1);
  } finally {
    h.connection.dispose();
  }
});

test("interrupted snapshot is replaced only after pending writes drain", async () => {
  const h = harness(),
    first = new Socket(),
    second = new Socket();
  try {
    h.connection.connect(first.websocket);
    await sync(first, true);
    snapshot(first, "partial", false);
    first.close();
    h.connection.connect(second.websocket);
    h.term.finish();
    await tick();
    expect(second.commands("terminal.attach").at(-1)?.payload).toMatchObject({
      seq: null,
    });
    await sync(second, true, "0", "second");
    snapshot(second, "replacement", true, 0, "second");
    h.term.finish();
    expect(h.term.text).toBe("replacement");
    expect(h.term.resets).toBe(2);
  } finally {
    h.connection.dispose();
  }
});

test("unacknowledged input and old lease events are never replayed on reconnect", async () => {
  const h = harness(),
    first = new Socket(),
    second = new Socket();
  try {
    await ready(h, first);
    h.connection.transmit("once\r");
    expect(first.commands("terminal.input")).toHaveLength(1);
    first.close();
    expect(h.writable.at(-1)).toBe(false);
    expect(h.notices.at(-1)).toContain("未自动重发");
    h.connection.connect(second.websocket);
    await sync(second, false, "0", "second");
    lease(second, "stream");
    h.connection.transmit("blocked");
    expect(second.commands("terminal.input")).toHaveLength(0);
    output(second, 0, "", "second");
    lease(second, "second");
    h.connection.transmit("new\r");
    expect(second.commands("terminal.input")).toHaveLength(1);
    const input = second.commands("terminal.input")[0];
    if (input.type !== "terminal.input") throw Error("missing input");
    expect(atob(input.payload.dataBase64)).toBe("new\r");
  } finally {
    h.connection.dispose();
  }
});

test("epoch change requires authoritative reset and out-of-order snapshot chunks fail closed", async () => {
  const h = harness(),
    first = new Socket(),
    second = new Socket();
  try {
    await ready(h, first);
    first.close();
    h.connection.connect(second.websocket);
    await sync(second, false, "0", "invalid", "new-epoch");
    await tick();
    expect(second.commands("terminal.attach").at(-1)?.payload).toMatchObject({
      seq: null,
      epoch: null,
    });
    await sync(second, true, "0", "second");
    snapshot(second, "skipped", true, 1, "second");
    expect(second.readyState).toBe(3);
    expect(h.term.writes).toHaveLength(0);
  } finally {
    h.connection.dispose();
  }
});

test("reconnect can acquire input during continuous nonempty output", async () => {
  const h = harness(),
    socket = new Socket();
  try {
    h.connection.connect(socket.websocket);
    await tick();
    const attach = socket.commands("terminal.attach").at(-1)!;
    if (attach.type !== "terminal.attach") throw Error("missing attach");
    socket.emit({
      type: "terminal.sync",
      payload: {
        sessionId: "s",
        requestId: attach.payload.requestId,
        streamId: "stream",
        epoch: "epoch",
        seq: "0",
        head: "10",
        revision: "0",
        cols: 80,
        rows: 24,
        reset: true,
      },
    });
    snapshot(socket, "screen");
    h.term.finish();
    expect(socket.commands("terminal.lease.claim")).toHaveLength(0);
    output(socket, 0, "12345");
    h.term.finish();
    expect(socket.commands("terminal.lease.claim")).toHaveLength(0);
    output(socket, 5, "67890more");
    h.term.finish();
    expect(socket.commands("terminal.lease.claim")).toHaveLength(1);
    lease(socket);
    h.connection.transmit("\x03");
    expect(socket.commands("terminal.input")).toHaveLength(1);
  } finally {
    h.connection.dispose();
  }
});

test("authoritative snapshots preserve ANSI continuation in the actual xterm parser", async () => {
  const { spawnSync } = await import("node:child_process");
  const { createRequire } = await import("node:module");
  const { resolve } = await import("node:path");
  const require = createRequire(resolve("apps/web/package.json"));
  const { Terminal } =
    require("@xterm/xterm") as typeof import("../../apps/web/node_modules/@xterm/xterm");
  const fixtures = spawnSync(
    "cargo",
    [
      "run",
      "--quiet",
      "--locked",
      "-p",
      "ergent-terminal",
      "--example",
      "snapshot_fixtures",
    ],
    { encoding: "utf8" },
  );
  expect(fixtures.status, fixtures.stderr).toBe(0);
  const write = (term: InstanceType<typeof Terminal>, text: string) =>
    new Promise<void>((done) =>
      term.write(
        Uint8Array.from(atob(text), (c) => c.charCodeAt(0)),
        done,
      ),
    );
  const contents = (term: InstanceType<typeof Terminal>) =>
    Array.from({ length: term.rows }, (_, row) =>
      term.buffer.active.getLine(row)?.translateToString(true),
    );
  for (const fixture of JSON.parse(fixtures.stdout) as {
    name: string;
    setup: string;
    snapshot: string;
    suffix: string;
  }[]) {
    const source = new Terminal({ cols: 20, rows: 6 }),
      restored = new Terminal({ cols: 20, rows: 6 });
    try {
      await write(source, fixture.setup);
      await write(restored, fixture.snapshot);
      expect(contents(restored), `${fixture.name}: snapshot`).toEqual(
        contents(source),
      );
      await write(source, fixture.suffix);
      await write(restored, fixture.suffix);
      expect(contents(restored), `${fixture.name}: continuation`).toEqual(
        contents(source),
      );
      expect(
        [restored.buffer.active.cursorX, restored.buffer.active.cursorY],
        fixture.name,
      ).toEqual([source.buffer.active.cursorX, source.buffer.active.cursorY]);
    } finally {
      source.dispose();
      restored.dispose();
    }
  }
});

test("overflow preserves a usable lease long enough to interrupt a fast producer", async () => {
  const h = harness(),
    socket = new Socket();
  try {
    await ready(h, socket);
    socket.emit({
      type: "terminal.resync",
      payload: { sessionId: "s", streamId: "stream", reason: "replay_expired" },
    });
    h.connection.transmit("\x03");
    expect(socket.commands("terminal.input")).toHaveLength(1);
    expect(h.writable.at(-1)).toBe(true);
    await new Promise((r) => setTimeout(r, 550));
    expect(h.writable.at(-1)).toBe(false);
    expect(socket.commands("terminal.attach")).toHaveLength(2);
  } finally {
    h.connection.dispose();
  }
});

test("final output is acknowledged only after the renderer applies its tail", async () => {
  const h = harness(),
    socket = new Socket();
  try {
    await ready(h, socket);
    await new Promise((r) => setTimeout(r, 60));
    output(socket, 0, "final");
    const pulls = socket.commands("terminal.pull").length;
    socket.emit({
      type: "terminal.ended",
      payload: {
        sessionId: "s",
        streamId: "stream",
        epoch: "epoch",
        seq: "5",
        complete: false,
        available: true,
        truncated: false,
      },
    });
    expect(socket.commands("terminal.pull")).toHaveLength(pulls);
    h.connection.transmit("blocked");
    expect(socket.commands("terminal.input")).toHaveLength(0);
    h.term.finish();
    expect(socket.commands("terminal.pull").at(-1)?.payload).toMatchObject({
      seq: "5",
    });
    socket.emit({
      type: "terminal.ended",
      payload: {
        sessionId: "s",
        streamId: "stream",
        epoch: "epoch",
        seq: "5",
        complete: true,
        available: true,
        truncated: false,
      },
    });
    expect(h.notices.at(-1)).toBe("终端已退出 · 最后输出已接收");
    expect(h.term.text).toBe("screenfinal");
    socket.close();
    h.connection.connect(null);
    expect(h.notices.at(-1)).toBe("终端已退出 · 最后输出已接收");
    await new Promise((r) => setTimeout(r, 60));
    expect(socket.commands("terminal.pull")).toHaveLength(pulls + 1);
  } finally {
    h.connection.dispose();
  }
});

test("expired final output leaves a durable loss notice without stale callback pulls", async () => {
  const h = harness(),
    socket = new Socket();
  try {
    await ready(h, socket);
    await new Promise((r) => setTimeout(r, 60));
    output(socket, 0, "tail");
    const pulls = socket.commands("terminal.pull").length;
    socket.emit({
      type: "terminal.ended",
      payload: {
        sessionId: "s",
        streamId: "stream",
        epoch: "epoch",
        seq: "9",
        complete: false,
        available: false,
        truncated: true,
      },
    });
    h.term.finish();
    expect(socket.commands("terminal.pull")).toHaveLength(pulls);
    expect(h.notices.at(-1)).toBe("终端已退出 · 最后输出可能不完整");
    expect(h.writable.at(-1)).toBe(false);
    socket.close();
    h.connection.connect(new Socket().websocket);
    expect(h.notices.at(-1)).toBe("终端已退出 · 最后输出可能不完整");
  } finally {
    h.connection.dispose();
  }
});

test("terminating lifecycle immediately blocks claim input and resize", async () => {
  let lifecycle = "running";
  const term = new Surface(),
    socket = new Socket();
  const connection = new TerminalConnection(
    "s",
    term,
    () => {},
    () => {},
    () => {},
    () => {},
    () => lifecycle,
  );
  try {
    connection.connect(socket.websocket);
    await sync(socket, true);
    snapshot(socket, "screen");
    term.finish();
    lease(socket);
    lifecycle = "terminating";
    connection.lifecycleChanged();
    const before = socket.sent.length;
    connection.claim();
    connection.transmit("no");
    connection.resize(100, 30);
    lease(socket);
    expect(socket.sent).toHaveLength(before);
  } finally {
    connection.dispose();
  }
});

test("stale final and input outcomes cannot disable a replacement session or attachment", async () => {
  const socket = new Socket(),
    old = harness(),
    current = harness();
  try {
    await ready(old, socket);
    const oldAttach = socket.commands("terminal.attach").at(-1)!;
    if (oldAttach.type !== "terminal.attach") throw Error("missing attach");
    old.connection.dispose();
    await ready(current, socket);
    for (const sessionId of ["other-session", "s"]) {
      socket.emit({
        type: "terminal.unavailable",
        payload: { sessionId, requestId: oldAttach.payload.requestId },
      });
      socket.emit({
        type: "terminal.input.rejected",
        payload: {
          sessionId,
          inputId: "obsolete-input",
          message: "old rejection",
        },
      });
    }
    socket.emit({
      type: "terminal.ended",
      payload: {
        sessionId: "other-session",
        streamId: "stream",
        epoch: "epoch",
        seq: "0",
        complete: false,
        available: false,
        truncated: true,
      },
    });
    expect(socket.readyState).toBe(1);
    expect(current.writable.at(-1)).toBe(true);
    current.connection.transmit("current");
    expect(socket.commands("terminal.input")).toHaveLength(1);
  } finally {
    old.connection.dispose();
    current.connection.dispose();
  }
});

test("lost and terminating notices remain truthful during transport changes", async () => {
  let lifecycle = "terminating";
  const notices: string[] = [];
  const connection = new TerminalConnection(
    "s",
    new Surface(),
    (value) => notices.push(value),
    () => {},
    () => {},
    () => {},
    () => lifecycle,
  );
  try {
    const first = new Socket();
    connection.connect(first.websocket);
    first.close();
    expect(notices.at(-1)).toBe("正在结束 · 等待进程退出");
    lifecycle = "lost";
    connection.lifecycleChanged();
    expect(notices.at(-1)).toBe("终端状态未知 · 无法确认进程退出或最后输出");
    const second = new Socket();
    connection.connect(second.websocket);
    second.close();
    connection.connect(null);
    expect(notices.at(-1)).toBe("终端状态未知 · 无法确认进程退出或最后输出");
  } finally {
    connection.dispose();
  }
});

test("resize pauses readiness immediately and old output cannot re-enable input", async () => {
  const h = harness(),
    socket = new Socket();
  try {
    await ready(h, socket);
    h.connection.resize(100, 30);
    expect(h.writable.at(-1)).toBe(false);
    expect(h.notices.at(-1)).toBe("正在同步终端…");
    const claims = socket.commands("terminal.lease.claim").length;
    await new Promise((r) => setTimeout(r, 60));
    output(socket, 0, "old-size-output");
    h.term.finish();
    expect(socket.commands("terminal.lease.claim")).toHaveLength(claims);
    h.connection.transmit("not-ready");
    expect(socket.commands("terminal.input")).toHaveLength(0);
    socket.emit({
      type: "terminal.resync",
      payload: { sessionId: "s", streamId: "stream", reason: "resized" },
    });
    await sync(socket, true, "0", "resized", "epoch", { cols: 100, rows: 30 });
    snapshot(socket, "new-size", true, 0, "resized");
    expect([h.term.cols, h.term.rows]).toEqual([100, 30]);
    // Even an early lease cannot bypass a delayed renderer callback. Unsent
    // keystrokes stay unsent after recovery instead of replaying into the shell.
    lease(socket, "resized");
    expect(h.writable.at(-1)).toBe(false);
    h.connection.transmit("still-not-ready");
    expect(socket.commands("terminal.input")).toHaveLength(0);
    h.term.finish();
    lease(socket, "resized");
    expect(h.writable.at(-1)).toBe(true);
    h.connection.transmit("ready");
    expect(socket.commands("terminal.input")).toHaveLength(1);
  } finally {
    h.connection.dispose();
  }
});

test("same-size authoritative reset synchronously normalizes browser viewport metrics", async () => {
  const h = harness(),
    socket = new Socket();
  try {
    h.connection.connect(socket.websocket);
    await sync(socket, true);
    expect(h.term.sizes).toEqual([
      [80, 25],
      [80, 24],
    ]);
    snapshot(socket, "screen");
    h.term.finish();
    socket.close();
    const next = new Socket();
    h.connection.connect(next.websocket);
    await sync(next, false, "0", "resume");
    expect(h.term.sizes).toEqual([
      [80, 25],
      [80, 24],
    ]);
    expect(h.term.resets).toBe(1);
  } finally {
    h.connection.dispose();
  }
});
