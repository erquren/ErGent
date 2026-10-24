import { t, useLanguage } from "./i18n";
import { randomId, copyText } from "./browser";
import React, { useEffect, useRef, useState } from "react";
import { createRoot } from "react-dom/client";
import {
  ArrowUpRight,
  ArrowRight,
  ChevronRight,
  LogOut,
  Plus,
  Terminal as TerminalIcon,
  X,
  Monitor,
  Copy,
  Pencil,
  Sun,
  Moon,
  Palette,
  Info,
  Check,
  Languages,
  Settings2,
  PanelLeftClose,
  PanelLeftOpen,
} from "lucide-react";
import type {
  Event,
  MachineInfo,
  SessionInfo,
  ToolStatus,
} from "../../../packages/protocol-ts";
import { api, ApiError, waitOperation } from "./api";
import { AuthScreen } from "./AuthScreen";
import { TerminalView } from "./TerminalView";
import "./style.css";
import { useAppearance, terminalTheme, terminalThemes } from "./theme";
import { GlassPopover } from "./GlassPopover";
import "./glass.css";
import { toolLabel } from "./tool-status";
import { InventoryReceiver, INVENTORY_TIMEOUT_MS } from "./inventory";
import { normalizeName, nameValidationMessage } from "./names";
import { CreatedSessions } from "./created-sessions";
import { DeviceManager } from "./DeviceManager";
import { DeviceTerminals } from "./DeviceTerminals";
import { DeviceMenu } from "./DeviceMenu";
import { ModalDialog } from "./ModalDialog";
import "./devices.css";

function App() {
  const { preference, setPreference } = useLanguage();
  const { appearance, resolved, setAppearance } = useAppearance();
  const themePicker = (
    <GlassPopover
      label={t("网页外观")}
      className="appearance-control"
      value={appearance}
      icon={resolved === "dark" ? <Moon size={18} /> : <Sun size={18} />}
    >
      {(close) => (
        <>
          <div className="popover-heading">{t("外观")}</div>
          {(
            [
              ["system", t("跟随系统"), resolved === "dark" ? Moon : Sun],
              ["light", t("白天模式"), Sun],
              ["dark", t("黑夜模式"), Moon],
            ] as const
          ).map(([value, label, Icon]) => (
            <button
              className="appearance-option"
              key={value}
              aria-pressed={appearance === value}
              onClick={() => {
                setAppearance(value);
                close();
              }}
            >
              <Icon size={17} />
              <span>{t(label)}</span>
              {appearance === value && <Check size={16} />}
            </button>
          ))}
        </>
      )}
    </GlassPopover>
  );
  const languagePicker = (
    <GlassPopover
      label={t("界面语言")}
      className="language-control"
      value={preference}
      icon={<Languages size={18} />}
    >
      {(close) => (
        <>
          <div className="popover-heading">{t("语言")}</div>
          {(
            [
              ["auto", t("跟随设备")],
              ["zh", "简体中文"],
              ["en", "English"],
            ] as const
          ).map(([value, label]) => (
            <button
              key={value}
              className="appearance-option"
              aria-pressed={preference === value}
              onClick={() => {
                setPreference(value);
                close();
              }}
            >
              <span
                lang={
                  value === "zh" ? "zh-CN" : value === "en" ? "en" : undefined
                }
              >
                {t(label)}
              </span>
              {preference === value && <Check size={16} />}
            </button>
          ))}
        </>
      )}
    </GlassPopover>
  );
  const [navOpen, setNavOpen] = useState(false);
  const [sidebarCollapsed, setSidebarCollapsed] = useState(false);
  const [managerOpen, setManagerOpen] = useState(false);
  const [terminalListMachineId, setTerminalListMachineId] = useState<
    string | null
  >(null);
  const [collapsedMachines, setCollapsedMachines] = useState<Set<string>>(
    new Set(),
  );
  const removedMachines = useRef(new Set<string>());
  useEffect(() => {
    const viewport = window.visualViewport;
    const update = () => {
      if (!viewport || viewport.scale === 1) {
        document.documentElement.style.setProperty(
          "--viewport-height",
          `${viewport?.height ?? innerHeight}px`,
        );
        document.documentElement.style.setProperty(
          "--viewport-top",
          `${viewport?.offsetTop ?? 0}px`,
        );
      }
    };
    const escape = (e: KeyboardEvent) => {
      if (e.key === "Escape") setNavOpen(false);
    };
    update();
    viewport?.addEventListener("resize", update);
    viewport?.addEventListener("scroll", update);
    window.addEventListener("resize", update);
    window.addEventListener("keydown", escape);
    return () => {
      viewport?.removeEventListener("resize", update);
      viewport?.removeEventListener("scroll", update);
      window.removeEventListener("resize", update);
      window.removeEventListener("keydown", escape);
    };
  }, []);
  const [user, setUser] = useState<string | null>(null);
  const [machines, setMachines] = useState<MachineInfo[]>([]),
    [sessions, setSessions] = useState<SessionInfo[]>([]);
  const [selected, setSelected] = useState<string | null>(null),
    [socket, setSocket] = useState<WebSocket | null>(null);
  const [finishedSession, setFinishedSession] = useState<SessionInfo | null>(
    null,
  );
  const selectedRef = useRef(selected);
  selectedRef.current = selected;
  useEffect(() => {
    setFinishedSession((session) =>
      session?.id === selected ? session : null,
    );
  }, [selected]);
  const [connected, setConnected] = useState(false),
    [error, setError] = useState("");
  const [modal, setModal] = useState<
      "create" | "enroll" | "rename" | "terminal-rename" | "remove" | null
    >(null),
    [token, setToken] = useState("");
  const [editingSession, setEditingSession] = useState("");
  const [terminalName, setTerminalName] = useState("");
  const [deviceName, setDeviceName] = useState("");
  const [machineId, setMachineId] = useState(""),
    [cwd, setCwd] = useState(""),
    [title, setTitle] = useState("Terminal");
  const [busy, setBusy] = useState(false),
    [confirmClose, setConfirmClose] = useState(false);
  const [toolNotices, setToolNotices] = useState<
    {
      eventId: string;
      sessionId: string;
      machineId: string;
      title: string;
      status: ToolStatus;
    }[]
  >([]);
  const wsRef = useRef<WebSocket | null>(null);
  const createdSessions = useRef(new CreatedSessions());
  useEffect(() => {
    createdSessions.current = new CreatedSessions();
    removedMachines.current = new Set();
    setCollapsedMachines(new Set());
    if (!user) return;
    let disposed = false;
    let timer: ReturnType<typeof setTimeout>;
    let previousTools = new Map<string, string>();
    let receivedInventory = false;
    const connect = () => {
      if (disposed) return;
      receivedInventory = false;
      const ws = new WebSocket(
        `${location.protocol === "https:" ? "wss" : "ws"}://${location.host}/api/v1/ws/browser`,
        "ergent.preview.v3",
      );
      const inventory = new InventoryReceiver();
      let inventoryTimer: ReturnType<typeof setTimeout> | undefined;
      wsRef.current = ws;
      ws.onopen = () => {
        if (disposed || wsRef.current !== ws) return;
        setConnected(true);
        setSocket(ws);
      };
      ws.onmessage = (raw) => {
        if (disposed || wsRef.current !== ws) return;
        let event: Event;
        try {
          if (
            typeof raw.data !== "string" ||
            new TextEncoder().encode(raw.data).length > 64 * 1024
          )
            throw new Error("Invalid message size");
          const decoded = inventory.accept(JSON.parse(raw.data));
          if (inventory.inProgress && inventoryTimer === undefined)
            inventoryTimer = setTimeout(() => ws.close(), INVENTORY_TIMEOUT_MS);
          if (!decoded) return;
          clearTimeout(inventoryTimer);
          inventoryTimer = undefined;
          event = decoded;
        } catch {
          clearTimeout(inventoryTimer);
          ws.close();
          return;
        }
        if (event.type === "inventory.sync") {
          for (const machine of event.payload.machines) {
            if (machine.revoked) removedMachines.current.add(machine.id);
            if (removedMachines.current.has(machine.id)) machine.revoked = true;
          }
          event.payload.sessions = event.payload.sessions.filter(
            (session) => !removedMachines.current.has(session.machineId),
          );
          const previousSelection = createdSessions.current.get(
            selectedRef.current,
          );
          if (
            previousSelection &&
            removedMachines.current.has(previousSelection.machineId)
          ) {
            setSelected(null);
            setConfirmClose(false);
          }
          const newNotices = event.payload.sessions.flatMap((s) => {
            const status = s.toolStatus;
            if (
              !receivedInventory ||
              !status ||
              previousTools.get(s.id) === status.eventId ||
              s.lifecycle !== "running" ||
              !event.payload.machines.some(
                (m) => m.id === s.machineId && m.online,
              ) ||
              ![
                "completed",
                "waiting_for_approval",
                "failed",
                "interrupted",
              ].includes(status.state)
            )
              return [];
            const m = event.payload.machines.find((m) => m.id === s.machineId);
            return [
              {
                eventId: status.eventId,
                sessionId: s.id,
                machineId: s.machineId,
                title: `${m?.name ?? t("设备")} / ${s.title}`,
                status,
              },
            ];
          });
          setToolNotices((items) =>
            [...newNotices, ...items]
              .filter(
                (notice) => !removedMachines.current.has(notice.machineId),
              )
              .slice(0, 4),
          );
          previousTools = new Map(
            event.payload.sessions
              .filter((s) => s.toolStatus)
              .map((s) => [s.id, s.toolStatus!.eventId]),
          );
          receivedInventory = true;
          setMachines(event.payload.machines);
          setSessions(
            createdSessions.current.receive(
              event.payload.machines,
              event.payload.sessions,
            ),
          );
          // Keep only the selected final surface so its pull stream can drain.
          // Exited sessions still disappear from the sidebar and are not re-opened.
          const observed = createdSessions.current.get(selectedRef.current);
          setFinishedSession(
            observed && !["running", "terminating"].includes(observed.lifecycle)
              ? observed
              : null,
          );
        }
      };
      ws.onclose = () => {
        clearTimeout(inventoryTimer);
        if (disposed) return;
        setConnected(false);
        setSocket(null);
        timer = setTimeout(() => {
          api("/auth/me")
            .then(connect)
            .catch(() => setUser(null));
        }, 1500);
      };
    };
    connect();
    return () => {
      disposed = true;
      createdSessions.current = new CreatedSessions();
      clearTimeout(timer);
      wsRef.current?.close();
      wsRef.current = null;
      setSocket(null);
      setConnected(false);
      setSelected(null);
      selectedRef.current = null;
      setFinishedSession(null);
      setSessions([]);
      setMachines([]);
    };
  }, [user]);
  const active =
      sessions.find((s) => s.id === selected) ??
      (finishedSession?.id === selected ? finishedSession : undefined),
    machine = machines.find((m) => m.id === active?.machineId);
  const visible = machines.filter((m) => !m.revoked),
    online = visible.filter((m) => m.online);
  const terminalListMachine = visible.find(
    (m) => m.id === terminalListMachineId,
  );
  const act = async (fn: () => Promise<void>) => {
    setError("");
    setBusy(true);
    try {
      await fn();
    } catch (e) {
      setError((e as Error).message);
    } finally {
      setBusy(false);
    }
  };
  const newTerminal = (m?: MachineInfo) => {
    const target = m || online[0];
    if (!target) {
      setError("请先接入一台在线设备");
      return;
    }
    setMachineId(target.id);
    setCwd(target.defaultCwd);
    const names = new Set(
      sessions.filter((s) => s.machineId === target.id).map((s) => s.title),
    );
    let number = 1;
    while (names.has(`${t("终端")} ${number}`)) number++;
    setTitle(`${t("终端")} ${number}`);
    setNavOpen(false);
    setModal("create");
  };
  const create = () =>
    act(async () => {
      const tracker = createdSessions.current;
      const result = await api<{ operationId: string; sessionId: string }>(
        `/machines/${machineId}/sessions`,
        "POST",
        { title: normalizeName(title), cwd, cols: 120, rows: 36 },
        randomId(),
      );
      setModal(null);
      const operation = await waitOperation(result.operationId);
      if (tracker !== createdSessions.current) return;
      const session = operation.result?.session;
      if (
        !session ||
        session.id !== result.sessionId ||
        session.machineId !== machineId
      )
        throw new Error(t("创建结果缺少会话信息，请刷新会话列表"));
      const created = tracker.accept(session);
      if (created) {
        setCollapsedMachines((items) => {
          const next = new Set(items);
          next.delete(created.machineId);
          return next;
        });
        setSessions((items) =>
          items.some((item) => item.id === created.id)
            ? items
            : [...items, created],
        );
        setSelected(created.id);
        setTerminalListMachineId(null);
      }
    });
  const enroll = () =>
    act(async () => {
      const result = await api<{ token: string }>("/enrollments", "POST", {});
      setToken(result.token);
      setNavOpen(false);
      setModal("enroll");
    });
  const editMachine = (m: MachineInfo) => {
    setError("");
    setMachineId(m.id);
    setDeviceName(m.name);
    setNavOpen(false);
    setModal("rename");
  };
  const confirmRemoveMachine = (m: MachineInfo) => {
    setError("");
    setMachineId(m.id);
    setDeviceName(m.name);
    setNavOpen(false);
    setModal("remove");
  };
  const removeMachine = () => {
    if (busy) return;
    void act(async () => {
      // A 404 means another window has already removed this device.
      try {
        await api(`/machines/${machineId}/revoke`, "POST", {});
      } catch (err) {
        if (!(err instanceof ApiError) || err.status !== 404) throw err;
      }
      removedMachines.current.add(machineId);
      createdSessions.current.revoke(machineId);
      setMachines((items) =>
        items.map((m) =>
          m.id === machineId ? { ...m, revoked: true, online: false } : m,
        ),
      );
      setSessions(createdSessions.current.visible());
      setFinishedSession((session) =>
        session?.machineId === machineId ? null : session,
      );
      setToolNotices((items) =>
        items.filter((notice) => notice.machineId !== machineId),
      );
      if (active?.machineId === machineId) setSelected(null);
      setConfirmClose(false);
      setModal(null);
    });
  };
  const renameMachine = () =>
    act(async () => {
      const result = await api<{ id: string; name: string }>(
        `/machines/${machineId}`,
        "PATCH",
        { name: normalizeName(deviceName) },
      );
      setMachines((items) =>
        items.map((m) =>
          m.id === result.id ? { ...m, name: result.name } : m,
        ),
      );
      setModal(null);
    });
  const renameTerminal = () =>
    act(async () => {
      await api(`/sessions/${editingSession}`, "PATCH", {
        title: normalizeName(terminalName),
      });
      setModal(null);
    });
  const changeTerminalTheme = (terminal_theme: string) =>
    act(async () => {
      if (!active) return;
      await api(`/sessions/${active.id}`, "PATCH", {
        terminalTheme: terminal_theme,
      });
    });
  const palette = terminalTheme(active?.terminalTheme, resolved);
  const close = () =>
    act(async () => {
      if (!active) return;
      const tracker = createdSessions.current;
      const result = await api<{ operationId: string }>(
        `/sessions/${active.id}/close`,
        "POST",
        {},
        randomId(),
      );
      setConfirmClose(false);
      const operation = await waitOperation(result.operationId);
      if (tracker !== createdSessions.current) return;
      const session = operation.result?.session;
      if (session?.id === active.id && session.machineId === active.machineId) {
        const observed = tracker.observe(session);
        if (!observed) return;
        setSessions(tracker.visible());
        if (
          selectedRef.current === observed.id &&
          !["running", "terminating"].includes(observed.lifecycle)
        )
          setFinishedSession(observed);
      }
    });

  if (!user)
    return (
      <AuthScreen
        controls={
          <>
            {languagePicker}
            {themePicker}
          </>
        }
        onAuthenticated={setUser}
      />
    );
  return (
    <div
      className={`app ${navOpen ? "nav-open" : ""} ${sidebarCollapsed ? "sidebar-collapsed" : ""}`}
    >
      {navOpen && (
        <button
          className="nav-backdrop"
          aria-label={t("关闭设备列表")}
          onClick={() => setNavOpen(false)}
        />
      )}
      <aside
        className="sidebar"
        id="machine-navigation"
        aria-label={t("设备与终端")}
      >
        <div className="brand">
          <img className="product-mark" src="/branding/logo.svg" alt="" />
          <img
            className="product-wordmark"
            src="/branding/wordmark.png"
            alt="ErGent"
          />
          <button
            className="sidebar-close"
            aria-label={t("收起设备列表")}
            title={t("收起设备列表")}
            onClick={() => {
              setSidebarCollapsed(true);
              setNavOpen(false);
            }}
          >
            <PanelLeftClose size={18} />
          </button>
        </div>
        <div className="section-heading">
          <span>{t("MACHINES")}</span>
          <div className="device-heading-actions">
            <button
              aria-label={t("设备管理")}
              title={t("设备管理")}
              onClick={() => {
                setNavOpen(false);
                setError("");
                setManagerOpen(true);
              }}
            >
              <Settings2 size={16} />
            </button>
            <button
              aria-label={t("接入设备")}
              title={t("接入设备")}
              onClick={enroll}
            >
              <Plus size={16} />
            </button>
          </div>
        </div>
        <nav className="machine-list">
          {visible.map((m) => (
            <div className="machine-group" key={m.id}>
              <div className="machine-heading">
                <button
                  type="button"
                  className="machine-name machine-toggle"
                  aria-label={t(
                    collapsedMachines.has(m.id)
                      ? "展开 {name} 的终端"
                      : "收起 {name} 的终端",
                    { name: m.name },
                  )}
                  aria-expanded={!collapsedMachines.has(m.id)}
                  aria-controls={`machine-sessions-${m.id}`}
                  title={m.name}
                  onClick={() =>
                    setCollapsedMachines((items) => {
                      const next = new Set(items);
                      if (next.has(m.id)) next.delete(m.id);
                      else next.add(m.id);
                      return next;
                    })
                  }
                >
                  <ChevronRight size={16} className="machine-chevron" />
                  <strong>{m.name}</strong>
                </button>
                <DeviceMenu
                  machine={m}
                  disabled={busy}
                  onRename={editMachine}
                  onRemove={confirmRemoveMachine}
                />
                <i className={`dot ${m.online ? "green" : ""}`} />
                <button
                  aria-label={t("在 {name} 新建终端", { name: m.name })}
                  onClick={() => newTerminal(m)}
                  disabled={!m.online}
                >
                  <Plus size={15} />
                </button>
              </div>
              <div
                className="session-list"
                id={`machine-sessions-${m.id}`}
                hidden={collapsedMachines.has(m.id)}
              >
                {sessions
                  .filter((s) => s.machineId === m.id)
                  .map((s) => (
                    <button
                      className={`session-card ${s.id === selected ? "selected" : ""}`}
                      key={s.id}
                      onClick={() => {
                        setSelected(s.id);
                        setNavOpen(false);
                        setConfirmClose(false);
                      }}
                    >
                      <TerminalIcon size={15} />
                      <div>
                        <strong>{s.title}</strong>
                        {s.toolStatus && (
                          <small
                            className={`tool-state state-${s.toolStatus.state}`}
                            title={
                              m.online
                                ? toolLabel(s.toolStatus)
                                : t("离线 · 上次状态")
                            }
                          >
                            {m.online
                              ? toolLabel(s.toolStatus)
                              : `${t("离线")} · ${toolLabel(s.toolStatus)}`}
                          </small>
                        )}
                      </div>
                      <i
                        className={`dot ${s.lifecycle === "running" && m.online ? "green" : ""}`}
                      />
                    </button>
                  ))}
                {!sessions.some((s) => s.machineId === m.id) && (
                  <div className="no-sessions">{t("暂无终端")}</div>
                )}
              </div>
            </div>
          ))}
          {visible.length === 0 && (
            <div className="sidebar-empty">{t("暂无设备")}</div>
          )}
        </nav>
        <button
          className="new-terminal"
          onClick={() => newTerminal()}
          disabled={!online.length}
        >
          <Plus size={17} />
          {t("新建终端")}
        </button>
        <div className="sidebar-bottom">
          <span className="avatar">{user[0].toUpperCase()}</span>
          <div>
            <strong>{user}</strong>
          </div>
          <button
            aria-label={t("退出登录")}
            onClick={() =>
              act(async () => {
                await api("/auth/logout", "POST", {});
                setUser(null);
                setSessions([]);
                setMachines([]);
                setSelected(null);
                setToolNotices([]);
                setManagerOpen(false);
                setTerminalListMachineId(null);
                setModal(null);
              })
            }
          >
            <LogOut size={17} />
          </button>
        </div>
      </aside>
      <main className="main">
        <header className="topbar">
          <button
            className="desktop-nav-toggle"
            aria-label={t(sidebarCollapsed ? "展开设备列表" : "收起设备列表")}
            title={t(sidebarCollapsed ? "展开设备列表" : "收起设备列表")}
            aria-expanded={!sidebarCollapsed}
            aria-controls="machine-navigation"
            onClick={() => setSidebarCollapsed(!sidebarCollapsed)}
          >
            {sidebarCollapsed ? (
              <PanelLeftOpen size={18} />
            ) : (
              <PanelLeftClose size={18} />
            )}
          </button>
          <button
            className="mobile-nav-toggle"
            aria-expanded={navOpen}
            aria-controls="machine-navigation"
            onClick={() => setNavOpen(!navOpen)}
          >
            {t("设备 / 终端")}
          </button>
          <div>
            <strong className="desktop-workspace-title">
              {machine?.name || t("Overview")}
            </strong>
            <strong
              className="mobile-workspace-title"
              title={machine?.name}
              role={active ? "heading" : undefined}
              aria-level={active ? 1 : undefined}
            >
              {active?.title || machine?.name || t("Overview")}
            </strong>
          </div>
          <div className="header-controls">
            {languagePicker}
            {themePicker}
          </div>
          <div className="connection">
            <i className={`dot ${connected ? "green" : ""}`} />
            {connected ? t("工作台已连接") : t("重新连接中")}
            <span className="divider" />
            {t("{count} 台设备在线", { count: online.length })}
          </div>
        </header>
        {error && !modal && !managerOpen && !terminalListMachine && (
          <div className="banner error" role="alert">
            {t(error)}
            <button aria-label={t("关闭提示")} onClick={() => setError("")}>
              <X size={15} />
            </button>
          </div>
        )}
        {active ? (
          <div className="terminal-page">
            <div className="terminal-heading">
              <div className="page-heading">
                <div>
                  <h1>
                    <span className="terminal-title" title={active.title}>
                      {active.title}
                    </span>
                    <span className="badge">
                      {active.lifecycle === "exited"
                        ? t("已退出")
                        : active.lifecycle === "lost"
                          ? t("状态未知")
                          : active.lifecycle === "terminating"
                            ? t("正在结束")
                            : !machine?.online
                              ? t("OFFLINE")
                              : t("运行中")}
                    </span>
                  </h1>
                </div>
                <div className="page-actions">
                  {active.lifecycle !== "running" &&
                    active.lifecycle !== "terminating" && (
                      <button
                        className="terminal-dismiss"
                        onClick={() => setSelected(null)}
                      >
                        {t("关闭已结束的终端视图")}
                      </button>
                    )}
                  <button
                    aria-label={t("重命名终端")}
                    onClick={() => {
                      setEditingSession(active.id);
                      setTerminalName(active.title);
                      setError("");
                      setModal("terminal-rename");
                    }}
                  >
                    <Pencil size={15} />
                  </button>
                  {confirmClose ? (
                    <>
                      <span>{t("结束该终端及其任务？")}</span>
                      <button
                        className="danger"
                        onClick={close}
                        disabled={busy}
                      >
                        {t("确认结束")}
                      </button>
                      <button onClick={() => setConfirmClose(false)}>
                        {t("取消")}
                      </button>
                    </>
                  ) : (
                    <button
                      disabled={
                        active.lifecycle !== "running" || !machine?.online
                      }
                      aria-label={t("结束终端")}
                      title={t("结束终端")}
                      onClick={() => setConfirmClose(true)}
                    >
                      <X size={14} />
                    </button>
                  )}
                </div>
              </div>
              <div className="tool-summary">
                <span
                  className={
                    active.toolStatus
                      ? `tool-state state-${active.toolStatus.state}`
                      : "muted"
                  }
                >
                  {active.toolStatus
                    ? `${machine?.online ? "" : t("离线 · 上次状态：")}${toolLabel(active.toolStatus)}`
                    : null}
                </span>
                <div className="terminal-utilities">
                  <GlassPopover
                    label={t("Codex 状态接入说明")}
                    className="help-control"
                    icon={<Info size={17} />}
                  >
                    {() => (
                      <div className="tool-help-content">
                        <div className="popover-heading">
                          {t("Codex 状态接入说明")}
                        </div>
                        <p>{t("在新终端中执行一次：")}</p>
                        <code>
                          {machine?.os === "windows"
                            ? '\"%ERGENT_AGENT_BIN%\" setup-codex'
                            : '\"$ERGENT_AGENT_BIN\" setup-codex'}
                        </code>
                        <p>
                          {t(
                            "通过以下命令启动 Codex，并在 /hooks 中信任新增 hooks：",
                          )}
                        </p>
                        <code>
                          {machine?.os === "windows"
                            ? '\"%ERGENT_AGENT_BIN%\" codex'
                            : '\"$ERGENT_AGENT_BIN\" codex'}
                        </code>
                        <p className="privacy-note">
                          {t(
                            "只同步状态，不上传提示词或回答。Windows 命令适用于 cmd。",
                          )}
                        </p>
                      </div>
                    )}
                  </GlassPopover>
                  <GlassPopover
                    label={t("终端配色")}
                    className="palette-control"
                    value={active.terminalTheme || "auto"}
                    icon={
                      <>
                        <Palette size={17} />
                        <span className="palette-current">
                          {t(
                            terminalThemes.find(
                              ([v]) => v === (active.terminalTheme || "auto"),
                            )?.[1] || "跟随网页",
                          )}
                        </span>
                      </>
                    }
                  >
                    {(close) => (
                      <>
                        <div className="popover-heading">{t("终端配色")}</div>
                        <div className="palette-grid">
                          {terminalThemes.map(([value, label]) => {
                            const colors = terminalTheme(value, resolved);
                            return (
                              <button
                                key={value}
                                disabled={busy}
                                className="palette-option"
                                aria-label={t(label)}
                                aria-pressed={
                                  (active.terminalTheme || "auto") === value
                                }
                                onClick={() => {
                                  void changeTerminalTheme(value);
                                  close();
                                }}
                              >
                                <span
                                  className="palette-preview"
                                  style={{
                                    background: colors.background,
                                    color: colors.foreground,
                                  }}
                                >
                                  <span className="palette-prompt">
                                    ❯{" "}
                                    <span style={{ color: colors.blue }}>
                                      ergent
                                    </span>
                                    <span className="palette-cursor" />
                                  </span>
                                  <span className="palette-dots">
                                    {[
                                      colors.red,
                                      colors.green,
                                      colors.yellow,
                                      colors.blue,
                                      colors.magenta,
                                    ].map((color, i) => (
                                      <i
                                        key={i}
                                        style={{ background: color }}
                                      />
                                    ))}
                                  </span>
                                </span>
                                <span className="palette-label">
                                  {t(label)}
                                  {(active.terminalTheme || "auto") ===
                                    value && <Check size={13} />}
                                </span>
                              </button>
                            );
                          })}
                        </div>
                      </>
                    )}
                  </GlassPopover>
                </div>
              </div>
            </div>
            <div
              className="terminal-frame"
              style={
                {
                  "--terminal-bg": palette.background,
                  "--terminal-fg": palette.foreground,
                } as React.CSSProperties
              }
            >
              <div className="terminal-toolbar">
                <div>
                  <TerminalIcon size={14} />
                  <strong>{active.shell.split(/[/\\]/).pop()}</strong>
                  <span>·</span>
                  <span>{machine?.name}</span>
                </div>
                <div>
                  <i className={`dot ${machine?.online ? "green" : ""}`} />
                  {machine?.os} / {machine?.arch}
                </div>
              </div>
              <TerminalView
                key={active.id}
                theme={palette}
                session={active}
                socket={machine?.online ? socket : null}
              />
            </div>
            <div className="page-foot">
              <span>{t("终端运行在你的设备上 · 关闭网页不会结束任务")}</span>
            </div>
          </div>
        ) : (
          <div className="overview">
            <div className="overview-heading">
              <div>
                <h1>{t("设备与终端")}</h1>
              </div>
              <button
                className="primary"
                onClick={() => newTerminal()}
                disabled={!online.length}
              >
                <Plus size={17} />
                {t("新建终端")}
              </button>
            </div>
            <div className="stats">
              <div>
                <span>{t("在线设备")}</span>
                <strong>{online.length}</strong>
              </div>
              <div>
                <span>{t("运行中的终端")}</span>
                <strong>
                  {
                    sessions.filter(
                      (s) =>
                        s.lifecycle === "running" &&
                        online.some((m) => m.id === s.machineId),
                    ).length
                  }
                </strong>
              </div>
            </div>
            <div className="devices-title">
              <h2>{t("你的设备")}</h2>
            </div>
            <div className="device-grid">
              {visible.map((m) => (
                <article className="device-card" key={m.id}>
                  <div className="device-top">
                    <div className="device-icon">
                      <Monitor size={22} />
                    </div>
                    <span>
                      <i className={`dot ${m.online ? "green" : ""}`} />
                      {m.online ? t("ONLINE") : t("OFFLINE")}
                    </span>
                  </div>
                  <div className="device-name-row">
                    <h3 title={m.name}>{m.name}</h3>
                    <DeviceMenu
                      machine={m}
                      disabled={busy}
                      onRename={editMachine}
                      onRemove={confirmRemoveMachine}
                    />
                  </div>
                  <p>
                    {m.os} <span>·</span> {m.arch}
                  </p>
                  <div className="device-bottom">
                    <span>
                      {t("{count} 个终端", {
                        count: sessions.filter((s) => s.machineId === m.id)
                          .length,
                      })}
                    </span>
                    <button
                      onClick={() => {
                        setError("");
                        setTerminalListMachineId(m.id);
                      }}
                      aria-label={t("查看 {name} 的终端", { name: m.name })}
                      title={t("查看终端")}
                    >
                      <ArrowUpRight size={20} />
                    </button>
                  </div>
                </article>
              ))}
              <button className="add-device" onClick={enroll}>
                <div>
                  <Plus size={23} />
                </div>
                <strong>{t("接入设备")}</strong>
                <span>{t("macOS、Linux 或 Windows")}</span>
              </button>
            </div>
          </div>
        )}
      </main>
      {toolNotices.length > 0 && (
        <aside
          className="tool-notifications"
          aria-label={t("工具通知")}
          aria-live="polite"
        >
          {toolNotices.map((n) => (
            <div className="tool-notification" key={n.eventId}>
              <button
                onClick={() => {
                  setSelected(n.sessionId);
                  setCollapsedMachines((items) => {
                    const next = new Set(items);
                    next.delete(n.machineId);
                    return next;
                  });
                  setNavOpen(false);
                  setToolNotices((items) =>
                    items.filter((i) => i.eventId !== n.eventId),
                  );
                }}
              >
                <strong>{toolLabel(n.status)}</strong>
                <span>{n.title}</span>
              </button>
              <button
                aria-label={t("关闭工具通知")}
                onClick={() =>
                  setToolNotices((items) =>
                    items.filter((i) => i.eventId !== n.eventId),
                  )
                }
              >
                <X size={16} />
              </button>
            </div>
          ))}
        </aside>
      )}
      {terminalListMachine && (
        <DeviceTerminals
          machine={terminalListMachine}
          sessions={sessions.filter(
            (s) => s.machineId === terminalListMachine.id,
          )}
          busy={busy}
          error={modal ? "" : error}
          onClose={() => {
            setTerminalListMachineId(null);
            setError("");
          }}
          onCreate={() => newTerminal(terminalListMachine)}
          onSelect={(session) => {
            setSelected(session.id);
            setConfirmClose(false);
            setNavOpen(false);
            setCollapsedMachines((items) => {
              const next = new Set(items);
              next.delete(session.machineId);
              return next;
            });
            setTerminalListMachineId(null);
          }}
        />
      )}
      {managerOpen && (
        <DeviceManager
          machines={visible}
          sessions={sessions}
          busy={busy}
          error={modal ? "" : error}
          onClose={() => {
            setManagerOpen(false);
            setError("");
          }}
          onEnroll={enroll}
          onRename={editMachine}
          onRemove={confirmRemoveMachine}
        />
      )}
      {modal && (
        <ModalDialog
          className="modal"
          labelledBy="modal-title"
          describedBy={
            modal === "remove" ? "remove-device-description" : undefined
          }
          busy={busy}
          onClose={() => {
            setModal(null);
            setToken("");
            setError("");
          }}
        >
          <div className="modal-heading">
            <h2 id="modal-title">
              {modal === "create"
                ? t("新建终端")
                : modal === "terminal-rename"
                  ? t("重命名终端")
                  : modal === "rename"
                    ? t("重命名设备")
                    : modal === "remove"
                      ? t("移除「{name}」？", { name: deviceName })
                      : t("接入设备")}
            </h2>
            <button
              aria-label={t("关闭弹窗")}
              onClick={() => {
                setModal(null);
                setToken("");
                setError("");
              }}
              disabled={busy}
            >
              <X size={20} />
            </button>
          </div>
          {error && (
            <div className="error" role="alert">
              {t(error)}
            </div>
          )}
          {modal === "remove" ? (
            <>
              <p id="remove-device-description">
                {t(
                  "将断开连接并撤销接入权限，再次使用需要重新注册。设备上的文件不会删除。",
                )}
              </p>
              {sessions.some(
                (session) =>
                  session.machineId === machineId &&
                  ["running", "terminating"].includes(session.lifecycle),
              ) && (
                <p className="remove-device-warning">
                  {t(
                    "该设备有 {count} 个未结束的终端，移除后将无法通过工作台访问。这不会结束设备上的任务。",
                    {
                      count: sessions.filter(
                        (session) =>
                          session.machineId === machineId &&
                          ["running", "terminating"].includes(
                            session.lifecycle,
                          ),
                      ).length,
                    },
                  )}
                </p>
              )}
              <div className="dialog-actions">
                <button
                  autoFocus
                  data-dialog-autofocus
                  disabled={busy}
                  onClick={() => {
                    setModal(null);
                    setError("");
                  }}
                >
                  {t("取消")}
                </button>
                <button
                  className="danger"
                  disabled={busy}
                  onClick={removeMachine}
                >
                  {t(busy ? "正在移除…" : "移除设备")}
                </button>
              </div>
            </>
          ) : modal === "terminal-rename" ? (
            <form
              onSubmit={(e) => {
                e.preventDefault();
                void renameTerminal();
              }}
            >
              <label>
                {t("终端名称")}
                <input
                  autoFocus
                  required
                  value={terminalName}
                  onChange={(e) => {
                    setTerminalName(e.target.value);
                    e.target.setCustomValidity(
                      t(nameValidationMessage(e.target.value)),
                    );
                  }}
                />
              </label>
              <button
                className="primary full"
                disabled={busy || !terminalName.trim()}
              >
                {t("保存名称")}
              </button>
            </form>
          ) : modal === "rename" ? (
            <form
              onSubmit={(e) => {
                e.preventDefault();
                void renameMachine();
              }}
            >
              <label>
                {t("设备名称")}
                <input
                  autoFocus
                  required
                  value={deviceName}
                  onChange={(e) => {
                    setDeviceName(e.target.value);
                    e.target.setCustomValidity(
                      t(nameValidationMessage(e.target.value)),
                    );
                  }}
                />
              </label>
              <button
                className="primary full"
                disabled={busy || !deviceName.trim()}
              >
                {t("保存名称")}
              </button>
            </form>
          ) : modal === "create" ? (
            <form
              onSubmit={(e) => {
                e.preventDefault();
                void create();
              }}
            >
              <label>
                {t("设备")}
                <select
                  value={machineId}
                  onChange={(e) => {
                    setMachineId(e.target.value);
                    setCwd(
                      machines.find((m) => m.id === e.target.value)
                        ?.defaultCwd || "",
                    );
                  }}
                >
                  {online.map((m) => (
                    <option key={m.id} value={m.id}>
                      {m.name}
                    </option>
                  ))}
                </select>
              </label>
              <label>
                {t("终端名称")}
                <input
                  value={title}
                  onChange={(e) => {
                    setTitle(e.target.value);
                    e.target.setCustomValidity(
                      t(nameValidationMessage(e.target.value)),
                    );
                  }}
                  required
                />
              </label>
              <label>
                {t("设备上的工作目录")}
                <input
                  value={cwd}
                  onChange={(e) => setCwd(e.target.value)}
                  placeholder="/home/me/project"
                  required
                />
              </label>
              <button className="primary full" disabled={busy}>
                {t("创建并连接")}
                <ArrowRight size={17} />
              </button>
            </form>
          ) : (
            <>
              <p>
                {t(
                  "在目标设备上运行 Agent 注册命令，然后输入下面的一次性凭据。凭据 10 分钟内有效。",
                )}
              </p>
              <code className="command-block">
                ergent-client enroll --server {location.origin}{" "}
                {t('--name "我的开发机"')}
              </code>
              <div className="token-label">
                {t("一次性注册凭据")}
                <button
                  onClick={() =>
                    void copyText(token).catch(() =>
                      setError("复制失败，请手动复制"),
                    )
                  }
                >
                  <Copy size={14} />
                  {t("复制")}
                </button>
              </div>
              <code className="token">{token}</code>
              <p className="form-note">
                {t("注册后运行")}
                <code>ergent-client run</code>
                {t("。使用默认配置路径时，在同一目录运行两条命令。")}
              </p>
              <button
                className="primary full"
                onClick={() => {
                  setModal(null);
                  setToken("");
                }}
              >
                {t("完成，等待设备连接")}
              </button>
            </>
          )}
        </ModalDialog>
      )}
    </div>
  );
}
createRoot(document.getElementById("root")!).render(<App />);
