import { useId, type ReactNode } from "react";

// A collapsible settings row, and the underline tab strip that groups them.
//
// Limits was one card holding nine always-open editors — about 2,000px of
// controls with a single Save at the very bottom, so changing the daily limit
// meant scrolling past the lifeline editor to find it. Collapsed, the same nine
// become five short rows per tab that can be read at a glance: title, what it
// currently says, and whether you've changed it.

/**
 * The second level of tabs, inside a screen.
 *
 * Deliberately UNLIKE `WardTabs`: that one is a filled segmented control, this
 * one is an underline strip. Two identical strips stacked would read as one
 * confusing double row rather than as "whose rules" above "which rules".
 */
export function SubTabs<T extends string>({
  value,
  options,
  onChange,
  label,
  trailing,
}: {
  value: T;
  options: { id: T; label: string; count?: number }[];
  onChange: (id: T) => void;
  label: string;
  /** Rides the spare space at the right end of the strip. Limits puts the
   *  ward's live status there — it costs no vertical space, and it belongs
   *  beside the rules rather than in an identity block repeating the name the
   *  switcher above is already showing. */
  trailing?: ReactNode;
}) {
  return (
    <div className="subtabs-row">
      <div role="tablist" aria-label={label} className="subtabs">
        {options.map((o) => (
          <button
            key={o.id}
            type="button"
            role="tab"
            aria-selected={o.id === value}
            className="subtab"
            onClick={() => onChange(o.id)}
          >
            {o.label}
            {o.count ? <span className="subtab-dot" aria-hidden="true" /> : null}
          </button>
        ))}
      </div>
      {trailing && <div className="subtabs-trailing">{trailing}</div>}
    </div>
  );
}

/**
 * One collapsible setting.
 *
 * `changed` shows the unsaved-change dot. A changed section is opened by its
 * caller rather than closed away — an edit hidden behind a shut row is an edit
 * a guardian cannot find to undo, and it would still be signed by Save.
 */
export function Section({
  title,
  summary,
  changed,
  open,
  onToggle,
  children,
}: {
  title: string;
  summary: ReactNode;
  changed?: boolean;
  open: boolean;
  onToggle: () => void;
  children: ReactNode;
}) {
  const bodyId = useId();
  return (
    <div className={`acc ${open ? "acc-open" : ""}`}>
      <button
        type="button"
        className="acc-head"
        aria-expanded={open}
        aria-controls={bodyId}
        onClick={onToggle}
      >
        <span className="acc-main">
          <span className="acc-title">
            {title}
            {changed && (
              <>
                <span className="acc-dot" aria-hidden="true" />
                <span className="sr-only"> — changed, not saved yet</span>
              </>
            )}
          </span>
          <span className="acc-summary">{summary}</span>
        </span>
        <span className="acc-chevron" aria-hidden="true">
          ›
        </span>
      </button>
      {open && (
        <div className="acc-body" id={bodyId}>
          {children}
        </div>
      )}
    </div>
  );
}

/** The container that makes a run of Sections read as one settings list. */
export function SectionList({ children }: { children: ReactNode }) {
  return <div className="acc-list">{children}</div>;
}
