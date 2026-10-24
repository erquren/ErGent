import { t, useLanguage } from "./i18n";
import { TerminalConnection } from "./terminal-connection";
import { useEffect, useId, useLayoutEffect, useRef, useState } from "react";
import { ChevronDown, MoreHorizontal } from "lucide-react";
import { Terminal, type ITheme } from "@xterm/xterm";
import { FitAddon } from "@xterm/addon-fit";
import { TerminalCopyDialog } from "./TerminalCopyDialog";
import {
  captureTerminalText,
  type TerminalTextSnapshot,
} from "./terminal-text";
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
  const [moreKeys, setMoreKeys] = useState(false);
  const [keyboardOpen, setKeyboardOpen] = useState(false);
  const [alternate, setAlternate] = useState(false);
  const [copySnapshot, setCopySnapshot] = useState<TerminalTextSnapshot | null>(
    null,
  );
  const extraKeysId = useId();
  const ctrlRef = useRef(false);
  const terminal = useRef<Terminal | null>(null);
  const resizeTerminal = useRef<() => void>(() => {});
  const sendInput = useRef<(data: string) => void>(() => {});
  const setControl = (value: boolean) => {
    ctrlRef.current = value;
    setCtrl(value);
  };
  const shortcut = (data: string) => {
    setControl(false);
    sendInput.current(data);
  };
  const arrow = (code: string) =>
    shortcut(
      `\x1b${terminal.current?.modes.applicationCursorKeysMode ? "O" : "["}${code}`,
    );
  const navigate = (direction: "previous" | "next" | "latest") => {
    const term = terminal.current;
    if (!term) return;
    setControl(false);
    // Read the live buffer: output may switch modes before React renders.
    if (term.buffer.active.type === "alternate") {
      if (!writable || !running) return;
      const key =
        direction === "previous"
          ? "\x1b[5~"
          : direction === "next"
            ? "\x1b[6~"
            : `\x1b${term.modes.applicationCursorKeysMode ? "O" : "["}F`;
      sendInput.current(key);
    } else if (direction === "latest") {
      term.scrollToBottom();
    } else {
      term.scrollPages(direction === "previous" ? -1 : 1);
    }
  };
  const claim = useRef<() => void>(() => {});
  const connection = useRef<TerminalConnection | null>(null);
  useEffect(() => {
    setWritable(false);
    setControl(false);
    setHistoryLost(false);
    setAlternate(false);
    setCopySnapshot(null);
    if (!container.current) return;
    const term = new Terminal({
      cursorBlink: true,
      disableStdin: true,
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
    const bufferChange = term.buffer.onBufferChange((buffer) =>
      setAlternate(buffer.type === "alternate"),
    );
    setKeyboardOpen(false);
    const host = container.current;
    // Own touch scrolling so terminal mouse reporting cannot swallow gestures.
    let touchY: number | null = null;
    let remainder = 0;
    const touchStart = (event: TouchEvent) => {
      touchY = event.touches.length === 1 ? event.touches[0].clientY : null;
      remainder = 0;
      event.stopPropagation();
    };
    const touchMove = (event: TouchEvent) => {
      if (touchY === null || event.touches.length !== 1) return;
      const y = event.touches[0].clientY;
      const rowHeight = host
        .querySelector(".xterm-screen")
        ?.getBoundingClientRect().height;
      const lineHeight = rowHeight ? rowHeight / term.rows : 18;
      remainder += touchY - y;
      touchY = y;
      const lines = Math.trunc(remainder / lineHeight);
      if (lines) {
        term.scrollLines(lines);
        remainder -= lines * lineHeight;
      }
      event.preventDefault();
      event.stopPropagation();
    };
    const touchEnd = (event: TouchEvent) => {
      touchY = null;
      // Suppress compatibility mouse events which would focus the textarea.
      if (event.cancelable) event.preventDefault();
      event.stopPropagation();
    };
    const focused = () => {
      setKeyboardOpen(true);
      // :focus changes the mobile header immediately. Gate input before the
      // next keystroke, rather than waiting for an asynchronous ResizeObserver.
      resize();
    };
    const blurred = () => {
      setKeyboardOpen(false);
      resize();
    };
    host.addEventListener("touchstart", touchStart, {
      capture: true,
      passive: true,
    });
    host.addEventListener("touchmove", touchMove, {
      capture: true,
      passive: false,
    });
    host.addEventListener("touchend", touchEnd, {
      capture: true,
      passive: false,
    });
    host.addEventListener("touchcancel", touchEnd, true);
    term.textarea?.addEventListener("focus", focused);
    term.textarea?.addEventListener("blur", blurred);
    const inputWritable = (value: boolean) => {
      // Match the renderer's native input state to the transport gate. During
      // recovery/read-only leases, keystrokes must not appear to be accepted or
      // queue up for later replay. Expose the boundary to assistive technology
      // without blurring or setting native readOnly: changing native editability
      // during a viewport resize can disturb a mobile software keyboard.
      term.options.disableStdin = !value;
      if (term.textarea) {
        term.textarea.setAttribute("aria-readonly", String(!value));
      }
    };
    inputWritable(false);
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
    resizeTerminal.current = resize;
    const transport = new TerminalConnection(
      session.id,
      term,
      setNotice,
      (value) => {
        canWrite = value;
        inputWritable(value);
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
      host.removeEventListener("touchstart", touchStart, true);
      host.removeEventListener("touchmove", touchMove, true);
      host.removeEventListener("touchend", touchEnd, true);
      host.removeEventListener("touchcancel", touchEnd, true);
      term.textarea?.removeEventListener("focus", focused);
      term.textarea?.removeEventListener("blur", blurred);
      transport.dispose();
      observer.disconnect();
      input.dispose();
      bufferChange.dispose();
      connection.current = null;
      sendInput.current = () => {};
      claim.current = () => {};
      terminal.current = null;
      resizeTerminal.current = () => {};
      term.dispose();
    };
  }, [session.id]);
  useLayoutEffect(() => {
    resizeTerminal.current();
  }, [moreKeys]);
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
        <div className="key-row primary-keys">
          <button
            className="keyboard-toggle"
            disabled={!keyboardOpen && (!writable || !running)}
            aria-label={t(keyboardOpen ? "收起键盘" : "键盘")}
            onPointerDown={(e) => e.preventDefault()}
            onClick={() => {
              setMoreKeys(false);
              setControl(false);
              if (keyboardOpen) terminal.current?.blur();
              else terminal.current?.focus();
            }}
          >
            {t(keyboardOpen ? "收起键盘" : "键盘")}
          </button>
          <div
            className="terminal-navigation"
            role="group"
            aria-label={t("终端翻页")}
          >
            {(
              [
                ["previous", "上一页", "PageUp"],
                ["next", "下一页", "PageDown"],
                ["latest", "回到最新", "End"],
              ] as const
            ).map(([direction, label, key]) => (
              <button
                key={direction}
                type="button"
                disabled={alternate && (!writable || !running)}
                title={
                  alternate ? t("向当前应用发送 {key}", { key }) : t(label)
                }
                onPointerDown={(event) => event.preventDefault()}
                onClick={() => navigate(direction)}
              >
                {t(label)}
              </button>
            ))}
          </div>
          <button
            disabled={!writable || !running}
            onPointerDown={(e) => e.preventDefault()}
            onClick={() => shortcut("\x1b")}
          >
            Esc
          </button>
          <button
            aria-label={t("方向{key}", { key: "↑" })}
            disabled={!writable || !running}
            onPointerDown={(e) => e.preventDefault()}
            onClick={() => arrow("A")}
          >
            ↑
          </button>
          <button
            aria-label={t(moreKeys ? "收起快捷键" : "更多快捷键")}
            title={t(moreKeys ? "收起快捷键" : "更多快捷键")}
            aria-expanded={moreKeys}
            aria-controls={extraKeysId}
            onPointerDown={(e) => e.preventDefault()}
            onClick={() => {
              setControl(false);
              setMoreKeys(!moreKeys);
            }}
          >
            {moreKeys ? (
              <ChevronDown size={16} />
            ) : (
              <MoreHorizontal size={16} />
            )}
          </button>
        </div>
        <div id={extraKeysId} className="extra-keys" hidden={!moreKeys}>
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
              ["Tab", "\t"],
              ["Ctrl+D", "\x04"],
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
          <div className="key-row">
            {[
              ["↓", "B"],
              ["←", "D"],
              ["→", "C"],
            ].map(([label, code]) => (
              <button
                key={label}
                aria-label={t("方向{key}", { key: label })}
                disabled={!writable || !running}
                onPointerDown={(e) => e.preventDefault()}
                onClick={() => arrow(code)}
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
          <button
            className="terminal-copy-trigger"
            aria-label={t("复制终端内容")}
            onPointerDown={(event) => event.preventDefault()}
            onClick={(event) => {
              const term = terminal.current;
              if (!term) return;
              // Capture before blur/keyboard dismissal can resize the terminal.
              setCopySnapshot(captureTerminalText(term));
              term.blur();
              event.currentTarget.focus();
              setControl(false);
              setMoreKeys(false);
            }}
          >
            {t("复制")}
          </button>
          {!writable && socket && session.lifecycle === "running" && (
            <button onClick={() => claim.current()}>{t("接管终端")}</button>
          )}
        </span>
      </div>
      {copySnapshot && (
        <TerminalCopyDialog
          snapshot={copySnapshot}
          onClose={() => setCopySnapshot(null)}
        />
      )}
    </div>
  );
}
