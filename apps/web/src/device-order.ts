import type { MachineInfo } from "../../../packages/protocol-ts";

export function deviceOrderKey(user: string): string {
  return `ergent.device-order.${encodeURIComponent(user)}`;
}

export function readDeviceOrder(user: string): string[] {
  try {
    const value: unknown = JSON.parse(
      localStorage.getItem(deviceOrderKey(user)) ?? "[]",
    );
    return Array.isArray(value)
      ? [...new Set(value.filter((id): id is string => typeof id === "string"))]
      : [];
  } catch {
    return [];
  }
}

export function orderDevices(
  machines: MachineInfo[],
  order: string[],
): MachineInfo[] {
  const rank = new Map(order.map((id, index) => [id, index]));
  return [...machines].sort(
    (a, b) => (rank.get(a.id) ?? Infinity) - (rank.get(b.id) ?? Infinity),
  );
}

export function moveDevice(
  order: string[],
  from: string,
  to: string,
): string[] {
  const source = order.indexOf(from);
  const target = order.indexOf(to);
  if (source < 0 || target < 0 || source === target) return order;
  const next = [...order];
  next.splice(source, 1);
  next.splice(target, 0, from);
  return next;
}
