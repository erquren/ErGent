import { useEffect, useId, useRef, useState } from "react";
import { X } from "lucide-react";
import { ModalDialog } from "./ModalDialog";
import { t } from "./i18n";
import type { TerminalTextSnapshot } from "./terminal-text";
import "./terminal-copy.css";

export function TerminalCopyDialog({
  snapshot,
  onClose,
}: {
  snapshot: TerminalTextSnapshot;
  onClose: () => void;
}) {
  const id = useId();
  const textArea = useRef<HTMLTextAreaElement>(null);
  const [scope, setScope] = useState<"screen" | "history" | "selection">(
    snapshot.selection ? "selection" : "screen",
  );
  const [selected, setSelected] = useState("");
  const [message, setMessage] = useState("");
  const text = snapshot[scope];
  useEffect(() => {
    const field = textArea.current!;
    const update = () =>
      setSelected(field.value.slice(field.selectionStart, field.selectionEnd));
    const selectionChanged = () => {
      if (document.activeElement === field) update();
    };
    field.addEventListener("select", update);
    document.addEventListener("selectionchange", selectionChanged);
    return () => {
      field.removeEventListener("select", update);
      document.removeEventListener("selectionchange", selectionChanged);
    };
  }, [scope]);
  const copy = async () => {
    const field = textArea.current!;
    const selection = field.value.slice(
      field.selectionStart,
      field.selectionEnd,
    );
    const value = selection || text;
    let copied = false;
    if (navigator.clipboard?.writeText) {
      try {
        await navigator.clipboard.writeText(value);
        copied = true;
      } catch {
        /* Offer native selection below. */
      }
    }
    if (!copied) {
      // LAN/Tailscale HTTP has no Clipboard API. Copy inside the modal: an
      // off-dialog textarea is inert while showModal() is active.
      const start = field.selectionStart;
      const end = field.selectionEnd;
      field.focus({ preventScroll: true });
      if (selection) field.setSelectionRange(start, end);
      else field.select();
      try {
        copied = document.execCommand("copy");
      } catch {
        /* Keep text selected. */
      }
    }
    setMessage(copied ? "已复制" : "请长按选中的文字，使用系统菜单复制。");
  };
  return (
    <ModalDialog
      className="terminal-copy-dialog"
      labelledBy={`${id}-title`}
      describedBy={`${id}-help`}
      onClose={onClose}
    >
      <header>
        <h2 id={`${id}-title`}>{t("复制终端内容")}</h2>
        <button
          data-dialog-autofocus
          aria-label={t("关闭复制面板")}
          onClick={onClose}
        >
          <X size={20} />
        </button>
      </header>
      <div className="terminal-copy-scopes" aria-label={t("复制范围")}>
        {[
          ...(snapshot.selection ? ["selection" as const] : []),
          "screen" as const,
          ...(!snapshot.alternate ? ["history" as const] : []),
        ].map((value) => (
          <button
            key={value}
            aria-pressed={scope === value}
            onClick={() => {
              setScope(value);
              setSelected("");
              setMessage("");
            }}
          >
            {t(
              value === "screen"
                ? "当前画面"
                : value === "history"
                  ? "已有滚动记录"
                  : "终端选中文字",
            )}
          </button>
        ))}
      </div>
      <p id={`${id}-help`}>
        {t(
          snapshot.alternate
            ? "全屏应用（如 Codex）仅提供当前画面，已被覆盖的内容不在此处。"
            : "仅包含终端当前保留的记录，不包含已清除或丢失的历史。",
        )}{" "}
        {t("内容已固定，可长按选字，不会随新输出移动。")}
      </p>
      <textarea
        key={scope}
        ref={textArea}
        readOnly
        value={text}
        aria-label={t("待复制文本")}
        spellCheck={false}
        onSelect={(event) => {
          const field = event.currentTarget;
          setSelected(
            field.value.slice(field.selectionStart, field.selectionEnd),
          );
        }}
      />
      <footer>
        <button
          onClick={() => {
            textArea.current?.focus({ preventScroll: true });
            textArea.current?.select();
          }}
          disabled={!text}
        >
          {t("全选")}
        </button>
        <button
          className="primary"
          disabled={!text}
          onPointerDown={(event) => event.preventDefault()}
          onClick={() => void copy()}
        >
          {t(selected ? "复制选中文字" : "复制全部文本")}
        </button>
        <span role="status">{t(message)}</span>
      </footer>
    </ModalDialog>
  );
}
