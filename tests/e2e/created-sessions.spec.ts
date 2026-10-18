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
  outputTruncated: false,
  toolStatus: null,
};

test("confirmed removal rejects delayed inventories and create results without affecting other devices", () => {
  const tracker = new CreatedSessions();
  const otherMachine = { ...machine, id: "other" };
  const otherSession = {
    ...session,
    id: "other-session",
    machineId: otherMachine.id,
  };
  tracker.receive([machine, otherMachine], [session, otherSession]);
  tracker.revoke(machine.id);
  expect(tracker.visible()).toEqual([otherSession]);
  expect(tracker.get(session.id)).toBeUndefined();
  expect(tracker.accept(session)).toBeUndefined();
  expect(
    tracker.receive([machine, otherMachine], [session, otherSession]),
  ).toEqual([otherSession]);
});

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

test("HTTP close observation stays coherent across pending create, stale inventory and late results", () => {
  const tracker = new CreatedSessions();
  tracker.receive([machine], []);
  tracker.accept(session);
  const terminating = { ...session, lifecycle: "terminating" };
  expect(tracker.observe(terminating)).toEqual(terminating);
  expect(tracker.receive([machine], [])).toEqual([terminating]);
  expect(tracker.receive([machine], [session])).toEqual([terminating]);
  expect(tracker.accept(session)).toEqual(terminating);
  const exited = { ...session, lifecycle: "exited", exitCode: 7 };
  tracker.observe(exited);
  expect(tracker.visible()).toEqual([]);
  expect(tracker.receive([machine], [session])).toEqual([]);
  expect(tracker.accept(session)).toBeUndefined();
});

test("lifecycle observations preserve newer inventory metadata and do not cross Agent epochs", () => {
  const tracker = new CreatedSessions();
  const renamed = { ...session, title: "Newer name", terminalTheme: "nord" };
  tracker.receive([machine], [renamed]);
  tracker.observe({ ...session, lifecycle: "terminating" });
  expect(tracker.visible()).toEqual([{ ...renamed, lifecycle: "terminating" }]);
  const newerEpoch = { ...renamed, agentEpoch: "new-epoch" };
  expect(tracker.receive([machine], [newerEpoch])).toEqual([newerEpoch]);
  tracker.observe({ ...session, lifecycle: "exited", exitCode: 0 });
  expect(tracker.visible()).toEqual([newerEpoch]);
});

test("new inventory metadata cannot reverse a same-epoch terminating or exited lifecycle", () => {
  const tracker = new CreatedSessions();
  tracker.receive([machine], [{ ...session, lifecycle: "terminating" }]);
  expect(
    tracker.receive([machine], [{ ...session, title: "Renamed" }]),
  ).toEqual([{ ...session, title: "Renamed", lifecycle: "terminating" }]);
  tracker.receive(
    [machine],
    [{ ...session, lifecycle: "exited", exitCode: 0 }],
  );
  expect(
    tracker.receive([machine], [{ ...session, lifecycle: "terminating" }]),
  ).toEqual([]);
});

test("truncation survives stale same-epoch HTTP results and inventories but not a new epoch", () => {
  const tracker = new CreatedSessions();
  tracker.receive([machine], [session]);
  const exited = { ...session, lifecycle: "exited", exitCode: 0 };
  expect(
    tracker.observe({ ...exited, outputTruncated: true })?.outputTruncated,
  ).toBe(true);
  expect(tracker.observe(exited)?.outputTruncated).toBe(true);
  tracker.receive([machine], [exited]);
  expect(tracker.observe(session)?.outputTruncated).toBe(true);
  const nextEpoch = { ...session, agentEpoch: "new-epoch" };
  expect(tracker.receive([machine], [nextEpoch])).toEqual([nextEpoch]);
  expect(tracker.observe(exited)).toEqual(nextEpoch);
});

test("selected final metadata stays monotonic but disappears with its owning machine", () => {
  const tracker = new CreatedSessions();
  tracker.receive([machine], [session]);
  const final = {
    ...session,
    lifecycle: "exited",
    exitCode: 0,
    outputTruncated: true,
  };
  tracker.observe(final);
  tracker.receive([machine], [session]);
  expect(tracker.get(session.id)).toMatchObject(final);
  expect(tracker.visible()).toEqual([]);
  tracker.receive([{ ...machine, revoked: true }], [final]);
  expect(tracker.get(session.id)).toBeUndefined();
  tracker.receive([], []);
  expect(tracker.get(session.id)).toBeUndefined();
});
