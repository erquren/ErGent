import { ChevronRight, Plus, Terminal, X } from "lucide-react";
import type { MachineInfo, SessionInfo } from "../../../packages/protocol-ts";
import { ModalDialog } from "./ModalDialog";
import { t } from "./i18n";

export function DeviceTerminals({
  machine,
  sessions,
  busy,
  error,
  onClose,
  onCreate,
  onSelect,
}: {
  machine: MachineInfo;
  sessions: SessionInfo[];
  busy: boolean;
  error: string;
  onClose: () => void;
  onCreate: () => void;
  onSelect: (session: SessionInfo) => void;
}) {
  return (
    <ModalDialog
      className="device-manager device-terminals"
      labelledBy="device-terminals-title"
      onClose={onClose}
      busy={busy}
    >
      <header className="device-manager-heading">
        <div className="device-terminals-heading">
          <h2 id="device-terminals-title">{machine.name}</h2>
          <div className="managed-device-status">
            <span>
              <i className={`dot ${machine.online ? "green" : ""}`} />
              {t(machine.online ? "在线" : "离线")}
            </span>
            <span>{t("{count} 个终端", { count: sessions.length })}</span>
          </div>
        </div>
        <div className="device-heading-actions">
          <button
            aria-label={t("新建终端")}
            title={t("新建终端")}
            disabled={busy || !machine.online}
            onClick={onCreate}
          >
            <Plus size={20} />
          </button>
          <button
            autoFocus
            data-dialog-autofocus
            aria-label={t("关闭终端列表")}
            disabled={busy}
            onClick={onClose}
          >
            <X size={20} />
          </button>
        </div>
      </header>
      <div className="device-manager-list">
        {error && (
          <p className="error" role="alert">
            {t(error)}
          </p>
        )}
        {sessions.length === 0 ? (
          <p className="device-manager-empty">{t("暂无终端")}</p>
        ) : (
          <ul>
            {sessions.map((session) => (
              <li key={session.id}>
                <button
                  className="device-terminal-row"
                  disabled={busy}
                  onClick={() => onSelect(session)}
                >
                  <Terminal size={19} />
                  <span>{session.title}</span>
                  <ChevronRight size={17} />
                </button>
              </li>
            ))}
          </ul>
        )}
      </div>
    </ModalDialog>
  );
}
