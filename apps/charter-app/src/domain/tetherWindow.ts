import type { Tethering } from "./types";

// The hotspot window presets + the "which preset is selected" derivation.
// Extracted from the Limits screen because the derivation had a real bug
// (2026-07-23, decented): "Rest of today" was hardcoded unselectable — tapping
// it saved `until = end-of-day` correctly but the chip could never highlight,
// reading as broken.

export const TETHER_WINDOWS: { label: string; minutes: number | null }[] = [
  { label: "1 hour", minutes: 60 },
  { label: "2 hours", minutes: 120 },
  { label: "Rest of today", minutes: null },
  { label: "Until I turn it off", minutes: -1 },
];

/** End-of-day (local) as unix seconds — the "rest of today" window. */
export function endOfTodayUnix(): number {
  const d = new Date();
  d.setHours(23, 59, 59, 0);
  return Math.floor(d.getTime() / 1000);
}

/**
 * Which preset chip the stored posture corresponds to. Order matters:
 * "Rest of today" is matched FIRST (±5 min of end-of-day), so late-evening
 * saves don't get mislabelled as the nearest hour bucket.
 */
export function tetherWindowLabel(
  t: Tethering,
  nowUnix: number,
  endOfDay: number = endOfTodayUnix(),
): string {
  if (t.until == null) return "Until I turn it off";
  if (Math.abs(t.until - endOfDay) < 300) return "Rest of today";
  const mins = Math.max(1, Math.round((t.until - nowUnix) / 60));
  return (
    TETHER_WINDOWS.find(
      (w) => w.minutes != null && w.minutes > 0 && Math.abs(mins - w.minutes) < 5,
    )?.label ?? "Until I turn it off"
  );
}
