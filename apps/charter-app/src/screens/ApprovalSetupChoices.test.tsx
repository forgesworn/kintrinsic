import { act } from "react";
import { createRoot, type Root } from "react-dom/client";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import ApprovalSetupChoices from "./ApprovalSetupChoices";

describe("approval set-up choices", () => {
  let container: HTMLDivElement;
  let root: Root;
  const onLocal = vi.fn();
  const onSignet = vi.fn();
  const onHeartwood = vi.fn();

  function render(busy = false) {
    act(() => {
      root = createRoot(container);
      root.render(
        <ApprovalSetupChoices busy={busy} onLocal={onLocal} onSignet={onSignet} onHeartwood={onHeartwood} />,
      );
    });
  }
  const button = (label: string) =>
    [...container.querySelectorAll("button")].find((b) => b.textContent === label);

  beforeEach(() => {
    vi.clearAllMocks();
    container = document.createElement("div");
    document.body.append(container);
  });
  afterEach(() => {
    act(() => root.unmount());
    container.remove();
  });

  it("offers 'Set up on this phone' as the one visible, primary choice", () => {
    render();
    const local = button("Set up on this phone");
    expect(local).toBeDefined();
    expect(local!.closest("details")).toBeNull();
    expect(local!.className).toContain("btn-primary");
  });

  it("tucks Signet and Heartwood under a collapsed 'Other ways' section", () => {
    render();
    const details = container.querySelector("details")!;
    expect(details.open).toBe(false);
    expect(details.querySelector("summary")!.textContent).toBe(
      "Other ways (for people who already use Signet or Heartwood)",
    );
    expect(button("Set up with Signet")!.closest("details")).toBe(details);
    expect(button("Set up with Heartwood")!.closest("details")).toBe(details);
    // One plain sentence on each.
    expect(details.textContent).toMatch(/My Signet is a separate app/);
    expect(details.textContent).toMatch(/Heartwood keeps your guardian identity on a separate device,\s+reached through My Signet/);
  });

  it("keeps each button wired to its own action", () => {
    render();
    act(() => button("Set up on this phone")!.click());
    act(() => button("Set up with Signet")!.click());
    act(() => button("Set up with Heartwood")!.click());
    expect(onLocal).toHaveBeenCalledTimes(1);
    expect(onSignet).toHaveBeenCalledTimes(1);
    expect(onHeartwood).toHaveBeenCalledTimes(1);
  });

  it("disables every choice while busy", () => {
    render(true);
    for (const label of ["Set up on this phone", "Set up with Signet", "Set up with Heartwood"]) {
      expect(button(label)!.disabled).toBe(true);
    }
  });
});
