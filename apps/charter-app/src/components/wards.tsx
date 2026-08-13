import type { ReactNode } from "react";
import { Avatar, Pill } from "./ui";
import type { Child } from "../domain/types";

// One way to choose a ward, and one way to title one.
//
// Before this there were three gestures for the same act — a brand-red
// segmented control (Limits), a scrolling strip of filter pills (Activity), and
// a list of cards (Home, Family) — and the ward's name, avatar and status pill
// were drawn five different ways across five screens. That is most of why every
// tab looked like the last one: the chrome varied and the content didn't.
//
// Unifying the chrome is what lets the BODIES differ. Limits can now be a
// settings list, Activity a timeline, Home a dashboard, and each still says
// "this is Robin" in the same voice at the top.

/**
 * The ward switcher: one segment per child, sticky beneath the app header.
 *
 * `allId` adds a leading "everyone" segment (Activity wants one; Limits, which
 * edits exactly one ward's rules, must not have one).
 */
export function WardTabs({
  wards,
  selected,
  onSelect,
  allId,
  allLabel = "Everyone",
  label = "Choose a child",
}: {
  wards: Child[];
  selected: string;
  onSelect: (id: string) => void;
  allId?: string;
  allLabel?: string;
  label?: string;
}) {
  const options: { id: string; name: string }[] = [
    ...(allId !== undefined ? [{ id: allId, name: allLabel }] : []),
    ...wards.map((c) => ({ id: c.id, name: c.name })),
  ];
  // One ward and no "everyone" segment is not a choice — drawing a switcher
  // with a single, permanently-selected button is pure furniture.
  if (options.length < 2) return null;

  return (
    <div role="group" aria-label={label} className="seg">
      {options.map((o) => (
        <button
          key={o.id}
          type="button"
          className="seg-btn"
          aria-pressed={o.id === selected}
          onClick={() => onSelect(o.id)}
        >
          {o.name}
        </button>
      ))}
    </div>
  );
}

/**
 * A ward's name at the head of a section: avatar, name, one status pill, and an
 * optional link onward.
 *
 * One pill, not two. A screen that shows both "Allowed now" and "2 devices
 * connected" is answering a question nobody asked twice over — each screen
 * passes the ONE fact that matters where it is.
 */
export function WardHeading({
  child,
  status,
  tone = "neutral",
  onOpen,
  openLabel,
  as = "h2",
}: {
  child: Child;
  status?: ReactNode;
  tone?: "ok" | "warn" | "blocked" | "neutral";
  onOpen?: () => void;
  openLabel?: string;
  as?: "h2" | "div";
}) {
  const Title = as;
  return (
    <div className="ward-heading">
      <Avatar name={child.name} color={child.color} size={30} />
      <Title className="ward-heading-name">{child.name}</Title>
      {status != null && <Pill tone={tone}>{status}</Pill>}
      {onOpen && openLabel && (
        <button type="button" className="ward-heading-link" onClick={onOpen}>
          {openLabel} <span aria-hidden="true">›</span>
        </button>
      )}
    </div>
  );
}
