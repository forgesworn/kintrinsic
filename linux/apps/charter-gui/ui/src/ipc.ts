// Mirrors the charter-ipc D-Bus contract. Kept in LOCKSTEP with
// crates/charter-ipc/src/contract.rs — a test pins these values.

export const CONTRACT = {
  busName: "org.forgesworn.charterd",
  objectPath: "/org/forgesworn/charterd",
  interface: "org.forgesworn.Charter1",
  methods: [
    "SubmitRequest",
    "QueryStatus",
    "ListRequests",
    "CancelRequest",
    "TimeLeft",
  ] as const,
  signals: ["RequestUpdated", "TimeLeftChanged", "LockStateChanged"] as const,
};

export type Op = "install.flatpak" | "exec.allow" | "time.extend";

// M20: field names are snake_case to match the charter-ipc `TimeLeftView` JSON
// on the wire EXACTLY (the Rust DTO is the source of truth). A previous
// camelCase shape silently failed to read `schedule_seconds`/`budget_seconds`,
// breaking the lock-reason and countdown surfaces. `-1` means unbounded.
export interface TimeLeftView {
  effective_seconds: number;
  schedule_seconds: number;
  budget_seconds: number;
  extension_seconds: number;
  locked: boolean;
  reason?: string;
  // Unix seconds until the next window opens, when schedule-locked.
  next_open?: number;
  // True if served from cache while offline.
  offline: boolean;
}

// ExecMeta deliberately has NO sha256 — the hash never leaves charterd.
export interface ExecMeta {
  name: string;
  size: number;
  origin?: string;
}

// The client surface. Only publish-once (`submitRequest`) + reads. There is
// **no** enact / approve / install method — every effect is brokered.
export interface CharterdClient {
  submitRequest(op: Op, paramsJson: string): Promise<string>;
  queryStatus(reqId: string): Promise<unknown[]>;
  timeLeft(): Promise<TimeLeftView>;
}
