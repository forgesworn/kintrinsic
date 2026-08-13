import type { Device } from "../domain/types";

/**
 * The guardian's "let them install something" window.
 *
 * Kintrinsic's Device-Owner baseline closes the install capability itself — both
 * install restrictions, every tick — which is deliberate and is also why a ward
 * could not update an app from their store at all. The only stand-down that
 * exists is the signed, expiring `maintenance` clause, and until now Kintrinsic
 * only offered it when a phone's *own* Kintrinsic update was failing, so the
 * everyday case ("Robin wants the new version of a game", 2026-07-30) had no
 * route at all: not remote, not at the glass.
 *
 * So the same clause is offered plainly. It is the honest shape for this,
 * because it is the shape the ward's phone already enforces: guardian-initiated,
 * signed, capped, self-closing, and re-derived every tick so a reboot inside the
 * window does not extend it by a second.
 */

/**
 * How long to open for. Long enough for a real store download on a real
 * house's wifi — 15 minutes was chosen for a cabled repair, and a large app
 * update that gets cut off mid-download is a window that failed.
 */
export const INSTALL_WINDOW_MINUTES = 30;

/**
 * The ward's core refuses anything longer (`MAX_MAINTENANCE_SECS` = 1 hour), so
 * a guardian can never be offered a window their own phone would ignore. Mirror
 * it here rather than discover it as a silently-dropped clause.
 */
export const MAX_INSTALL_WINDOW_MINUTES = 60;

/**
 * Only a paired phone. The Linux warden has no install lock to stand down (the
 * clause is Android-only), so offering it on a laptop row would be a button that
 * does nothing — the exact dishonesty the `linuxBehind` copy exists to avoid.
 */
export function canOpenInstallWindow(device: Device): boolean {
  return device.pairing === "paired" && device.platform === "android";
}

export interface InstallWindow {
  open: boolean;
  secondsLeft: number;
}

/**
 * Level-triggered from a timestamp, never a latched "opened" flag: a boolean set
 * when the guardian tapped would still read "open" long after the ward's phone
 * had re-locked, which is precisely the kind of disagreement between the two
 * screens that costs a family their trust in what Kintrinsic says.
 */
export function installWindow(
  openUntilUnix: number | undefined,
  nowUnix: number,
): InstallWindow {
  if (!openUntilUnix) return { open: false, secondsLeft: 0 };
  const secondsLeft = openUntilUnix - nowUnix;
  if (secondsLeft <= 0) return { open: false, secondsLeft: 0 };
  return { open: true, secondsLeft };
}

/**
 * Open windows by child, as `{ childId: untilUnix }`. Kept in its own
 * localStorage key rather than component state, because otherwise reloading
 * Kintrinsic forgets a window is open — which loses the "Close now" button for
 * exactly as long as the loosening lasts, and shows a phone as shut while it is
 * genuinely open. Per CHILD, not per device: the clause is signed for a child
 * and delivered to all of their devices.
 */
export const INSTALL_WINDOW_KEY = "charter.installWindows";

export type OpenWindows = Record<string, number>;

/** Drop anything already expired, so the store can't grow without bound and a
 *  stale entry can never resurrect a window that closed days ago. */
export function pruneWindows(windows: OpenWindows, nowUnix: number): OpenWindows {
  const live: OpenWindows = {};
  for (const [childId, untilUnix] of Object.entries(windows)) {
    if (typeof untilUnix === "number" && untilUnix > nowUnix) live[childId] = untilUnix;
  }
  return live;
}

/** `untilUnix` of null closes it — the record loses the child entirely. */
export function putWindow(
  windows: OpenWindows,
  childId: string,
  untilUnix: number | null,
  nowUnix: number,
): OpenWindows {
  const live = pruneWindows(windows, nowUnix);
  if (untilUnix === null) {
    delete live[childId];
    return live;
  }
  return { ...live, [childId]: untilUnix };
}

export function readWindows(nowUnix: number): OpenWindows {
  try {
    const raw = localStorage.getItem(INSTALL_WINDOW_KEY);
    if (!raw) return {};
    const parsed: unknown = JSON.parse(raw);
    if (!parsed || typeof parsed !== "object") return {};
    return pruneWindows(parsed as OpenWindows, nowUnix);
  } catch {
    // Unreadable storage must never take the screen down; the window still
    // closes on the phone regardless of what this remembers.
    return {};
  }
}

export function writeWindow(
  childId: string,
  untilUnix: number | null,
  nowUnix: number,
): void {
  try {
    const next = putWindow(readWindows(nowUnix), childId, untilUnix, nowUnix);
    localStorage.setItem(INSTALL_WINDOW_KEY, JSON.stringify(next));
  } catch {
    /* storage full or blocked — the phone's own expiry is the real guarantee */
  }
}

/**
 * "14 minutes left" / "40 seconds left". Rounds UP while a minute is still
 * running, so the number never claims less time than the phone will actually
 * allow — a guardian reading "0 minutes" with 50 seconds left would give up.
 */
export function installWindowLabel(secondsLeft: number): string {
  if (secondsLeft <= 0) return "closed";
  if (secondsLeft < 60) {
    const s = Math.ceil(secondsLeft);
    return `${s} second${s === 1 ? "" : "s"} left`;
  }
  const m = Math.ceil(secondsLeft / 60);
  return `${m} minute${m === 1 ? "" : "s"} left`;
}

/**
 * How the account of a window reads. Kept here, and pure, because the wording
 * is the feature: a guardian who opened a window for one update wants to know
 * in one glance whether one thing came through or eleven.
 */
export interface InstallAccount {
  startedAt: number;
  endedAt?: number;
  changes: { pkg: string; label: string; kind: "installed" | "updated"; at: number }[];
}

/**
 * "Nothing was installed" is the common and reassuring answer, and it must read
 * as a real finding rather than a blank. An ABSENT account says nothing at all
 * — a phone that has never had a window opened, or one too old to report — and
 * the two must never look alike.
 */
export function accountSummary(account: InstallAccount | undefined): string | null {
  if (!account) return null;
  const installed = account.changes.filter((c) => c.kind === "installed").length;
  const updated = account.changes.filter((c) => c.kind === "updated").length;
  if (installed === 0 && updated === 0) return "Nothing was installed or updated.";
  const parts: string[] = [];
  if (installed > 0) parts.push(`${installed} app${installed === 1 ? "" : "s"} installed`);
  if (updated > 0) parts.push(`${updated} updated`);
  return `${parts.join(", ")}.`;
}
