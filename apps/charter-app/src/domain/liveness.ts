// Is a device actually THERE, right now?
//
// "Connected" used to mean three different things depending on which screen you
// read it on: hardcoded (Home said Connected even with no device at all), "has a
// paired device" (the Family ward pill), or "is paired" (the device row). None of
// them meant the device was reachable — so a phone that had been off for a week
// still read "Connected", which is the one thing a guardian would take it to mean.
//
// It means live now, and it means it in one place so the screens can't drift.

/**
 * How stale a device's last STATUS may be and still count as here.
 *
 * A ward device emits STATUS on a 60-second heartbeat, so anything under a
 * minute is normal even on a perfectly healthy device. Five minutes leaves room
 * for a doze cycle or a slow poll without calling a live device dead — and a
 * guardian who sees "Connected" on a device that went quiet four minutes ago has
 * not been misled in any way that matters.
 */
export const LIVE_WITHIN_MS = 5 * 60 * 1000;

export type Liveness =
  /** No device, or one that never finished pairing. */
  | { kind: "not-set-up" }
  /** Paired, but it has never once reported in. */
  | { kind: "never-seen" }
  /** Reporting in now. */
  | { kind: "live"; lastSeenAt: number }
  /** Paired and known, but quiet for longer than we'd expect. */
  | { kind: "quiet"; lastSeenAt: number };

export function liveness(
  opts: { paired: boolean; lastSeenAt?: number },
  now: number,
): Liveness {
  if (!opts.paired) return { kind: "not-set-up" };
  if (!opts.lastSeenAt) return { kind: "never-seen" };
  return now - opts.lastSeenAt <= LIVE_WITHIN_MS
    ? { kind: "live", lastSeenAt: opts.lastSeenAt }
    : { kind: "quiet", lastSeenAt: opts.lastSeenAt };
}

/** How long ago, in words a parent would use. */
export function seenLabel(lastSeenAt: number, now: number): string {
  const secs = Math.max(0, Math.floor((now - lastSeenAt) / 1000));
  if (secs < 90) return "seen moments ago";
  if (secs < 3600) return `seen ${Math.floor(secs / 60)} min ago`;
  if (secs < 86400) return `seen ${Math.floor(secs / 3600)} h ago`;
  return `not seen for ${Math.floor(secs / 86400)} d`;
}

/**
 * One verdict for a ward who may hold several devices.
 *
 * The BEST of them wins: a child whose phone is live and whose old laptop has
 * been shut in a drawer for a month is reachable, and saying "not seen for 30 d"
 * beside their name would be true of a device and false of the child. Per-device
 * detail belongs on the per-device lines, which now show it.
 */
export function bestLiveness(
  devices: { pairing?: string; lastSeenAt?: number }[],
  now: number,
): Liveness {
  const each = devices.map((d) =>
    liveness({ paired: d.pairing === "paired", lastSeenAt: d.lastSeenAt }, now),
  );
  const live = each.find((l) => l.kind === "live");
  if (live) return live;
  const quiet = each
    .filter((l): l is Extract<Liveness, { kind: "quiet" }> => l.kind === "quiet")
    .sort((a, b) => b.lastSeenAt - a.lastSeenAt)[0];
  if (quiet) return quiet;
  if (each.some((l) => l.kind === "never-seen")) return { kind: "never-seen" };
  return { kind: "not-set-up" };
}

/** The short status chip: what it says, and how alarmed to look. */
export function livenessChip(l: Liveness, now: number): {
  text: string;
  tone: "ok" | "neutral" | "warn";
} {
  switch (l.kind) {
    case "not-set-up":
      return { text: "Not set up", tone: "neutral" };
    case "never-seen":
      // Not "Offline": nothing has gone wrong yet, it just hasn't spoken.
      return { text: "Waiting to hear from it", tone: "neutral" };
    case "live":
      return { text: "Connected", tone: "ok" };
    case "quiet":
      // Say WHEN rather than a bare "Offline" — a device last seen 6 minutes
      // ago and one last seen in March are very different problems.
      return { text: seenLabel(l.lastSeenAt, now), tone: "warn" };
  }
}
