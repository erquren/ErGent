import { t, useLanguage } from "./i18n";
import { randomId } from "./browser";
import { useEffect, useRef, useState } from "react";
import { Terminal, type ITheme } from "@xterm/xterm";
import { FitAddon } from "@xterm/addon-fit";
import type {
  BrowserCommand,
  Event,
  SessionInfo,
} from "../../../packages/protocol-ts";
import "@xterm/xterm/css/xterm.css";

const bytes = (data: string) =>
  Uint8Array.from(atob(data), (c) => c.charCodeAt(0));
function base64(data: Uint8Array) {
  let text = "";
  for (const byte of data) text += String.fromCharCode(byte);
  return btoa(text);
}

export function TerminalView({
  session,
  socket,
  theme,
}: {
  session: SessionInfo;
  socket: WebSocket | null;
  theme: ITheme;
}) {
  useLanguage();
  const themeRef = useRef(theme);
  themeRef.current = theme;
  const container = useRef<HTMLDivElement>(null);
  const [writable, setWritable] = useState(false);
  const [notice, setNotice] = useState("正在同步终端…");
  const [dimensions, setDimensions] = useState("");
  const [ctrl, setCtrl] = useState(false);
  const ctrlRef = useRef(false);
  const terminal = useRef<Terminal | null>(null);
  const sendInput = useRef<(data: string) => void>(() => {});
  const setControl = (value: boolean) => {
    ctrlRef.current = value;
    setCtrl(value);
  };
  const shortcut = (data: string) => {
    setControl(false);
    sendInput.current(data);
  };
  const claim = useRef<() => void>(() => {});
  useEffect(() => {
    setWritable(false);
    setControl(false);
    if (!socket || !container.current) {
      setNotice("连接已断开，本机任务继续运行");
      return;
    }
    let disposed = false,
      lease: string | null = null,
      sequence: bigint | null = null,
      queued = 0;
    let receivingSnapshot = true;
    const pending = new Set<string>();
    const term = new Terminal({
      cursorBlink: true,
      fontFamily: '"SFMono-Regular", Consolas, "Liberation Mono", monospace',
      fontSize: 13,
      lineHeight: 1.35,
      scrollback: 2000,
      theme: themeRef.current,
    });
    const fit = new FitAddon();
    term.loadAddon(fit);
    term.open(container.current);
    terminal.current = term;
    fit.fit();
    const send = (command: BrowserCommand) => {
      if (socket.readyState === WebSocket.OPEN && !disposed)
        socket.send(JSON.stringify(command));
    };
    const resize = () => {
      if (!lease) return;
      fit.fit();
      const cols = Math.max(20, Math.min(500, term.cols)),
        rows = Math.max(5, Math.min(200, term.rows));
      send({
        type: "terminal.resize",
        payload: { sessionId: session.id, leaseId: lease, cols, rows },
      });
      setDimensions(`${cols} × ${rows}`);
    };
    const observer = new ResizeObserver(resize);
    observer.observe(container.current);
    const write = (data: string, done?: () => void) => {
      const value = bytes(data);
      queued += value.length;
      if (queued > 2 * 1024 * 1024) {
        setNotice("输出较快，重新连接以同步画面…");
        socket.close();
        return;
      }
      term.write(value, () => {
        queued -= value.length;
        if (!disposed) done?.();
      });
    };
    const message = (raw: MessageEvent) => {
      let event: Event;
      try {
        event = JSON.parse(raw.data);
      } catch {
        socket.close();
        return;
      }
      const p = event.payload;
      if ("sessionId" in p && p.sessionId !== session.id) return;
      switch (event.type) {
        case "terminal.snapshot": {
          const p = event.payload;
          if (p.first) {
            receivingSnapshot = true;
            sequence = BigInt(p.seq);
            term.reset();
            term.resize(p.cols, p.rows);
          }
          write(
            p.dataBase64,
            p.last
              ? () => {
                  receivingSnapshot = false;
                  setNotice("只读 · 可以接管此终端");
                  send({
                    type: "terminal.lease.claim",
                    payload: { sessionId: session.id, force: false },
                  });
                }
              : undefined,
          );
          break;
        }
        case "terminal.output": {
          const p = event.payload,
            next = BigInt(p.seq);
          if (sequence === null || next !== sequence + 1n) {
            setNotice("输出序列缺失，正在重新同步…");
            socket.close();
            return;
          }
          sequence = next;
          write(p.dataBase64);
          break;
        }
        case "terminal.lease.changed":
          lease = event.payload.leaseId;
          setWritable(event.payload.writable);
          if (!event.payload.writable) setControl(false);
          setNotice(
            event.payload.writable
              ? t("已连接 · 输入在设备上执行")
              : t("只读 · 其他窗口可能正在操作"),
          );
          if (lease) {
            resize();
            if (!matchMedia("(pointer: coarse)").matches && innerWidth > 760)
              term.focus();
          }
          break;
        case "terminal.input.ack":
          pending.delete(event.payload.inputId);
          break;
        case "error":
          setNotice(event.payload.message);
          break;
      }
    };
    socket.addEventListener("message", message);
    const onClose = () => {
      lease = null;
      setWritable(false);
      setNotice(
        pending.size
          ? t("连接中断：部分输入结果未知，未自动重发")
          : t("连接中断，任务继续运行"),
      );
    };
    socket.addEventListener("close", onClose);
    const transmit = (data: string) => {
      if (!lease || receivingSnapshot || socket.readyState !== WebSocket.OPEN)
        return;
      const encoded = new TextEncoder().encode(data);
      for (let offset = 0; offset < encoded.length; offset += 8192) {
        const inputId = randomId();
        pending.add(inputId);
        if (pending.size > 256) {
          setNotice("输入尚未确认，已暂停连接；不会自动重发");
          socket.close();
          break;
        }
        send({
          type: "terminal.input",
          payload: {
            sessionId: session.id,
            leaseId: lease,
            inputId,
            dataBase64: base64(encoded.slice(offset, offset + 8192)),
          },
        });
      }
    };
    sendInput.current = transmit;
    const input = term.onData((data) => {
      if (ctrlRef.current) {
        setControl(false);
        if (data.length === 1 && /[a-zA-Z@\[\\\]\^_? ]/.test(data)) {
          data =
            data === "?"
              ? "\x7f"
              : String.fromCharCode(data.toUpperCase().charCodeAt(0) & 31);
        }
      }
      transmit(data);
    });
    claim.current = () =>
      send({
        type: "terminal.lease.claim",
        payload: { sessionId: session.id, force: true },
      });
    const renew = setInterval(() => {
      if (lease)
        send({
          type: "terminal.lease.renew",
          payload: { sessionId: session.id, leaseId: lease },
        });
    }, 10000);
    send({ type: "terminal.attach", payload: { sessionId: session.id } });
    return () => {
      send({ type: "terminal.detach", payload: { sessionId: session.id } });
      disposed = true;
      clearInterval(renew);
      observer.disconnect();
      input.dispose();
      socket.removeEventListener("message", message);
      socket.removeEventListener("close", onClose);
      sendInput.current = () => {};
      terminal.current = null;
      term.dispose();
    };
  }, [session.id, socket]);
  useEffect(() => {
    if (terminal.current) terminal.current.options.theme = theme;
  }, [theme]);
  return (
    <div className="terminal-shell">
      <div className="terminal-canvas" ref={container} />
      <div className="terminal-keys" aria-label={t("终端快捷键")}>
        <div className="key-row">
          <button
            disabled={!writable}
            aria-pressed={ctrl}
            onPointerDown={(e) => e.preventDefault()}
            onClick={() => setControl(!ctrlRef.current)}
          >
            Ctrl
          </button>
          {[
            ["Ctrl+C", "\x03"],
            ["Ctrl+D", "\x04"],
            ["Esc", "\x1b"],
            ["Tab", "\t"],
          ].map(([label, value]) => (
            <button
              key={label}
              disabled={!writable}
              onPointerDown={(e) => e.preventDefault()}
              onClick={() => shortcut(value)}
            >
              {label}
            </button>
          ))}
          <button
            disabled={!writable}
            onClick={() => terminal.current?.focus()}
          >
            {t("键盘")}
          </button>
          <button onClick={() => terminal.current?.blur()}>
            {t("收起键盘")}
          </button>
        </div>
        <div className="key-row">
          {[
            ["↑", "A"],
            ["↓", "B"],
            ["←", "D"],
            ["→", "C"],
          ].map(([label, code]) => (
            <button
              key={label}
              aria-label={t("方向{key}", { key: label })}
              disabled={!writable}
              onPointerDown={(e) => e.preventDefault()}
              onClick={() =>
                shortcut(
                  `\x1b${terminal.current?.modes.applicationCursorKeysMode ? "O" : "["}${code}`,
                )
              }
            >
              {label}
            </button>
          ))}
          {[
            ["Ctrl+A", "\x01"],
            ["Ctrl+E", "\x05"],
            ["Ctrl+U", "\x15"],
            ["Enter", "\r"],
          ].map(([label, value]) => (
            <button
              key={label}
              disabled={!writable}
              onPointerDown={(e) => e.preventDefault()}
              onClick={() => shortcut(value)}
            >
              {label}
            </button>
          ))}
        </div>
        {ctrl && (
          <div className="ctrl-picker" aria-label={t("选择 Ctrl 组合键")}>
            <span>{t("选择组合键 · 再点 Ctrl 取消")}</span>
            <div className="key-row">
              {Array.from("ABCDEFGHIJKLMNOPQRSTUVWXYZ[\\]^_?").map((key) => (
                <button
                  key={key}
                  disabled={!writable}
                  aria-label={t("发送 Ctrl+{key}", { key })}
                  onPointerDown={(e) => e.preventDefault()}
                  onClick={() =>
                    shortcut(
                      key === "?"
                        ? "\x7f"
                        : String.fromCharCode(key.charCodeAt(0) & 31),
                    )
                  }
                >
                  {key}
                </button>
              ))}
            </div>
          </div>
        )}
      </div>
      <div className="terminal-status">
        <span>
          <i className={writable ? "dot green" : "dot"} />
          {t(notice)}
        </span>
        <span>
          {dimensions}
          {!writable && socket && session.lifecycle === "running" && (
            <button onClick={() => claim.current()}>{t("接管终端")}</button>
          )}
        </span>
      </div>
    </div>
  );
}
