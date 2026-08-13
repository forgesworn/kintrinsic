import type { DayUsage } from "./usageHistory";
import { emptyWeekMessage, humanDuration, weekSummary } from "./usageHistory";

// The weekly picture (design memo B1) — a calm bar chart of a child's screen
// time, one bar per day, stacked by device, against the agreed daily allowance.
// Going over shows as the red part of the bar (an overdraft to notice and wind
// back), never a slammed door. Constitution guard: reflective, not a scoreboard
// — no streaks, no praise/blame, no animation, no nudges.

export interface DeviceMeta {
  machine: string;
  label: string;
  platform: "linux" | "android";
}

/** Calm device shades, assigned by device order. Deliberately no reds — the
 *  red family is reserved for the overdraft wash so it always reads on top. */
const DEVICE_SHADES = ["#3c7e70", "#9a6516", "#5a648c", "#7e5a8c"];
const OVER = "var(--blocked, #b3261e)";
/** Simultaneous use across devices — counted once, shown as its own shade. */
const BOTH = "#46505e";

function platformGlyph(p: "linux" | "android"): string {
  return p === "android" ? "📱" : "🖥";
}

export function WeeklyPicture({
  days,
  devices,
  outOfHours = null,
}: {
  days: DayUsage[];
  /** The child's devices, in the same machine order as each day's perDevice. */
  devices: DeviceMeta[];
  /**
   * The week's out-of-hours line (`outOfHoursLine`, spec 2026-08-03),
   * already formatted by the caller — rendered HERE, directly beside
   * `weekSummary`'s own output, rather than by the caller (review fix I1,
   * 2026-08-04), and consulted by the empty-state placeholder below so the
   * two can never disagree (review fix C1, 2026-08-04): a week with zero
   * COUNTED screen time but real out-of-hours use must never print "No
   * screen time recorded" beside a live out-of-hours line. `null`/omitted
   * when there is nothing to say.
   *
   * Deliberately NOT the same "week" as `weekSummary`'s own line printed
   * right beside it (review fix I2, 2026-08-04): `days`/`weekSummary` are the
   * last 7 ROLLING days ending today, in the guardian's tz; `outOfHours` is a
   * CALENDAR week that resets at `week_start`, in the device's tz. Absent
   * ever after a reset would otherwise read as "the night use stopped" when
   * the chart above still shows the bars that produced it — `outOfHoursLine`
   * itself now says "so far this week" rather than plain "this week" so the
   * two lines can never be mistaken for describing the same window.
   */
  outOfHours?: string | null;
}) {
  const allowanceSecs = days[0]?.allowanceSecs ?? 0;
  const maxTotal = Math.max(0, ...days.map((d) => d.totalSecs));
  const anyUsage = maxTotal > 0;

  // Scale so the allowance line sits comfortably and overdrafts have room above.
  const top = Math.max(maxTotal * 1.12, allowanceSecs * 1.35, 3600);

  // Geometry (viewBox units).
  const W = 320;
  const H = 150;
  const padL = 4;
  const padR = 4;
  const padTop = 8;
  const baseY = H - 22; // room for day labels
  const plotH = baseY - padTop;
  const n = days.length;
  const slot = (W - padL - padR) / n;
  const barW = Math.min(26, slot * 0.6);

  const y = (secs: number) => baseY - (Math.min(secs, top) / top) * plotH;
  const allowanceY = allowanceSecs > 0 ? y(allowanceSecs) : null;

  const shade = (i: number) => DEVICE_SHADES[i % DEVICE_SHADES.length];

  return (
    <div>
      {anyUsage ? (
        <svg
          viewBox={`0 0 ${W} ${H}`}
          width="100%"
          role="img"
          aria-label={weekSummary(days)}
          style={{ display: "block" }}
        >
          {/* allowance line */}
          {allowanceY != null && (
            <>
              <line
                x1={padL}
                x2={W - padR}
                y1={allowanceY}
                y2={allowanceY}
                stroke="var(--text-3, #847d72)"
                strokeWidth="1"
                strokeDasharray="3 3"
              />
              <text
                x={W - padR}
                y={allowanceY - 3}
                textAnchor="end"
                fontSize="7"
                fill="var(--text-3, #847d72)"
              >
                agreed {humanDuration(allowanceSecs)}
              </text>
            </>
          )}

          {days.map((d, di) => {
            const cx = padL + slot * di + slot / 2;
            const x = cx - barW / 2;
            // Stack device segments from the base up. On union days the
            // per-device seconds sum to MORE than the bar (simultaneous use
            // counts once), so scale them into the exclusive portion and draw
            // the shared time as its own "both at once" cap.
            const scalarSum = d.perDevice.reduce((s, p) => s + p.secs, 0);
            const exclusive = d.totalSecs - d.overlapSecs;
            const scale = d.overlapSecs > 0 && scalarSum > 0 ? exclusive / scalarSum : 1;
            let cursor = 0;
            const segs = d.perDevice.map((pd, i) => {
              const secs = pd.secs * scale;
              const y0 = y(cursor);
              const y1 = y(cursor + secs);
              cursor += secs;
              return secs > 0 ? (
                <rect
                  key={pd.machine}
                  x={x}
                  y={y1}
                  width={barW}
                  height={Math.max(0, y0 - y1)}
                  fill={shade(i)}
                  rx="1.5"
                />
              ) : null;
            });
            const both =
              d.overlapSecs > 0 ? (
                <rect
                  x={x}
                  y={y(d.totalSecs)}
                  width={barW}
                  height={Math.max(0, y(exclusive) - y(d.totalSecs))}
                  fill={BOTH}
                  rx="1.5"
                />
              ) : null;
            // Overdraft wash: from the allowance line up to the bar top.
            const over =
              d.overdraftSecs > 0 && allowanceY != null ? (
                <rect
                  x={x - 1}
                  y={y(d.totalSecs)}
                  width={barW + 2}
                  height={Math.max(0, allowanceY - y(d.totalSecs))}
                  fill={OVER}
                  opacity="0.45"
                  rx="1.5"
                />
              ) : null;
            return (
              <g key={d.dayKey}>
                {segs}
                {both}
                {over}
                <text
                  x={cx}
                  y={H - 8}
                  textAnchor="middle"
                  fontSize="8"
                  fill={d.overdraftSecs > 0 ? OVER : "var(--text-2, #4c463f)"}
                  fontWeight={d.overdraftSecs > 0 ? 600 : 400}
                >
                  {d.label}
                </text>
              </g>
            );
          })}
        </svg>
      ) : (
        // Out-of-hours-aware (review fix C1, 2026-08-04): a ward whose ONLY
        // device use this week was an always-available app during locked
        // hours has `anyUsage === false` here (out-of-hours seconds are
        // deliberately never counted into `totalSecs`) while `outOfHours`
        // below is genuinely live — exactly the case this feature exists to
        // describe honestly. `emptyWeekMessage` says the honest, narrower
        // thing ("no COUNTED screen time") instead of the blanket claim,
        // so it never contradicts the out-of-hours line printed right after it.
        <p className="card-sub" style={{ margin: "8px 0" }}>
          {emptyWeekMessage(outOfHours != null)}
        </p>
      )}

      {/* summary + out-of-hours, side by side. With no COUNTED usage the
          placeholder above already says so — `weekSummary` would print a
          near-identical second sentence right under it ("No screen time
          recorded yet this week." twice, in a row). `outOfHours` is
          independent of `anyUsage` (out-of-hours use can be real even when
          counted usage is zero) and renders directly beside `weekSummary`'s
          own line, never below the device legend. */}
      {anyUsage && (
        <p className="card-sub" style={{ marginTop: 6 }}>
          {weekSummary(days)}
        </p>
      )}
      {outOfHours && (
        <p className="card-sub" style={{ marginTop: anyUsage ? 2 : 6 }}>
          {outOfHours}
        </p>
      )}
      {anyUsage && devices.length > 0 && (
        <div
          style={{
            display: "flex",
            flexWrap: "wrap",
            gap: 12,
            marginTop: 6,
            fontSize: "0.8rem",
            color: "var(--text-2)",
          }}
        >
          {devices.map((dev, i) => (
            <span key={dev.machine} style={{ display: "inline-flex", alignItems: "center", gap: 5 }}>
              <span
                aria-hidden
                style={{ width: 10, height: 10, borderRadius: 3, background: shade(i), display: "inline-block" }}
              />
              {platformGlyph(dev.platform)} {dev.label}
            </span>
          ))}
          {days.some((d) => d.overlapSecs > 0) && (
            <span style={{ display: "inline-flex", alignItems: "center", gap: 5 }}>
              <span
                aria-hidden
                style={{ width: 10, height: 10, borderRadius: 3, background: BOTH, display: "inline-block" }}
              />
              both at once
            </span>
          )}
          {allowanceSecs > 0 && (
            <span style={{ display: "inline-flex", alignItems: "center", gap: 5 }}>
              <span
                aria-hidden
                style={{ width: 10, height: 10, borderRadius: 3, background: OVER, opacity: 0.45, display: "inline-block" }}
              />
              over the agreed time
            </span>
          )}
        </div>
      )}
    </div>
  );
}
