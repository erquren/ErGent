import type { IBuffer, Terminal } from "@xterm/xterm";

function bufferText(buffer: IBuffer, start: number, end: number): string {
  let text = "";
  for (let row = start; row < Math.min(end, buffer.length); row++) {
    const line = buffer.getLine(row);
    if (!line) continue;
    if (row > start && !line.isWrapped) text += "\n";
    // Keep spaces at soft wraps, including indentation in code blocks.
    const continues = row + 1 < end && buffer.getLine(row + 1)?.isWrapped;
    text += line.translateToString(!continues);
  }
  return text.replace(/\n+$/, "");
}

export function captureTerminalText(term: Terminal) {
  const buffer = term.buffer.active;
  return {
    screen: bufferText(buffer, buffer.viewportY, buffer.viewportY + term.rows),
    history: bufferText(buffer, 0, buffer.length),
    selection: term.getSelection(),
    alternate: buffer.type === "alternate",
  };
}

export type TerminalTextSnapshot = ReturnType<typeof captureTerminalText>;
