import "./rule-editor.css";
import { Banner, Button, Card } from "../components/ui";
import type { Child } from "../domain/types";
import { wardPath, wardSettingsPath } from "../domain/wardRoute";
import { useCharter } from "../store/store";
import { AppRuleEditor, DeviceLimits, SECTION_TITLE, type SectionId } from "./Limits";
import { ruleSummaries } from "./ruleSummaries";

// A child's rules. On the child page they are one list in three plain groups:
// each row shows what the rule currently says and opens its own editor page
// (`#/ward/<id>/rule/<section>`). The editor page is the real thing: one rule,
// expanded, with Cancel / Save pinned to the bottom.

// These are FREE TIME's rules (the child's other times start from them). App
// allowances (the old "Named times" screen) sit with the apps: they decide what
// counts inside Free time, not when a time runs.
const GROUPS: { title: string; sections: SectionId[] }[] = [
  { title: "Time", sections: ["schedule", "budget", "listening"] },
  { title: "Apps & web", sections: ["always-available", "web", "apps", "named-times"] },
  { title: "Safety", sections: ["lifeline", "tethering"] },
];

/** Whether any of the child's devices is paired and can actually be reached. */
function hasLiveDevice(child: Child): boolean {
  return child.devices.some(
    (d) => d.pairing === "paired" && d.devicePubkey && /^[0-9a-f]{64}$/.test(d.devicePubkey),
  );
}

export function ChildRules({ child }: { child: Child }) {
  const devicePolicy = child.policies.find((p) => p.scope.kind === "device");
  // Unreachable for a child added in this app (`addChild` mints a device-scope
  // policy), but a restored backup from an older household might lack one.
  // Rows still show, but nothing opens until a device is set up.
  const summaries = ruleSummaries(
    devicePolicy ?? { id: "", scope: { kind: "device" } },
    Math.floor(Date.now() / 1000),
  );

  return (
    <div className="stack">
      {/* Honesty at the point of action: signed rules reach PAIRED devices
          within seconds; without one there is nothing to enforce them yet. */}
      {!hasLiveDevice(child) && (
        <Banner tone="info">
          <span>
            Nothing is enforcing these yet — they start working the moment{" "}
            {child.name}'s phone or computer is set up.{" "}
            <button
              type="button"
              className="btn btn-ghost"
              style={{ minHeight: 48, padding: "0 2px" }}
              onClick={() => {
                window.location.hash = `/${wardPath(child.id)}/add-device`;
              }}
            >
              Set up a device
            </button>
          </span>
        </Banner>
      )}
      {!devicePolicy && (
        <Banner tone="warn">
          {child.name} has no charter to edit yet. Setting up a device creates one.
        </Banner>
      )}
      {GROUPS.map((g) => (
        <section key={g.title} aria-label={g.title}>
          <h3 className="rule-group-title">{g.title}</h3>
          <div className="list">
            {g.sections.map((id) => {
              const body = (
                <>
                  <span className="row-main">
                    <span className="row-title">{SECTION_TITLE[id]}</span>
                    <br />
                    <span className="row-sub">{summaries[id]}</span>
                  </span>
                  {devicePolicy && (
                    <span className="row-chevron" aria-hidden="true">
                      ›
                    </span>
                  )}
                </>
              );
              return devicePolicy ? (
                <button
                  key={id}
                  type="button"
                  className="row"
                  onClick={() => {
                    window.location.hash = `/${wardPath(child.id)}/rule/${id}`;
                  }}
                >
                  {body}
                </button>
              ) : (
                <div key={id} className="row">
                  {body}
                </div>
              );
            })}
          </div>
        </section>
      ))}
    </div>
  );
}

/** Where a finished or cancelled editor goes when the shell gives it nowhere
 *  better: the child's settings, where the rules are. */
function toChildSettings(childId: string): void {
  window.location.hash = `/${wardSettingsPath(childId)}`;
}

/** The full-screen editor for one rule (`#/ward/<id>/rule/<section>`). The
 *  title and back arrow come from the app shell. */
export default function RuleEditor({
  childId,
  section,
  onDone,
}: {
  childId: string;
  section: SectionId;
  /** Leave the page (back to the child's settings). Defaults to those. */
  onDone?: () => void;
}) {
  const { state } = useCharter();
  const child = state.children.find((c) => c.id === childId);
  if (!child) return null;
  const policy = child.policies.find((p) => p.scope.kind === "device");
  if (!policy) {
    return (
      <Banner tone="warn">
        {child.name} has no charter to edit yet. Setting up a device creates one.
      </Banner>
    );
  }
  return (
    <DeviceLimits
      key={`${child.id}:${section}`}
      child={child}
      policy={policy}
      only={section}
      onDone={onDone ?? (() => toChildSettings(child.id))}
    />
  );
}

/** One singled-out game or app (`#/ward/<id>/rule/app/<policyId>`): block it or
 *  hold it to set hours, with Remove in the body. */
export function AppRulePage({
  childId,
  policyId,
  onDone,
}: {
  childId: string;
  policyId: string;
  onDone?: () => void;
}) {
  const { state } = useCharter();
  const child = state.children.find((c) => c.id === childId);
  if (!child) return null;
  const done = onDone ?? (() => toChildSettings(child.id));
  const policy = child.policies.find((p) => p.id === policyId && p.scope.kind === "app");
  if (!policy) {
    return (
      <Card>
        <p className="card-sub" style={{ marginBottom: 14 }}>
          That game or app isn’t singled out any more.
        </p>
        <Button block onClick={done}>
          Back
        </Button>
      </Card>
    );
  }
  return <AppRuleEditor key={policy.id} child={child} policy={policy} onDone={done} />;
}
