import { describe, it, expect } from "vitest";
import {
  appLockNote,
  deviceHealthNotes,
  humaniseDuration,
  statusAgeLabel,
  HEALTH_STALE_SECS,
} from "./deviceHealth";

describe("humaniseDuration", () => {
  it("renders sub-hour durations in minutes", () => {
    expect(humaniseDuration(45 * 60)).toBe("45 min");
  });
  it("renders exact hours with no trailing minutes", () => {
    expect(humaniseDuration(2 * 3600)).toBe("2 h");
  });
  it("renders the example from the spec: 2h10m -> '2 h 10 min'", () => {
    expect(humaniseDuration(2 * 3600 + 10 * 60)).toBe("2 h 10 min");
  });
  it("never renders a bare 0 for a tiny positive duration", () => {
    expect(humaniseDuration(10)).toBe("1 min");
  });
});

describe("statusAgeLabel", () => {
  const NOW_MS = 2_000_000 * 1000; // an arbitrary "now", in ms

  it("reads a very recent reading as 'moments ago'", () => {
    expect(statusAgeLabel(2_000_000 - 10, NOW_MS)).toBe("moments ago");
  });
  it("reads minutes for anything under an hour", () => {
    expect(statusAgeLabel(2_000_000 - 12 * 60, NOW_MS)).toBe("12 min ago");
  });
  it("reads hours for anything under a day — the spec's own example", () => {
    expect(statusAgeLabel(2_000_000 - 3 * 3600, NOW_MS)).toBe("3 h ago");
  });
  it("falls back to a clock-time stamp for a reading a day or more old — the spec's own example", () => {
    const tsSecs = 2_000_000 - 25 * 3600; // > 24h old
    const label = statusAgeLabel(tsSecs, NOW_MS);
    expect(label).toMatch(/^as of \d{2}:\d{2}$/);
  });
});

describe("deviceHealthNotes (G-2: STATUS fields parsed but never shown)", () => {
  it("shows nothing at all for an older ward that omits every field", () => {
    expect(deviceHealthNotes(undefined)).toEqual({ notes: [], stale: false });
    expect(deviceHealthNotes({})).toEqual({ notes: [], stale: false });
    // parseStatus's own defaults for an absent wire field (false/0) must also
    // render nothing — "absent" and "reported clean" must look identical.
    expect(
      deviceHealthNotes({
        pausedByAdmin: false,
        enforcementGapSecs: 0,
        relayUnreachablePolls: 0,
        transportUnavailable: false,
        usageUnsaved: false,
      }),
    ).toEqual({ notes: [], stale: false });
  });

  it("notes an admin pause, with the reading's age", () => {
    const now = 1_000_000 * 1000;
    const { notes, stale } = deviceHealthNotes({ pausedByAdmin: true, ts: 1_000_000 - 60 }, now);
    expect(notes).toHaveLength(1);
    expect(notes[0].key).toBe("paused");
    expect(notes[0].text).toMatch(/administrator/i);
    expect(notes[0].text).toContain("moments ago");
    expect(stale).toBe(false);
  });

  it("notes a humanised enforcement gap", () => {
    const { notes } = deviceHealthNotes({ enforcementGapSecs: 2 * 3600 + 10 * 60 });
    expect(notes).toHaveLength(1);
    expect(notes[0].key).toBe("enforcementGap");
    expect(notes[0].text).toContain("2 h 10 min");
  });

  it("notes relay trouble with the poll count", () => {
    const { notes } = deviceHealthNotes({ relayUnreachablePolls: 5 });
    expect(notes).toHaveLength(1);
    expect(notes[0].key).toBe("relayTrouble");
    expect(notes[0].text).toContain("5");
  });

  it("notes an unavailable transport", () => {
    const { notes } = deviceHealthNotes({ transportUnavailable: true });
    expect(notes).toHaveLength(1);
    expect(notes[0].key).toBe("transportUnavailable");
  });

  it("notes the usage-save fail-safe pause", () => {
    const { notes } = deviceHealthNotes({ usageUnsaved: true });
    expect(notes).toHaveLength(1);
    expect(notes[0].key).toBe("usageUnsaved");
    expect(notes[0].text).toMatch(/screen time is paused/i);
  });

  it("shows no note when usageUnsaved is absent or false", () => {
    expect(deviceHealthNotes({ usageUnsaved: false }).notes).toHaveLength(0);
    expect(deviceHealthNotes({}).notes).toHaveLength(0);
  });

  it("notes a ward clock that went backwards (guardian-local, G-1)", () => {
    const { notes } = deviceHealthNotes({ clockWentBackwards: true });
    expect(notes).toHaveLength(1);
    expect(notes[0].key).toBe("clockBackwards");
    expect(notes[0].text).toMatch(/clock went backwards/i);
  });

  it("combines every present field, independently", () => {
    const { notes } = deviceHealthNotes({
      pausedByAdmin: true,
      enforcementGapSecs: 600,
      relayUnreachablePolls: 3,
      transportUnavailable: true,
      usageUnsaved: true,
      clockWentBackwards: true,
    });
    expect(notes.map((n) => n.key)).toEqual([
      "paused",
      "enforcementGap",
      "relayTrouble",
      "transportUnavailable",
      "usageUnsaved",
      "clockBackwards",
    ]);
  });

  // F5 (review 2026-09-27, second round): a stale reading must say so rather
  // than assert "right now" from data that may be hours or days old.
  describe("staleness (F5)", () => {
    it("is not stale just inside the freshness boundary", () => {
      const now = 1_000_000 * 1000;
      const { stale } = deviceHealthNotes(
        { pausedByAdmin: true, ts: 1_000_000 - (HEALTH_STALE_SECS - 1) },
        now,
      );
      expect(stale).toBe(false);
    });

    it("is stale once the reading is older than the threshold, and the note still shows its age", () => {
      const now = 1_000_000 * 1000;
      const { notes, stale } = deviceHealthNotes(
        { pausedByAdmin: true, ts: 1_000_000 - (HEALTH_STALE_SECS + 1) },
        now,
      );
      expect(stale).toBe(true);
      expect(notes[0].text).toMatch(/ago|as of/);
    });

    it("a fixture with no ts at all is never marked stale (nothing to judge it against)", () => {
      const { stale } = deviceHealthNotes({ pausedByAdmin: true });
      expect(stale).toBe(false);
    });
  });
});

// The Linux app-lock line ("App lock: on"/"App lock: off"), shown next to
// health/version on a ward's card. Absent means Android, or a Linux ward
// that predates the field — never shown as either state.
describe("appLockNote", () => {
  it("shows nothing at all when the ward never reported the field", () => {
    expect(appLockNote(undefined)).toBeUndefined();
  });

  it("reads armed as a plain, unworried line", () => {
    const note = appLockNote(true);
    expect(note).toEqual({ tone: "ok", text: "App lock: on" });
  });

  it("reads unarmed with warning styling and a short hint on how to arm it", () => {
    const note = appLockNote(false);
    expect(note?.tone).toBe("warn");
    expect(note?.text).toBe("App lock: off");
    expect(note?.hint).toMatch(/sudo charter-setup --arm-app-lock/);
  });
});
