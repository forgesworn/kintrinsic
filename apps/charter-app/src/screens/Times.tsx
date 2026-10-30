import { useState } from "react";
import { Banner, Button } from "../components/ui";
import { canUseTimes, timesGate, unknownTimeTags } from "../domain/times";
import { MAX_NAMED_TIMES, MAX_TIME_LABEL_LEN } from "../wire/grant";
import type { Child, Device } from "../domain/types";
import { wardPath } from "../domain/wardRoute";
import { newTimePath, timePath } from "../routes";
import { useCharter } from "../store/store";
import { scheduleSummary, buildSchedule } from "./Limits";
import { timeCardSummary, windowsOnDevice, windowsSummary } from "./timesView";

// A child's Times: Free time first (always there, never deleted), then one card
// per time the guardian has made, then "Add a time".
//
// Free time's RULES are the Rules list below this section, relabelled as Free
// time's: its card here is the way to its hours. A named time carves out of
// Free time (it starts from Free time's bans and departs from them), so the
// order on the page is the order of the idea: Free time, the times that carve
// from it, then the rules Free time is made of.

function DeviceHours({ device, hours }: { device: Device; hours: string }) {
  return (
    <span className="row-sub" style={{ display: "block" }}>
      <span aria-hidden="true">{device.platform === "android" ? "📱" : "💻"}</span> {device.label}:{" "}
      {hours}
    </span>
  );
}

export function TimesSection({ child }: { child: Child }) {
  const { deviceStatus } = useCharter();
  const policy = child.policies.find((p) => p.scope.kind === "device");
  const [adding, setAdding] = useState(false);
  const [name, setName] = useState("");
  if (!policy) return null;

  const times = policy.times ?? [];
  const paired = child.devices.filter((d) => d.pairing === "paired");
  const capable = paired.filter((d) => canUseTimes(d, deviceStatus));
  // Named, never counted. Unlike other features a device that has not reported
  // is NOT assumed capable: an old ward would run a tagged window as Free time.
  const tooOld = paired.filter((d) => timesGate(d, deviceStatus) === "old");
  const unreported = paired.filter((d) => timesGate(d, deviceStatus) === "unreported");
  const orphaned = unknownTimeTags(policy);
  const full = times.length >= MAX_NAMED_TIMES;
  const hasLearning = times.some((t) => t.id === "learning");
  const go = (route: string) => {
    window.location.hash = `/${route}`;
  };

  return (
    <div className="stack">
      {tooOld.length > 0 && (
        <Banner tone="warn">
          {tooOld.map((d) => d.label).join(" and ")} {tooOld.length > 1 ? "need" : "needs"} updating to
          use times. Until then {tooOld.length > 1 ? "they keep" : "it keeps"} running Free time only:
          nothing you set for another time reaches {tooOld.length > 1 ? "them" : "it"}.
        </Banner>
      )}
      {unreported.length > 0 && (
        <Banner tone="warn">
          {unreported.map((d) => d.label).join(" and ")} {unreported.length > 1 ? "need" : "needs"} to
          check in once before {unreported.length > 1 ? "they" : "it"} can run times. Open Kintrinsic on{" "}
          {unreported.length > 1 ? "them" : "it"}; this turns on by itself when it reports.
        </Banner>
      )}
      {orphaned.length > 0 && (
        <Banner tone="warn">
          Some hours are set for a time that isn’t set up any more, so those hours stay locked. Add the
          time back, or remove its hours.
        </Banner>
      )}

      <div className="list">
        <button type="button" className="row" onClick={() => go(`${wardPath(child.id)}/rule/schedule`)}>
          <span className="row-main">
            <span className="row-title">Free time</span>
            <br />
            <span className="row-sub">
              {scheduleSummary(buildSchedule(policy.schedule), policy.schedule !== undefined)}
              {" · "}everything except what the rules below ban
            </span>
          </span>
          <span className="row-chevron" aria-hidden="true">
            ›
          </span>
        </button>

        {times.map((t) => {
          const lines = capable.map((d) => ({
            device: d,
            hours: windowsSummary(windowsOnDevice(policy, t.id, d.id)),
          }));
          return (
            <button
              key={t.id}
              type="button"
              className="row"
              onClick={() => go(timePath(child.id, t.id))}
            >
              <span className="row-main">
                <span className="row-title">{t.label}</span>
                <br />
                <span className="row-sub">{timeCardSummary(t)}</span>
                {lines.length === 0 ? (
                  <span className="row-sub" style={{ display: "block" }}>
                    No device can run this yet
                  </span>
                ) : (
                  lines.map((l) => <DeviceHours key={l.device.id} device={l.device} hours={l.hours} />)
                )}
              </span>
              <span className="row-chevron" aria-hidden="true">
                ›
              </span>
            </button>
          );
        })}
      </div>

      {adding ? (
        <div className="card" role="group" aria-label="Add a time">
          <div className="field-label">What kind of time?</div>
          <div className="stack" style={{ gap: 8 }}>
            {!hasLearning && (
              <Button variant="secondary" block onClick={() => go(newTimePath(child.id, "learning"))}>
                Learning time
              </Button>
            )}
            <div className="field" style={{ margin: 0 }}>
              <label className="field-label" htmlFor={`new-time-${child.id}`}>
                Or make your own
              </label>
              <div style={{ display: "flex", gap: 8 }}>
                <input
                  id={`new-time-${child.id}`}
                  className="input"
                  placeholder="Name it (e.g. Reading time)"
                  value={name}
                  maxLength={MAX_TIME_LABEL_LEN}
                  onChange={(e) => setName(e.target.value)}
                  onKeyDown={(e) =>
                    e.key === "Enter" && name.trim() && go(newTimePath(child.id, "custom", name.trim()))
                  }
                />
                <Button
                  disabled={!name.trim()}
                  onClick={() => go(newTimePath(child.id, "custom", name.trim()))}
                >
                  Custom
                </Button>
              </div>
            </div>
            <Button
              variant="ghost"
              block
              onClick={() => {
                setAdding(false);
                setName("");
              }}
            >
              Cancel
            </Button>
          </div>
        </div>
      ) : (
        <Button variant="secondary" block disabled={full} onClick={() => setAdding(true)}>
          Add a time
        </Button>
      )}
      {full && (
        <p className="card-sub" style={{ margin: 0 }}>
          {child.name} has the most times a child can have ({MAX_NAMED_TIMES}).
        </p>
      )}
      <p className="card-sub" style={{ margin: 0 }}>
        A time is a stretch of the day with its own rules, like Learning time on weekday mornings. Outside
        every time, the device is locked.
      </p>
    </div>
  );
}
