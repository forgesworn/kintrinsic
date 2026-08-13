// Frozen cross-stack USAGE_SYNC payload vectors — the SAME file the Rust side
// asserts (charter-proto/tests/usage_sync_vectors.rs), so payload drift
// breaks loudly on both stacks. The TS side is the PRODUCER: the builder must
// emit deep-equal JSON for the pinned inputs.
import { readFileSync } from "node:fs";
import { resolve } from "node:path";
import { describe, expect, it } from "vitest";
import { emptyMinutes, encodeMinutes, setMinute } from "../insights/minuteSet";
import { buildUsageSyncForDevice, type UsageSyncPayload } from "./usageSync";

const VECTORS_PATH = resolve(
  process.cwd(),
  "../../core/crates/charter-testkit/vectors/usage_sync/usage_sync_vectors.json",
);
interface Doc {
  vectors: { name: string; payload: Record<string, unknown> }[];
}
const doc = JSON.parse(readFileSync(VECTORS_PATH, "utf8")) as Doc;
const SUBJECT = "cd".repeat(32);
const RECEIVER = "aa".repeat(64 / 2);

function pinned(name: string): Record<string, unknown> {
  const v = doc.vectors.find((v) => v.name === name);
  if (!v) throw new Error(`missing vector ${name}`);
  return v.payload;
}

describe("usageSync frozen vectors (TS producer)", () => {
  it("scalar-only: builder emits the pinned payload", () => {
    const built = buildUsageSyncForDevice({
      subject: SUBJECT,
      receiverMachine: RECEIVER,
      dayKey: "2026-06-29",
      ts: 1782734400,
      // One other device, 1800s, no journal -> scalar-only payload.
      devices: [
        { machine: RECEIVER, secs: 600 },
        { machine: "bb".repeat(32), secs: 1800 },
      ],
    });
    expect(JSON.parse(JSON.stringify(built))).toEqual(pinned("scalar-only"));
  });

  it("with-union-bitmap: builder emits the pinned payload", () => {
    // The pinned bitmap is minutes {718,719,720}; 180s spent elsewhere.
    const bits = emptyMinutes();
    for (const m of [718, 719, 720]) setMinute(bits, m);
    const built = buildUsageSyncForDevice({
      subject: SUBJECT,
      receiverMachine: RECEIVER,
      dayKey: "2026-06-29",
      ts: 1782734520,
      devices: [
        { machine: RECEIVER, secs: 0 },
        { machine: "bb".repeat(32), secs: 180, minutesB64: encodeMinutes(bits) },
      ],
    });
    expect(JSON.parse(JSON.stringify(built))).toEqual(pinned("with-union-bitmap"));
  });

  it("with-week-scalars: the TS type serializes the pinned shape", () => {
    // Weekly publishing isn't wired yet; pin the SHAPE so the field names
    // can't drift before it is.
    const p: UsageSyncPayload = {
      v: 1,
      subject: SUBJECT,
      ts: 1782734460,
      dayKey: "2026-06-29",
      spentElsewhereTodaySecs: 5400,
      weekKey: "2026-06-29",
      spentElsewhereWeekSecs: 21600,
    };
    expect(JSON.parse(JSON.stringify(p))).toEqual(pinned("with-week-scalars"));
  });

  it("a journal-less contributing device suppresses the union bitmap", () => {
    const bits = emptyMinutes();
    setMinute(bits, 10);
    const built = buildUsageSyncForDevice({
      subject: SUBJECT,
      receiverMachine: RECEIVER,
      dayKey: "2026-06-29",
      ts: 1,
      devices: [
        { machine: RECEIVER, secs: 0 },
        { machine: "bb".repeat(32), secs: 60, minutesB64: encodeMinutes(bits) },
        { machine: "cc".repeat(32), secs: 120 }, // no journal
      ],
    });
    expect(built.spentElsewhereTodaySecs).toBe(180);
    expect(built.elsewhereMinutesToday).toBeUndefined();
  });
});
