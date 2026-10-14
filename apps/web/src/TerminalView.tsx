import { t, useLanguage } from "./i18n";
import { TerminalConnection } from "./terminal-connection";
import { useEffect, useRef, useState } from "react";
import { Terminal, type ITheme } from "@xterm/xterm";
import { FitAddon } from "@xterm/addon-fit";
import type { SessionInfo } from "../../../packages/protocol-ts";
import "@xterm/xterm/css/xterm.css";

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
  const lifecycleRef = useRef(session.lifecycle);
  lifecycleRef.current = session.lifecycle;
  const running = session.lifecycle === "running";
  const container = useRef<HTMLDivElement>(null);
  const [writable, setWritable] = useState(false);
  const [notice, setNotice] = useState("正在同步终端…");
  const [dimensions, setDimensions] = useState("");
  const [historyLost, setHistoryLost] = useState(false);
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
  const connection = useRef<TerminalConnection | null>(null);
  useEffect(() => {
    setWritable(false);
    setControl(false);
    setHistoryLost(false);
    if (!container.current) return;
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
    let canWrite = false;
    const resize = () => {
      if (!canWrite || lifecycleRef.current !== "running") return;
      fit.fit();
      const cols = Math.max(20, Math.min(500, term.cols)),
        rows = Math.max(5, Math.min(200, term.rows));
      transport.resize(cols, rows);
      setDimensions(`${cols} × ${rows}`);
    };
    const transport = new TerminalConnection(
      session.id,
      term,
      setNotice,
      (value) => {
        canWrite = value;
        setWritable(value);
        if (!value) setControl(false);
      },
      () => {
        resize();
        if (!matchMedia("(pointer: coarse)").matches && innerWidth > 760)
          term.focus();
      },
      () => setHistoryLost(true),
      () => lifecycleRef.current,
    );
    connection.current = transport;
    const observer = new ResizeObserver(resize);
    observer.observe(container.current);
    sendInput.current = (data) => transport.transmit(data);
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
      transport.transmit(data);
    });
    claim.current = () => transport.claim();
    return () => {
      transport.dispose();
      observer.disconnect();
      input.dispose();
      connection.current = null;
      sendInput.current = () => {};
      claim.current = () => {};
      terminal.current = null;
      term.dispose();
    };
  }, [session.id]);
  useEffect(() => {
    connection.current?.connect(socket);
  }, [session.id, socket]);
  useEffect(() => {
    connection.current?.lifecycleChanged(session.outputTruncated);
  }, [session.id, session.lifecycle, session.outputTruncated]);
  useEffect(() => {
    if (terminal.current) terminal.current.options.theme = theme;
  }, [theme]);
  return (
    <div className="terminal-shell">
      <div className="terminal-canvas">
        <div className="terminal-host" ref={container} />
      </div>
      <div className="terminal-keys" aria-label={t("终端快捷键")}>
        <div className="key-row">
          <button
            disabled={!writable || !running}
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
              disabled={!writable || !running}
              onPointerDown={(e) => e.preventDefault()}
              onClick={() => shortcut(value)}
            >
              {label}
            </button>
          ))}
          <button
            disabled={!writable || !running}
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
              disabled={!writable || !running}
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
              disabled={!writable || !running}
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
                  disabled={!writable || !running}
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
          {historyLost && (
            <span title={t("已恢复当前画面；较早滚动历史不可用")}>
              {t("历史已重置")} ·{" "}
            </span>
          )}
          {dimensions}
          {!writable && socket && session.lifecycle === "running" && (
            <button onClick={() => claim.current()}>{t("接管终端")}</button>
          )}
        </span>
      </div>
    </div>
  );
}
