import type { MachineInfo, SessionInfo } from "../../../packages/protocol-ts";

// HTTP operation results can arrive before their WebSocket inventory. Keep a
// created session visible until the inventory acknowledges it, without letting
// an older create result overwrite newer inventory metadata or revive an exit.
export class CreatedSessions {
  private inventory = new Map<string, SessionInfo>();
  private pending = new Map<string, SessionInfo>();
  private availableMachines = new Set<string>();

  receive(machines: MachineInfo[], sessions: SessionInfo[]): SessionInfo[] {
    this.inventory = new Map(sessions.map((session) => [session.id, session]));
    this.availableMachines = new Set(
      machines
        .filter((machine) => !machine.revoked)
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

  accept(session: SessionInfo): SessionInfo | undefined {
    if (!this.availableMachines.has(session.machineId)) return undefined;
    const current = this.inventory.get(session.id) ?? session;
    if (current.lifecycle !== "running") return undefined;
    if (!this.inventory.has(session.id)) this.pending.set(session.id, current);
    return current;
  }

  visible(): SessionInfo[] {
    return [...this.inventory.values(), ...this.pending.values()].filter(
      (session) => session.lifecycle === "running",
    );
  }
}
