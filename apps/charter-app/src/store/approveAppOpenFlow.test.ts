import { describe, expect, it, vi } from "vitest";
import { runApproveAppOpenFlow } from "./approveAppOpenFlow";

/**
 * N2 (review round 2, 2026-08-03): the CLAUSE (the AppHold that actually
 * opens the app) must be signed and confirmed SENT before the Decision echo
 * (the courtesy "closes the ward's UI loop" signal) is ever published. The
 * original order was the reverse, which let a declined clause-sign confirm
 * slip through silently: the request still got marked approved and the
 * activity log still said "You let them open Minecraft" while no hold ever
 * landed and the app stayed blocked.
 */
describe("runApproveAppOpenFlow", () => {
  it("sends the Decision echo only AFTER the clause is confirmed sent, in that order", async () => {
    const calls: string[] = [];
    const saveHold = vi.fn(async () => {
      calls.push("saveHold");
      return { outcome: "sent" as const };
    });
    const sendDecision = vi.fn(async () => {
      calls.push("sendDecision");
      return true;
    });

    const result = await runApproveAppOpenFlow({ saveHold, sendDecision });

    expect(result).toEqual({ outcome: "approved" });
    expect(calls).toEqual(["saveHold", "sendDecision"]);
  });

  it("a CANCELLED clause sign publishes NO grant and refuses the whole approval", async () => {
    const saveHold = vi.fn(async () => ({ outcome: "cancelled" as const }));
    const sendDecision = vi.fn(async () => true);

    const result = await runApproveAppOpenFlow({ saveHold, sendDecision });

    expect(result).toEqual({
      outcome: "refused",
      message: "Nothing was changed — the hold wasn't signed.",
    });
    // The whole point: the Decision echo must NEVER be attempted once the
    // clause didn't send — a caller wiring `sendDecision` to the real
    // `signDecisionIfConnected` never publishes a grant in this branch.
    expect(sendDecision).not.toHaveBeenCalled();
  });

  it("an UNDELIVERED clause (no signer / no device) also refuses, with its own honest message", async () => {
    const saveHold = vi.fn(async () => ({ outcome: "undelivered" as const, why: "no-signer" as const }));
    const sendDecision = vi.fn(async () => true);

    const result = await runApproveAppOpenFlow({ saveHold, sendDecision });

    expect(result.outcome).toBe("refused");
    expect(sendDecision).not.toHaveBeenCalled();
  });

  it("a declined DECISION echo (the clause already sent) is reported separately — not a refusal", async () => {
    const saveHold = vi.fn(async () => ({ outcome: "sent" as const }));
    const sendDecision = vi.fn(async () => false);

    const result = await runApproveAppOpenFlow({ saveHold, sendDecision });

    // The hold DID land (the app can open) — this is not "nothing changed",
    // so it must not be reported as a refusal.
    expect(result).toEqual({ outcome: "decision-cancelled" });
  });
});

describe("runApproveAppOpenFlow: a hold queued for a device that isn't home", () => {
  it("still goes on to the Decision echo, because a signed, held hold WILL open the app — refusing would be untrue", async () => {
    const calls: string[] = [];
    const queued = {
      outcome: "queued" as const,
      message: "Not sent yet — it'll reach Robin's laptop when it's on the home network.",
      devices: ["Robin's laptop"],
    };
    const result = await runApproveAppOpenFlow({
      saveHold: async () => {
        calls.push("saveHold");
        return queued;
      },
      sendDecision: async () => {
        calls.push("sendDecision");
        return true;
      },
    });
    expect(result).toEqual({ outcome: "approved", holdQueued: queued });
    expect(calls).toEqual(["saveHold", "sendDecision"]);
  });

  it("a held hold whose Decision echo is then declined is still reported as the echo being declined", async () => {
    const result = await runApproveAppOpenFlow({
      saveHold: async () => ({ outcome: "queued" as const, message: "m" }),
      sendDecision: async () => false,
    });
    expect(result).toEqual({ outcome: "decision-cancelled" });
  });
});
