import type { Event, InventoryChunk } from "../../../packages/protocol-ts";

type Inventory = Extract<Event, { type: "inventory.sync" }>;
const CHUNK_BYTES = 32 * 1024;
export const INVENTORY_TIMEOUT_MS = 60_000;

/** One receiver per WebSocket. Until completion the previous UI inventory is untouched. */
export class InventoryReceiver {
  private nextTransfer = 1n;
  private nextIndex = 0;
  private parts: Uint8Array[] = [];
  private started = 0;

  get inProgress() {
    return this.nextIndex !== 0;
  }

  accept(event: Event, now = Date.now()): Inventory | null {
    if (event.type !== "inventory.chunk") {
      if (this.inProgress || event.type === "inventory.sync")
        throw new Error("Expected a bounded inventory transfer");
      return null;
    }
    const chunk: InventoryChunk = event.payload;
    if (
      chunk.transferId !== this.nextTransfer.toString() ||
      chunk.index !== this.nextIndex ||
      typeof chunk.last !== "boolean" ||
      typeof chunk.dataBase64 !== "string" ||
      chunk.dataBase64.length > Math.ceil(CHUNK_BYTES / 3) * 4
    )
      throw new Error("Invalid or replayed inventory chunk");
    if (!this.inProgress) this.started = now;
    if (now - this.started >= INVENTORY_TIMEOUT_MS)
      throw new Error("Inventory transfer timed out");
    const binary = atob(chunk.dataBase64);
    if (
      !binary.length ||
      binary.length > CHUNK_BYTES ||
      (!chunk.last && binary.length !== CHUNK_BYTES)
    )
      throw new Error("Invalid inventory chunk boundary");
    this.parts.push(Uint8Array.from(binary, (char) => char.charCodeAt(0)));
    if (!chunk.last) {
      this.nextIndex++;
      return null;
    }
    const bytes = new Uint8Array(
      this.parts.reduce((total, part) => total + part.length, 0),
    );
    let offset = 0;
    for (const part of this.parts) {
      bytes.set(part, offset);
      offset += part.length;
    }
    const value: Inventory = JSON.parse(
      new TextDecoder("utf-8", { fatal: true }).decode(bytes),
    );
    if (
      value.type !== "inventory.sync" ||
      !value.payload ||
      !Array.isArray(value.payload.machines) ||
      !Array.isArray(value.payload.sessions)
    )
      throw new Error("Invalid inventory payload");
    for (const items of [value.payload.machines, value.payload.sessions]) {
      const ids = new Set<string>();
      for (const item of items) {
        if (!item || typeof item.id !== "string" || ids.has(item.id))
          throw new Error("Invalid or duplicated inventory ID");
        ids.add(item.id);
      }
    }
    this.parts = [];
    this.nextIndex = 0;
    this.nextTransfer++;
    return value;
  }
}
