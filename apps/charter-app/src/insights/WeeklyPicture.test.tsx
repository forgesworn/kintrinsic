// Render-level pin for review fix C1 (2026-08-04): the guardian's weekly
// card must never show two contradictory sentences at once — "No screen
// time recorded yet this week" alongside a live "N nights this week · Xm"
// out-of-hours line. That combination is exactly what a ward whose ONLY
// device use was an always-available app during locked hours would have
// produced before this fix, because `out_of_hours_*_secs` is deliberately
// never added into the counted `usedTodaySecs`/`totalSecs` totals — this is
// the feature's own core case, not an edge case.
//
// No component-render test existed anywhere in this app before this file;
// `react-dom/server`'s `renderToStaticMarkup` is used instead of adding a
// new test-only dependency (no @testing-library/react) — react-dom is
// already a runtime dependency of the app.
import { describe, expect, it } from "vitest";
import { renderToStaticMarkup } from "react-dom/server";
import { WeeklyPicture } from "./WeeklyPicture";
import { emptyUsageHistory, outOfHoursLine, recordUsage, weeklyView } from "./usageHistory";

const NOW = new Date(2026, 6, 24, 12, 0, 0).getTime(); // 2026-07-24 (Fri) 12:00 local
const PHONE = "p".repeat(64);
const DEVICES = [{ machine: PHONE, label: "Phone", platform: "android" as const }];
const NO_SCREEN_TIME_PLACEHOLDER = "No screen time recorded yet this week";

describe("WeeklyPicture — out-of-hours coherence (review fix C1, 2026-08-04)", () => {
  it("never prints the blanket 'no screen time recorded' placeholder beside a live out-of-hours line", () => {
    // Zero COUNTED screen time all week (an empty usage history) but real
    // out-of-hours use — the exact shape a locked-hours-only ward produces.
    const days = weeklyView(emptyUsageHistory, [PHONE], null, NOW);
    const outOfHours = outOfHoursLine(3, 135 * 60); // "3 nights this week · 2h 15m"
    expect(outOfHours).not.toBeNull();

    const html = renderToStaticMarkup(
      <WeeklyPicture days={days} devices={DEVICES} outOfHours={outOfHours} />,
    );

    expect(html).not.toContain(NO_SCREEN_TIME_PLACEHOLDER);
    expect(html).toContain("No counted screen time this week.");
    expect(html).toContain(outOfHours as string);
  });

  it("keeps the original gentle placeholder when there is truly nothing to say at all", () => {
    const days = weeklyView(emptyUsageHistory, [PHONE], null, NOW);
    const html = renderToStaticMarkup(
      <WeeklyPicture days={days} devices={DEVICES} outOfHours={null} />,
    );
    expect(html).toContain(
      "No screen time recorded yet this week — it fills in as the devices are used.",
    );
  });

  it("omits the out-of-hours line entirely when there is nothing to say, same as its caller", () => {
    const days = weeklyView(emptyUsageHistory, [PHONE], null, NOW);
    const html = renderToStaticMarkup(
      <WeeklyPicture days={days} devices={DEVICES} outOfHours={null} />,
    );
    expect(html).not.toContain("this week ·");
  });

  it("renders the out-of-hours line directly after weekSummary's own line, before the device legend (review fix I1)", () => {
    const h = recordUsage(emptyUsageHistory, PHONE, "2026-07-24", 3600); // some counted usage too
    const days = weeklyView(h, [PHONE], null, NOW);
    const outOfHours = outOfHoursLine(1, 12 * 60); // "1 night this week · 12m"

    const html = renderToStaticMarkup(
      <WeeklyPicture days={days} devices={DEVICES} outOfHours={outOfHours} />,
    );

    const summaryIdx = html.indexOf("This week:");
    const outOfHoursIdx = html.indexOf(outOfHours as string);
    const legendIdx = html.indexOf("Phone"); // the device-legend row's label

    expect(summaryIdx).toBeGreaterThan(-1);
    expect(outOfHoursIdx).toBeGreaterThan(-1);
    expect(legendIdx).toBeGreaterThan(-1);
    expect(outOfHoursIdx).toBeGreaterThan(summaryIdx);
    expect(outOfHoursIdx).toBeLessThan(legendIdx);
  });
});
