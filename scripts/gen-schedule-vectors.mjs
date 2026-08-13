// Generate schedule parity vectors from the SDK's canonical evaluateSchedule.
// node CANNOT import the .ts source, so we import from dist/ — run `npm run
// build` first.
//
// Usage (from repo root):
//   npm run build && node scripts/gen-schedule-vectors.mjs
//
// Output: core/crates/charter-testkit/vectors/schedule/schedule_vectors.json
// Each vector pins the live TS result so the Rust port must match exactly.

import { evaluateSchedule } from "../dist/schedule/index.js";
import { writeFileSync, mkdirSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

const __dirname = dirname(fileURLToPath(import.meta.url));
const OUT_DIR = join(__dirname, "..", "core", "crates", "charter-testkit", "vectors", "schedule");
mkdirSync(OUT_DIR, { recursive: true });

const clause = (windows, opts = {}) => ({
  schemaVersion: 1,
  kind: "schedule",
  mechanism: "static-data",
  windows,
  timezone: opts.timezone ?? "Europe/London",
  endDate: opts.endDate ?? null,
  revoked: opts.revoked ?? false,
});

// 0=Sun..6=Sat; minutes are minute-of-day in the clause tz.
const WEEKDAYS = [1, 2, 3, 4, 5];
const afterSchool = clause([{ daysOfWeek: WEEKDAYS, startMinute: 960, endMinute: 1200 }]); // 16:00-20:00
const fridayNight = clause([{ daysOfWeek: [5], startMinute: 1320, endMinute: 360 }]); // Fri 22:00 -> Sat 06:00 (crosses midnight)
const twoWindows = clause([
  { daysOfWeek: [6, 0], startMinute: 540, endMinute: 720 }, // weekend 09:00-12:00
  { daysOfWeek: [6, 0], startMinute: 840, endMinute: 1080 }, // weekend 14:00-18:00
]);
const empty = clause([]); // always allow
const revoked = clause([{ daysOfWeek: WEEKDAYS, startMinute: 960, endMinute: 1200 }], { revoked: true });
const expired = clause([{ daysOfWeek: WEEKDAYS, startMinute: 960, endMinute: 1200 }], { endDate: 1000 });
const badTz = clause([{ daysOfWeek: WEEKDAYS, startMinute: 960, endMinute: 1200 }], { timezone: "Not/AZone" });

const cases = [
  // After-school weekday window, BST (June) and GMT (January).
  ["weekday_in_window_bst", afterSchool, "2026-06-29T17:00:00Z"], // Mon 18:00 BST -> allow
  ["weekday_before_window_bst", afterSchool, "2026-06-29T14:00:00Z"], // Mon 15:00 BST -> lock
  ["weekday_in_window_gmt", afterSchool, "2026-01-26T17:00:00Z"], // Mon 17:00 GMT -> allow
  ["weekday_after_window_gmt", afterSchool, "2026-01-26T20:30:00Z"], // Mon 20:30 GMT -> lock
  ["weekend_not_in_weekday_window", afterSchool, "2026-06-28T17:00:00Z"], // Sun -> lock
  // Midnight-crossing Friday-night window.
  ["friday_2300_in_window", fridayNight, "2026-06-26T22:00:00Z"], // Fri 23:00 BST -> allow
  ["saturday_0500_in_window", fridayNight, "2026-06-27T04:00:00Z"], // Sat 05:00 BST -> allow
  ["saturday_0700_out_of_window", fridayNight, "2026-06-27T06:30:00Z"], // Sat 07:30 BST -> lock
  // Two windows same day.
  ["weekend_first_window", twoWindows, "2026-06-27T09:00:00Z"], // Sat 10:00 BST -> allow
  ["weekend_gap_locked", twoWindows, "2026-06-27T12:00:00Z"], // Sat 13:00 BST -> lock
  ["weekend_second_window", twoWindows, "2026-06-27T15:00:00Z"], // Sat 16:00 BST -> allow
  // Special clauses.
  ["empty_always_allow", empty, "2026-06-29T03:00:00Z"],
  ["revoked_blocks", revoked, "2026-06-29T17:00:00Z"],
  ["expired_blocks", expired, "2026-06-29T17:00:00Z"],
  ["bad_tz_fails_open", badTz, "2026-06-29T03:00:00Z"], // malformed tz -> allow no_clause
];

const vectors = cases.map(([name, c, iso]) => {
  const now = new Date(iso);
  return {
    name,
    clause: c,
    now_unix: Math.floor(now.getTime() / 1000),
    expect: evaluateSchedule(c, now),
  };
});

const path = join(OUT_DIR, "schedule_vectors.json");
writeFileSync(path, JSON.stringify({ vectors }, null, 2) + "\n");
console.log("wrote", path, `(${vectors.length} vectors)`);
