import { expect, test } from "@playwright/test";
import type { Terminal } from "@xterm/xterm";
import { captureTerminalText } from "../../apps/web/src/terminal-text";

function terminal(
  lines: { text: string; wrapped?: boolean }[],
  viewportY = 0,
  rows = 3,
  alternate = false,
) {
  return {
    rows,
    getSelection: () => "selected text",
    buffer: {
      active: {
        type: alternate ? "alternate" : "normal",
        viewportY,
        length: lines.length,
        getLine: (index: number) =>
          lines[index] && {
            isWrapped: !!lines[index].wrapped,
            translateToString: (trim: boolean) =>
              trim ? lines[index].text.trimEnd() : lines[index].text,
          },
      },
    },
  } as unknown as Terminal;
}

test("copy rejoins soft wraps without losing code indentation or separating spaces", () => {
  const result = captureTerminalText(
    terminal([
      { text: "    return hello " },
      { text: "world;  ", wrapped: true },
      { text: "" },
      { text: "next line" },
      { text: "" },
    ]),
  );
  expect(result.history).toBe("    return hello world;\n\nnext line");
  expect(result.selection).toBe("selected text");
});

test("current view respects scroll position and excludes offscreen history", () => {
  const result = captureTerminalText(
    terminal(
      [
        { text: "old" },
        { text: "visible" },
        { text: "  continuation", wrapped: true },
        { text: "new" },
      ],
      1,
      2,
    ),
  );
  expect(result.screen).toBe("visible  continuation");
  expect(result.history).toBe("old\nvisible  continuation\nnew");
});

test("full-screen copy uses the active alternate buffer and freezes its contents", () => {
  const lines = [{ text: "Codex response" }, { text: "    code" }];
  const result = captureTerminalText(terminal(lines, 0, 2, true));
  lines[0].text = "later redraw";
  expect(result.alternate).toBe(true);
  expect(result.screen).toBe("Codex response\n    code");
});
