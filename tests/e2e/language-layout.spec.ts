import { expect, test, type Page } from "@playwright/test";
import { resolve, sep } from "node:path";

const origin = "http://layout.ergent.test";
const dist = resolve("apps/web/dist");
const machine = {
  id: "layout-device",
  name: "Development workstation with a very long name",
  os: "linux",
  arch: "x86_64",
  online: true,
  revoked: false,
  defaultCwd: "/tmp",
};
const session = {
  id: "layout-terminal",
  machineId: machine.id,
  agentEpoch: "epoch",
  title: "A long terminal title for testing language switching",
  terminalTheme: null,
  cwd: "/tmp",
  shell: "/bin/bash",
  lifecycle: "running",
  cols: 80,
  rows: 24,
  exitCode: null,
  outputTruncated: false,
  toolStatus: null,
};

async function openApp(page: Page, authenticated = true) {
  await page.routeWebSocket("**/api/v1/ws/browser", (socket) => {
    socket.send(
      JSON.stringify({
        type: "inventory.chunk",
        payload: {
          transferId: "1",
          index: 0,
          last: true,
          dataBase64: Buffer.from(
            JSON.stringify({
              type: "inventory.sync",
              payload: {
                machines: [machine],
                sessions: [session],
              },
            }),
          ).toString("base64"),
        },
      }),
    );
  });
  await page.route(`${origin}/**`, (route) => {
    const url = new URL(route.request().url());
    if (url.pathname.endsWith("/auth/setup"))
      return route.fulfill({ json: { data: { required: false } } });
    if (url.pathname.endsWith("/auth/csrf"))
      return route.fulfill({ json: { data: { csrfToken: "layout-test" } } });
    if (url.pathname.endsWith("/auth/me"))
      return route.fulfill(
        authenticated
          ? { json: { data: { username: "tester" } } }
          : { status: 401, json: { error: { message: "需要登录" } } },
      );
    if (url.pathname.startsWith("/api/")) return route.fulfill({ status: 404 });
    const path = resolve(
      dist,
      url.pathname === "/" ? "index.html" : `.${url.pathname}`,
    );
    if (!path.startsWith(dist + sep)) return route.fulfill({ status: 404 });
    return route.fulfill({ path });
  });
  await page.goto(origin);
  await expect(
    page.locator(authenticated ? ".device-card" : ".login-box form"),
  ).toBeVisible();
}

async function language(page: Page, value: "en" | "zh") {
  await page.locator(".language-control > button").click();
  await page
    .getByRole("button", {
      name: value === "en" ? "English" : "简体中文",
      exact: true,
    })
    .click();
  await expect(page.locator("html")).toHaveAttribute(
    "lang",
    value === "en" ? "en" : "zh-CN",
  );
}

async function geometry(page: Page, selectors: string[]) {
  return page.evaluate(
    (selectors) =>
      selectors.map((selector) => {
        const rect = document.querySelector(selector)!.getBoundingClientRect();
        return { x: rect.x, y: rect.y, width: rect.width, height: rect.height };
      }),
    selectors,
  );
}

async function noHorizontalOverflow(page: Page) {
  expect(
    await page.evaluate(
      () => document.documentElement.scrollWidth <= innerWidth,
    ),
  ).toBe(true);
}

for (const width of [320, 390, 768, 1024, 1440]) {
  test(`language changes preserve terminal geometry at ${width}px`, async ({
    page,
  }) => {
    await page.setViewportSize({ width, height: 900 });
    await openApp(page);
    if (width <= 760) await page.locator(".mobile-nav-toggle").click();
    await page.locator(".session-card").click();
    await expect(page.locator(".terminal-canvas")).toBeVisible();
    const selectors = [
      ".topbar",
      ".terminal-heading",
      ".terminal-frame",
      ".terminal-canvas",
    ];
    const initial = await geometry(page, selectors);
    for (const locale of ["en", "zh", "en"] as const) {
      await language(page, locale);
      expect(await geometry(page, selectors)).toEqual(initial);
      await noHorizontalOverflow(page);
    }
    await page
      .getByRole("button", { name: "Close terminal", exact: true })
      .click();
    const confirmation = await geometry(page, selectors);
    await language(page, "zh");
    expect(await geometry(page, selectors)).toEqual(confirmation);
    await expect(page.locator(".page-actions .danger")).toBeInViewport();
    await noHorizontalOverflow(page);
    await page.getByRole("button", { name: "取消", exact: true }).click();
    expect(await geometry(page, selectors)).toEqual(initial);
  });

  test(`language changes preserve workspace layout at ${width}px`, async ({
    page,
  }) => {
    await page.setViewportSize({ width, height: 900 });
    await openApp(page);
    const selectors = [".topbar", ".sidebar", ".stats", ".device-card"];
    const initial = await geometry(page, selectors);
    await language(page, "en");
    expect(await geometry(page, selectors)).toEqual(initial);
    await noHorizontalOverflow(page);
  });

  test(`login stays inside the viewport in both languages at ${width}px`, async ({
    page,
  }) => {
    await page.setViewportSize({ width, height: 900 });
    await openApp(page, false);
    const initial = await geometry(page, [".login-form", ".login-box"]);
    await language(page, "en");
    expect(await geometry(page, [".login-form", ".login-box"])).toEqual(
      initial,
    );
    await noHorizontalOverflow(page);
    await expect(page.locator(".login-box form > button")).toBeInViewport();
  });
}
