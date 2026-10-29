import type { CSSProperties } from "react";

import { CHILD_COLORS } from "./childColors";

/** The ward's colour spot. Shared by "add a child" and "edit a child" so the
 *  two can't drift into offering different palettes for the same field.
 *  `groupName` keeps each instance's radios their own group — several cards
 *  can be on screen at once, and a shared name would make picking a colour for
 *  one ward silently deselect another's. */
export function ColorChoice({
  value,
  onChange,
  groupName,
}: {
  value: string;
  onChange: (c: string) => void;
  groupName: string;
}) {
  return (
    <fieldset style={{ border: 0, margin: 0, padding: 0 }} className="field">
      <legend className="field-label" style={{ padding: 0 }}>
        Colour
      </legend>
      <div style={{ display: "flex", gap: 12, flexWrap: "wrap" }}>
        {CHILD_COLORS.map((c) => {
          const selected = c === value;
          return (
            <label
              key={c}
              className="color-option"
              aria-label={`Use this colour${selected ? " (selected)" : ""}`}
            >
              <input
                type="radio"
                name={groupName}
                className="color-input"
                value={c}
                checked={selected}
                onChange={() => onChange(c)}
                style={srOnlyStyle}
              />
              <span
                className="color-swatch"
                aria-hidden="true"
                style={{
                  display: "inline-block",
                  width: 32,
                  height: 32,
                  borderRadius: "50%",
                  background: c,
                  boxShadow: selected
                    ? "0 0 0 3px var(--surface), 0 0 0 5px var(--text)"
                    : "var(--shadow-1)",
                }}
              />
            </label>
          );
        })}
      </div>
    </fieldset>
  );
}

const srOnlyStyle: CSSProperties = {
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

