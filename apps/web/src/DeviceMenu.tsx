import { useId, useRef, useState } from "react";
import { MoreHorizontal, Pencil, Trash2 } from "lucide-react";
import type { MachineInfo } from "../../../packages/protocol-ts";
import { t } from "./i18n";

// Native popovers escape scrolling containers, including the mobile drawer.
export function DeviceMenu({
  machine,
  disabled,
  onRename,
  onRemove,
}: {
  machine: MachineInfo;
  disabled: boolean;
  onRename: (machine: MachineInfo) => void;
  onRemove: (machine: MachineInfo) => void;
}) {
  const id = useId();
  const trigger = useRef<HTMLButtonElement>(null);
  const panel = useRef<HTMLDivElement>(null);
  const [open, setOpen] = useState(false);
  const choose = (action: (machine: MachineInfo) => void) => {
    panel.current?.hidePopover();
    trigger.current?.focus();
    action(machine);
  };
  return (
    <>
      <button
        ref={trigger}
        type="button"
        className="device-menu-trigger"
        aria-label={t("管理 {name}", { name: machine.name })}
        title={t("管理设备")}
        aria-expanded={open}
        aria-controls={id}
        popoverTarget={id}
        disabled={disabled}
      >
        <MoreHorizontal size={18} />
      </button>
      <div
        ref={panel}
        id={id}
        popover="auto"
        className="device-menu"
        aria-label={t("管理 {name}", { name: machine.name })}
        onToggle={() =>
          setOpen(panel.current?.matches(":popover-open") ?? false)
        }
        onBeforeToggle={(event) => {
          if (event.newState !== "open") return;
          const rect = trigger.current!.getBoundingClientRect();
          const element = panel.current!;
          element.style.left = `${Math.max(8, Math.min(rect.right - 176, innerWidth - 184))}px`;
          element.style.top = `${Math.max(8, Math.min(rect.bottom + 6, innerHeight - 112))}px`;
        }}
      >
        <button type="button" onClick={() => choose(onRename)}>
          <Pencil size={16} />
          {t("重命名设备")}
        </button>
        <button
          type="button"
          className="device-remove"
          onClick={() => choose(onRemove)}
        >
          <Trash2 size={16} />
          {t("移除设备")}
        </button>
      </div>
    </>
  );
}
