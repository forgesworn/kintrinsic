import type { CSSProperties } from "react";
import { Button, Card, SectionLabel } from "../components/ui";
import { isSetUp, useCharter, type CharterState } from "../store/store";

// The Home "getting started" card. Onboarding lives HERE, not Family — Family is
// for ongoing management. It shows only until the child's computer is connected,
// then it disappears and Home becomes the daily dashboard.

/** True once at least one child has a connected computer — the moment the
 *  getting-started has done its job and Home flips to the dashboard. */
export function isSetUpEnough(state: CharterState): boolean {
  return state.children.some((c) => isSetUp(c));
}

function nav(to: string): void {
  window.location.hash = `/${to}`;
}

/** Bring the Family "add a child" area into view after navigating there. */
function scrollToWhenReady(anchorId: string, tries = 24): void {
  const el = document.getElementById(anchorId);
  if (el) {
    el.scrollIntoView({ behavior: "smooth", block: "start" });
    return;
  }
  if (tries > 0) requestAnimationFrame(() => scrollToWhenReady(anchorId, tries - 1));
}
function goToFamilyChildren(): void {
  nav("family");
  scrollToWhenReady("setup-children");
}

function StepRow({
  done,
  n,
  title,
  detail,
  action,
}: {
  done: boolean;
  n: number;
  title: string;
  detail: string;
  action: { label: string; onClick: () => void };
}) {
  return (
    <li style={rowStyle}>
      <span aria-hidden="true" style={markerStyle(done)}>
        {done ? "✓" : n}
      </span>
      <span style={{ flex: 1, minWidth: 0 }}>
        <span style={{ display: "block", fontWeight: 700, color: done ? "var(--text)" : "var(--text)" }}>
          {title}
          {done && <span style={srOnly}> — done</span>}
        </span>
        <span className="card-sub" style={{ display: "block", marginTop: 2 }}>
          {detail}
        </span>
        {!done && (
          <span style={{ display: "block", marginTop: 10 }}>
            <Button variant="secondary" onClick={action.onClick}>
              {action.label}
            </Button>
          </span>
        )}
      </span>
    </li>
  );
}

export default function Onboarding() {
  const { state } = useCharter();
  const hasChild = state.children.length > 0;
  const connected = isSetUpEnough(state);

  // Done: once a computer is connected, Home becomes the dashboard.
  if (connected) return null;

  const done = (hasChild ? 1 : 0) + (connected ? 1 : 0);

  return (
    <Card tinted style={{ marginBottom: 18 }}>
      <div className="row-between" style={{ alignItems: "baseline" }}>
        <h2 className="card-title" style={{ margin: 0 }}>
          Welcome to Kintrinsic
        </h2>
        <span className="card-sub" aria-hidden="true">
          {done} of 2
        </span>
      </div>
      <p className="card-sub" style={{ marginTop: 4, marginBottom: 14 }}>
        Kintrinsic is two parts: <b>Kintrinsic on your child's computer</b> — that's what
        actually enforces the limits — and <b>this app</b>, your remote control
        (a preview for now). So you start on the computer.
      </p>

      {/* The real, working step: set it up on the computer. */}
      <div style={calloutStyle}>
        <span className="card-title" style={{ display: "block", marginBottom: 2 }}>
          Start on the child's computer
        </span>
        <span className="card-sub" style={{ display: "block", marginBottom: 12 }}>
          Install Kintrinsic on their Linux computer and set screen-time limits right
          there — that works today, no phone needed. The guide walks every step,
          and can be handed to ChatGPT or Claude to talk you through.
        </span>
        <Button onClick={() => nav("setup")}>Open the setup guide</Button>
      </div>

      <SectionLabel>Then, in the app</SectionLabel>
      <ol style={listStyle} aria-label="Getting started">
        <StepRow
          done={hasChild}
          n={1}
          title="Add your child"
          detail="Just a name and a color — there's no account to create for them."
          action={{ label: "Add a child", onClick: goToFamilyChildren }}
        />
        <StepRow
          done={connected}
          n={2}
          title="Connect their computer"
          detail="Once Kintrinsic is installed on their computer, it shows a short code — enter it here to link it (preview)."
          action={{ label: "Connect a computer", onClick: goToFamilyChildren }}
        />
      </ol>
    </Card>
  );
}

// ---------------------------------------------------------------------------
// Styles
// ---------------------------------------------------------------------------

const calloutStyle: CSSProperties = {
  border: "1px solid var(--line)",
  borderRadius: 12,
  padding: "14px 16px",
  background: "var(--surface)",
  marginBottom: 16,
};

const listStyle: CSSProperties = { margin: 0, padding: 0, listStyle: "none" };

const rowStyle: CSSProperties = {
  display: "flex",
  gap: 14,
  alignItems: "flex-start",
  paddingTop: 16,
  marginTop: 16,
  borderTop: "1px solid var(--line)",
};

function markerStyle(done: boolean): CSSProperties {
  return {
    flex: "0 0 auto",
    width: 28,
    height: 28,
    borderRadius: "50%",
    display: "inline-flex",
    alignItems: "center",
    justifyContent: "center",
    fontWeight: 700,
    fontSize: 14,
    marginTop: 1,
    background: done ? "var(--ok)" : "var(--surface)",
    color: done ? "#fff" : "var(--text-2)",
    boxShadow: done ? "none" : "inset 0 0 0 2px var(--line)",
  };
}

const srOnly: CSSProperties = {
  position: "absolute",
  width: 1,
  height: 1,
  padding: 0,
  margin: -1,
  overflow: "hidden",
  clip: "rect(0 0 0 0)",
  whiteSpace: "nowrap",
  border: 0,
};
