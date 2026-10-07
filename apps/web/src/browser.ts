// getRandomValues is available on LAN HTTP as well as secure contexts.
export function randomId(): string {
  const bytes = crypto.getRandomValues(new Uint8Array(16));
  bytes[6] = (bytes[6] & 0x0f) | 0x40;
  bytes[8] = (bytes[8] & 0x3f) | 0x80;
  const hex = Array.from(bytes, (byte) =>
    byte.toString(16).padStart(2, "0"),
  ).join("");
  return `${hex.slice(0, 8)}-${hex.slice(8, 12)}-${hex.slice(12, 16)}-${hex.slice(16, 20)}-${hex.slice(20)}`;
}

export async function copyText(value: string): Promise<void> {
  if (navigator.clipboard?.writeText)
    return navigator.clipboard.writeText(value);
  const input = document.createElement("textarea");
  input.value = value;
  input.style.position = "fixed";
  input.style.opacity = "0";
  document.body.appendChild(input);
  const previous = document.activeElement;
  try {
    input.select();
    if (!document.execCommand("copy")) throw new Error("复制失败，请手动复制");
  } finally {
    input.remove();
    if (previous instanceof HTMLElement) previous.focus();
  }
}
