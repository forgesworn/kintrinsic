import { useMemo } from "react";
import type { Child, ChildRequest } from "../domain/types";
import { EmptyState } from "../components/ui";
import { ParentApprovalNote, RequestCard } from "../components/RequestCard";
import { WardHeading } from "../components/wards";
import { useCharter } from "../store/store";
import AnsweredDelivery from "./AnsweredDelivery";
import { wardPath } from "../domain/wardRoute";

// =============================================================================
// Approvals — every pending ask, grouped by child, newest first.
//
// No route mounts this any more: asks are answered from Home's "Waiting for
// you" and from the child page, both built from the same RequestCard. It stays
// as the all-children list those two are slices of, until the old screens are
// removed (design doc, build plan step 4).
// =============================================================================

export default function Approvals() {
  const { state } = useCharter();

  // Pending requests grouped by child, each group newest-first, and the
  // groups themselves ordered by their most recent request.
  const groups = useMemo(() => {
    const pending = state.requests
      .filter((r) => r.status === "pending")
      .sort((a, b) => b.createdAt - a.createdAt);

    const byChild = new Map<string, ChildRequest[]>();
    for (const r of pending) {
      const list = byChild.get(r.childId) ?? [];
      list.push(r);
      byChild.set(r.childId, list);
    }

    const childById = new Map(state.children.map((c) => [c.id, c]));
    return Array.from(byChild.entries())
      .map(([childId, requests]) => ({
        child: childById.get(childId),
        requests,
      }))
      .filter(
        (g): g is { child: Child; requests: ChildRequest[] } =>
          g.child !== undefined,
      );
  }, [state.requests, state.children]);

  if (groups.length === 0) {
    return (
      <div className="stack">
        <AnsweredDelivery />
        <EmptyState emoji="🎉" title="All caught up">
          No requests right now. We'll let you know the moment someone asks.
        </EmptyState>
      </div>
    );
  }

  return (
    <div className="stack">
      <AnsweredDelivery />
      <ParentApprovalNote />

      {groups.map(({ child, requests }) => (
        <section key={child.id} aria-label={`Requests from ${child.name}`}>
          <WardHeading
            child={child}
            onOpen={() => {
              window.location.hash = `/${wardPath(child.id)}`;
            }}
            openLabel={`${child.name}'s page`}
          />
          <div className="stack">
            {requests.map((req) => (
              <RequestCard key={req.id} request={req} child={child} />
            ))}
          </div>
        </section>
      ))}
    </div>
  );
}
