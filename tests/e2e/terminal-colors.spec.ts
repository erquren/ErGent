import { expect, test } from "@playwright/test";
import { createRequire } from "node:module";
import { resolve } from "node:path";

const require = createRequire(resolve("apps/web/package.json"));
const xterm = require.resolve("@xterm/xterm");
const xtermCss = require.resolve("@xterm/xterm/css/xterm.css");
const colorCss = resolve("apps/web/src/terminal-colors.css");

for (const theme of [
  {
    name: "dark",
    cursor: "#ffffff",
    background: "#000000",
    foreground: "#ffffff",
    green: "#8cdb79",
    blue: "#78b5ff",
    black: "#555555",
  },
  {
    name: "light",
    cursor: "#000000",
    background: "#ffffff",
    foreground: "#000000",
    green: "#24732a",
    blue: "#185abd",
    black: "#111111",
  },
]) {
  test(`writable directory colors are readable in the ${theme.name} terminal`, async ({
    page,
  }) => {
    await page.setContent(
      '<div class="terminal-host hide-ansi-green-background" style="width:800px;height:400px"></div>',
    );
    await page.addStyleTag({ path: xtermCss });
    await page.addStyleTag({ path: colorCss });
    await page.addScriptTag({ path: xterm });
    await page.evaluate(async (theme) => {
      const term = new (window as any).Terminal({ theme, cols: 80, rows: 10 });
      term.open(document.querySelector(".terminal-host"));
      (window as any).term = term;
      // Split escape sequences between writes to verify the parser still owns them.
      await new Promise<void>((done) => term.write("\x1b[34;", done));
      await new Promise<void>((done) =>
        term.write(
          "42mother\x1b[0m \x1b[30;42msticky\x1b[0m \x1b[32mgreen\x1b[0m \x1b[31;42mred-on-green\x1b[0m \x1b[32;40;7mreverse\x1b[0m \x1b[38;5;4;48;5;2mindexed\x1b[0m \x1b[31;44mred-on-blue\x1b[0m normal",
          done,
        ),
      );
      await new Promise((done) =>
        requestAnimationFrame(() => requestAnimationFrame(done)),
      );
    }, theme);
    const read = async (content: string) =>
      page
        .locator(".xterm-rows span")
        .filter({ hasText: new RegExp(`^\\s*${content}\\s*$`) })
        .evaluate((element) => {
          const css = getComputedStyle(element);
          return { color: css.color, background: css.backgroundColor };
        });
    const rgb = (hex: string) =>
      `rgb(${[1, 3, 5].map((index) => parseInt(hex.slice(index, index + 2), 16)).join(", ")})`;
    expect(await read("other")).toEqual({
      color: rgb(theme.blue),
      background: "rgba(0, 0, 0, 0)",
    });
    expect(await read("sticky")).toEqual({
      color: rgb(theme.foreground),
      background: "rgba(0, 0, 0, 0)",
    });
    expect(await read("green")).toEqual({
      color: rgb(theme.green),
      background: "rgba(0, 0, 0, 0)",
    });
    expect((await read("red-on-green")).background).toBe("rgba(0, 0, 0, 0)");
    expect(await read("reverse")).toEqual({
      color: rgb(theme.foreground),
      background: "rgba(0, 0, 0, 0)",
    });
    expect((await read("indexed")).background).toBe("rgba(0, 0, 0, 0)");
    expect((await read("red-on-blue")).background).toBe(rgb(theme.blue));
    expect(await read("normal")).toEqual({
      color: rgb(theme.foreground),
      background: "rgba(0, 0, 0, 0)",
    });
    // The terminal buffer still retains the actual SGR colors for applications.
    expect(
      await page.evaluate(() => {
        const line = (window as any).term.buffer.active.getLine(0);
        return [
          line.getCell(0).getFgColor(),
          line.getCell(0).getBgColor(),
          line.getCell(6).getFgColor(),
          line.getCell(6).getBgColor(),
        ];
      }),
    ).toEqual([4, 2, 0, 2]);
    // A focused block cursor must remain visible when placed over green text.
    await page.evaluate(async () => {
      const term = (window as any).term;
      term.focus();
      await new Promise<void>((done) => term.write("\x1b[1;1H", done));
      await new Promise((done) =>
        requestAnimationFrame(() => requestAnimationFrame(done)),
      );
    });
    await expect(page.locator(".xterm-cursor-block")).toHaveCSS(
      "background-color",
      rgb(theme.cursor),
    );
    await page.evaluate(async () => {
      const term = (window as any).term;
      term.blur();
      await new Promise<void>((done) => term.write("\x1b[1;70H", done));
      await new Promise((done) =>
        requestAnimationFrame(() => requestAnimationFrame(done)),
      );
    });
    // Toggling is immediate, and restores the original attributes.
    await page
      .locator(".terminal-host")
      .evaluate((host) => host.classList.remove("hide-ansi-green-background"));
    expect((await read("other")).background).toBe(rgb(theme.green));
    expect(await read("sticky")).toEqual({
      color: rgb(theme.black),
      background: rgb(theme.green),
    });
    expect((await read("red-on-green")).background).toBe(rgb(theme.green));
  });
}
