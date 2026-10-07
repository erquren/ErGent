import { t, useLanguage } from "./i18n";
import { randomId, copyText } from "./browser";
import React, { useEffect, useRef, useState } from "react";
import { createRoot } from "react-dom/client";
import {
  ArrowUpRight,
  ChevronDown,
  Folder,
  Laptop,
  LogOut,
  Plus,
  Server,
  Terminal as TerminalIcon,
  X,
  Monitor,
  Copy,
  ArrowRight,
  Radio,
  Pencil,
  Sun,
  Moon,
  Palette,
  Info,
  Check,
  Languages,
} from "lucide-react";
import type {
  Event,
  MachineInfo,
  SessionInfo,
  ToolStatus,
} from "../../../packages/protocol-ts";
import { api, csrf, waitOperation } from "./api";
import { TerminalView } from "./TerminalView";
import "./style.css";
import { useAppearance, terminalTheme, terminalThemes } from "./theme";
import { GlassPopover } from "./GlassPopover";
import "./glass.css";
import { toolLabel } from "./tool-status";

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
          <p className="popover-description">{t("选择此刻的光线。")}</p>
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
  const [user, setUser] = useState<string | null>(null),
    [loading, setLoading] = useState(true);
  const [machines, setMachines] = useState<MachineInfo[]>([]),
    [sessions, setSessions] = useState<SessionInfo[]>([]);
  const [selected, setSelected] = useState<string | null>(null),
    [socket, setSocket] = useState<WebSocket | null>(null);
  const [connected, setConnected] = useState(false),
    [error, setError] = useState("");
  const [modal, setModal] = useState<
      "create" | "enroll" | "rename" | "terminal-rename" | null
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
  const [username, setUsername] = useState("admin"),
    [password, setPassword] = useState("");
  const [toolNotices, setToolNotices] = useState<
    { eventId: string; sessionId: string; title: string; status: ToolStatus }[]
  >([]);
  const wsRef = useRef<WebSocket | null>(null);
  useEffect(() => {
    csrf()
      .then(() => api<{ username: string }>("/auth/me"))
      .then((v) => setUser(v.username))
      .catch(() => {})
      .finally(() => setLoading(false));
  }, []);
  useEffect(() => {
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
        "ergent.preview.v1",
      );
      wsRef.current = ws;
      ws.onopen = () => {
        setConnected(true);
        setSocket(ws);
      };
      ws.onmessage = (raw) => {
        let event: Event;
        try {
          event = JSON.parse(raw.data);
        } catch {
          ws.close();
          return;
        }
        if (event.type === "inventory.sync") {
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
                title: `${m?.name ?? t("设备")} / ${s.title}`,
                status,
              },
            ];
          });
          if (newNotices.length)
            setToolNotices((items) => [...newNotices, ...items].slice(0, 4));
          previousTools = new Map(
            event.payload.sessions
              .filter((s) => s.toolStatus)
              .map((s) => [s.id, s.toolStatus!.eventId]),
          );
          receivedInventory = true;
          setMachines(event.payload.machines);
          setSessions(
            event.payload.sessions.filter((s) => s.lifecycle === "running"),
          );
          const ended = new Set(
            event.payload.sessions
              .filter((s) => s.lifecycle !== "running")
              .map((s) => s.id),
          );
          setSelected((id) => (id && ended.has(id) ? null : id));
        }
      };
      ws.onclose = () => {
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
      clearTimeout(timer);
      wsRef.current?.close();
      setSocket(null);
      setConnected(false);
    };
  }, [user]);
  const active = sessions.find((s) => s.id === selected),
    machine = machines.find((m) => m.id === active?.machineId);
  const visible = machines.filter((m) => !m.revoked),
    online = visible.filter((m) => m.online);
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
      const result = await api<{ operationId: string; sessionId: string }>(
        `/machines/${machineId}/sessions`,
        "POST",
        { title, cwd, cols: 120, rows: 36 },
        randomId(),
      );
      setModal(null);
      await waitOperation(result.operationId);
      setSelected(result.sessionId);
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
  const renameMachine = () =>
    act(async () => {
      const result = await api<{ id: string; name: string }>(
        `/machines/${machineId}`,
        "PATCH",
        { name: deviceName },
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
        title: terminalName,
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
      const result = await api<{ operationId: string }>(
        `/sessions/${active.id}/close`,
        "POST",
        {},
        randomId(),
      );
      setConfirmClose(false);
      await waitOperation(result.operationId);
    });

  if (loading)
    return (
      <div className="loading">
        <img className="loading-logo" src="/branding/logo.svg" alt="ErGent" />
        <span>{t("Connecting to your workspace…")}</span>
      </div>
    );
  if (!user)
    return (
      <main className="login">
        <section className="login-story">
          <div className="wordmark">
            <img className="product-mark" src="/branding/logo.svg" alt="" />
            <img
              className="product-wordmark"
              src="/branding/wordmark.png"
              alt="ErGent"
            />
          </div>
          <div>
            <div className="eyebrow">{t("YOUR MACHINES. ONE WORKSPACE.")}</div>
            <h1>
              {t("开发现场，")}
              <br />
              {t("随时在场")}
              <span>{t("。")}</span>
            </h1>
            <p>
              {t("让每一台开发机，都成为浏览器里的工作台。")}
              <br />
              {t("真实终端，本机运行，随时继续。")}
            </p>
            <div className="platforms">
              <span>macOS</span>
              <span>Linux</span>
              <span>Windows</span>
            </div>
          </div>
          <div className="story-foot">
            {t("A personal command center for your work.")}
            <ArrowUpRight size={18} />
          </div>
        </section>
        <section className="login-form">
          <div className="header-controls">
            {languagePicker}
            {themePicker}
          </div>
          <div className="login-box">
            <span className="eyebrow">{t("WELCOME BACK")}</span>
            <h2>{t("进入你的工作台")}</h2>
            <p>{t("使用服务器上创建的管理员账号登录。")}</p>
            <form
              onSubmit={(e) => {
                e.preventDefault();
                void act(async () => {
                  await csrf();
                  const result = await api<{ user: { username: string } }>(
                    "/auth/login",
                    "POST",
                    { username, password },
                  );
                  setPassword("");
                  setUser(result.user.username);
                });
              }}
            >
              <label>
                {t("用户名")}
                <input
                  autoComplete="username"
                  value={username}
                  onChange={(e) => setUsername(e.target.value)}
                  required
                />
              </label>
              <label>
                {t("密码")}
                <input
                  type="password"
                  autoComplete="current-password"
                  value={password}
                  onChange={(e) => setPassword(e.target.value)}
                  required
                />
              </label>
              {error && <div className="error">{t(error)}</div>}
              <button className="primary full" disabled={busy}>
                {t("连接工作台")}
                <ArrowRight size={17} />
              </button>
            </form>
            <div className="login-note">
              <i className="dot green" />
              {t("Self-hosted · Your infrastructure")}
            </div>
          </div>
        </section>
      </main>
    );
  return (
    <div className={`app ${navOpen ? "nav-open" : ""}`}>
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
        </div>
        <div className="workspace-label">
          <div className="workspace-icon">P</div>
          <div>
            <strong>{t("Personal workspace")}</strong>
            <small>{t("你的开发控制台")}</small>
          </div>
          <ChevronDown size={14} />
        </div>
        <div className="section-heading">
          <span>{t("MACHINES")}</span>
          <button
            aria-label={t("接入设备")}
            title={t("接入设备")}
            onClick={enroll}
          >
            <Plus size={16} />
          </button>
        </div>
        <nav className="machine-list">
          {visible.map((m) => (
            <div className="machine-group" key={m.id}>
              <div className="machine-heading">
                <span className="machine-name">
                  {m.os === "linux" ? (
                    <Server size={16} />
                  ) : (
                    <Laptop size={16} />
                  )}
                  <strong>{m.name}</strong>
                </span>
                <button
                  aria-label={t("重命名 {name}", { name: m.name })}
                  title={t("重命名设备")}
                  onClick={() => editMachine(m)}
                >
                  <Pencil size={14} />
                </button>
                <i className={`dot ${m.online ? "green" : ""}`} />
                <button
                  aria-label={t("在 {name} 新建终端", { name: m.name })}
                  onClick={() => newTerminal(m)}
                  disabled={!m.online}
                >
                  <Plus size={15} />
                </button>
              </div>
              <div className="session-list">
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
                        <small>
                          {s.cwd.split(/[/\\]/).filter(Boolean).pop() || s.cwd}
                        </small>
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
            <div className="sidebar-empty">
              {t("还没有接入设备。")}
              <br />
              {t("安装 Agent 后即可开始。")}
            </div>
          )}
        </nav>
        <button
          className="new-terminal"
          onClick={() => newTerminal()}
          disabled={!online.length}
        >
          <Plus size={17} />
          {t("新建终端")}
          <span>＋</span>
        </button>
        <div className="sidebar-bottom">
          <span className="avatar">{user[0].toUpperCase()}</span>
          <div>
            <strong>{user}</strong>
            <small>{t("Personal account")}</small>
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
            className="mobile-nav-toggle"
            aria-expanded={navOpen}
            aria-controls="machine-navigation"
            onClick={() => setNavOpen(!navOpen)}
          >
            {t("设备 / 终端")}
          </button>
          <div>
            <span className="muted">{t("Workspace")}</span>
            <span className="slash">/</span>
            <strong>{machine?.name || t("Overview")}</strong>
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
        {error && (
          <div className="banner error" role="alert">
            {t(error)}
            <button aria-label={t("关闭提示")} onClick={() => setError("")}>
              <X size={15} />
            </button>
          </div>
        )}
        {active ? (
          <div className="terminal-page">
            <div className="page-heading">
              <div>
                <div className="eyebrow">{t("REMOTE TERMINAL")}</div>
                <h1>
                  <span className="terminal-title" title={active.title}>
                    {active.title}
                  </span>
                  <span className="badge">
                    {!machine?.online ? t("OFFLINE") : t("运行中")}
                  </span>
                </h1>
                <p>
                  <Folder size={14} />
                  {active.cwd}
                </p>
              </div>
              <div className="page-actions">
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
                    <button className="danger" onClick={close} disabled={busy}>
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
                  : t("等待任务")}
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
                        {t("让任务状态与你同步")}
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
                      <p className="popover-description">
                        {t("为这个终端选择一种氛围。")}
                      </p>
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
                                    <i key={i} style={{ background: color }} />
                                  ))}
                                </span>
                              </span>
                              <span className="palette-label">
                                {t(label)}
                                {(active.terminalTheme || "auto") === value && (
                                  <Check size={13} />
                                )}
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
              <span>ERGENT / 0.1</span>
            </div>
          </div>
        ) : (
          <div className="overview">
            <div className="overview-heading">
              <div>
                <div className="eyebrow">
                  {t("YOUR PERSONAL COMMAND CENTER")}
                </div>
                <h1>
                  {t("所有设备，一个工作台")}
                  <span>{t("。")}</span>
                </h1>
                <p>{t("选择一个终端继续工作，或开启新的开发会话。")}</p>
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
                <span>{t("CONNECTED MACHINES")}</span>
                <strong>{online.length}</strong>
                <small>{t("在线开发设备")}</small>
              </div>
              <div>
                <span>{t("RUNNING TERMINALS")}</span>
                <strong>
                  {
                    sessions.filter(
                      (s) =>
                        s.lifecycle === "running" &&
                        online.some((m) => m.id === s.machineId),
                    ).length
                  }
                </strong>
                <small>{t("可以继续的会话")}</small>
              </div>
              <div className="stats-note">
                <Radio size={23} />
                <strong>{t("工作留在本机")}</strong>
                <small>
                  {t("浏览器只是入口。代码与进程")}
                  <br />
                  {t("始终在你的设备上运行。")}
                </small>
              </div>
            </div>
            <div className="devices-title">
              <h2>{t("你的设备")}</h2>
              <button onClick={enroll}>
                <Plus size={15} />
                {t("接入设备")}
              </button>
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
                    <button
                      aria-label={t("重命名 {name}", { name: m.name })}
                      title={t("重命名设备")}
                      onClick={() => editMachine(m)}
                    >
                      <Pencil size={16} />
                    </button>
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
                      disabled={!m.online}
                      onClick={() => newTerminal(m)}
                      aria-label={t("打开 {name}", { name: m.name })}
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
                <strong>{t("接入一台开发机")}</strong>
                <span>{t("macOS、Linux 或 Windows")}</span>
              </button>
            </div>
            <div className="getting-started">
              <div className="step-number">01</div>
              <div>
                <strong>{t("从一个真实终端开始")}</strong>
                <p>
                  {t(
                    "接入设备后，可以直接运行 codex、claude、opencode，或你熟悉的任何终端工具。",
                  )}
                </p>
              </div>
              <TerminalIcon size={26} />
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
      {modal && (
        <div className="modal-backdrop">
          <section
            className="modal"
            role="dialog"
            aria-modal="true"
            aria-labelledby="modal-title"
          >
            <div className="modal-heading">
              <h2 id="modal-title">
                {modal === "create"
                  ? t("新建终端")
                  : modal === "terminal-rename"
                    ? t("重命名终端")
                    : modal === "rename"
                      ? t("重命名设备")
                      : t("接入设备")}
              </h2>
              <button
                aria-label={t("关闭弹窗")}
                onClick={() => {
                  setModal(null);
                  setToken("");
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
            {modal === "terminal-rename" ? (
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
                    maxLength={120}
                    value={terminalName}
                    onChange={(e) => setTerminalName(e.target.value)}
                  />
                </label>
                <p className="form-note">
                  {t("修改显示名称，不会中断终端或正在运行的任务。")}
                </p>
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
                    maxLength={120}
                    value={deviceName}
                    onChange={(e) => setDeviceName(e.target.value)}
                  />
                </label>
                <p className="form-note">
                  {t("名称保存在服务器，设备离线或重连后仍然保留。")}
                </p>
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
                    maxLength={120}
                    onChange={(e) => setTitle(e.target.value)}
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
                <p className="form-note">
                  {t("创建设备默认 Shell，随后可直接输入 AI CLI 命令。")}
                </p>
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
                  ergent-agent enroll --server {location.origin}{" "}
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
                  <code>ergent-agent run</code>
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
          </section>
        </div>
      )}
    </div>
  );
}
createRoot(document.getElementById("root")!).render(<App />);
