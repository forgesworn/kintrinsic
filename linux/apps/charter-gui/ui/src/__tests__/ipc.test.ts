import { describe, it, expect } from "vitest";
import { readFileSync } from "node:fs";
import { fileURLToPath } from "node:url";
import { dirname, resolve } from "node:path";
import { CONTRACT, type CharterdClient } from "../ipc";

// M21: a REAL cross-language lockstep — read the Rust contract source of truth
// (charter-ipc/src/contract.rs) and assert the TS mirror matches, so a Rust-side
// rename/add/remove of a method or signal fails this test instead of silently
// drifting. (Previously this only re-pinned the TS literals against themselves.)
const here = dirname(fileURLToPath(import.meta.url)); // .../ui/src/__tests__
const CONTRACT_RS = resolve(
  here,
  "../../../../../crates/charter-ipc/src/contract.rs",
);

function rustConst(src: string, name: string): string | undefined {
  return src.match(new RegExp(`pub const ${name}:\\s*&str\\s*=\\s*"([^"]+)"`))?.[1];
}
function rustConstValues(src: string, prefix: string): string[] {
  return [...src.matchAll(new RegExp(`pub const ${prefix}\\w+:\\s*&str\\s*=\\s*"([^"]+)"`, "g"))]
    .map((m) => m[1])
    .sort();
}

describe("ipc contract", () => {
  it("stays in lockstep with the Rust contract.rs source of truth", () => {
    const src = readFileSync(CONTRACT_RS, "utf8");
    expect(rustConst(src, "BUS_NAME")).toBe(CONTRACT.busName);
    expect(rustConst(src, "OBJECT_PATH")).toBe(CONTRACT.objectPath);
    expect(rustConst(src, "INTERFACE")).toBe(CONTRACT.interface);
    // The TS method/signal sets must EQUAL the Rust METHOD_*/SIGNAL_* consts.
    expect(rustConstValues(src, "METHOD_")).toEqual([...CONTRACT.methods].sort());
    expect(rustConstValues(src, "SIGNAL_")).toEqual([...CONTRACT.signals].sort());
  });

  it("exposes no enact/approve/install method (no-local-authority)", () => {
    for (const m of CONTRACT.methods) {
      const lower = m.toLowerCase();
      expect(lower).not.toContain("enact");
      expect(lower).not.toContain("approve");
      expect(lower).not.toContain("grant");
      expect(lower).not.toContain("install");
    }
  });

  it("a client implementation surface only submits + reads", () => {
    const client: CharterdClient = {
      submitRequest: async () => "req",
      queryStatus: async () => [],
      timeLeft: async () => ({
        effective_seconds: -1,
        schedule_seconds: -1,
        budget_seconds: -1,
        extension_seconds: 0,
        locked: false,
        offline: false,
      }),
    };
    const keys = Object.keys(client);
    expect(keys.sort()).toEqual(["queryStatus", "submitRequest", "timeLeft"]);
    expect(keys).not.toContain("enact");
    expect(keys).not.toContain("approve");
  });
});
