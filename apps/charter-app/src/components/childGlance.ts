import { anyOnHomeNetwork, bestLiveness } from "../domain/liveness";
import { remainingNow } from "../domain/timeLeft";
import type { Child } from "../domain/types";
import type { DeviceStatus } from "../wire/status";
import { freshestStatusFor, liveStatusFor } from "../store/liveStatus";
import { isSetUp } from "../store/statusFor";

// A child's state in a glance: the two short lines on their bottom-bar button.
//
// It reuses the same derivations as the child page's "Now" card
// (freshestStatusFor -> liveStatusFor), so the bar and the card cannot
// disagree; this only picks the short words.

export interface ChildGlance {
  /** The short line under the name: "2h 15m left", "Locked · 4pm". */
  line: string;
  /** How to colour the line. */
  tone: "ok" | "low" | "out" | "idle";
  /** Time left as a share of today's cap (0..1), or null when there is none to show. */
  fraction: number | null;
  /** Whether any of the child's devices is reachable right now. */
  online: boolean;
  /** The whole thing as a sentence, for the button's aria-label. */
  spoken: string;
}

/** Under this many seconds left the line turns amber. */
const LOW_BELOW_SECS = 15 * 60;

/** The longest line that fits the pill without being cut off. */
const MAX_LINE = 14;

/** The daily cap (if any) for the child's whole-device limit. */
export function dailyCap(child: Child): number | null {
  const device = child.policies.find((p) => p.scope.kind === "device");
  const daily = device?.budget?.paused ? null : device?.budget?.dailyMinutes;
  return daily ?? null;
}

/** `2h 15m` · `45m` · `<1m`. Whole minutes only: the bar is not the place for seconds. */
function compactLeft(secs: number): string {
  const mins = Math.floor(secs / 60);
  if (mins < 1) return "<1m";
  if (mins < 60) return `${mins}m`;
  const m = mins % 60;
  return m === 0 ? `${Math.floor(mins / 60)}h` : `${Math.floor(mins / 60)}h ${m}m`;
}

/** `2 hours 15 minutes` · `45 minutes` · `less than a minute`. */
function spokenLeft(secs: number): string {
  const mins = Math.floor(secs / 60);
  if (mins < 1) return "less than a minute";
  const unit = (n: number, word: string) => `${n} ${word}${n === 1 ? "" : "s"}`;
  const h = Math.floor(mins / 60);
  const m = mins % 60;
  if (h === 0) return unit(m, "minute");
  return m === 0 ? unit(h, "hour") : `${unit(h, "hour")} ${unit(m, "minute")}`;
}

/** "Opens at 4:00 PM" -> "4pm". Today's opening only; anything further out is too long for the bar. */
function shortOpening(text: string | undefined): string | null {
  const m = /^Opens at (\d{1,2}):(\d{2}) (AM|PM)$/.exec(text ?? "");
  if (!m) return null;
  return `${m[1]}${m[2] === "00" ? "" : `:${m[2]}`}${m[3].toLowerCase()}`;
}

/**
 * `sessions` are the device keys the carrier holds a home-network session
 * with; such a device is reachable whatever its last relay heartbeat says,
 * as on the Now card.
 */
export function childGlance(
  child: Child,
  deviceStatus: Record<string, DeviceStatus>,
  now: number,
  sessions: ReadonlySet<string> = new Set(),
): ChildGlance {
  const online =
    bestLiveness(child.devices, now).kind === "live" || anyOnHomeNetwork(child.devices, sessions);
  const where = online ? "online" : "offline";

  if (!isSetUp(child)) {
    return {
      line: "Set up",
      tone: "idle",
      fraction: null,
      online,
      spoken: `${child.name}, not set up yet`,
    };
  }

  const s = liveStatusFor(child, freshestStatusFor(child, deviceStatus, now), now);

  if (s.allowedNow) {
    if (s.minutesLeftToday == null) {
      return {
        line: "No limit",
        tone: "ok",
        fraction: null,
        online,
        spoken: `${child.name}, ${where}, no time limit today`,
      };
    }
    // Counted down from the device's report, as the Now card does.
    const secs = remainingNow(s.secondsLeftToday, s.asOf, now) ?? s.minutesLeftToday * 60;
    const cap = dailyCap(child);
    return {
      line: `${compactLeft(secs)} left`,
      tone: secs < LOW_BELOW_SECS ? "low" : "ok",
      fraction: cap ? Math.max(0, Math.min(1, secs / 60 / cap)) : null,
      online,
      spoken: `${child.name}, ${where}, ${spokenLeft(secs)} left today`,
    };
  }

  if (s.reason === "budget") {
    return {
      line: "Time's up",
      tone: "out",
      fraction: 0,
      online,
      spoken: `${child.name}, ${where}, time's up for today`,
    };
  }
  if (s.reason === "standdown") {
    return {
      line: "Stopped",
      tone: "out",
      fraction: null,
      online,
      spoken: `${child.name}, ${where}, stopped until you allow them back on`,
    };
  }
  const opens = shortOpening(s.nextWindowText);
  const withOpening = opens ? `Locked · ${opens}` : "";
  return {
    line: withOpening && withOpening.length <= MAX_LINE ? withOpening : "Locked",
    tone: "out",
    fraction: null,
    online,
    spoken: `${child.name}, ${where}, locked${s.nextWindowText ? `. ${s.nextWindowText}` : ""}`,
  };
}
