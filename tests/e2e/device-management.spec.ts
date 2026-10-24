import { expect, test, type Locator, type Page } from "@playwright/test";
import { resolve, sep } from "node:path";
import type { MachineInfo, SessionInfo } from "../../packages/protocol-ts";

const origin = "http://devices.ergent.test";
const dist = resolve("apps/web/dist");
const machine: MachineInfo = {
  id: "online",
  name: "Development PC",
  os: "windows",
  arch: "x86_64",
  online: true,
  revoked: false,
  defaultCwd: "C:\\Projects",
};
const session: SessionInfo = {
  id: "terminal",
  machineId: machine.id,
  agentEpoch: "epoch",
  title: "Build terminal",
  terminalTheme: null,
  cwd: machine.defaultCwd,
  shell: "cmd.exe",
  lifecycle: "running",
  cols: 80,
  rows: 24,
  exitCode: null,
  outputTruncated: false,
  toolStatus: null,
};

async function mockServer(page: Page) {
  const state = {
    machines: [
      { ...machine },
      {
        ...machine,
        id: "offline",
        name: "Offline Linux",
        os: "linux",
        online: false,
      },
    ],
    sessions: [{ ...session }],
    revokeStatus: 200,
    revokeGate: undefined as Promise<void> | undefined,
    enrollmentStatus: 200,
    requests: [] as string[],
    broadcasts: [] as (() => void)[],
    broadcast() {
      this.broadcasts.forEach((send) => send());
    },
  };
  await page.routeWebSocket("**/api/v1/ws/browser", (socket) => {
    let transfer = 0;
    const send = () =>
      socket.send(
        JSON.stringify({
          type: "inventory.chunk",
          payload: {
            transferId: String(++transfer),
            index: 0,
            last: true,
            dataBase64: Buffer.from(
              JSON.stringify({
                type: "inventory.sync",
                payload: { machines: state.machines, sessions: state.sessions },
              }),
            ).toString("base64"),
          },
        }),
      );
    state.broadcasts.push(send);
    send();
  });
  await page.route(`${origin}/**`, async (route) => {
    const request = route.request();
    const url = new URL(request.url());
    if (url.pathname.startsWith("/api/")) {
      state.requests.push(`${request.method()} ${url.pathname}`);
      if (url.pathname.endsWith("/auth/setup"))
        return route.fulfill({ json: { data: { required: false } } });
      if (url.pathname.endsWith("/auth/csrf"))
        return route.fulfill({ json: { data: { csrfToken: "test-csrf" } } });
      if (url.pathname.endsWith("/auth/me"))
        return route.fulfill({ json: { data: { username: "tester" } } });
      if (url.pathname.endsWith("/revoke")) {
        expect(request.method()).toBe("POST");
        expect(request.headers()["x-csrf-token"]).toBe("test-csrf");
        await state.revokeGate;
        return route.fulfill(
          state.revokeStatus === 200
            ? { json: { data: { revoked: true } } }
            : {
                status: state.revokeStatus,
                json: {
                  error: {
                    message:
                      state.revokeStatus === 404
                        ? "资源不存在"
                        : "设备连接不可用",
                  },
                },
              },
        );
      }
      if (request.method() === "PATCH") {
        const id = url.pathname.split("/").pop();
        const device = state.machines.find((item) => item.id === id)!;
        device.name = request.postDataJSON().name;
        state.broadcast();
        return route.fulfill({ json: { data: { id, name: device.name } } });
      }
      if (url.pathname.endsWith("/enrollments"))
        return route.fulfill(
          state.enrollmentStatus === 200
            ? { json: { data: { token: "test-enrollment" } } }
            : { status: 503, json: { error: { message: "内部错误" } } },
        );
      return route.fulfill({ status: 404 });
    }
    const path = resolve(
      dist,
      url.pathname === "/" ? "index.html" : `.${url.pathname}`,
    );
    if (!path.startsWith(dist + sep)) return route.fulfill({ status: 404 });
    return route.fulfill({ path });
  });
  await page.goto(origin);
  await expect(page.locator(".device-card")).toHaveCount(2);
  return state;
}

async function openManager(page: Page) {
  await page.getByRole("button", { name: "设备管理", exact: true }).click();
  return page.getByRole("dialog", { name: "设备管理", exact: true });
}

async function expectOutsideInteractionsToKeepOpen(
  page: Page,
  dialog: Locator,
) {
  await expect(dialog).toBeVisible();
  const bounds = (await dialog.boundingBox())!;
  const outside = { x: 4, y: 4 };
  expect(bounds.x > outside.x || bounds.y > outside.y).toBe(true);

  // Dragging a text selection beyond the dialog must not discard its contents.
  const field = dialog.locator("input, textarea").first();
  const start = (await (
    (await field.count()) ? field : dialog.getByRole("heading").first()
  ).boundingBox())!;
  await page.mouse.move(start.x + start.width / 2, start.y + start.height / 2);
  await page.mouse.down();
  await page.mouse.move(outside.x, outside.y, { steps: 5 });
  await page.mouse.up();
  await expect(dialog).toBeVisible();

  await page.mouse.click(outside.x, outside.y);
  await expect(dialog).toBeVisible();
}

for (const kind of [
  "terminal-rename",
  "create",
  "device-rename",
  "remove",
  "enroll",
  "manager",
  "terminals",
  "copy",
] as const) {
  test(`${kind} dialog survives outside clicks and drag release`, async ({
    page,
  }) => {
    await mockServer(page);
    let dialog: Locator;
    if (kind === "terminal-rename" || kind === "copy") {
      await page.locator(".session-card").click();
      const name = kind === "copy" ? "复制终端内容" : "重命名终端";
      await page.getByRole("button", { name, exact: true }).click();
      dialog = page.getByRole("dialog", { name, exact: true });
    } else if (kind === "create") {
      await page
        .getByRole("button", {
          name: `在 ${machine.name} 新建终端`,
          exact: true,
        })
        .click();
      dialog = page.getByRole("dialog", { name: "新建终端", exact: true });
    } else if (kind === "terminals") {
      await page
        .getByRole("button", {
          name: `查看 ${machine.name} 的终端`,
          exact: true,
        })
        .click();
      dialog = page.getByRole("dialog", { name: machine.name, exact: true });
    } else {
      const manager = await openManager(page);
      dialog = manager;
      if (kind === "enroll") {
        await manager
          .getByRole("button", { name: "接入设备", exact: true })
          .click();
        dialog = page.getByRole("dialog", { name: "接入设备", exact: true });
        await expect(dialog).toContainText("test-enrollment");
      } else if (kind === "device-rename" || kind === "remove") {
        await manager
          .getByRole("button", { name: `管理 ${machine.name}`, exact: true })
          .click();
        await page
          .getByRole("button", {
            name: kind === "remove" ? "移除设备" : "重命名设备",
            exact: true,
          })
          .click();
        dialog = page.getByRole("dialog", {
          name: kind === "remove" ? `移除「${machine.name}」？` : "重命名设备",
          exact: true,
        });
      }
    }

    const field = dialog.locator("input").first();
    if (await field.count()) await field.fill("Unsaved draft");
    await expectOutsideInteractionsToKeepOpen(page, dialog);
    if (await field.count()) await expect(field).toHaveValue("Unsaved draft");
    if (kind === "enroll")
      await expect(dialog).toContainText("test-enrollment");

    if (kind === "terminal-rename") {
      await dialog
        .getByRole("button", { name: "关闭弹窗", exact: true })
        .click();
    } else {
      await page.keyboard.press("Escape");
    }
    await expect(dialog).toHaveCount(0);
  });
}

test("overview device opens its terminal list and selects an existing terminal without creating one", async ({
  page,
}) => {
  const state = await mockServer(page);
  state.sessions.push({
    ...session,
    id: "other",
    machineId: "offline",
    title: "Other device terminal",
  });
  state.broadcast();
  const trigger = page.getByRole("button", {
    name: `查看 ${machine.name} 的终端`,
    exact: true,
  });
  await trigger.click();
  const drawer = page.getByRole("dialog", { name: machine.name, exact: true });
  await expect(drawer.getByRole("listitem")).toHaveCount(1);
  await expect(drawer).not.toContainText("Other device terminal");
  await page.keyboard.press("Escape");
  await expect(drawer).toHaveCount(0);
  await expect(trigger).toBeFocused();
  await trigger.click();
  await drawer
    .getByRole("button", { name: session.title, exact: true })
    .click();
  await expect(drawer).toHaveCount(0);
  await expect(page.locator(".terminal-title")).toHaveText(session.title);
  expect(
    state.requests.filter((request) => /^POST .*\/sessions$/.test(request)),
  ).toEqual([]);
});

test("mobile terminal list creates on the selected device and opens the result", async ({
  page,
}) => {
  await page.setViewportSize({ width: 390, height: 844 });
  const state = await mockServer(page);
  state.machines[1].online = true;
  state.machines[1].name = "Long-device-name-".repeat(6);
  state.broadcast();
  const target = state.machines[1];
  let created: SessionInfo;
  await page.route(
    `${origin}/api/v1/machines/offline/sessions`,
    async (route) => {
      expect(route.request().method()).toBe("POST");
      expect(route.request().headers()["x-csrf-token"]).toBe("test-csrf");
      created = {
        ...session,
        id: "created",
        machineId: target.id,
        title: route.request().postDataJSON().title,
      };
      await route.fulfill({
        json: { data: { operationId: "create-test", sessionId: created.id } },
      });
    },
  );
  await page.route(`${origin}/api/v1/operations/create-test`, (route) =>
    route.fulfill({
      json: {
        data: {
          status: "succeeded",
          sessionId: created.id,
          result: { session: created },
        },
      },
    }),
  );
  await page
    .getByRole("button", { name: `查看 ${target.name} 的终端`, exact: true })
    .click();
  const drawer = page.getByRole("dialog", { name: target.name, exact: true });
  await expect(drawer.getByText("暂无终端", { exact: true })).toBeVisible();
  expect(await drawer.boundingBox()).toMatchObject({
    x: 0,
    y: 0,
    width: 390,
    height: 844,
  });
  expect(
    await drawer.evaluate(
      (element) => element.scrollWidth <= element.clientWidth,
    ),
  ).toBe(true);
  await page.screenshot({
    path: test.info().outputPath("terminal-list-mobile.png"),
  });
  await drawer.getByRole("button", { name: "新建终端", exact: true }).click();
  const form = page.getByRole("dialog", { name: "新建终端", exact: true });
  await expect(
    form.getByRole("combobox", { name: "设备", exact: true }),
  ).toHaveValue(target.id);
  await page.keyboard.press("Escape");
  await expect(form).toHaveCount(0);
  await expect(
    drawer.getByRole("button", { name: "新建终端", exact: true }),
  ).toBeFocused();
  await drawer.getByRole("button", { name: "新建终端", exact: true }).click();
  await form
    .getByLabel("终端名称", { exact: true })
    .fill("New terminal from list");
  await form.getByRole("button", { name: "创建并连接", exact: true }).click();
  await expect(page.getByRole("dialog")).toHaveCount(0);
  await expect(page.locator(".terminal-title")).toHaveText(
    "New terminal from list",
  );
});

test("offline terminal list updates live and closes when device is revoked", async ({
  page,
}) => {
  const state = await mockServer(page);
  state.sessions.push({
    ...session,
    id: "offline-session",
    machineId: "offline",
    title: "Last session",
  });
  state.broadcast();
  await page
    .getByRole("button", { name: "查看 Offline Linux 的终端", exact: true })
    .click();
  const drawer = page.getByRole("dialog", {
    name: "Offline Linux",
    exact: true,
  });
  await expect(
    drawer.getByRole("button", { name: "新建终端", exact: true }),
  ).toBeDisabled();
  await expect(
    drawer.getByRole("button", { name: "Last session", exact: true }),
  ).toBeEnabled();
  state.machines[1].online = true;
  state.sessions[1].title = "Updated terminal";
  state.broadcast();
  await expect(
    drawer.getByRole("button", { name: "新建终端", exact: true }),
  ).toBeEnabled();
  await expect(
    drawer.getByRole("button", { name: "Updated terminal", exact: true }),
  ).toBeVisible();
  state.machines[1].revoked = true;
  state.broadcast();
  await expect(drawer).toHaveCount(0);
});
async function removeFromManager(page: Page, name: string) {
  const drawer = await openManager(page);
  await drawer
    .getByRole("button", { name: `管理 ${name}`, exact: true })
    .click();
  await page.getByRole("button", { name: "移除设备", exact: true }).click();
  return page.getByRole("dialog", { name: `移除「${name}」？`, exact: true });
}

test("device drawer lists online/offline devices, renames, and restores keyboard focus", async ({
  page,
}) => {
  await mockServer(page);
  const drawer = await openManager(page);
  await expect(drawer.getByRole("listitem")).toHaveCount(2);
  await expect(
    drawer.getByRole("listitem").filter({ hasText: machine.name }),
  ).toContainText("1 个终端");
  await expect(
    drawer.getByRole("listitem").filter({ hasText: "Offline Linux" }),
  ).toContainText("离线");
  await drawer
    .getByRole("button", { name: "管理 Offline Linux", exact: true })
    .click();
  await page.getByRole("button", { name: "重命名设备", exact: true }).click();
  const rename = page.getByRole("dialog", { name: "重命名设备", exact: true });
  await rename
    .getByLabel("设备名称", { exact: true })
    .fill("Linux workstation");
  await rename.getByRole("button", { name: "保存名称" }).click();
  await expect(
    drawer.getByRole("heading", { name: "Linux workstation" }),
  ).toBeVisible();
  await page.keyboard.press("Escape");
  await expect(drawer).toHaveCount(0);
  await expect(
    page.getByRole("button", { name: "设备管理", exact: true }),
  ).toBeFocused();
});

test("removal is explicit, warns about active terminals, and submits once while pending", async ({
  page,
}) => {
  const state = await mockServer(page);
  await page.locator(".session-card").click();
  let confirmation = await removeFromManager(page, machine.name);
  await expect(confirmation).toContainText("1 个未结束的终端");
  await expect(confirmation).toContainText("这不会结束设备上的任务");
  await expect(
    confirmation.getByRole("button", { name: "取消", exact: true }),
  ).toBeFocused();
  await confirmation.getByRole("button", { name: "取消", exact: true }).click();
  expect(
    state.requests.filter((request) => request.endsWith("/revoke")),
  ).toEqual([]);
  const drawer = page.getByRole("dialog", { name: "设备管理", exact: true });
  await drawer.getByRole("button", { name: `管理 ${machine.name}` }).click();
  await page.getByRole("button", { name: "移除设备", exact: true }).click();
  confirmation = page.getByRole("dialog", {
    name: `移除「${machine.name}」？`,
  });
  let release!: () => void;
  state.revokeGate = new Promise((resolve) => {
    release = resolve;
  });
  await confirmation
    .getByRole("button", { name: "移除设备", exact: true })
    .click();
  await expect(
    confirmation.getByRole("button", { name: "正在移除…" }),
  ).toBeDisabled();
  await page.keyboard.press("Escape");
  await expect(confirmation).toBeVisible();
  release();
  await expect(confirmation).toHaveCount(0);
  await expect(drawer.getByRole("listitem")).toHaveCount(1);
  // An older snapshot must not resurrect a confirmed removal.
  state.broadcast();
  await expect(drawer.getByRole("listitem")).toHaveCount(1);
  await expect(
    drawer.getByRole("button", { name: "关闭设备管理" }),
  ).toBeFocused();
  await drawer.getByRole("button", { name: "关闭设备管理" }).click();
  await expect(page.locator(".session-card")).toHaveCount(0);
  await expect(page.locator(".terminal-page")).toHaveCount(0);
  await expect(page.locator(".device-card")).toHaveCount(1);
  expect(
    state.requests.filter((request) => request.endsWith("/revoke")),
  ).toEqual(["POST /api/v1/machines/online/revoke"]);
});

test("failed removal keeps device and confirmation available for retry", async ({
  page,
}) => {
  const state = await mockServer(page);
  state.revokeStatus = 503;
  const confirmation = await removeFromManager(page, "Offline Linux");
  await expect(confirmation.locator(".remove-device-warning")).toHaveCount(0);
  await confirmation
    .getByRole("button", { name: "移除设备", exact: true })
    .click();
  await expect(confirmation.getByRole("alert")).toHaveText("设备连接不可用");
  await expect(
    confirmation.getByRole("button", { name: "移除设备", exact: true }),
  ).toBeEnabled();
  state.revokeStatus = 200;
  await confirmation
    .getByRole("button", { name: "移除设备", exact: true })
    .click();
  await expect(confirmation).toHaveCount(0);
  await expect(
    page
      .getByRole("dialog", { name: "设备管理", exact: true })
      .getByRole("listitem"),
  ).toHaveCount(1);
});

test("card menu removes an already revoked device and remote revocation closes its terminal", async ({
  page,
}) => {
  const state = await mockServer(page);
  state.revokeStatus = 404;
  await page
    .locator(".device-card")
    .filter({ hasText: "Offline Linux" })
    .getByRole("button", { name: "管理 Offline Linux" })
    .click();
  await page.getByRole("button", { name: "移除设备", exact: true }).click();
  await page
    .getByRole("dialog")
    .getByRole("button", { name: "移除设备", exact: true })
    .click();
  await expect(page.locator(".device-card")).toHaveCount(1);
  await page.locator(".session-card").click();
  await expect(page.locator(".terminal-page")).toBeVisible();
  state.machines[0].revoked = true;
  state.broadcast();
  await expect(page.locator(".terminal-page")).toHaveCount(0);
  const drawer = await openManager(page);
  await expect(drawer.getByText("暂无设备", { exact: true })).toBeVisible();
  await drawer.getByRole("button", { name: "接入设备", exact: true }).click();
  await expect(
    page.getByRole("dialog", { name: "接入设备", exact: true }),
  ).toContainText("test-enrollment");
});

test("enrollment errors are visible inside device management", async ({
  page,
}) => {
  const state = await mockServer(page);
  state.enrollmentStatus = 503;
  const drawer = await openManager(page);
  await drawer.getByRole("button", { name: "接入设备", exact: true }).click();
  await expect(drawer.getByRole("alert")).toHaveText("内部错误");
});

test("mobile English dark drawer and confirmation fit, including long device names", async ({
  page,
}) => {
  await page.addInitScript(() => {
    localStorage.setItem("ergent.language", "en");
  });
  await page.emulateMedia({ colorScheme: "dark" });
  await page.setViewportSize({ width: 390, height: 844 });
  const state = await mockServer(page);
  state.machines[1].name = "Long-device-".repeat(10);
  state.broadcast();
  await page
    .getByRole("button", { name: "Devices / Terminals", exact: true })
    .click();
  await page
    .getByRole("button", { name: "Device management", exact: true })
    .click();
  const drawer = page.getByRole("dialog", {
    name: "Device management",
    exact: true,
  });
  const bounds = await drawer.boundingBox();
  expect(bounds).toMatchObject({ x: 0, y: 0, width: 390, height: 844 });
  expect(
    await drawer.evaluate(
      (element) => element.scrollWidth <= element.clientWidth,
    ),
  ).toBe(true);
  await page.screenshot({
    path: test.info().outputPath("devices-mobile-dark.png"),
    fullPage: true,
  });
  await drawer
    .getByRole("button", {
      name: `Manage ${state.machines[1].name}`,
      exact: true,
    })
    .click();
  await page
    .getByRole("button", { name: "Remove device", exact: true })
    .click();
  const confirmation = page.getByRole("dialog", {
    name: `Remove “${state.machines[1].name}”?`,
    exact: true,
  });
  await expect(confirmation).toContainText(
    "Files on the device will not be deleted",
  );
  expect(
    await confirmation.evaluate(
      (element) => element.scrollWidth <= element.clientWidth,
    ),
  ).toBe(true);
  await page.screenshot({
    path: test.info().outputPath("remove-mobile-dark.png"),
    fullPage: true,
  });
  await confirmation
    .getByRole("button", { name: "Cancel", exact: true })
    .click();
  await expect(drawer).toBeVisible();
});
