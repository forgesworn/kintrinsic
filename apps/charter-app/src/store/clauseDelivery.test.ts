import { describe, expect, it } from "vitest";
import {
  clauseDeliveryPrecheck,
  undeliveredNote,
  type ClauseDelivery,
} from "./clauseDelivery";

describe("clauseDeliveryPrecheck", () => {
  it("lets a change through when a signer is connected and a device can receive it", () => {
    expect(clauseDeliveryPrecheck(true, 1)).toBeNull();
  });

  it("refuses before signing when no signer is connected", () => {
    expect(clauseDeliveryPrecheck(false, 1)).toEqual({
      outcome: "undelivered",
      why: "no-signer",
    });
  });

  // The case that cost a real ward her rules: the phone was paired on its own
  // side, polling and heartbeating happily, but absent from the guardian's
  // device record — so the publish loop had no recipient and the clause was
  // never built. Zero deliverable devices must be caught, not signed into a void.
  it("refuses when the ward has no device the guardian can reach", () => {
    expect(clauseDeliveryPrecheck(true, 0)).toEqual({
      outcome: "undelivered",
      why: "no-device",
    });
  });

  // No signer AND no device: report the signer, since connecting is the first
  // move either way and two problems at once is not a useful thing to be told.
  it("names the missing signer first when both are missing", () => {
    expect(clauseDeliveryPrecheck(false, 0)).toEqual({
      outcome: "undelivered",
      why: "no-signer",
    });
  });
});

describe("undeliveredNote", () => {
  it("adds nothing when the change actually travelled", () => {
    expect(undeliveredNote({ outcome: "sent" })).toBe("");
  });

  it("adds nothing for a cancelled change (nothing was claimed)", () => {
    expect(undeliveredNote({ outcome: "cancelled" })).toBe("");
  });

  it("never lets a saved-but-unsent rule read as done", () => {
    for (const why of ["no-signer", "no-device"] as const) {
      const note = undeliveredNote({ outcome: "undelivered", why });
      expect(note).not.toBe("");
      // Says it was kept, and says it has not gone anywhere.
      expect(note).toMatch(/saved here/i);
      expect(note).toMatch(/not sent|hasn’t been sent/i);
    }
  });

  it("points at the remedy that matches the cause", () => {
    expect(undeliveredNote({ outcome: "undelivered", why: "no-signer" })).toMatch(
      /parent approval/i,
    );
    expect(undeliveredNote({ outcome: "undelivered", why: "no-device" })).toMatch(
      /phone isn’t set up/i,
    );
  });

  it("accepts every ClauseDelivery shape without throwing", () => {
    const all: ClauseDelivery[] = [
      { outcome: "sent" },
      { outcome: "cancelled" },
      { outcome: "undelivered", why: "no-signer" },
      { outcome: "undelivered", why: "no-device" },
    ];
    for (const d of all) expect(typeof undeliveredNote(d)).toBe("string");
  });
});
