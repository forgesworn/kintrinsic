// Vitest setup. jsdom provides localStorage; nothing else needed yet.
// Kept as a seam for future @testing-library/jest-dom matchers.

// Tells React's `act` (from "react", React 18.3+) that this jsdom environment
// is a real test environment — without it, every `act(...)` call in a
// render-level test (see Approvals.dismiss.test.tsx) prints a spurious
// "not configured to support act" warning.
(globalThis as unknown as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true;

export {};
