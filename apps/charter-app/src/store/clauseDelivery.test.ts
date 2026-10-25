import { describe, expect, it } from "vitest";
import {
  clauseDeliveryPrecheck,
  queuedSaveWarning,
  queuedSendLine,
  splitSentence,
  undeliveredNote,
  waitingAnswerNote,
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

describe("a change held for a device that is not on the home network yet", () => {
  const queued: ClauseDelivery = {
    outcome: "queued",
    message: "Not sent yet — it'll reach Robin's laptop when it's on the home network.",
  };

  it("is saved but plainly not sent, in the activity line", () => {
    expect(undeliveredNote(queued)).toBe(
      " — saved here. Not sent yet: it'll reach Robin's laptop when it's on the home network.",
    );
  });

  it("a sent change, with or without tracked wraps, adds no note", () => {
    expect(undeliveredNote({ outcome: "sent" })).toBe("");
    expect(undeliveredNote({ outcome: "sent", wraps: ["w1"] })).toBe("");
  });
});

describe("the words for a held change", () => {
  const all: ClauseDelivery = {
    outcome: "queued",
    message: "Not sent yet — it'll reach Robin's laptop when it's on the home network.",
    devices: ["Robin's laptop"],
  };
  const partial: ClauseDelivery = { ...all, reached: ["Mia's phone"] } as ClauseDelivery;

  it("a held rule save warns plainly, naming who it is waiting for", () => {
    expect(queuedSaveWarning(all)).toBe("Saved here. It'll reach Robin's laptop when it's on the home network.");
    expect(queuedSaveWarning({ ...all, devices: ["A", "B"] } as ClauseDelivery)).toBe(
      "Saved here. It'll reach A and B when they're on the home network.",
    );
    expect(queuedSaveWarning(partial)).toBe(
      "Saved here. It'll reach Robin's laptop when it's on the home network. Mia's phone already got it.",
    );
  });

  it("is silent for every outcome that is not a held change", () => {
    expect(queuedSaveWarning({ outcome: "sent" })).toBeNull();
    expect(queuedSaveWarning({ outcome: "cancelled" })).toBeNull();
    expect(queuedSaveWarning({ outcome: "undelivered", why: "no-signer" })).toBeNull();
  });

  it("a partial batch is written up as who got it and who is waiting", () => {
    expect(splitSentence(["Mia's phone"], ["Robin's laptop"])).toBe(
      "Reached Mia's phone. Robin's laptop will get it when it's on the home network.",
    );
    expect(splitSentence(["A", "B"], ["C", "D"])).toBe("Reached A and B. C and D will get it when they're on the home network.");
    expect(undeliveredNote(partial)).toBe(" — Reached Mia's phone. Robin's laptop will get it when it's on the home network.");
    expect(queuedSendLine(partial as Extract<ClauseDelivery, { outcome: "queued" }>)).toBe(
      "Reached Mia's phone. Robin's laptop will get it when it's on the home network.",
    );
  });

  it("an all-queued change keeps the plain 'not sent yet' sentence", () => {
    expect(queuedSendLine(all as Extract<ClauseDelivery, { outcome: "queued" }>)).toBe(
      "Not sent yet — it'll reach Robin's laptop when it's on the home network.",
    );
  });

  it("an answer the carrier is holding says it is waiting, not that it was sent", () => {
    expect(waitingAnswerNote(["Robin's laptop"])).toBe(" — waiting for Robin's laptop to join the home network");
  });
});
