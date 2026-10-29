import { useEffect, useRef, type ReactNode } from "react";
import { Button } from "./ui";

// The one save model for a rule editor page: a bar pinned to the bottom edge
// with Cancel and Save, and a guard that asks before unsaved changes are lost.

/**
 * The page-level guard: while a rule editor is mounted it can veto a
 * navigation. App asks `confirmLeave` before it changes route, so the header
 * back arrow, Android's back button and any link on the page all pass through
 * the same question.
 */
let guard: (() => boolean) | null = null;

/** True when it is fine to leave the current page (no guard, nothing unsaved,
 *  or the guardian agreed to drop it). */
export function confirmLeave(): boolean {
  return guard ? guard() : true;
}

/**
 * Register the unsaved-changes guard for this page. Returns `allowLeaving`,
 * which a Save or Cancel that is about to navigate calls first so it is not
 * asked about changes it has just saved or deliberately dropped.
 */
export function useLeaveGuard(dirty: boolean, message: string): { allowLeaving: () => void } {
  const state = useRef({ dirty, message, bypass: false });
  state.current.dirty = dirty;
  state.current.message = message;
  const allowLeaving = useRef(() => {
    state.current.bypass = true;
  });
  useEffect(() => {
    const g = () =>
      state.current.bypass || !state.current.dirty || window.confirm(state.current.message);
    guard = g;
    return () => {
      if (guard === g) guard = null;
    };
  }, []);
  return { allowLeaving: allowLeaving.current };
}

export function RuleSaveBar({
  dirty,
  saving,
  blocked,
  onSave,
  onCancel,
  note,
}: {
  dirty: boolean;
  saving: boolean;
  /** A blocking validation error: Save stays disabled until it is fixed. */
  blocked?: boolean;
  onSave: () => void;
  onCancel: () => void;
  /** Sits just above the buttons (the "you'll confirm these" line). */
  note?: ReactNode;
}) {
  return (
    <div className="rule-bar" role="group" aria-label="Save changes">
      {note && <div className="rule-bar-note">{note}</div>}
      <div className="rule-bar-actions">
        <Button variant="secondary" onClick={onCancel} disabled={saving}>
          Cancel
        </Button>
        <Button variant="primary" onClick={onSave} disabled={!dirty || saving || !!blocked}>
          {saving ? "Saving…" : "Save"}
        </Button>
      </div>
    </div>
  );
}
