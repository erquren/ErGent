import { expect, test } from "@playwright/test";
import { CreatedSessions } from "../../apps/web/src/created-sessions";
import type { MachineInfo, SessionInfo } from "../../packages/protocol-ts";

const machine: MachineInfo = {
  id: "machine",
  name: "Test",
  os: "linux",
  arch: "x86_64",
  online: true,
  revoked: false,
  defaultCwd: "/tmp",
};
const session: SessionInfo = {
  id: "created",
  machineId: machine.id,
  agentEpoch: "epoch",
  title: "Created",
  terminalTheme: null,
  cwd: "/tmp",
  shell: "/bin/sh",
  lifecycle: "running",
  cols: 80,
  rows: 24,
  exitCode: null,
  toolStatus: null,
};

test("create result survives inventory lag and is acknowledged once", () => {
  const tracker = new CreatedSessions();
  tracker.receive([machine], []);
  expect(tracker.accept(session)).toEqual(session);
  expect(tracker.visible()).toEqual([session]);
  expect(tracker.receive([machine], [])).toEqual([session]);
  expect(tracker.receive([machine], [session])).toEqual([session]);
  expect(
    tracker.receive(
      [machine],
      [{ ...session, lifecycle: "exited", exitCode: 0 }],
    ),
  ).toEqual([]);
});

test("late operation results neither overwrite inventory metadata nor revive exited sessions", () => {
  const tracker = new CreatedSessions();
  const renamed = {
    ...session,
    title: "Newer server name",
    terminalTheme: "nord",
  };
  tracker.receive([machine], [renamed]);
  expect(tracker.accept(session)).toEqual(renamed);
  expect(tracker.visible()).toEqual([renamed]);
  tracker.receive(
    [machine],
    [{ ...renamed, lifecycle: "exited", exitCode: 0 }],
  );
  expect(tracker.accept(session)).toBeUndefined();
  expect(tracker.visible()).toEqual([]);
});

test("revoking a machine clears an unacknowledged create result", () => {
  const tracker = new CreatedSessions();
  tracker.receive([machine], []);
  tracker.accept(session);
  expect(tracker.receive([{ ...machine, revoked: true }], [])).toEqual([]);
  expect(tracker.accept(session)).toBeUndefined();
});
