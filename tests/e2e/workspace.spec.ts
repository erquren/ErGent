import {
  test,
  expect,
  request as requestFactory,
  type APIRequestContext,
} from "@playwright/test";
import { spawn, spawnSync, type ChildProcess } from "node:child_process";
import { mkdtempSync, readFileSync, mkdirSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { resolve, join } from "node:path";
import { randomUUID } from "node:crypto";

const root = process.cwd(),
  port = 17779,
  origin = `http://127.0.0.1:${port}`;
const folder = mkdtempSync(join(tmpdir(), "ergent-e2e-"));
const suffix = process.platform === "win32" ? ".exe" : "";
const serverBin = resolve(root, `target/debug/ergent-server${suffix}`),
  agentBin = resolve(root, `target/debug/ergent-agent${suffix}`);
const database = join(folder, "server.db"),
  config = join(folder, "agent.json");
const password = "ergent-e2e-password-only";
const fixtureBin = join(folder, "fixture-bin");
mkdirSync(fixtureBin);
if (process.platform !== "win32")
  writeFileSync(
    join(fixtureBin, "codex"),
    `#!/bin/sh\nexec '${process.execPath.replaceAll("'", "'\\''")}' '${resolve(root, "tests/fixtures/codex.cjs").replaceAll("'", "'\\''")}' "$@"\n`,
    { mode: 0o755 },
  );
const children: ChildProcess[] = [];
let server: ChildProcess,
  agent: ChildProcess,
  api: APIRequestContext,
  csrfToken = "",
  machineId = "";
function start(binary: string, args: string[], cwd = root) {
  const child = spawn(binary, args, {
    cwd,
    env: {
      ...process.env,
      PATH: `${fixtureBin}${process.platform === "win32" ? ";" : ":"}${process.env.PATH}`,
      ...(process.platform === "win32" ? {} : { SHELL: "/bin/sh" }),
    },
    stdio: ["ignore", "pipe", "pipe"],
  });
  child.stderr?.on("data", (d) => process.stderr.write(d));
  children.push(child);
  return child;
}
function startServer() {
  return start(serverBin, [
    "--database",
    database,
    "run",
    "--listen",
    `127.0.0.1:${port}`,
    "--origin",
    origin,
    "--web",
    resolve(root, "apps/web/dist"),
  ]);
}
async function stop(child: ChildProcess) {
  if (child.exitCode !== null || child.signalCode) return;
  const exited = new Promise<void>((r) => child.once("exit", () => r()));
  child.kill("SIGINT");
  await Promise.race([exited, new Promise((r) => setTimeout(r, 4000))]);
  if (child.exitCode === null && !child.signalCode) {
    child.kill("SIGKILL");
    await exited;
  }
}
async function login(client: APIRequestContext, username = "admin") {
  const pre = await client.get("/api/v1/auth/csrf");
  const csrf = (await pre.json()).data.csrfToken;
  const response = await client.post("/api/v1/auth/login", {
    data: { username, password },
    headers: { Origin: origin, "X-CSRF-Token": csrf },
  });
  expect(response.ok()).toBeTruthy();
  return (await response.json()).data.csrfToken;
}
const headers = () => ({ Origin: origin, "X-CSRF-Token": csrfToken });
async function online() {
  await expect
    .poll(
      async () => {
        try {
          return (
            await (await api.get("/api/v1/machines")).json()
          ).data.items.some(
            (m: { id: string; online: boolean }) =>
              m.id === machineId && m.online,
          );
        } catch {
          return false;
        }
      },
      { timeout: 15000 },
    )
    .toBeTruthy();
}

test.beforeAll(async () => {
  const admin = spawnSync(serverBin, ["--database", database, "admin-create"], {
    input: password,
  });
  expect(admin.status, admin.stderr?.toString()).toBe(0);
  server = startServer();
  await expect
    .poll(async () => {
      try {
        return (await fetch(`${origin}/health/ready`)).ok;
      } catch {
        return false;
      }
    })
    .toBeTruthy();
  api = await requestFactory.newContext({ baseURL: origin });
  csrfToken = await login(api);
  const enroll = await api.post("/api/v1/enrollments", {
    data: {},
    headers: headers(),
  });
  const token = (await enroll.json()).data.token;
  const registered = spawnSync(
    agentBin,
    ["--config", config, "enroll", "--server", origin, "--name", "Test Mac"],
    { input: token },
  );
  expect(registered.status, registered.stderr?.toString()).toBe(0);
  machineId = JSON.parse(readFileSync(config, "utf8")).machineId;
  agent = start(agentBin, ["--config", config, "run"], folder);
  await online();
});
test.afterAll(async () => {
  for (const child of [...children].reverse()) await stop(child);
  await api?.dispose();
});

test("real terminals, card switching, browser refresh, server restart, and write ownership", async ({
  page,
  context,
}) => {
  const errors: string[] = [];
  page.on("pageerror", (e) => errors.push(e.message));
  await page.goto(origin);
  await page.getByLabel("用户名").fill("admin");
  await page.getByLabel("密码").fill(password);
  await page.getByRole("button", { name: "连接工作台" }).click();
  await expect(
    page.getByRole("heading", { name: "所有设备，一个工作台。" }),
  ).toBeVisible();
  await page.screenshot({ path: "test-results/workspace-overview.png" });
  await page.getByRole("button", { name: "在 Test Mac 新建终端" }).click();
  await page.getByLabel("终端名称").fill("Build terminal");
  await page.getByRole("button", { name: "创建并连接" }).click();
  await expect(page.getByText("已连接 · 输入在设备上执行")).toBeVisible();
  await page.locator(".xterm-helper-textarea").focus();
  await page.keyboard.type(
    "export ERGENT_PROBE=still_here; printf 'FIRST_%s\\n' 'SUCCESS'\n",
  );
  await expect(page.locator(".xterm-rows")).toContainText("FIRST_SUCCESS");
  await page.getByRole("button", { name: "在 Test Mac 新建终端" }).click();
  await page.getByLabel("终端名称").fill("Second terminal");
  await page.getByRole("button", { name: "创建并连接" }).click();
  await expect(
    page.getByRole("heading", { name: /Second terminal/ }),
  ).toBeVisible();
  await page
    .locator(".session-card")
    .filter({ hasText: "Build terminal" })
    .click();
  await expect(page.getByText("已连接 · 输入在设备上执行")).toBeVisible();
  await expect(page.locator(".xterm-rows")).toContainText("FIRST_SUCCESS");
  await page.reload();
  await page
    .locator(".session-card")
    .filter({ hasText: "Build terminal" })
    .click();
  await expect(page.getByText("已连接 · 输入在设备上执行")).toBeVisible();
  await page.locator(".xterm-helper-textarea").focus();
  await page.keyboard.type('printf "PERSIST_%s\\n" "$ERGENT_PROBE"\n');
  await expect(page.locator(".xterm-rows")).toContainText("PERSIST_still_here");
  const second = await context.newPage();
  await second.goto(origin);
  await second
    .locator(".session-card")
    .filter({ hasText: "Build terminal" })
    .click();
  await expect(second.getByRole("button", { name: "接管终端" })).toBeVisible();
  await second.getByRole("button", { name: "接管终端" }).click();
  await expect(second.getByText("已连接 · 输入在设备上执行")).toBeVisible();
  await expect(page.getByRole("button", { name: "接管终端" })).toBeVisible();
  await second.close();
  await page.getByRole("button", { name: "接管终端" }).click();
  await expect(page.getByText("已连接 · 输入在设备上执行")).toBeVisible();
  await stop(server);
  server = startServer();
  await online();
  await expect(page.getByText("已连接 · 输入在设备上执行")).toBeVisible({
    timeout: 15000,
  });
  await page.locator(".xterm-helper-textarea").focus();
  await page.keyboard.type('printf "RESTART_%s\\n" "$ERGENT_PROBE"\n');
  await expect(page.locator(".xterm-rows")).toContainText("RESTART_still_here");
  await page.screenshot({ path: "test-results/workspace-terminal.png" });
  expect(errors).toEqual([]);
});

test("mobile layout, terminal shortcuts and compact viewport", async ({
  browser,
}) => {
  const context = await browser.newContext({
    locale: "zh-CN",
    viewport: { width: 390, height: 844 },
    isMobile: true,
    hasTouch: true,
    storageState: await api.storageState(),
  });
  const page = await context.newPage();
  const inputs: string[] = [];
  page.on("websocket", (ws) =>
    ws.on("framesent", ({ payload }) => {
      const message = JSON.parse(payload.toString());
      if (message.type === "terminal.input")
        inputs.push(
          Buffer.from(message.payload.dataBase64, "base64").toString(),
        );
    }),
  );
  const response = await api.post(`/api/v1/machines/${machineId}/sessions`, {
    headers: { ...headers(), "Idempotency-Key": randomUUID() },
    data: { title: "Mobile terminal", cwd: folder, cols: 80, rows: 24 },
  });
  const sessionId = (await response.json()).data.sessionId;
  try {
    await page.goto(origin);
    await expect(page.locator(".sidebar")).toBeHidden();
    await page.getByRole("button", { name: "设备 / 终端" }).tap();
    await page.getByRole("button", { name: /Mobile terminal/ }).tap();
    await expect(page.locator(".sidebar")).toBeHidden();
    await expect(
      page.getByRole("button", { name: "Ctrl+C", exact: true }),
    ).toBeEnabled();
    await page.getByRole("button", { name: "Ctrl+C", exact: true }).tap();
    await page.getByRole("button", { name: "Ctrl", exact: true }).tap();
    await page.getByRole("button", { name: "发送 Ctrl+L", exact: true }).tap();
    await expect(
      page.getByRole("button", { name: "Ctrl", exact: true }),
    ).toHaveAttribute("aria-pressed", "false");
    await page.getByRole("button", { name: "方向↑", exact: true }).tap();
    await expect.poll(() => inputs).toEqual(["\x03", "\x0c", "\x1b[A"]);
    await page.getByRole("button", { name: "键盘", exact: true }).tap();
    await expect(page.locator(".xterm-helper-textarea")).toBeFocused();
    await page.keyboard.type("garbage");
    await page.getByRole("button", { name: "Ctrl+U", exact: true }).tap();
    await page.keyboard.type("printf MOBILE_OK");
    await page.getByRole("button", { name: "Enter", exact: true }).tap();
    await expect(page.locator(".xterm-rows")).toContainText("MOBILE_OK");
    for (const viewport of [
      { width: 390, height: 400 },
      { width: 844, height: 390 },
      { width: 320, height: 568 },
    ]) {
      await page.setViewportSize(viewport);
      await expect
        .poll(async () =>
          page.evaluate(
            () => document.documentElement.scrollWidth <= innerWidth,
          ),
        )
        .toBeTruthy();
      const canvas = await page.locator(".terminal-canvas").boundingBox();
      expect(canvas!.width).toBeGreaterThan(250);
      expect(canvas!.height).toBeGreaterThan(50);
      const keys = await page.locator(".terminal-keys").boundingBox();
      expect(keys!.y + keys!.height).toBeLessThanOrEqual(viewport.height);
      await expect(page.locator(".sidebar")).toBeHidden();
    }
    await page.screenshot({ path: "test-results/mobile-terminal.png" });
  } finally {
    await context.close();
    await api.post(`/api/v1/sessions/${sessionId}/close`, {
      headers: { ...headers(), "Idempotency-Key": randomUUID() },
      data: {},
    });
  }
});

test("rename persists across reconnect and exited terminals disappear", async ({
  browser,
}) => {
  const context = await browser.newContext({
    locale: "zh-CN",
    storageState: await api.storageState(),
  });
  const page = await context.newPage();
  const observer = await context.newPage();
  try {
    await page.goto(origin);
    await observer.goto(origin);
    await page
      .getByRole("button", { name: "重命名 Test Mac", exact: true })
      .first()
      .click();
    await page.getByLabel("设备名称", { exact: true }).fill("研发机 ARM64");
    await page.getByRole("button", { name: "保存名称" }).click();
    await expect(observer.locator(".machine-name")).toContainText(
      "研发机 ARM64",
    );
    await stop(server);
    server = startServer();
    await online();
    await page.reload();
    await expect(page.locator(".machine-name")).toContainText("研发机 ARM64");
    for (const name of [" ", "x".repeat(121), "bad\nname"]) {
      expect(
        (
          await api.patch(`/api/v1/machines/${machineId}`, {
            headers: headers(),
            data: { name },
          })
        ).status(),
      ).toBe(422);
    }
    expect(
      (
        await api.patch(`/api/v1/machines/${machineId}`, {
          data: { name: "forged" },
        })
      ).status(),
    ).toBe(403);
    for (const title of ["Exit naturally", "Close explicitly"]) {
      await page
        .getByRole("button", { name: "在 研发机 ARM64 新建终端", exact: true })
        .click();
      await page.getByLabel("终端名称").fill(title);
      await page.getByRole("button", { name: "创建并连接" }).click();
      await expect(page.getByText("已连接 · 输入在设备上执行")).toBeVisible();
      if (title === "Exit naturally") {
        await page.locator(".xterm-helper-textarea").focus();
        await page.keyboard.type("exit\n");
      } else {
        await page
          .getByRole("button", { name: "结束终端", exact: true })
          .click();
        await page
          .getByRole("button", { name: "确认结束", exact: true })
          .click();
      }
      await expect(
        page.locator(".session-card").filter({ hasText: title }),
      ).toHaveCount(0);
      await expect(
        page.getByRole("heading", { name: "所有设备，一个工作台。" }),
      ).toBeVisible();
      await expect(
        observer.locator(".session-card").filter({ hasText: title }),
      ).toHaveCount(0);
      await page.reload();
      await expect(
        page.locator(".session-card").filter({ hasText: title }),
      ).toHaveCount(0);
    }
  } finally {
    await api.patch(`/api/v1/machines/${machineId}`, {
      headers: headers(),
      data: { name: "Test Mac" },
    });
    await context.close();
  }
});

test("Codex hooks report neutral states, notify once and survive reconnect", async ({
  browser,
}) => {
  test.skip(process.platform === "win32", "Unix offline Codex fixture");
  const context = await browser.newContext({
    locale: "zh-CN",
    storageState: await api.storageState(),
  });
  const page = await context.newPage();
  let sessionId = "";
  try {
    const created = await api.post(`/api/v1/machines/${machineId}/sessions`, {
      headers: { ...headers(), "Idempotency-Key": randomUUID() },
      data: { title: "Codex status probe", cwd: folder, cols: 100, rows: 30 },
    });
    sessionId = (await created.json()).data.sessionId;
    await page.goto(origin);
    await page
      .locator(".session-card")
      .filter({ hasText: "Codex status probe" })
      .click();
    await expect(page.getByText("已连接 · 输入在设备上执行")).toBeVisible();
    const send = async (text: string) => {
      await page.locator(".xterm-helper-textarea").focus();
      await page.keyboard.type(text + "\n");
    };
    await send('"$ERGENT_AGENT_BIN" codex');
    await expect(page.locator(".tool-summary")).toContainText(
      "已启动 · 等待任务",
    );
    await expect(page.locator(".xterm-rows")).toContainText(
      "CODEX_FIXTURE_READY",
    );
    await send("prompt");
    await expect(page.locator(".tool-summary")).toContainText("处理中");
    await send("approval");
    await expect(page.locator(".tool-summary")).toContainText("等待确认");
    await expect(page.locator(".tool-notifications")).toContainText("等待确认");
    await page.getByRole("button", { name: "关闭工具通知" }).click();
    await send("resume");
    await expect(page.locator(".tool-summary")).toContainText("处理中");
    await send("finish");
    await expect(page.locator(".tool-summary")).toContainText("本轮已完成");
    await expect(page.locator(".tool-notification")).toHaveCount(1);
    await send("finish");
    await expect(page.locator(".xterm-rows")).toContainText(
      "FIXTURE_ACK_finish",
    );
    await expect(page.locator(".tool-notification")).toHaveCount(1);
    const inventory = await (
      await api.get(`/api/v1/machines/${machineId}/sessions`)
    ).text();
    expect(inventory).not.toContain("PRIVATE_PROMPT_SENTINEL");
    expect(inventory).not.toContain("PRIVATE_ANSWER_SENTINEL");
    await stop(server);
    server = startServer();
    await online();
    await page.reload();
    await page
      .locator(".session-card")
      .filter({ hasText: "Codex status probe" })
      .click();
    await expect(page.locator(".tool-summary")).toContainText("本轮已完成");
    await expect(page.locator(".tool-notification")).toHaveCount(0);
    await expect(page.getByText("已连接 · 输入在设备上执行")).toBeVisible();
    await send("prompt");
    await expect(page.locator(".tool-summary")).toContainText("处理中");
    await send("interrupt");
    await expect(page.locator(".tool-summary")).toContainText("已中断");
    await send("quit");
    await expect(page.locator(".tool-summary")).toContainText("已退出");
  } finally {
    await context.close();
    if (sessionId)
      await api.post(`/api/v1/sessions/${sessionId}/close`, {
        headers: { ...headers(), "Idempotency-Key": randomUUID() },
        data: {},
      });
  }
});

test("terminal preferences persist and themes update without interrupting the PTY", async ({
  browser,
}) => {
  const context = await browser.newContext({
    locale: "zh-CN",
    storageState: await api.storageState(),
    colorScheme: "dark",
  });
  const page = await context.newPage();
  const observer = await context.newPage();
  let sessionId = "";
  try {
    await page.goto(origin);
    await expect(page.locator("html")).toHaveAttribute("data-theme", "dark");
    await expect(
      page.getByRole("button", { name: "网页外观", exact: true }),
    ).toHaveAttribute("data-value", "system");
    const created = await api.post(`/api/v1/machines/${machineId}/sessions`, {
      headers: { ...headers(), "Idempotency-Key": randomUUID() },
      data: { title: "Theme probe", cwd: folder, cols: 80, rows: 24 },
    });
    sessionId = (await created.json()).data.sessionId;
    await page
      .locator(".session-card")
      .filter({ hasText: "Theme probe" })
      .click();
    await expect(page.getByText("已连接 · 输入在设备上执行")).toBeVisible();
    await page.locator(".xterm-helper-textarea").focus();
    await page.keyboard.type("export THEME_PROBE=alive\n");
    await expect(page.locator(".terminal-frame")).toHaveCSS(
      "background-color",
      "rgb(0, 0, 0)",
    );
    await expect(page.locator(".xterm-screen")).toHaveCSS(
      "color",
      "rgb(255, 255, 255)",
    );
    await observer.goto(origin);
    await page.getByRole("button", { name: "重命名终端", exact: true }).click();
    await page.getByLabel("终端名称", { exact: true }).fill("编译工作台");
    await page.getByRole("button", { name: "保存名称" }).click();
    await expect(
      observer.locator(".session-card").filter({ hasText: "编译工作台" }),
    ).toBeVisible();
    await expect(
      page.getByRole("heading", { name: /编译工作台/ }),
    ).toBeVisible();
    await page.getByRole("button", { name: "网页外观", exact: true }).click();
    await page.getByRole("button", { name: "白天模式", exact: true }).click();
    await expect(observer.locator("html")).toHaveAttribute(
      "data-theme",
      "light",
    );
    await expect(page.locator(".terminal-frame")).toHaveCSS(
      "background-color",
      "rgb(255, 255, 255)",
    );
    await expect(page.locator(".xterm-screen")).toHaveCSS(
      "color",
      "rgb(0, 0, 0)",
    );
    await page.screenshot({ path: "test-results/theme-light.png" });
    await page.getByRole("button", { name: "终端配色", exact: true }).click();
    await page.screenshot({ path: "test-results/glass-palette.png" });
    await page.keyboard.press("Escape");
    await expect(
      page.getByRole("button", { name: "终端配色", exact: true }),
    ).toBeFocused();
    await expect(page.locator(".glass-panel")).toHaveCount(0);
    await page
      .getByRole("button", { name: "Codex 状态接入说明", exact: true })
      .click();
    await expect(page.locator(".tool-help-content")).toContainText(
      "setup-codex",
    );
    await page.getByRole("heading", { name: /编译工作台/ }).click();
    await expect(page.locator(".glass-panel")).toHaveCount(0);

    await page.getByRole("button", { name: "终端配色", exact: true }).click();
    await page.getByRole("button", { name: "Dracula", exact: true }).click();
    await expect(page.locator(".terminal-frame")).toHaveCSS(
      "background-color",
      "rgb(40, 42, 54)",
    );
    await page.getByRole("button", { name: "网页外观", exact: true }).click();
    await page.getByRole("button", { name: "黑夜模式", exact: true }).click();
    await expect(page.locator(".terminal-frame")).toHaveCSS(
      "background-color",
      "rgb(40, 42, 54)",
    );
    for (const data of [
      { title: " " },
      { title: "x".repeat(121) },
      { title: "bad\nname" },
      { terminalTheme: "unknown" },
      {},
    ]) {
      expect(
        (
          await api.patch(`/api/v1/sessions/${sessionId}`, {
            headers: headers(),
            data,
          })
        ).status(),
      ).toBe(422);
    }
    const anon = await requestFactory.newContext({ baseURL: origin });
    expect(
      (
        await anon.patch(`/api/v1/sessions/${sessionId}`, {
          headers: { Origin: origin },
          data: { title: "forbidden" },
        })
      ).status(),
    ).toBe(401);
    await anon.dispose();
    expect(
      (
        await api.patch(`/api/v1/sessions/${sessionId}`, {
          data: { title: "forbidden" },
        })
      ).status(),
    ).toBe(403);
    await stop(server);
    server = startServer();
    await online();
    await page.reload();
    await expect(
      page.getByRole("button", { name: "网页外观", exact: true }),
    ).toHaveAttribute("data-value", "dark");
    await page
      .locator(".session-card")
      .filter({ hasText: "编译工作台" })
      .click();
    await expect(
      page.getByRole("button", { name: "终端配色", exact: true }),
    ).toHaveAttribute("data-value", "dracula");
    await expect(page.getByText("已连接 · 输入在设备上执行")).toBeVisible();
    await page.locator(".xterm-helper-textarea").focus();
    await page.keyboard.type("printf 'THEME_%s\\n' \"$THEME_PROBE\"\n");
    await expect(page.locator(".xterm-rows")).toContainText("THEME_alive");
    await page.getByRole("button", { name: "终端配色", exact: true }).click();
    await page.getByRole("button", { name: "跟随网页", exact: true }).click();
    await page.screenshot({ path: "test-results/theme-dark.png" });
    await page.getByRole("button", { name: "网页外观", exact: true }).click();
    await page.getByRole("button", { name: "跟随系统", exact: true }).click();
    await page.emulateMedia({ colorScheme: "light" });
    await expect(page.locator("html")).toHaveAttribute("data-theme", "light");
    await expect(page.locator(".terminal-frame")).toHaveCSS(
      "background-color",
      "rgb(255, 255, 255)",
    );
    await page.emulateMedia({ colorScheme: "dark" });
    await expect(page.locator(".terminal-frame")).toHaveCSS(
      "background-color",
      "rgb(0, 0, 0)",
    );
  } finally {
    await context.close();
    if (sessionId)
      await api.post(`/api/v1/sessions/${sessionId}/close`, {
        headers: { ...headers(), "Idempotency-Key": randomUUID() },
        data: {},
      });
  }
});

test("language follows the device, persists across tabs, and keeps the terminal alive", async ({
  browser,
}) => {
  for (const [locale, expected] of [
    ["zh-TW", "zh-CN"],
    ["en-US", "en"],
    ["fr-FR", "en"],
  ]) {
    const context = await browser.newContext({ locale });
    const page = await context.newPage();
    await page.goto(origin);
    await expect(page.locator("html")).toHaveAttribute("lang", expected);
    await expect(
      page.getByRole("button", {
        name: expected === "en" ? "Sign in" : "连接工作台",
        exact: true,
      }),
    ).toBeVisible();
    await context.close();
  }
  const context = await browser.newContext({
    locale: "en-US",
    storageState: await api.storageState(),
  });
  const page = await context.newPage();
  const observer = await context.newPage();
  let sessionId = "",
    sockets = 0;
  page.on("websocket", () => sockets++);
  try {
    await page.goto(origin);
    await observer.goto(origin);
    await expect(
      page.getByRole("heading", { name: "Every device. One workspace." }),
    ).toBeVisible();
    expect(await page.locator(".main").innerText()).not.toMatch(
      /[\u3400-\u9fff]/,
    );
    await page.screenshot({ path: "test-results/language-english.png" });
    const response = await api.post(`/api/v1/machines/${machineId}/sessions`, {
      headers: { ...headers(), "Idempotency-Key": randomUUID() },
      data: {
        title: "语言保留 Language probe",
        cwd: folder,
        cols: 80,
        rows: 24,
      },
    });
    sessionId = (await response.json()).data.sessionId;
    await page
      .locator(".session-card")
      .filter({ hasText: "Language probe" })
      .click();
    await expect(
      page.getByText("Connected · Input runs on your device", { exact: true }),
    ).toBeVisible();
    await page.locator(".xterm-helper-textarea").focus();
    await page.keyboard.type("export LANGUAGE_PROBE=alive\n");
    const connectedSockets = sockets;
    await page.getByRole("button", { name: "Language", exact: true }).click();
    await page.getByRole("button", { name: "简体中文", exact: true }).click();
    await expect(page.locator("html")).toHaveAttribute("lang", "zh-CN");
    await expect(observer.locator("html")).toHaveAttribute("lang", "zh-CN");
    await expect(
      page.getByText("已连接 · 输入在设备上执行", { exact: true }),
    ).toBeVisible();
    await expect(
      page.getByRole("heading", { name: /语言保留 Language probe/ }),
    ).toBeVisible();
    expect(sockets).toBe(connectedSockets);
    await page.getByRole("button", { name: "界面语言", exact: true }).click();
    await page.getByRole("button", { name: "English", exact: true }).click();
    await expect(
      page.getByRole("button", { name: "Terminal colors", exact: true }),
    ).toBeVisible();
    await page
      .getByRole("button", { name: "Terminal colors", exact: true })
      .click();
    await expect(
      page.getByRole("button", { name: "Classic light", exact: true }),
    ).toBeVisible();
    await page.keyboard.press("Escape");
    await page.locator(".xterm-helper-textarea").focus();
    await page.keyboard.type("printf 'LANG_%s\\n' \"$LANGUAGE_PROBE\"\n");
    await expect(page.locator(".xterm-rows")).toContainText("LANG_alive");
    expect(sockets).toBe(connectedSockets);
    await page.reload();
    await expect(
      page.getByRole("button", { name: "Language", exact: true }),
    ).toHaveAttribute("data-value", "en");
    await page.setViewportSize({ width: 320, height: 568 });
    await page.getByRole("button", { name: "Language", exact: true }).click();
    await expect(
      page.getByRole("button", { name: "Use device language", exact: true }),
    ).toBeVisible();
    await expect
      .poll(() =>
        page.evaluate(() => document.documentElement.scrollWidth <= innerWidth),
      )
      .toBeTruthy();
    await page.screenshot({ path: "test-results/language-mobile.png" });
    await page
      .getByRole("button", { name: "Use device language", exact: true })
      .click();
    await expect(
      page.getByRole("button", { name: "Language", exact: true }),
    ).toHaveAttribute("data-value", "auto");
  } finally {
    await context.close();
    if (sessionId)
      await api.post(`/api/v1/sessions/${sessionId}/close`, {
        headers: { ...headers(), "Idempotency-Key": randomUUID() },
        data: {},
      });
  }
});

test("authorization, CSRF, enrollment replay, and create idempotency", async () => {
  const anon = await requestFactory.newContext({ baseURL: origin });
  expect((await anon.get("/api/v1/machines")).status()).toBe(401);
  expect((await api.post("/api/v1/enrollments", { data: {} })).status()).toBe(
    403,
  );
  const token = (
    await (
      await api.post("/api/v1/enrollments", { headers: headers(), data: {} })
    ).json()
  ).data.token;
  const data = {
    enrollmentToken: token,
    name: "Replay probe",
    os: "test",
    arch: "test",
  };
  const results = await Promise.all([
    anon.post("/api/v1/agent/enroll", { data }),
    anon.post("/api/v1/agent/enroll", { data }),
  ]);
  expect(results.filter((r) => r.ok())).toHaveLength(1);
  expect(results.filter((r) => r.status() === 401)).toHaveLength(1);
  const key = randomUUID(),
    body = { title: "Idempotent", cwd: folder, cols: 80, rows: 24 };
  const createHeaders = { ...headers(), "Idempotency-Key": key };
  const first = await api.post(`/api/v1/machines/${machineId}/sessions`, {
    data: body,
    headers: createHeaders,
  });
  const again = await api.post(`/api/v1/machines/${machineId}/sessions`, {
    data: body,
    headers: createHeaders,
  });
  const a = (await first.json()).data,
    b = (await again.json()).data;
  expect(first.status()).toBe(202);
  expect(a.operationId).toBe(b.operationId);
  expect(a.sessionId).toBe(b.sessionId);
  expect(
    (
      await api.post(`/api/v1/machines/${machineId}/sessions`, {
        data: { ...body, title: "Different" },
        headers: createHeaders,
      })
    ).status(),
  ).toBe(409);
  await expect
    .poll(
      async () =>
        (await (await api.get(`/api/v1/operations/${a.operationId}`)).json())
          .data.status,
    )
    .toBe("succeeded");
  await anon.dispose();
});

test("cross-user isolation, agent restart, close, and device revocation", async () => {
  const otherAdmin = spawnSync(
    serverBin,
    ["--database", database, "admin-create", "--username", "other"],
    { input: password },
  );
  expect(otherAdmin.status).toBe(0);
  const other = await requestFactory.newContext({ baseURL: origin });
  const otherCsrf = await login(other, "other");
  expect(
    (await (await other.get("/api/v1/machines")).json()).data.items,
  ).toEqual([]);
  expect(
    (await other.get(`/api/v1/machines/${machineId}/sessions`)).status(),
  ).toBe(404);
  expect(
    (
      await other.post(`/api/v1/machines/${machineId}/sessions`, {
        headers: {
          Origin: origin,
          "X-CSRF-Token": otherCsrf,
          "Idempotency-Key": randomUUID(),
        },
        data: { title: "Unauthorized", cwd: folder, cols: 80, rows: 24 },
      })
    ).status(),
  ).toBe(404);
  expect(
    (
      await other.patch(`/api/v1/machines/${machineId}`, {
        headers: { Origin: origin, "X-CSRF-Token": otherCsrf },
        data: { name: "Not my device" },
      })
    ).status(),
  ).toBe(404);
  const badOrigin = await api.get("/api/v1/ws/browser", {
    headers: {
      Origin: "https://untrusted.example",
      Connection: "Upgrade",
      Upgrade: "websocket",
      "Sec-WebSocket-Key": "dGhlIHNhbXBsZSBub25jZQ==",
      "Sec-WebSocket-Version": "13",
      "Sec-WebSocket-Protocol": "ergent.preview.v1",
    },
  });
  expect(badOrigin.status()).toBe(403);
  const old = (
    await (await api.get(`/api/v1/machines/${machineId}/sessions`)).json()
  ).data.items;
  expect(
    old.some((s: { lifecycle: string }) => s.lifecycle === "running"),
  ).toBeTruthy();
  expect(
    (
      await other.patch(`/api/v1/sessions/${old[0].id}`, {
        headers: { Origin: origin, "X-CSRF-Token": otherCsrf },
        data: { title: "not yours", terminalTheme: "dark" },
      })
    ).status(),
  ).toBe(404);
  // Simulate a daemon crash; the new epoch must not pretend old PTYs survived.
  const exited = new Promise<void>((resolve) =>
    agent.once("exit", () => resolve()),
  );
  agent.kill("SIGKILL");
  await exited;
  agent = start(agentBin, ["--config", config, "run"], folder);
  await online();
  await expect
    .poll(async () =>
      (
        await (await api.get(`/api/v1/machines/${machineId}/sessions`)).json()
      ).data.items.every(
        (s: { lifecycle: string }) => s.lifecycle !== "running",
      ),
    )
    .toBeTruthy();
  const created = (
    await (
      await api.post(`/api/v1/machines/${machineId}/sessions`, {
        data: { title: "Close probe", cwd: folder, cols: 80, rows: 24 },
        headers: { ...headers(), "Idempotency-Key": randomUUID() },
      })
    ).json()
  ).data;
  await expect
    .poll(
      async () =>
        (
          await (
            await api.get(`/api/v1/operations/${created.operationId}`)
          ).json()
        ).data.status,
    )
    .toBe("succeeded");
  expect(
    (
      await api.post(`/api/v1/sessions/${created.sessionId}/close`, {
        data: {},
        headers: { ...headers(), "Idempotency-Key": randomUUID() },
      })
    ).status(),
  ).toBe(202);
  await expect
    .poll(
      async () =>
        (
          await (await api.get(`/api/v1/machines/${machineId}/sessions`)).json()
        ).data.items.find((s: { id: string }) => s.id === created.sessionId)
          .lifecycle,
    )
    .toBe("exited");
  expect(
    (
      await api.post(`/api/v1/machines/${machineId}/revoke`, {
        data: {},
        headers: headers(),
      })
    ).ok(),
  ).toBeTruthy();
  const device = JSON.parse(readFileSync(config, "utf8"));
  const reconnect = await api.get("/api/v1/ws/agent", {
    headers: {
      Authorization: `Bearer ${device.secret}`,
      Connection: "Upgrade",
      Upgrade: "websocket",
      "Sec-WebSocket-Key": "dGhlIHNhbXBsZSBub25jZQ==",
      "Sec-WebSocket-Version": "13",
      "Sec-WebSocket-Protocol": "ergent.preview.v1",
    },
  });
  expect(reconnect.status()).toBe(401);
  await other.dispose();
});
