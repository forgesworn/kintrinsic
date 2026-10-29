import { describe, expect, it } from "vitest";
import { approvalTemplate, CHARTER_GUARDIAN_STATEMENT, GUARDIAN_PIN_SCRYPT } from "./sdk";

describe("guardian-keys SDK import", () => {
  it("resolves the reference builders from the monorepo source", () => {
    const t = approvalTemplate("ab".repeat(32), { issuedAt: 1 });
    expect(t.kind).toBe(CHARTER_GUARDIAN_STATEMENT);
    expect(GUARDIAN_PIN_SCRYPT.N).toBe(32768);
  });
});
