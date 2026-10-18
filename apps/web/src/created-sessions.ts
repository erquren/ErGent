import type { MachineInfo, SessionInfo } from "../../../packages/protocol-ts";

const visible = (session: SessionInfo) =>
  session.lifecycle === "running" || session.lifecycle === "terminating";

// Metadata may change, but a stale result cannot reverse a known lifecycle in
// the same Agent epoch. A new epoch is an independent authoritative inventory.
function preserveLifecycle(
  current: SessionInfo | undefined,
  next: SessionInfo,
): SessionInfo {
  if (current?.agentEpoch !== next.agentEpoch) return next;
  next = {
    ...next,
    outputTruncated: current.outputTruncated || next.outputTruncated,
  };
  if (
    ["exited", "lost"].includes(current.lifecycle) ||
    (current.lifecycle === "terminating" && next.lifecycle === "running")
  )
    return {
      ...next,
      lifecycle: current.lifecycle,
      exitCode: current.exitCode,
    };
  return next;
}

// HTTP operation results can arrive before their WebSocket inventory. Keep a
// created session visible until inventory acknowledges it, and retain lifecycle
// observations across both channels without replacing newer inventory metadata.
export class CreatedSessions {
  private inventory = new Map<string, SessionInfo>();
  private pending = new Map<string, SessionInfo>();
  private availableMachines = new Set<string>();
  private revokedMachines = new Set<string>();

  revoke(machineId: string) {
    this.revokedMachines.add(machineId);
    this.availableMachines.delete(machineId);
    for (const items of [this.inventory, this.pending]) {
      for (const [id, session] of items) {
        if (session.machineId === machineId) items.delete(id);
      }
    }
  }

  receive(machines: MachineInfo[], sessions: SessionInfo[]): SessionInfo[] {
    for (const machine of machines) {
      if (machine.revoked) this.revoke(machine.id);
    }
    this.inventory = new Map(
      sessions
        .filter((session) => !this.revokedMachines.has(session.machineId))
        .map((session) => [
          session.id,
          preserveLifecycle(
            this.inventory.get(session.id) ?? this.pending.get(session.id),
            session,
          ),
        ]),
    );
    this.availableMachines = new Set(
      machines
        .filter(
          (machine) =>
            !machine.revoked && !this.revokedMachines.has(machine.id),
        )
        .map((machine) => machine.id),
    );
    for (const [id, session] of this.pending) {
      if (
        this.inventory.has(id) ||
        !this.availableMachines.has(session.machineId)
      )
        this.pending.delete(id);
    }
    return this.visible();
  }

  observe(session: SessionInfo): SessionInfo | undefined {
    if (!this.availableMachines.has(session.machineId)) return undefined;
    const current =
      this.inventory.get(session.id) ?? this.pending.get(session.id);
    let observed = session;
    if (current) {
      // HTTP results must not replace current inventory names/themes, or a newer
      // Agent epoch. Only advance lifecycle within the current epoch.
      const lifecycle = preserveLifecycle(current, session);
      observed =
        current.agentEpoch === session.agentEpoch
          ? {
              ...current,
              lifecycle: lifecycle.lifecycle,
              exitCode: lifecycle.exitCode,
              outputTruncated: lifecycle.outputTruncated,
            }
          : current;
    }
    if (this.inventory.has(session.id))
      this.inventory.set(session.id, observed);
    else this.pending.set(session.id, observed);
    return observed;
  }

  accept(session: SessionInfo): SessionInfo | undefined {
    const observed = this.observe(session);
    return observed && visible(observed) ? observed : undefined;
  }

  get(id: string | null): SessionInfo | undefined {
    if (!id) return undefined;
    const session = this.inventory.get(id) ?? this.pending.get(id);
    return session && this.availableMachines.has(session.machineId)
      ? session
      : undefined;
  }

  visible(): SessionInfo[] {
    return [...this.inventory.values(), ...this.pending.values()].filter(
      visible,
    );
  }
}
