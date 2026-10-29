import { act } from "react";
import { createRoot, type Root } from "react-dom/client";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import GuardianPin from "./GuardianPin";
import { familyWards } from "./guardiansView";
import { PHONE, button, device, family, mockStore, typeInto } from "./guardiansTestKit";

const store = vi.hoisted(() => ({ current: null as unknown }));
vi.mock("../store/store", () => ({ useCharter: () => store.current }));

describe("guardian PIN", () => {
  let container: HTMLDivElement;
  let root: Root;
  let s: ReturnType<typeof mockStore>;
  const onDone = vi.fn();

  beforeEach(() => {
    onDone.mockReset();
    const children = family([device("Robin's phone", PHONE, "android")]);
    s = mockStore({ children });
    store.current = s;
    container = document.createElement("div");
    document.body.append(container);
    root = createRoot(container);
    act(() => root.render(<GuardianPin wards={familyWards(children)} onDone={onDone} />));
  });
  afterEach(() => {
    act(() => root.unmount());
    container.remove();
  });

  function enter(pin: string, again: string) {
    const [a, b] = [...container.querySelectorAll("input")];
    act(() => typeInto(a, pin));
    act(() => typeInto(b, again));
  }

  it("says the child must not see it", () => {
    expect(container.textContent).toContain("Your child must not see it");
  });

  it("refuses a PIN that is too short", async () => {
    enter("1234", "1234");
    await act(async () => button(container, "Set guardian PIN")!.click());
    expect(container.querySelector('[role="alert"]')!.textContent).toBe("Choose a PIN of at least 6 digits.");
    expect(s.guardianKeys.setGuardianPin).not.toHaveBeenCalled();
  });

  it("refuses two PINs that don't match", async () => {
    enter("123456", "654321");
    await act(async () => button(container, "Set guardian PIN")!.click());
    expect(container.querySelector('[role="alert"]')!.textContent).toBe("The two PINs don't match.");
    expect(s.guardianKeys.setGuardianPin).not.toHaveBeenCalled();
  });

  it("sends a matching PIN and reports where it went", async () => {
    enter("123456", "123456");
    await act(async () => button(container, "Set guardian PIN")!.click());
    expect(s.guardianKeys.setGuardianPin).toHaveBeenCalledWith("123456");
    expect(container.textContent).toContain("Sent to Robin's phone.");
    await act(async () => button(container, "Continue")!.click());
    expect(onDone).toHaveBeenCalledOnce();
  });

  it("names a ward that needs updating instead of claiming success", async () => {
    s.guardianKeys.setGuardianPin.mockResolvedValueOnce({
      sent: [] as string[],
      held: [] as string[],
      failed: [{ to: PHONE, error: "Update Kintrinsic on Robin's phone first." }],
    });
    enter("123456", "123456");
    await act(async () => button(container, "Set guardian PIN")!.click());
    expect(container.textContent).toContain("The PIN didn't reach any phone");
    expect(container.textContent).toContain("Update Kintrinsic on Robin's phone first.");
    expect(button(container, "Continue")).toBeUndefined();
  });
});
