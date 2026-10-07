import { useEffect, useState } from "react";
import type { ITheme } from "@xterm/xterm";

export type Appearance = "system" | "light" | "dark";
const key = "ergent.appearance";
function saved(): Appearance {
  try {
    const value = localStorage.getItem(key);
    if (value === "light" || value === "dark") return value;
  } catch {
    /* Storage may be disabled. */
  }
  return "system";
}
export function useAppearance() {
  const [appearance, setAppearance] = useState<Appearance>(saved);
  const [systemDark, setSystemDark] = useState(
    () => matchMedia("(prefers-color-scheme: dark)").matches,
  );
  const resolved =
    appearance === "system" ? (systemDark ? "dark" : "light") : appearance;
  useEffect(() => {
    const media = matchMedia("(prefers-color-scheme: dark)");
    const changed = () => setSystemDark(media.matches);
    const stored = (e: StorageEvent) => {
      if (e.key === key || e.key === null) setAppearance(saved());
    };
    media.addEventListener("change", changed);
    window.addEventListener("storage", stored);
    changed();
    return () => {
      media.removeEventListener("change", changed);
      window.removeEventListener("storage", stored);
    };
  }, []);
  useEffect(() => {
    document.documentElement.dataset.theme = resolved;
    document.documentElement.style.colorScheme = resolved;
  }, [resolved]);
  return {
    appearance,
    resolved,
    setAppearance: (value: Appearance) => {
      setAppearance(value);
      try {
        localStorage.setItem(key, value);
      } catch {
        /* Keep in-memory preference. */
      }
    },
  };
}

export const terminalThemes = [
  ["auto", "跟随网页"],
  ["dark", "经典黑"],
  ["light", "经典白"],
  ["dracula", "Dracula"],
  ["nord", "Nord"],
  ["solarized", "Solarized 浅色"],
] as const;
const dark: ITheme = {
  background: "#000000",
  foreground: "#ffffff",
  cursor: "#ffffff",
  cursorAccent: "#000000",
  selectionBackground: "#ffffff40",
  black: "#555555",
  red: "#ff7777",
  green: "#8cdb79",
  yellow: "#f4d777",
  blue: "#78b5ff",
  magenta: "#d4a2ff",
  cyan: "#78dce8",
  white: "#dddddd",
  brightBlack: "#999999",
  brightRed: "#ffa0a0",
  brightGreen: "#b0ed97",
  brightYellow: "#ffe8a3",
  brightBlue: "#a5cfff",
  brightMagenta: "#e7c2ff",
  brightCyan: "#a8ecf2",
  brightWhite: "#ffffff",
};
const light: ITheme = {
  background: "#ffffff",
  foreground: "#000000",
  cursor: "#000000",
  cursorAccent: "#ffffff",
  selectionBackground: "#245fc433",
  black: "#111111",
  red: "#b42332",
  green: "#24732a",
  yellow: "#876000",
  blue: "#185abd",
  magenta: "#8e3299",
  cyan: "#087681",
  white: "#626970",
  brightBlack: "#666666",
  brightRed: "#c72a3b",
  brightGreen: "#177332",
  brightYellow: "#946200",
  brightBlue: "#2466c2",
  brightMagenta: "#a3399e",
  brightCyan: "#087c89",
  brightWhite: "#333333",
};
const palettes: Record<string, ITheme> = {
  dark,
  light,
  dracula: {
    ...dark,
    background: "#282a36",
    foreground: "#f8f8f2",
    cursor: "#f8f8f2",
    cursorAccent: "#282a36",
    selectionBackground: "#44475a",
    black: "#44475a",
    red: "#ff5555",
    green: "#50fa7b",
    yellow: "#f1fa8c",
    blue: "#8be9fd",
    magenta: "#bd93f9",
    cyan: "#8be9fd",
    white: "#f8f8f2",
  },
  nord: {
    ...dark,
    background: "#2e3440",
    foreground: "#eceff4",
    cursor: "#eceff4",
    cursorAccent: "#2e3440",
    selectionBackground: "#4c566a",
    black: "#4c566a",
    red: "#bf616a",
    green: "#a3be8c",
    yellow: "#ebcb8b",
    blue: "#81a1c1",
    magenta: "#b48ead",
    cyan: "#88c0d0",
    white: "#e5e9f0",
  },
  solarized: {
    ...light,
    background: "#fdf6e3",
    foreground: "#586e75",
    cursor: "#586e75",
    cursorAccent: "#fdf6e3",
    selectionBackground: "#eee8d5",
    black: "#073642",
    red: "#c23b32",
    green: "#687600",
    yellow: "#946b00",
    blue: "#2176ae",
    magenta: "#b02b73",
    cyan: "#187f79",
    white: "#657b83",
  },
};
export function terminalTheme(
  choice: string | null | undefined,
  appearance: "dark" | "light",
): ITheme {
  return palettes[choice ?? "auto"] ?? palettes[appearance];
}
