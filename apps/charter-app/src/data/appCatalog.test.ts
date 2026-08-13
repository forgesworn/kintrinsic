import { describe, expect, it } from "vitest";
import { APP_CATALOG, catalogLookup } from "./appCatalog";

describe("appCatalog", () => {
  it("every entry has a well-formed 64-hex signer digest", () => {
    for (const app of APP_CATALOG) {
      expect(app.signerCertSha256, app.packageName).toMatch(/^[0-9a-f]{64}$/);
      expect(app.source).toBe("staged");
      expect(app.packageName).toMatch(/^[A-Za-z][A-Za-z0-9_]*(\.[A-Za-z][A-Za-z0-9_]*)+$/);
      expect(app.label.length).toBeGreaterThan(0);
    }
  });

  it("package names are unique (no ambiguous digest)", () => {
    const names = APP_CATALOG.map((a) => a.packageName);
    expect(new Set(names).size).toBe(names.length);
  });

  it("looks up every curated entry by its package name (mechanism)", () => {
    // Content-agnostic: whatever is (or isn't) in the catalog, lookup must
    // round-trip each real entry. Vacuously true while the catalog is empty.
    for (const app of APP_CATALOG) {
      expect(catalogLookup(app.packageName)).toBe(app);
    }
  });

  it("returns undefined for an uncurated package (no trusted digest)", () => {
    expect(catalogLookup("com.evil.trojan")).toBeUndefined();
    expect(catalogLookup("")).toBeUndefined();
  });
});
