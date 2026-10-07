import { useEffect, useRef, useState, type ReactNode } from "react";

export function GlassPopover({
  label,
  icon,
  children,
  className = "",
  value,
}: {
  label: string;
  icon: ReactNode;
  children: (close: () => void) => ReactNode;
  className?: string;
  value?: string;
}) {
  const [open, setOpen] = useState(false);
  const root = useRef<HTMLDivElement>(null);
  const trigger = useRef<HTMLButtonElement>(null);
  const close = () => {
    setOpen(false);
    trigger.current?.focus();
  };
  useEffect(() => {
    if (!open) return;
    const outside = (event: PointerEvent) => {
      if (!root.current?.contains(event.target as Node)) setOpen(false);
    };
    const escape = (event: KeyboardEvent) => {
      if (event.key === "Escape") {
        setOpen(false);
        trigger.current?.focus();
      }
    };
    document.addEventListener("pointerdown", outside);
    document.addEventListener("keydown", escape);
    return () => {
      document.removeEventListener("pointerdown", outside);
      document.removeEventListener("keydown", escape);
    };
  }, [open]);
  return (
    <div ref={root} className={`glass-popover ${className}`}>
      <button
        ref={trigger}
        className="glass-trigger"
        type="button"
        aria-label={label}
        title={label}
        aria-expanded={open}
        data-value={value}
        onClick={() => setOpen(!open)}
      >
        {icon}
      </button>
      {open && (
        <section className="glass-panel" aria-label={label}>
          {children(close)}
        </section>
      )}
    </div>
  );
}
