import { expect, test } from "@playwright/test";
import type { MachineInfo } from "../../packages/protocol-ts";
import {
  deviceOrderKey,
  moveDevice,
  orderDevices,
  readDeviceOrder,
} from "../../apps/web/src/device-order";

const device = (id: string) => ({ id }) as MachineInfo;

test("moves devices in either direction and ignores invalid moves", () => {
  const original = ["a", "b", "c"];
  expect(moveDevice(original, "c", "a")).toEqual(["c", "a", "b"]);
  expect(moveDevice(original, "a", "c")).toEqual(["b", "c", "a"]);
  expect(moveDevice(original, "a", "a")).toBe(original);
  expect(moveDevice(original, "missing", "a")).toBe(original);
});

test("known devices follow saved order and new devices keep inventory order", () => {
  const result = orderDevices(
    [device("a"), device("b"), device("c"), device("d")],
    ["missing", "b", "a"],
  );
  expect(result.map(({ id }) => id)).toEqual(["b", "a", "c", "d"]);
});

test("invalid or inaccessible saved order falls back safely", () => {
  const previous = globalThis.localStorage;
  try {
    Object.defineProperty(globalThis, "localStorage", {
      configurable: true,
      value: {
        getItem: () => '["a", 1, "a", "b"]',
      },
    });
    expect(readDeviceOrder("tester")).toEqual(["a", "b"]);
    Object.defineProperty(globalThis, "localStorage", {
      configurable: true,
      value: {
        getItem: () => {
          throw new Error("denied");
        },
      },
    });
    expect(readDeviceOrder("tester")).toEqual([]);
    expect(deviceOrderKey("a/b")).not.toBe(deviceOrderKey("a"));
  } finally {
    Object.defineProperty(globalThis, "localStorage", {
      configurable: true,
      value: previous,
    });
  }
});
