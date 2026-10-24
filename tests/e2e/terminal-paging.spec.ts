import { expect, test, type Page, type WebSocketRoute } from "@playwright/test";
import { resolve, sep } from "node:path";
import type {
  BrowserCommand,
  Event,
  MachineInfo,
  SessionInfo,
} from "../../packages/protocol-ts";

const origin = "http://terminal-paging.ergent.test";
const dist = resolve("apps/web/dist");
const history = Array.from(
  { length: 150 },
  (_, index) => `HISTORY_${index + 1}\r\n`,
).join("");
const names = ["上一页", "下一页", "回到最新"];

// Serve the shipped frontend and exercise its actual xterm/transport integration.
// The terminal peer is deterministic, so these tests need no Rust server or PTY.
async function terminalPeer(page: Page, initialScreen = history) {
  const machine: MachineInfo = {
    id: "paging-machine",
    name: "Paging machine",
    os: "linux",
    arch: "test",
    online: true,
    revoked: false,
    defaultCwd: "/tmp",
  };
  const session: SessionInfo = {
    id: "paging-session",
    machineId: machine.id,
    agentEpoch: "epoch",
    title: "Paging terminal",
    terminalTheme: null,
    cwd: "/tmp",
    shell: "sh",
    lifecycle: "running",
    cols: 80,
    rows: 24,
    exitCode: null,
    outputTruncated: false,
    toolStatus: null,
  };
  let socket: WebSocketRoute;
  let streamId = "",
    revision = 0,
    seq = 0,
    transfer = 0;
  let snapshotPending = false,
    pending = "",
    screen = initialScreen,
    writable = true;
  const inputs: string[] = [];
  const send = (event: Event) => socket.send(JSON.stringify(event));
  const inventory = () =>
    send({
      type: "inventory.chunk",
      payload: {
        transferId: String(++transfer),
        index: 0,
        last: true,
        dataBase64: Buffer.from(
          JSON.stringify({
            type: "inventory.sync",
            payload: { machines: [machine], sessions: [session] },
          }),
        ).toString("base64"),
      },
    });
  const lease = (value: boolean) => {
    writable = value;
    send({
      type: "terminal.lease.changed",
      payload: {
        sessionId: session.id,
        streamId,
        leaseId: value ? "lease" : null,
        writable: value,
      },
    });
  };
  await page.routeWebSocket("**/api/v1/ws/browser", (ws) => {
    socket = ws;
    inventory();
    ws.onMessage((raw) => {
      const command = JSON.parse(raw.toString()) as BrowserCommand;
      switch (command.type) {
        case "terminal.attach":
          streamId = command.payload.requestId;
          snapshotPending = true;
          pending = "";
          seq = 0;
          send({
            type: "terminal.sync",
            payload: {
              sessionId: session.id,
              requestId: command.payload.requestId,
              streamId,
              epoch: session.agentEpoch,
              seq: "0",
              head: "0",
              revision: String(revision),
              cols: session.cols,
              rows: session.rows,
              reset: true,
            },
          });
          break;
        case "terminal.pull":
          if (snapshotPending) {
            snapshotPending = false;
            send({
              type: "terminal.snapshot",
              payload: {
                sessionId: session.id,
                streamId,
                epoch: session.agentEpoch,
                seq: "0",
                index: 0,
                first: true,
                last: true,
                dataBase64: Buffer.from(screen).toString("base64"),
              },
            });
          } else {
            const from = seq;
            seq += Buffer.byteLength(pending);
            const dataBase64 = Buffer.from(pending).toString("base64");
            pending = "";
            send({
              type: "terminal.output",
              payload: {
                sessionId: session.id,
                streamId,
                epoch: session.agentEpoch,
                from: String(from),
                seq: String(seq),
                dataBase64,
              },
            });
          }
          break;
        case "terminal.resize":
          session.cols = command.payload.cols;
          session.rows = command.payload.rows;
          revision++;
          send({
            type: "terminal.resync",
            payload: {
              sessionId: session.id,
              streamId,
              reason: "resized",
            },
          });
          break;
        case "terminal.lease.claim":
          lease(writable);
          break;
        case "terminal.input":
          inputs.push(
            Buffer.from(command.payload.dataBase64, "base64").toString(),
          );
          send({
            type: "terminal.input.ack",
            payload: { inputId: command.payload.inputId },
          });
          break;
      }
    });
  });
  await page.route(`${origin}/**`, async (route) => {
    const pathname = new URL(route.request().url()).pathname;
    if (pathname.startsWith("/api/")) {
      if (pathname.endsWith("/auth/setup"))
        return route.fulfill({ json: { data: { required: false } } });
      if (pathname.endsWith("/auth/csrf"))
        return route.fulfill({ json: { data: { csrfToken: "test" } } });
      if (pathname.endsWith("/auth/me"))
        return route.fulfill({ json: { data: { username: "tester" } } });
      return route.fulfill({ status: 404 });
    }
    const path = resolve(
      dist,
      pathname === "/" ? "index.html" : `.${pathname}`,
    );
    if (!path.startsWith(dist + sep)) return route.fulfill({ status: 404 });
    return route.fulfill({ path });
  });
  await page.goto(origin);
  await page
    .getByRole("button", { name: `查看 ${machine.name} 的终端`, exact: true })
    .click();
  await page
    .getByRole("dialog", { name: machine.name, exact: true })
    .getByRole("button", { name: session.title, exact: true })
    .click();
  await expect(page.getByText("已连接 · 输入在设备上执行")).toBeVisible();
  return {
    inputs,
    lease,
    write(text: string) {
      screen += text;
      pending += text;
    },
    finish() {
      session.lifecycle = "exited";
      session.exitCode = 0;
      inventory();
    },
  };
}

test.use({
  viewport: { width: 390, height: 844 },
  isMobile: true,
  hasTouch: true,
});

test("mobile paging browses normal scrollback without sending input or opening the keyboard", async ({
  page,
}) => {
  const peer = await terminalPeer(page, "NORMAL_SCREEN\r\n");
  peer.write(history);
  const viewport = page.locator(".xterm-viewport");
  const textarea = page.locator(".xterm-helper-textarea");
  const navigation = page.getByRole("group", { name: "终端翻页", exact: true });
  await expect(page.locator(".xterm-rows")).toContainText("HISTORY_150");
  await expect(navigation.getByRole("button")).toHaveText(names);
  await expect(page.locator(".extra-keys")).toBeHidden();
  const bottom = await viewport.evaluate((element) => element.scrollTop);
  expect(bottom).toBeGreaterThan(0);

  await navigation.getByRole("button", { name: "上一页", exact: true }).tap();
  await expect
    .poll(() => viewport.evaluate((element) => element.scrollTop))
    .toBeLessThan(bottom);
  await expect(page.locator(".xterm-rows")).not.toContainText("HISTORY_150");
  await expect(textarea).not.toBeFocused();
  await navigation.getByRole("button", { name: "下一页", exact: true }).tap();
  await expect
    .poll(() => viewport.evaluate((element) => element.scrollTop))
    .toBe(bottom);
  await navigation.getByRole("button", { name: "上一页", exact: true }).tap();
  await navigation.getByRole("button", { name: "上一页", exact: true }).tap();
  await navigation.getByRole("button", { name: "回到最新", exact: true }).tap();
  await expect
    .poll(() => viewport.evaluate((element) => element.scrollTop))
    .toBe(bottom);
  await expect(page.locator(".xterm-rows")).toContainText("HISTORY_150");
  await expect(textarea).not.toBeFocused();
  expect(peer.inputs).toEqual([]);

  // Reading local history remains available when another browser owns input.
  peer.lease(false);
  await expect(page.getByText("只读 · 其他窗口可能正在操作")).toBeVisible();
  await navigation.getByRole("button", { name: "上一页", exact: true }).tap();
  await expect
    .poll(() => viewport.evaluate((element) => element.scrollTop))
    .toBeLessThan(bottom);
  await navigation.getByRole("button", { name: "回到最新", exact: true }).tap();
  await expect
    .poll(() => viewport.evaluate((element) => element.scrollTop))
    .toBe(bottom);
  peer.finish();
  await expect(page.locator(".page-heading .badge")).toHaveText("已退出");
  await navigation.getByRole("button", { name: "上一页", exact: true }).tap();
  await expect
    .poll(() => viewport.evaluate((element) => element.scrollTop))
    .toBeLessThan(bottom);
  expect(peer.inputs).toEqual([]);
});

test("mobile paging forwards full-screen keys through the writable lease and tracks buffer switches", async ({
  page,
}) => {
  const peer = await terminalPeer(page, "\x1b[?1049hFULL_SCREEN");
  const navigation = page.getByRole("group", { name: "终端翻页", exact: true });
  const textarea = page.locator(".xterm-helper-textarea");
  await expect(page.locator(".xterm-rows")).toContainText("FULL_SCREEN");
  await navigation.getByRole("button", { name: "上一页", exact: true }).tap();
  await navigation.getByRole("button", { name: "下一页", exact: true }).tap();
  await navigation.getByRole("button", { name: "回到最新", exact: true }).tap();
  const expectedInputs = ["\x1b[5~", "\x1b[6~", "\x1b[F"];
  await expect.poll(() => peer.inputs).toEqual(expectedInputs);
  peer.write("\x1b[?1h\r\nAPPLICATION_CURSOR");
  await expect(page.locator(".xterm-rows")).toContainText("APPLICATION_CURSOR");
  await navigation.getByRole("button", { name: "回到最新", exact: true }).tap();
  expectedInputs.push("\x1bOF");
  await expect.poll(() => peer.inputs).toEqual(expectedInputs);
  peer.write("\x1b[?1l\r\nNORMAL_CURSOR");
  await expect(page.locator(".xterm-rows")).toContainText("NORMAL_CURSOR");
  await navigation.getByRole("button", { name: "回到最新", exact: true }).tap();
  expectedInputs.push("\x1b[F");
  await expect.poll(() => peer.inputs).toEqual(expectedInputs);
  await expect(textarea).not.toBeFocused();

  peer.lease(false);
  await expect(page.getByText("只读 · 其他窗口可能正在操作")).toBeVisible();
  for (const name of names) {
    const button = navigation.getByRole("button", { name, exact: true });
    await expect(button).toBeDisabled();
    // A real touch at a disabled control must not send an input command.
    const box = (await button.boundingBox())!;
    await page.touchscreen.tap(box.x + box.width / 2, box.y + box.height / 2);
  }
  expect(peer.inputs).toEqual(expectedInputs);
  await expect(textarea).not.toBeFocused();

  peer.write("\x1b[?1049l" + history);
  await expect(page.locator(".xterm-rows")).toContainText("HISTORY_150");
  for (const name of names)
    await expect(
      navigation.getByRole("button", { name, exact: true }),
    ).toBeEnabled();
  const viewport = page.locator(".xterm-viewport");
  const bottom = await viewport.evaluate((element) => element.scrollTop);
  await navigation.getByRole("button", { name: "上一页", exact: true }).tap();
  await expect
    .poll(() => viewport.evaluate((element) => element.scrollTop))
    .toBeLessThan(bottom);
  expect(peer.inputs).toEqual(expectedInputs);

  peer.write("\x1b[?1049hFULL_SCREEN_AGAIN");
  await expect(page.locator(".xterm-rows")).toContainText("FULL_SCREEN_AGAIN");
  await expect(
    navigation.getByRole("button", { name: "上一页", exact: true }),
  ).toBeDisabled();
  peer.lease(true);
  await expect(
    navigation.getByRole("button", { name: "上一页", exact: true }),
  ).toBeEnabled();
  await navigation.getByRole("button", { name: "上一页", exact: true }).tap();
  expectedInputs.push("\x1b[5~");
  await expect.poll(() => peer.inputs).toEqual(expectedInputs);
  peer.finish();
  for (const name of names)
    await expect(
      navigation.getByRole("button", { name, exact: true }),
    ).toBeDisabled();
});

test("paging buttons stay visible and tappable on touch devices and hidden on desktop", async ({
  page,
  browser,
}) => {
  const peer = await terminalPeer(page);
  const navigation = page.getByRole("group", { name: "终端翻页", exact: true });
  for (const size of [
    { width: 390, height: 400 },
    { width: 844, height: 390 },
    { width: 320, height: 568 },
    { width: 1440, height: 960 },
  ]) {
    await page.setViewportSize(size);
    await expect(page.getByText("已连接 · 输入在设备上执行")).toBeVisible();
    expect(
      await page.evaluate(
        () => document.documentElement.scrollWidth <= innerWidth,
      ),
    ).toBe(true);
    const canvas = (await page.locator(".terminal-canvas").boundingBox())!;
    expect(canvas.height).toBeGreaterThan(50);
    const primary = page.locator(".primary-keys");
    await expect(primary.getByRole("button")).toHaveText([
      "键盘",
      ...names,
      "Esc",
      "↑",
      "",
    ]);
    const keyboard = (await primary
      .getByRole("button", { name: "键盘", exact: true })
      .boundingBox())!;
    const keys = (await page.locator(".terminal-keys").boundingBox())!;
    expect(keys.height).toBeLessThan(65);
    for (const button of await primary.getByRole("button").all()) {
      const box = (await button.boundingBox())!;
      expect(Math.abs(box.y - keyboard.y)).toBeLessThan(1);
    }
    for (const name of names) {
      const button = navigation.getByRole("button", { name, exact: true });
      await expect(button).toBeVisible();
      const box = (await button.boundingBox())!;
      expect(box.height).toBeGreaterThanOrEqual(44);
      expect(box.x).toBeGreaterThanOrEqual(0);
      expect(box.x + box.width).toBeLessThanOrEqual(size.width);
      expect(box.y + box.height).toBeLessThanOrEqual(size.height);
      await button.tap();
    }
    await expect(page.locator(".xterm-helper-textarea")).not.toBeFocused();
  }
  expect(peer.inputs).toEqual([]);
  await page.setViewportSize({ width: 390, height: 844 });
  await page.screenshot({
    path: "test-results/terminal-paging-mobile.png",
  });
  const desktop = await browser.newContext({
    locale: "zh-CN",
    hasTouch: false,
    isMobile: false,
    viewport: { width: 1440, height: 960 },
  });
  try {
    const desktopPage = await desktop.newPage();
    await terminalPeer(desktopPage);
    for (const size of [
      { width: 1440, height: 960 },
      { width: 390, height: 844 },
    ]) {
      await desktopPage.setViewportSize(size);
      await expect(desktopPage.locator(".terminal-keys")).toBeHidden();
      await expect(desktopPage.locator(".xterm-screen")).toBeVisible();
      await expect(desktopPage.locator(".terminal-status")).toBeVisible();
    }
    await desktopPage.setViewportSize({ width: 1440, height: 960 });
    await desktopPage.screenshot({
      path: "test-results/terminal-desktop-no-shortcuts.png",
    });
  } finally {
    await desktop.close();
  }
});
