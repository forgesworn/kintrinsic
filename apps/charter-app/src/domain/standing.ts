// A ward's honest standing, for the moment a guardian is about to give time.
//
// Giving five minutes to a ward who is already an hour over — or who has
// twenty minutes left in the whole week — is a different decision from giving
// five minutes to a ward who has just run out. The numbers exist on the phone
// already (STATUS carries usage; the guardian's own app knows the limits it
// set), so the only thing missing was saying them out loud.

export type Standing = {
  /** Seconds used today beyond the daily allowance. 0 when within it. */
  overdraftTodaySecs: number;
  /** Seconds left in the weekly pot, or null when no weekly cap is set. */
  weekLeftSecs: number | null;
};

export type StandingInput = {
  usedTodaySecs?: number;
  usedWeekSecs?: number;
  dailyMinutes?: number | null;
  weeklyMinutes?: number | null;
};

export function computeStanding(i: StandingInput): Standing {
  const usedToday = Math.max(0, i.usedTodaySecs ?? 0);
  const daily = i.dailyMinutes != null ? i.dailyMinutes * 60 : null;
  const overdraftTodaySecs = daily == null ? 0 : Math.max(0, usedToday - daily);

  const weekly = i.weeklyMinutes != null ? i.weeklyMinutes * 60 : null;
  // A weekly figure needs BOTH a cap and a reading. Without the reading we say
  // nothing rather than imply the pot is full — an unknown is not "fine".
  const weekLeftSecs =
    weekly == null || i.usedWeekSecs == null
      ? null
      : Math.max(0, weekly - Math.max(0, i.usedWeekSecs));

  return { overdraftTodaySecs, weekLeftSecs };
}

/** "1h 5m" / "20m" / "45s" — short enough to sit inside a sentence. */
export function shortDuration(secs: number): string {
  const s = Math.max(0, Math.round(secs));
  if (s < 60) return `${s}s`;
  const m = Math.round(s / 60);
  if (m < 60) return `${m}m`;
  const h = Math.floor(m / 60);
  const rem = m % 60;
  return rem === 0 ? `${h}h` : `${h}h ${rem}m`;
}

/**
 * The sentence to show beside a "give time" control, or null when there is
 * nothing worth saying — a ward inside their limits with room left in the week
 * needs no warning, and crying wolf is how warnings stop being read.
 */
export function standingNote(s: Standing): string | null {
  const parts: string[] = [];
  if (s.overdraftTodaySecs > 0) {
    parts.push(`${shortDuration(s.overdraftTodaySecs)} over today`);
  }
  // Only mention the week when it is genuinely tight — under an hour left.
  if (s.weekLeftSecs != null && s.weekLeftSecs <= 60 * 60) {
    parts.push(
      s.weekLeftSecs === 0
        ? "nothing left this week"
        : `${shortDuration(s.weekLeftSecs)} left this week`,
    );
  }
  return parts.length ? parts.join(" · ") : null;
}

/**
 * A ward's standing, drawn from their own limits and the freshest reading any
 * of their devices has sent.
 *
 * Usage is taken as the MAXIMUM across devices rather than the sum: each
 * warden reports the pooled figure it enforces against, so summing would
 * double-count a ward with a laptop and a phone and invent an overdraft that
 * does not exist.
 */
type StandingStatus = {
  usedTodaySecs?: number;
  usedWeekSecs?: number;
  dailyMinutes?: number;
  weeklyMinutes?: number;
};
type StandingChild = {
  policies: {
    scope: { kind: string };
    budget?: {
      dailyMinutes?: number | null;
      weeklyMinutes?: number | null;
      paused?: boolean;
    };
  }[];
  devices: { devicePubkey?: string | null }[];
};

export function standingFor(
  child: StandingChild,
  deviceStatus: Record<string, StandingStatus>,
): Standing {
  const budget = child.policies.find((p) => p.scope.kind === "device")?.budget;
  // A paused budget is no cap at all — there is nothing to be over.
  if (budget?.paused) return { overdraftTodaySecs: 0, weekLeftSecs: null };

  let usedTodaySecs: number | undefined;
  let usedWeekSecs: number | undefined;
  // The device's OWN limits, for a ward whose rules were set on the machine
  // rather than from here. Without this the app has no cap to measure against
  // and silently reports "not over", which is not the same as "within limits".
  let deviceDaily: number | undefined;
  let deviceWeekly: number | undefined;
  for (const d of child.devices) {
    const st = d.devicePubkey ? deviceStatus[d.devicePubkey] : undefined;
    if (!st) continue;
    if (st.usedTodaySecs != null) {
      usedTodaySecs = Math.max(usedTodaySecs ?? 0, st.usedTodaySecs);
    }
    if (st.usedWeekSecs != null) {
      usedWeekSecs = Math.max(usedWeekSecs ?? 0, st.usedWeekSecs);
    }
    // Tightest wins: with two devices reporting different caps, the one that
    // binds soonest is the one the ward will actually meet.
    if (st.dailyMinutes != null) {
      deviceDaily = Math.min(deviceDaily ?? st.dailyMinutes, st.dailyMinutes);
    }
    if (st.weeklyMinutes != null) {
      deviceWeekly = Math.min(deviceWeekly ?? st.weeklyMinutes, st.weeklyMinutes);
    }
  }

  // A guardian-set limit is authoritative; otherwise fall back to whatever the
  // device says it is enforcing.
  return computeStanding({
    usedTodaySecs,
    usedWeekSecs,
    dailyMinutes: budget?.dailyMinutes ?? deviceDaily,
    weeklyMinutes: budget?.weeklyMinutes ?? deviceWeekly,
  });
}

/**
 * The limits a ward is under that were set ON THEIR DEVICE, not from here.
 * `null` when the guardian's own limits are in force, or nothing is reported.
 *
 * Precedence is winner-takes-all: the first schedule or budget sent from this
 * app discards the device's limits wholesale. A guardian about to do that is
 * owed the warning.
 */
export function deviceSetLimits(
  child: StandingChild,
  deviceStatus: Record<string, StandingStatus & { source?: string }>,
): { dailyMinutes?: number; weeklyMinutes?: number } | null {
  const budget = child.policies.find((p) => p.scope.kind === "device")?.budget;
  if (budget?.dailyMinutes != null || budget?.weeklyMinutes != null) return null;
  for (const d of child.devices) {
    const st = d.devicePubkey ? deviceStatus[d.devicePubkey] : undefined;
    if (!st || st.source !== "device-only") continue;
    if (st.dailyMinutes != null || st.weeklyMinutes != null) {
      return { dailyMinutes: st.dailyMinutes, weeklyMinutes: st.weeklyMinutes };
    }
  }
  return null;
}
