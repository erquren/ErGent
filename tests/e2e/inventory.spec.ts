import { test, expect } from "@playwright/test";
import {
  InventoryReceiver,
  INVENTORY_TIMEOUT_MS,
} from "../../apps/web/src/inventory";
import type { Event, SessionInfo } from "../../packages/protocol-ts";

type Inventory = Extract<Event, { type: "inventory.sync" }>;
function inventory(count: number): Inventory {
  return {
    type: "inventory.sync",
    payload: {
      machines: [],
      sessions: Array.from(
        { length: count },
        (_, i) =>
          ({
            id: `session-${i}`,
            title: '中文🙂\\"'.repeat(30),
            lifecycle: "exited",
          }) as SessionInfo,
      ),
    },
  };
}
function chunks(value: unknown, id = "1"): Event[] {
  const bytes = Buffer.from(JSON.stringify(value));
  const result: Event[] = [];
  for (let offset = 0; offset < bytes.length; offset += 32 * 1024)
    result.push({
      type: "inventory.chunk",
      payload: {
        transferId: id,
        index: result.length,
        last: offset + 32 * 1024 >= bytes.length,
        dataBase64: bytes
          .subarray(offset, offset + 32 * 1024)
          .toString("base64"),
      },
    });
  return result;
}

test("inventory larger than 64 KiB publishes all 1501 sessions atomically", () => {
  const value = inventory(1501),
    parts = chunks(value);
  expect(parts.length).toBeGreaterThan(2);
  const receiver = new InventoryReceiver();
  parts.forEach((part, i) => {
    expect(Buffer.byteLength(JSON.stringify(part))).toBeLessThanOrEqual(
      64 * 1024,
    );
    const complete = receiver.accept(part);
    expect(complete).toEqual(i === parts.length - 1 ? value : null);
  });
  expect(receiver.inProgress).toBe(false);
  expect(receiver.accept(chunks(inventory(0), "2")[0])).toEqual(inventory(0));
});

test("partial transfers, replays, missing chunks and reconnect never publish partial lists", () => {
  const parts = chunks(inventory(1501));
  const receiver = new InventoryReceiver();
  expect(receiver.accept(parts[0])).toBeNull();
  expect(() => receiver.accept(parts[0])).toThrow();
  expect(() => receiver.accept(parts[2])).toThrow();
  expect(() => receiver.accept(chunks(inventory(1), "2")[0])).toThrow();
  const reconnected = new InventoryReceiver();
  expect(() => reconnected.accept(parts[1])).toThrow();
  let result = null;
  for (const part of parts) result = reconnected.accept(part);
  expect(result).toEqual(inventory(1501));
  expect(() => reconnected.accept(parts[0])).toThrow();
});

test("invalid boundaries, duplicate IDs, unchunked inventories and expired transfers fail closed", () => {
  const parts = chunks(inventory(1501));
  const receiver = new InventoryReceiver();
  receiver.accept(parts[0], 1);
  expect(() => receiver.accept(parts[1], INVENTORY_TIMEOUT_MS + 1)).toThrow();
  expect(() => new InventoryReceiver().accept(inventory(0))).toThrow();
  const value = inventory(2);
  value.payload.sessions[1].id = value.payload.sessions[0].id;
  expect(() => new InventoryReceiver().accept(chunks(value)[0])).toThrow();
  const short = chunks(inventory(0))[0];
  if (short.type === "inventory.chunk") short.payload.last = false;
  expect(() => new InventoryReceiver().accept(short)).toThrow();
  const oversized = chunks(inventory(0))[0];
  if (oversized.type === "inventory.chunk")
    oversized.payload.dataBase64 = "A".repeat(64 * 1024);
  expect(() => new InventoryReceiver().accept(oversized)).toThrow();
});
