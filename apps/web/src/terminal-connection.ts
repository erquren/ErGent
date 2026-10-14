import type { BrowserCommand, Event } from "../../../packages/protocol-ts";
import { randomId } from "./browser";

const CHUNK_BYTES = 16 * 1024;
const MAX_PENDING_INPUTS = 256;
const OPEN = 1;
export interface TerminalSurface {
  cols: number;
  rows: number;
  reset(): void;
  resize(cols: number, rows: number): void;
  write(bytes: Uint8Array, callback: () => void): void;
}
type Cursor = {
  epoch: string;
  seq: bigint;
  revision: string;
  cols: number;
  rows: number;
};
type Binding = {
  socket: WebSocket;
  requestId: string;
  streamId: string | null;
  cursor: Cursor | null;
  head: bigint;
  snapshot: { index: number } | null;
  outstanding: boolean;
  ready: boolean;
  ending: boolean;
  lease: string | null;
  pending: Set<string>;
  timer?: ReturnType<typeof setTimeout>;
  renew?: ReturnType<typeof setInterval>;
  message: (event: MessageEvent) => void;
  close: () => void;
};
function encode(bytes: Uint8Array): string {
  let text = "";
  for (const byte of bytes) text += String.fromCharCode(byte);
  return btoa(text);
}
function decode(value: string): Uint8Array {
  // Bound before decoding, as well as afterward. No unbounded xterm write queue.
  if (value.length > Math.ceil(CHUNK_BYTES / 3) * 4)
    throw Error("oversized output");
  const bytes = Uint8Array.from(atob(value), (c) => c.charCodeAt(0));
  if (bytes.length > CHUNK_BYTES) throw Error("oversized output");
  return bytes;
}
function sequence(value: string): bigint {
  if (!/^\d{1,20}$/.test(value)) throw Error("invalid sequence");
  const seq = BigInt(value);
  if (seq > 18446744073709551615n) throw Error("invalid sequence");
  return seq;
}

/** The terminal belongs to a session. Only this transport binding is replaced. */
export class TerminalConnection {
  private binding: Binding | null = null;
  private applied: Cursor | null = null;
  private writing: Promise<void> = Promise.resolve();
  private disposed = false;
  private rendered = false;
  private completed = false;
  private ended = false;
  private outputLoss = false;

  constructor(
    private readonly sessionId: string,
    private readonly term: TerminalSurface,
    private readonly notice: (text: string) => void,
    private readonly writable: (value: boolean) => void,
    private readonly onLease: () => void,
    private readonly onHistoryLoss: () => void = () => {},
    private readonly lifecycle: () => string = () => "running",
  ) {}
  lifecycleChanged(truncated = false): void {
    this.outputLoss ||= truncated;
    if (this.lifecycle() === "running") return;
    if (this.binding) this.binding.lease = null;
    this.writable(false);
    this.notice(this.finalNotice());
  }
  connect(socket: WebSocket | null): void {
    const old = this.binding;
    if (old?.socket === socket) return;
    if (old) {
      this.send(old, {
        type: "terminal.detach",
        payload: { sessionId: this.sessionId },
      });
      this.unbind(old);
    }
    this.binding = null;
    this.writable(false);
    if (!socket || this.disposed) {
      this.notice(this.disconnectNotice(!!old?.pending.size));
      return;
    }
    const binding: Binding = {
      socket,
      requestId: "",
      streamId: null,
      cursor: null,
      head: 0n,
      snapshot: null,
      outstanding: false,
      ready: false,
      ending: false,
      lease: null,
      pending: new Set(),
      message: (raw) => this.message(binding, raw),
      close: () => {
        if (this.binding !== binding) return;
        this.unbind(binding);
        this.binding = null;
        this.writable(false);
        this.notice(this.disconnectNotice(!!binding.pending.size));
      },
    };
    this.binding = binding;
    socket.addEventListener("message", binding.message);
    socket.addEventListener("close", binding.close);
    binding.renew = setInterval(() => {
      if (binding.ready && binding.lease)
        this.send(binding, {
          type: "terminal.lease.renew",
          payload: { sessionId: this.sessionId, leaseId: binding.lease },
        });
    }, 10000);
    this.attach(binding);
  }
  private unbind(binding: Binding): void {
    clearTimeout(binding.timer);
    clearInterval(binding.renew);
    binding.lease = null;
    binding.socket.removeEventListener("message", binding.message);
    binding.socket.removeEventListener("close", binding.close);
  }
  private send(binding: Binding, command: BrowserCommand): void {
    if (
      this.binding === binding &&
      binding.socket.readyState === OPEN &&
      !this.disposed
    )
      binding.socket.send(JSON.stringify(command));
  }
  private attach(binding: Binding): void {
    clearTimeout(binding.timer);
    binding.requestId = randomId();
    const requestId = binding.requestId;
    binding.streamId = null;
    binding.ready = false;
    binding.lease = null;
    binding.outstanding = false;
    this.writable(false);
    this.notice(
      this.ended || this.lifecycle() !== "running"
        ? this.finalNotice()
        : "正在同步终端…",
    );
    // A socket close does not cancel writes already accepted by xterm. Drain that
    // single write first, then resume from what was actually applied, not received.
    void this.writing.then(() => {
      if (
        this.binding !== binding ||
        binding.requestId !== requestId ||
        this.disposed
      )
        return;
      const cursor = this.applied;
      this.send(binding, {
        type: "terminal.attach",
        payload: {
          sessionId: this.sessionId,
          requestId,
          epoch: cursor?.epoch ?? null,
          seq: cursor?.seq.toString() ?? null,
          revision: cursor?.revision ?? null,
          cols: cursor?.cols ?? null,
          rows: cursor?.rows ?? null,
        },
      });
      binding.timer = setTimeout(() => this.attach(binding), 3000);
    });
  }
  private pull(binding: Binding, delay = 0): void {
    clearTimeout(binding.timer);
    const run = () => {
      if (
        this.binding !== binding ||
        !binding.streamId ||
        !binding.cursor ||
        binding.outstanding
      )
        return;
      binding.outstanding = true;
      this.send(binding, {
        type: "terminal.pull",
        payload: {
          sessionId: this.sessionId,
          streamId: binding.streamId,
          seq: binding.cursor.seq.toString(),
        },
      });
      // A missing response is not a continuous stream. Retry attachment explicitly.
      binding.timer = setTimeout(() => this.attach(binding), 5000);
    };
    if (delay) binding.timer = setTimeout(run, delay);
    else run();
  }
  private message(binding: Binding, raw: MessageEvent): void {
    if (this.binding !== binding || this.disposed) return;
    try {
      if (typeof raw.data !== "string" || raw.data.length > 64 * 1024)
        throw Error("invalid frame");
      const event = JSON.parse(raw.data) as Event;
      const p = event.payload;
      if ("sessionId" in p && p.sessionId !== this.sessionId) return;
      switch (event.type) {
        case "terminal.sync": {
          const p = event.payload;
          if (p.requestId !== binding.requestId || binding.streamId) return;
          clearTimeout(binding.timer);
          if (p.cols < 20 || p.cols > 500 || p.rows < 5 || p.rows > 200)
            throw Error("invalid size");
          const cursor = {
            epoch: p.epoch,
            seq: sequence(p.seq),
            revision: p.revision,
            cols: p.cols,
            rows: p.rows,
          };
          if (
            !p.reset &&
            (!this.applied ||
              this.applied.epoch !== p.epoch ||
              this.applied.seq !== cursor.seq ||
              this.applied.revision !== p.revision ||
              this.term.cols !== p.cols ||
              this.term.rows !== p.rows)
          ) {
            this.applied = null;
            this.attach(binding);
            return;
          }
          const head = sequence(p.head);
          if (head < cursor.seq) throw Error("invalid replay watermark");
          binding.head = head;
          binding.streamId = p.streamId;
          binding.cursor = cursor;
          binding.snapshot = p.reset ? { index: 0 } : null;
          if (p.reset) {
            if (this.rendered) this.onHistoryLoss();
            this.applied = null;
            this.term.reset();
            this.term.resize(p.cols, p.rows);
          }
          this.synchronized(binding);
          this.pull(binding);
          return;
        }
        case "terminal.snapshot": {
          const p = event.payload;
          if (p.streamId !== binding.streamId) return;
          if (
            !binding.outstanding ||
            !binding.snapshot ||
            !binding.cursor ||
            p.epoch !== binding.cursor.epoch ||
            sequence(p.seq) !== binding.cursor.seq ||
            p.index !== binding.snapshot.index ||
            p.first !== (p.index === 0)
          )
            throw Error("invalid snapshot sequence");
          clearTimeout(binding.timer);
          const value = decode(p.dataBase64);
          binding.outstanding = false;
          binding.snapshot.index++;
          if (binding.snapshot.index > 512) throw Error("oversized snapshot");
          const cursor = { ...binding.cursor };
          this.write(binding, value, () => {
            if (p.last) {
              this.applied = cursor;
              binding.snapshot = null;
            }
          });
          return;
        }
        case "terminal.output": {
          const p = event.payload;
          if (p.streamId !== binding.streamId) return;
          if (
            !binding.outstanding ||
            binding.snapshot ||
            !binding.cursor ||
            p.epoch !== binding.cursor.epoch
          )
            throw Error("unexpected output");
          clearTimeout(binding.timer);
          const from = sequence(p.from),
            seq = sequence(p.seq),
            value = decode(p.dataBase64);
          if (
            from !== binding.cursor.seq ||
            seq !== from + BigInt(value.length)
          ) {
            this.notice("输出序列缺失，正在重新同步…");
            this.attach(binding);
            return;
          }
          const cursor = { ...binding.cursor, seq };
          binding.outstanding = false;
          if (!value.length) {
            binding.outstanding = false;
            this.synchronized(binding);
            this.pull(binding, 50);
          } else {
            this.write(binding, value, () => {
              this.applied = cursor;
              binding.cursor = cursor;
            });
          }
          return;
        }
        case "terminal.resync":
          if (event.payload.streamId === binding.streamId) {
            clearTimeout(binding.timer);
            binding.outstanding = false;
            if (event.payload.reason === "replay_expired" && binding.ready) {
              // Keep a recovered lease usable between snapshots. A producer faster
              // than the renderer must not prevent the user from sending Ctrl+C.
              this.notice("输出较快，稍后恢复画面；仍可输入以停止任务");
              binding.timer = setTimeout(() => this.attach(binding), 500);
            } else this.attach(binding);
          }
          return;
        case "terminal.ended": {
          const p = event.payload;
          if (p.streamId !== binding.streamId) return;
          if (!binding.cursor || p.epoch !== binding.cursor.epoch)
            throw Error("invalid final cursor");
          const final = sequence(p.seq);
          if (final < binding.cursor.seq) throw Error("invalid final cursor");
          this.ended = true;
          this.outputLoss ||= p.truncated;
          binding.ending = true;
          binding.lease = null;
          this.writable(false);
          if (!p.available) {
            this.outputLoss = true;
            clearTimeout(binding.timer);
            binding.streamId = null;
            binding.outstanding = false;
          } else if (p.complete) {
            if (
              !binding.outstanding ||
              binding.snapshot ||
              binding.cursor.seq !== final
            )
              throw Error("unapplied final cursor");
            clearTimeout(binding.timer);
            binding.outstanding = false;
            this.completed = true;
          }
          this.notice(this.finalNotice());
          return;
        }
        case "terminal.lease.changed":
          if (
            event.payload.streamId !== binding.streamId ||
            !binding.ready ||
            binding.ending ||
            this.lifecycle() !== "running"
          )
            return;
          binding.lease = event.payload.writable ? event.payload.leaseId : null;
          this.writable(!!binding.lease);
          this.notice(
            binding.lease
              ? "已连接 · 输入在设备上执行"
              : "只读 · 其他窗口可能正在操作",
          );
          if (binding.lease) this.onLease();
          return;
        case "terminal.input.ack":
          binding.pending.delete(event.payload.inputId);
          return;
        case "terminal.unavailable":
          if (event.payload.requestId !== binding.requestId || binding.streamId)
            return;
          this.ended = true;
          this.outputLoss = true;
          binding.ending = true;
          binding.lease = null;
          clearTimeout(binding.timer);
          this.writable(false);
          this.notice(this.finalNotice());
          return;
        case "terminal.input.rejected":
          if (!binding.pending.has(event.payload.inputId)) return;
          this.notice(event.payload.message);
          binding.socket.close();
          return;
        case "error":
          this.notice(event.payload.message);
          if (!binding.ready) {
            clearTimeout(binding.timer);
            binding.timer = setTimeout(() => this.attach(binding), 1000);
          }
          return;
      }
    } catch {
      this.notice("输出序列缺失，正在重新同步…");
      binding.socket.close();
    }
  }
  private write(
    binding: Binding,
    value: Uint8Array,
    applied: () => void,
  ): void {
    const streamId = binding.streamId;
    this.writing = new Promise<void>((resolve) => {
      this.term.write(value, () => {
        if (!this.disposed) {
          applied(); // This cursor describes bytes actually applied, even after socket close.
          this.rendered = true;
        }
        resolve();
        if (
          !this.disposed &&
          this.binding === binding &&
          binding.streamId === streamId
        ) {
          binding.outstanding = false;
          this.synchronized(binding);
          this.pull(binding);
        }
      });
    });
  }
  private disconnectNotice(pending: boolean): string {
    if (this.ended || this.lifecycle() !== "running") return this.finalNotice();
    return pending
      ? "连接中断：部分输入结果未知，未自动重发"
      : "连接中断，任务继续运行";
  }
  private finalNotice(): string {
    if (this.lifecycle() === "lost")
      return "终端状态未知 · 无法确认进程退出或最后输出";
    if (!this.ended && this.lifecycle() === "terminating")
      return "正在结束 · 等待进程退出";
    return this.outputLoss
      ? "终端已退出 · 最后输出可能不完整"
      : this.completed
        ? "终端已退出 · 最后输出已接收"
        : "终端已退出 · 正在接收最后输出";
  }
  private synchronized(binding: Binding): void {
    if (
      !binding.ready &&
      !binding.snapshot &&
      binding.cursor &&
      binding.cursor.seq >= binding.head
    ) {
      binding.ready = true;
      this.claim(false);
    }
  }
  claim(force = true): void {
    const b = this.binding;
    if (b?.streamId && b.ready && !b.ending && this.lifecycle() === "running")
      this.send(b, {
        type: "terminal.lease.claim",
        payload: { sessionId: this.sessionId, streamId: b.streamId, force },
      });
  }
  resize(cols: number, rows: number): void {
    const b = this.binding;
    if (b?.lease && b.ready && !b.ending && this.lifecycle() === "running")
      this.send(b, {
        type: "terminal.resize",
        payload: { sessionId: this.sessionId, leaseId: b.lease, cols, rows },
      });
  }
  transmit(data: string): void {
    const b = this.binding;
    if (
      !b?.lease ||
      !b.ready ||
      b.ending ||
      b.socket.readyState !== OPEN ||
      this.lifecycle() !== "running"
    )
      return;
    const encoded = new TextEncoder().encode(data);
    for (let offset = 0; offset < encoded.length; offset += 8192) {
      if (
        b.pending.size >= MAX_PENDING_INPUTS ||
        b.socket.bufferedAmount > 256 * 1024
      ) {
        this.notice("输入尚未确认，已暂停连接；不会自动重发");
        b.socket.close();
        return;
      }
      const inputId = randomId();
      b.pending.add(inputId);
      this.send(b, {
        type: "terminal.input",
        payload: {
          sessionId: this.sessionId,
          leaseId: b.lease,
          inputId,
          dataBase64: encode(encoded.subarray(offset, offset + 8192)),
        },
      });
    }
  }
  dispose(): void {
    this.connect(null);
    this.disposed = true;
  }
}
