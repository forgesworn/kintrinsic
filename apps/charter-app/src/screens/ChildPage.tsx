import { useMemo } from "react";
import { Card, SectionLabel } from "../components/ui";
import { ChildStatus } from "../components/ChildStatus";
import { RequestCard } from "../components/RequestCard";
import type { Child } from "../domain/types";
import { wardSettingsPath } from "../domain/wardRoute";
import { useCharter } from "../store/store";
import { ActivityEventRow, ChildWeek } from "./Activity";
import FamilyNotices from "./FamilyNotices";

// The child page (`#/ward/<id>`): everything about one child, as stacked
// sections with headings and no sub-tabs. Each section has an id so a deep link
// (`#/approvals/<id>`) can scroll to it.
//
//   family notices -> Now -> Waiting for you -> This week -> "{name}'s settings"
//
// The family-wide notices (getting started, key backup, an unclaimed phone, a
// removal against you, how this child's answers were delivered) come first and
// each shows only while it holds.
//
// The daily things only. Rules, devices, name and colour are on the child's
// settings page (`#/ward/<id>/settings`, see ChildSettings), reached from the
// ⚙ in the header or the row at the bottom.

/** How many recent events the week section lists before "All activity". */
const RECENT_EVENTS = 3;

export default function ChildPage({ child }: { child: Child }) {
  const { state } = useCharter();
  const now = Date.now();

  const pending = useMemo(
    () =>
      state.requests
        .filter((r) => r.childId === child.id && r.status === "pending")
        .sort((a, b) => b.createdAt - a.createdAt),
    [state.requests, child.id],
  );
  const recent = useMemo(
    () =>
      state.activity
        .filter((e) => e.childId === child.id)
        .sort((a, b) => b.ts - a.ts)
        .slice(0, RECENT_EVENTS),
    [state.activity, child.id],
  );

  return (
    <>
      <FamilyNotices childId={child.id} />

      <section id="now" className="scroll-anchor" aria-label="Now">
        <SectionLabel>Now</SectionLabel>
        <Card>
          <ChildStatus child={child} variant="page" />
        </Card>
      </section>

      {pending.length > 0 && (
        <section id="waiting" className="scroll-anchor" aria-label="Waiting for you">
          <SectionLabel>Waiting for you</SectionLabel>
          <div className="stack">
            {pending.map((req) => (
              <RequestCard key={req.id} request={req} child={child} />
            ))}
          </div>
        </section>
      )}

      <section id="week" className="scroll-anchor" aria-label="This week">
        <SectionLabel>This week</SectionLabel>
        <ChildWeek child={child} named={false} showAdjust={false} />
        {recent.length > 0 && (
          <ul className="list" style={{ listStyle: "none", margin: 0, padding: 0 }}>
            {recent.map((ev) => (
              <ActivityEventRow key={ev.id} ev={ev} now={now} />
            ))}
          </ul>
        )}
        <button
          type="button"
          className="home-link"
          onClick={() => {
            window.location.hash = `/activity/${encodeURIComponent(child.id)}`;
          }}
        >
          All activity <span aria-hidden="true">›</span>
        </button>
      </section>

      <div className="list admin-list" style={{ marginTop: 20 }}>
        <button
          type="button"
          className="row"
          onClick={() => {
            window.location.hash = `/${wardSettingsPath(child.id)}`;
          }}
        >
          <span className="row-main">
            <span className="row-title">{child.name}’s settings</span>
            <span className="row-sub" style={{ display: "block" }}>
              Rules, devices, name
            </span>
          </span>
          <span className="row-chevron" aria-hidden="true">
            ›
          </span>
        </button>
      </div>
    </>
  );
}

