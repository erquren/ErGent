import { useEffect, useRef, useState, type ReactNode } from "react";
import { ArrowRight, ArrowUpRight } from "lucide-react";
import { api, ApiError, csrf } from "./api";
import { t } from "./i18n";
import { setupValidationMessage } from "./setup";

type Screen = "loading" | "error" | "setup" | "login";

async function setupRequired(): Promise<boolean> {
  const status = await api<{ required: boolean }>("/auth/setup");
  if (typeof status?.required !== "boolean") throw new Error("无法连接服务");
  return status.required;
}

export function AuthScreen({
  controls,
  onAuthenticated,
}: {
  controls: ReactNode;
  onAuthenticated: (username: string) => void;
}) {
  const [screen, setScreen] = useState<Screen>("loading");
  const [attempt, setAttempt] = useState(0);
  const [username, setUsername] = useState("admin");
  const [password, setPassword] = useState("");
  const [passwordConfirmation, setPasswordConfirmation] = useState("");
  const [setupCode, setSetupCode] = useState("");
  const [error, setError] = useState("");
  const [notice, setNotice] = useState("");
  const [busy, setBusy] = useState(false);
  const submitting = useRef(false);

  useEffect(() => {
    let disposed = false;
    setScreen("loading");
    setError("");
    void (async () => {
      try {
        const required = await setupRequired();
        await csrf();
        if (disposed) return;
        if (required) {
          setScreen("setup");
          return;
        }
        try {
          const user = await api<{ username: string }>("/auth/me");
          if (typeof user?.username !== "string" || !user.username)
            throw new Error("无法连接服务");
          if (!disposed) onAuthenticated(user.username);
        } catch (err) {
          // Only an explicit unauthenticated response means login is needed.
          // Network, server, and malformed-response errors must remain errors.
          if (!(err instanceof ApiError) || err.status !== 401) throw err;
          if (!disposed) setScreen("login");
        }
      } catch {
        if (!disposed) setScreen("error");
      }
    })();
    return () => {
      disposed = true;
    };
  }, [attempt, onAuthenticated]);

  const clearSecrets = () => {
    setPassword("");
    setPasswordConfirmation("");
    setSetupCode("");
  };
  const finishSetup = (message: string) => {
    clearSecrets();
    setError("");
    setNotice(message);
    setScreen("login");
  };
  const submit = async () => {
    if (submitting.current || !["setup", "login"].includes(screen)) return;
    setError("");
    if (screen === "setup") {
      const message = setupValidationMessage(
        username,
        password,
        passwordConfirmation,
      );
      if (message) {
        setError(message);
        return;
      }
    }
    submitting.current = true;
    setBusy(true);
    try {
      await csrf();
      if (screen === "setup") {
        const result = await api<{ created: boolean }>("/auth/setup", "POST", {
          username,
          password,
          passwordConfirmation,
          setupCode,
        });
        if (result?.created !== true) throw new Error("无法确认账户状态");
        finishSetup("首个账户已创建。请使用刚设置的密码登录。");
      } else {
        const result = await api<{ user: { username: string } }>(
          "/auth/login",
          "POST",
          { username, password },
        );
        clearSecrets();
        onAuthenticated(result.user.username);
      }
    } catch (err) {
      if (screen === "setup") {
        // A response can be lost or another tab can finish setup first. Read
        // authoritative state before allowing another attempt; never auto-submit.
        try {
          if (!(await setupRequired())) {
            finishSetup("初始化已完成。请使用已创建的账户登录。");
            return;
          }
        } catch {
          // Keep the setup form and its error; an outage is not a completed setup.
        }
      }
      setError(
        err instanceof ApiError
          ? err.message
          : "无法连接服务，请检查连接后重试。",
      );
    } finally {
      submitting.current = false;
      setBusy(false);
    }
  };

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
      <section
        className={`login-form${screen === "setup" ? " setup-form" : ""}`}
      >
        <div className="header-controls">{controls}</div>
        <div className="login-box">
          {screen === "loading" ? (
            <div role="status" aria-live="polite">
              <h2>{t("正在检查工作台…")}</h2>
              <p>{t("正在连接服务器，请稍候。")}</p>
            </div>
          ) : screen === "error" ? (
            <>
              <h2>{t("暂时无法连接工作台")}</h2>
              <p role="alert">
                {t("无法确认账户状态，请检查服务器与网络后重试。")}
              </p>
              <button
                className="primary full"
                onClick={() => setAttempt((value) => value + 1)}
              >
                {t("重试连接")}
              </button>
            </>
          ) : (
            <>
              <span className="eyebrow">
                {t(screen === "setup" ? "首次使用" : "WELCOME BACK")}
              </span>
              <h2>
                {t(screen === "setup" ? "创建首个账户" : "进入你的工作台")}
              </h2>
              <p>
                {t(
                  screen === "setup"
                    ? "此服务器尚未设置账户。完成初始化后即可登录工作台。"
                    : "使用此服务器上的账户登录。",
                )}
              </p>
              {notice && (
                <p className="auth-notice" role="status">
                  {t(notice)}
                </p>
              )}
              <form
                onSubmit={(event) => {
                  event.preventDefault();
                  void submit();
                }}
              >
                <label>
                  {t("用户名")}
                  <input
                    name="username"
                    autoComplete="username"
                    value={username}
                    disabled={busy}
                    onChange={(event) => setUsername(event.target.value)}
                    required
                  />
                </label>
                <label>
                  {t("密码")}
                  <input
                    name="password"
                    type="password"
                    autoComplete={
                      screen === "setup" ? "new-password" : "current-password"
                    }
                    value={password}
                    disabled={busy}
                    onChange={(event) => setPassword(event.target.value)}
                    aria-describedby={
                      screen === "setup" ? "setup-password-help" : undefined
                    }
                    required
                  />
                </label>
                {screen === "setup" && (
                  <>
                    <p className="auth-help" id="setup-password-help">
                      {t("密码须为 12–1024 UTF-8 字节")}
                    </p>
                    <label>
                      {t("确认密码")}
                      <input
                        name="passwordConfirmation"
                        type="password"
                        autoComplete="new-password"
                        value={passwordConfirmation}
                        disabled={busy}
                        onChange={(event) =>
                          setPasswordConfirmation(event.target.value)
                        }
                        required
                      />
                    </label>
                    <label>
                      {t("一次性初始化码")}
                      <input
                        name="setupCode"
                        type="password"
                        autoComplete="off"
                        spellCheck={false}
                        autoCapitalize="none"
                        value={setupCode}
                        disabled={busy}
                        onChange={(event) => setSetupCode(event.target.value)}
                        aria-describedby="setup-code-help"
                        required
                      />
                    </label>
                    <p className="auth-help" id="setup-code-help">
                      {t(
                        "初始化码仅在服务器交互式终端显示，15 分钟内有效。需要新码时，对同一数据库运行 setup-code。创建首个账户后失效。",
                      )}
                    </p>
                  </>
                )}
                {error && (
                  <div className="error" role="alert">
                    {t(error)}
                  </div>
                )}
                <button className="primary full" disabled={busy}>
                  {t(
                    busy
                      ? screen === "setup"
                        ? "正在创建账户…"
                        : "正在登录…"
                      : screen === "setup"
                        ? "创建账户"
                        : "连接工作台",
                  )}
                  <ArrowRight size={17} />
                </button>
              </form>
            </>
          )}
          <div className="login-note">
            <i className="dot green" />
            {t("Self-hosted · Your infrastructure")}
          </div>
        </div>
      </section>
    </main>
  );
}
