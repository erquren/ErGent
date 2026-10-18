import { Monitor, Plus, X } from "lucide-react";
import type { MachineInfo, SessionInfo } from "../../../packages/protocol-ts";
import { DeviceMenu } from "./DeviceMenu";
import { ModalDialog } from "./ModalDialog";
import { t } from "./i18n";

export function DeviceManager({
  machines,
  sessions,
  busy,
  error,
  onClose,
  onEnroll,
  onRename,
  onRemove,
}: {
  machines: MachineInfo[];
  sessions: SessionInfo[];
  busy: boolean;
  error: string;
  onClose: () => void;
  onEnroll: () => void;
  onRename: (machine: MachineInfo) => void;
  onRemove: (machine: MachineInfo) => void;
}) {
  return (
    <ModalDialog
      className="device-manager"
      labelledBy="device-manager-title"
      onClose={onClose}
      busy={busy}
    >
      <header className="device-manager-heading">
        <h2 id="device-manager-title">{t("设备管理")}</h2>
        <button
          autoFocus
          data-dialog-autofocus
          aria-label={t("关闭设备管理")}
          disabled={busy}
          onClick={onClose}
        >
          <X size={20} />
        </button>
      </header>
      <div className="device-manager-list">
        {error && (
          <p className="error" role="alert">
            {t(error)}
          </p>
        )}
        {machines.length === 0 ? (
          <p className="device-manager-empty">{t("暂无设备")}</p>
        ) : (
          <ul>
            {machines.map((machine) => (
              <li key={machine.id} className="managed-device">
                <Monitor size={22} className="managed-device-icon" />
                <div className="managed-device-info">
                  <h3 title={machine.name}>{machine.name}</h3>
                  <p>
                    {machine.os} · {machine.arch}
                  </p>
                  <div className="managed-device-status">
                    <span>
                      <i className={`dot ${machine.online ? "green" : ""}`} />
                      {t(machine.online ? "在线" : "离线")}
                    </span>
                    <span>
                      {t("{count} 个终端", {
                        count: sessions.filter(
                          (session) => session.machineId === machine.id,
                        ).length,
                      })}
                    </span>
                  </div>
                </div>
                <DeviceMenu
                  machine={machine}
                  disabled={busy}
                  onRename={onRename}
                  onRemove={onRemove}
                />
              </li>
            ))}
          </ul>
        )}
      </div>
      <footer>
        <button className="primary full" disabled={busy} onClick={onEnroll}>
          <Plus size={17} />
          {t("接入设备")}
        </button>
      </footer>
    </ModalDialog>
  );
}
