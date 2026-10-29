import { afterEach } from "vitest";
import { resetAskDecisionLock } from "../components/askDecisionLock";

// Vitest setup. jsdom provides localStorage; nothing else needed yet.
// Kept as a seam for future @testing-library/jest-dom matchers.

// Tells React's `act` (from "react", React 18.3+) that this jsdom environment
// is a real test environment — without it, every `act(...)` call in a
// render-level test (see Approvals.dismiss.test.tsx) prints a spurious
// "not configured to support act" warning.
(globalThis as unknown as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true;

// jsdom has no layout, so `window.scrollTo` is a "not implemented" stub that logs
// an error on every call. The app scrolls to the top on a route change; a
// no-op keeps test output clean.
if (typeof window !== "undefined") {
  window.scrollTo = () => {};
}

// The one-ask-decision-at-a-time lock is module state; a test that leaves a
// send pending (on purpose) must not hold it against the next test.
afterEach(() => {
  resetAskDecisionLock();
});

export {};
