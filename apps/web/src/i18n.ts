import { useSyncExternalStore } from "react";
import { english, chinese } from "./messages";
export type Language = "zh" | "en";
export type LanguagePreference = Language | "auto";
const storageKey = "ergent.language";
function readPreference(): LanguagePreference {
  try {
    const value = localStorage.getItem(storageKey);
    if (value === "zh" || value === "en") return value;
  } catch {
    /* Private/restricted storage. */
  }
  return "auto";
}
// Use the device's primary language, not a secondary language added for input.
function deviceLanguage(): Language {
  return (navigator.languages?.[0] || navigator.language || "en")
    .toLowerCase()
    .startsWith("zh")
    ? "zh"
    : "en";
}
let preference = readPreference();
let language: Language = preference === "auto" ? deviceLanguage() : preference;
const listeners = new Set<() => void>();
function sync() {
  language = preference === "auto" ? deviceLanguage() : preference;
  document.documentElement.lang = language === "zh" ? "zh-CN" : "en";
  document.title =
    language === "zh"
      ? "ErGent · 你的设备，同一个工作台"
      : "ErGent · Your machines, one workspace";
  listeners.forEach((listener) => listener());
}
window.addEventListener("languagechange", sync);
window.addEventListener("storage", (event) => {
  if (event.key === storageKey || event.key === null) {
    preference = readPreference();
    sync();
  }
});
sync();
const reverse = new Map(
  Object.entries(english).map(([key, value]) => [value, key]),
);
export function t(
  key: string,
  values: Record<string, string | number> = {},
): string {
  // Canonical strings can be retained in async state without stale-language closures.
  const canonical = reverse.get(key) ?? key;
  const translated =
    language === "en"
      ? (english[values.count === 1 ? `${canonical}_one` : canonical] ??
        english[canonical] ??
        canonical)
      : (chinese[canonical] ?? canonical);
  return translated.replace(/\{(\w+)\}/g, (match, name: string) =>
    String(values[name] ?? match),
  );
}
export function useLanguage() {
  useSyncExternalStore(
    (listener) => {
      listeners.add(listener);
      return () => {
        listeners.delete(listener);
      };
    },
    () => `${preference}:${language}`,
  );
  return {
    language,
    preference,
    setPreference(value: LanguagePreference) {
      preference = value;
      try {
        localStorage.setItem(storageKey, value);
      } catch {
        /* Session-only preference. */
      }
      sync();
    },
  };
}
