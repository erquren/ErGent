import { useEffect, useRef, type ReactNode } from "react";

export function ModalDialog({
  children,
  className,
  labelledBy,
  describedBy,
  busy = false,
  onClose,
}: {
  children: ReactNode;
  className: string;
  labelledBy: string;
  describedBy?: string;
  busy?: boolean;
  onClose: () => void;
}) {
  const dialog = useRef<HTMLDialogElement>(null);
  const returnFocus = useRef(document.activeElement as HTMLElement | null);
  useEffect(() => {
    const element = dialog.current!;
    const previousFocus = returnFocus.current;
    element.showModal();
    element
      .querySelector<HTMLElement>(
        "[data-dialog-autofocus], input:not([disabled])",
      )
      ?.focus();
    return () => {
      element.close();
      if (previousFocus?.isConnected && !element.contains(previousFocus)) {
        if (innerWidth <= 760 && previousFocus.closest(".sidebar")) {
          document.querySelector<HTMLElement>(".mobile-nav-toggle")?.focus();
        } else previousFocus.focus();
      } else {
        const fallback =
          document.querySelector<HTMLElement>("dialog[open] button") ??
          document.querySelector<HTMLElement>(".device-heading-actions button");
        fallback?.focus();
      }
    };
  }, []);
  return (
    <dialog
      ref={dialog}
      className={className}
      aria-labelledby={labelledBy}
      aria-describedby={describedBy}
      aria-modal="true"
      aria-busy={busy}
      onCancel={(event) => {
        event.preventDefault();
        if (!busy) onClose();
      }}
      onClick={(event) => {
        if (event.target !== event.currentTarget || busy) return;
        const rect = event.currentTarget.getBoundingClientRect();
        if (
          event.clientX < rect.left ||
          event.clientX > rect.right ||
          event.clientY < rect.top ||
          event.clientY > rect.bottom
        )
          onClose();
      }}
    >
      {children}
    </dialog>
  );
}
