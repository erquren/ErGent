import { expect, test, type Page } from "@playwright/test";
import { resolve, sep } from "node:path";
import { setupValidationMessage } from "../../apps/web/src/setup";

const origin = "http://setup.ergent.test";
const password = "local-setup-password";
const setupCode = "local-test-code-not-a-real-secret";
const dist = resolve("apps/web/dist");

// Exercise the production web build without changing the existing real-server
// workspace fixture. Real empty-database coverage lives in setup-integration.
async function mockServer(page: Page) {
  const state = {
    required: true,
    status: 200,
    malformedStatus: false,
    csrfStatus: 200,
    meStatus: 401,
    postStatus: 200,
    postMessage: "初始化码无效或已过期",
    completeOnFailure: false,
    statusGate: undefined as Promise<void> | undefined,
    postGate: undefined as Promise<void> | undefined,
    requests: [] as string[],
    setupPosts: [] as Record<string, string>[],
    loginPosts: [] as Record<string, string>[],
  };
  await page.routeWebSocket("**/api/v1/ws/browser", () => {});
  await page.route(`${origin}/**`, async (route) => {
    const request = route.request();
    const url = new URL(request.url());
    state.requests.push(`${request.method()} ${url.pathname}${url.search}`);
    const json = (status: number, value: unknown) =>
      route.fulfill({ status, json: value });
    if (url.pathname === "/api/v1/auth/setup") {
      if (request.method() === "GET") {
        await state.statusGate;
        return json(
          state.status,
          state.malformedStatus
            ? { data: {} }
            : { data: { required: state.required } },
        );
      }
      state.setupPosts.push(request.postDataJSON());
      await state.postGate;
      if (state.postStatus === 200) {
        state.required = false;
        return json(200, { data: { created: true } });
      }
      if (state.completeOnFailure) state.required = false;
      return json(state.postStatus, { error: { message: state.postMessage } });
    }
    if (url.pathname === "/api/v1/auth/csrf")
      return json(state.csrfStatus, { data: { csrfToken: "test-csrf" } });
    if (url.pathname === "/api/v1/auth/me")
      return json(
        state.meStatus,
        state.meStatus === 200
          ? { data: { username: "admin" } }
          : { error: { message: "需要登录" } },
      );
    if (url.pathname === "/api/v1/auth/login") {
      state.loginPosts.push(request.postDataJSON());
      return json(200, {
        data: {
          user: { username: request.postDataJSON().username },
          csrfToken: "logged-in-csrf",
        },
      });
    }
    const path = resolve(
      dist,
      url.pathname === "/" ? "index.html" : `.${url.pathname}`,
    );
    if (!path.startsWith(dist + sep)) return route.fulfill({ status: 404 });
    return route.fulfill({ path });
  });
  return state;
}
async function fillSetup(page: Page) {
  await page.getByLabel("用户名", { exact: true }).fill("first-user");
  await page.getByLabel("密码", { exact: true }).fill(password);
  await page.getByLabel("确认密码", { exact: true }).fill(password);
  await page.getByLabel("一次性初始化码", { exact: true }).fill(setupCode);
}

const validPasswords = [
  "a".repeat(12),
  "a".repeat(1024),
  "密".repeat(4),
  "🔐".repeat(256),
  "  password  ",
];
for (const value of validPasswords)
  test(`setup validates a ${new TextEncoder().encode(value).length}-byte password (${value.codePointAt(0)})`, () => {
    expect(setupValidationMessage("user", value, value)).toBe("");
  });
for (const value of [
  "a".repeat(11),
  "a".repeat(1025),
  "密".repeat(3),
  "🔐".repeat(257),
  "a".repeat(12) + "\ud800",
])
  test(`setup rejects invalid password ${new TextEncoder().encode(value).length} bytes (${value.codePointAt(0)})`, () => {
    expect(setupValidationMessage("user", value, value)).not.toBe("");
  });
for (const value of [
  "",
  " ",
  " user",
  "user ",
  "\u0085user",
  "user\u0085",
  "user\nname",
  "a".repeat(121),
  "中".repeat(41),
  "\ud800",
])
  test(`setup rejects invalid username ${JSON.stringify(value)}`, () => {
    expect(setupValidationMessage(value, password, password)).not.toBe("");
  });
test("setup username UTF-8 boundary and password equality", () => {
  for (const username of [
    "u",
    "a".repeat(120),
    "中".repeat(40),
    "🔐".repeat(30),
  ])
    expect(setupValidationMessage(username, password, password)).toBe("");
  expect(setupValidationMessage("user", password, `${password} `)).toBe(
    "两次输入的密码不一致",
  );
});

test("setup waits for account status without flashing login or setup", async ({
  page,
}) => {
  const state = await mockServer(page);
  let release!: () => void;
  state.statusGate = new Promise<void>((resolve) => {
    release = resolve;
  });
  await page.goto(origin);
  await expect(page.getByRole("status")).toContainText("正在检查工作台");
  await expect(page.locator("form")).toHaveCount(0);
  release();
  await expect(
    page.getByRole("heading", { name: "创建首个账户" }),
  ).toBeVisible();
  expect(state.requests).not.toContain("GET /api/v1/auth/me");
});

for (const failure of [
  "setup unavailable",
  "malformed status",
  "csrf unavailable",
  "session unavailable",
])
  test(`setup distinguishes ${failure} from an empty database`, async ({
    page,
  }) => {
    const state = await mockServer(page);
    if (failure === "setup unavailable") state.status = 503;
    if (failure === "malformed status") state.malformedStatus = true;
    if (failure === "csrf unavailable") state.csrfStatus = 503;
    if (failure === "session unavailable") {
      state.required = false;
      state.meStatus = 500;
    }
    await page.goto(origin);
    await expect(
      page.getByRole("heading", { name: "暂时无法连接工作台" }),
    ).toBeVisible();
    await expect(page.locator("form")).toHaveCount(0);
    state.status = 200;
    state.malformedStatus = false;
    state.csrfStatus = 200;
    state.meStatus = 401;
    await page.getByRole("button", { name: "重试连接" }).click();
    await expect(
      page.getByRole("heading", {
        name: state.required ? "创建首个账户" : "进入你的工作台",
      }),
    ).toBeVisible();
  });

test("existing-account login remains available and restores authenticated sessions", async ({
  page,
}) => {
  const state = await mockServer(page);
  state.required = false;
  await page.goto(origin);
  await expect(
    page.getByRole("heading", { name: "进入你的工作台" }),
  ).toBeVisible();
  await expect(page.getByLabel("密码", { exact: true })).toHaveAttribute(
    "autocomplete",
    "current-password",
  );
  await expect(page.getByLabel("确认密码")).toHaveCount(0);
  await expect(page.getByLabel("一次性初始化码")).toHaveCount(0);
  state.meStatus = 200;
  await page.reload();
  await expect(
    page.getByRole("heading", { name: "所有设备，一个工作台。" }),
  ).toBeVisible();
});

test("setup masks secrets, confirms the password, submits once, then clears secrets before login", async ({
  page,
}) => {
  const state = await mockServer(page);
  await page.goto(origin);
  await fillSetup(page);
  for (const label of ["密码", "确认密码"]) {
    await expect(page.getByLabel(label, { exact: true })).toHaveAttribute(
      "type",
      "password",
    );
    await expect(page.getByLabel(label, { exact: true })).toHaveAttribute(
      "autocomplete",
      "new-password",
    );
  }
  await expect(page.getByLabel("一次性初始化码")).toHaveAttribute(
    "type",
    "password",
  );
  await expect(page.getByLabel("一次性初始化码")).toHaveAttribute(
    "autocomplete",
    "off",
  );
  await page.getByLabel("确认密码").fill("does-not-match");
  await page.getByRole("button", { name: "创建账户", exact: true }).click();
  await expect(page.getByRole("alert")).toHaveText("两次输入的密码不一致");
  expect(state.setupPosts).toHaveLength(0);
  await page.getByLabel("确认密码").fill(password);
  let release!: () => void;
  state.postGate = new Promise<void>((resolve) => {
    release = resolve;
  });
  await page.getByRole("button", { name: "创建账户", exact: true }).click();
  await expect(
    page.getByRole("button", { name: "正在创建账户…" }),
  ).toBeDisabled();
  await page.locator("form").evaluate((form) => {
    form.dispatchEvent(
      new Event("submit", { bubbles: true, cancelable: true }),
    );
    form.dispatchEvent(
      new Event("submit", { bubbles: true, cancelable: true }),
    );
  });
  await expect.poll(() => state.setupPosts.length).toBe(1);
  release();
  await expect(
    page.getByRole("heading", { name: "进入你的工作台" }),
  ).toBeVisible();
  await expect(page.getByRole("status")).toHaveText(
    "首个账户已创建。请使用刚设置的密码登录。",
  );
  await expect(page.getByLabel("用户名")).toHaveValue("first-user");
  await expect(page.getByLabel("密码", { exact: true })).toHaveValue("");
  await expect(page.getByLabel("确认密码")).toHaveCount(0);
  await expect(page.getByLabel("一次性初始化码")).toHaveCount(0);
  expect(state.setupPosts).toEqual([
    {
      username: "first-user",
      password,
      passwordConfirmation: password,
      setupCode,
    },
  ]);
  expect(state.loginPosts).toHaveLength(0);
  expect(page.url()).toBe(`${origin}/`);
  const storage = await page.evaluate(() =>
    JSON.stringify({
      local: { ...localStorage },
      session: { ...sessionStorage },
    }),
  );
  expect(storage).not.toContain(password);
  expect(storage).not.toContain(setupCode);
  expect(
    state.requests.every(
      (url) => !url.includes(password) && !url.includes(setupCode),
    ),
  ).toBeTruthy();
  await page.getByLabel("密码", { exact: true }).fill(password);
  await page.getByRole("button", { name: "连接工作台" }).click();
  await expect(
    page.getByRole("heading", { name: "所有设备，一个工作台。" }),
  ).toBeVisible();
  expect(state.loginPosts).toEqual([{ username: "first-user", password }]);
});

test("setup errors remain retryable and a completed setup in another tab switches to login", async ({
  page,
}) => {
  const state = await mockServer(page);
  state.postStatus = 400;
  await page.goto(origin);
  await fillSetup(page);
  await page.getByRole("button", { name: "创建账户", exact: true }).click();
  await expect(page.getByRole("alert")).toHaveText(state.postMessage);
  await expect(
    page.getByRole("heading", { name: "创建首个账户" }),
  ).toBeVisible();
  state.completeOnFailure = true;
  state.postStatus = 409;
  await page.getByRole("button", { name: "创建账户", exact: true }).click();
  await expect(
    page.getByRole("heading", { name: "进入你的工作台" }),
  ).toBeVisible();
  await expect(page.getByRole("status")).toHaveText(
    "初始化已完成。请使用已创建的账户登录。",
  );
  await expect(page.getByLabel("密码", { exact: true })).toHaveValue("");
});

test("setup refresh clears secrets and keeps language and appearance controls", async ({
  page,
}) => {
  await mockServer(page);
  await page.goto(origin);
  await fillSetup(page);
  await page.getByRole("button", { name: "界面语言", exact: true }).click();
  await page.getByRole("button", { name: "English", exact: true }).click();
  await expect(
    page.getByRole("heading", { name: "Create your first account" }),
  ).toBeVisible();
  await page.getByRole("button", { name: "Appearance", exact: true }).click();
  await page.getByRole("button", { name: "Dark mode", exact: true }).click();
  await expect(page.locator("html")).toHaveAttribute("data-theme", "dark");
  await page.reload();
  await expect(
    page.getByRole("heading", { name: "Create your first account" }),
  ).toBeVisible();
  await expect(page.locator("html")).toHaveAttribute("data-theme", "dark");
  for (const label of ["Password", "Confirm password", "One-time setup code"])
    await expect(page.getByLabel(label, { exact: true })).toHaveValue("");
  const desktopScreenshot = test.info().outputPath("setup-desktop.png");
  await page.screenshot({ path: desktopScreenshot, fullPage: true });
  await test.info().attach("First-account setup, desktop", {
    path: desktopScreenshot,
    contentType: "image/png",
  });
  await page.setViewportSize({ width: 390, height: 844 });
  await expect(
    page.getByRole("button", { name: "Create account", exact: true }),
  ).toBeVisible();
  expect(
    await page.evaluate(
      () => document.documentElement.scrollWidth <= innerWidth,
    ),
  ).toBeTruthy();
  const mobileScreenshot = test.info().outputPath("setup-mobile.png");
  await page.screenshot({ path: mobileScreenshot, fullPage: true });
  await test.info().attach("First-account setup, mobile", {
    path: mobileScreenshot,
    contentType: "image/png",
  });
});
