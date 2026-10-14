export function normalizeName(value: string): string {
  // Rust str::trim uses the Unicode White_Space property. JS trim differs for
  // a few code points; use the same property, and reject controls before trim.
  const trimmed = value.replace(/^\p{White_Space}+|\p{White_Space}+$/gu, "");
  if (
    !trimmed ||
    Array.from(trimmed).length > 120 ||
    /[\p{Cc}\uD800-\uDFFF]/u.test(value)
  )
    throw new Error("名称须为 1–120 个字符，且不能包含控制字符");
  return trimmed;
}
export function nameValidationMessage(value: string): string {
  try {
    normalizeName(value);
    return "";
  } catch (error) {
    return (error as Error).message;
  }
}
