import { describe, expect, it } from "vitest";
import { LEARNING_CATALOGUE } from "./learning_catalogue";
import { learningToGrant } from "../wire/clause";

// Every catalogue entry must satisfy the clause's fail-closed validation
// (site ⇒ url + non-empty domains; slug ids) — a bad entry would be rejected
// by the warden and silently never enforce.
describe("learning catalogue", () => {
  it("entries are wire-valid site apps", () => {
    expect(LEARNING_CATALOGUE.length).toBeGreaterThan(0);
    for (const app of LEARNING_CATALOGUE) {
      expect(app.id).toMatch(/^[a-z0-9-]+$/);
      expect(app.label.trim().length).toBeGreaterThan(0);
      if (app.kind === "site") {
        expect(app.url ?? "").toMatch(/^https:\/\//);
        expect(app.domains?.length ?? 0).toBeGreaterThan(0);
        for (const d of app.domains ?? []) {
          // Registrable domains only — no scheme, no path, no wildcard
          // (the enactor adds subdomain coverage itself).
          expect(d).toMatch(/^[a-z0-9.-]+\.[a-z]+$/);
          expect(d).not.toMatch(/[/*:]/);
        }
      }
    }
  });

  it("khan academy's pin excludes youtube.com (the walk-out hole)", () => {
    const khan = LEARNING_CATALOGUE.find((a) => a.id === "khan-academy");
    expect(khan).toBeDefined();
    expect(khan?.domains).toContain("youtube-nocookie.com");
    expect(khan?.domains).toContain("googlevideo.com");
    expect(khan?.domains).not.toContain("youtube.com");
  });

  it("ids are unique — the device keys launchers and profiles off them", () => {
    const ids = LEARNING_CATALOGUE.map((a) => a.id);
    expect(new Set(ids).size).toBe(ids.length);
  });

  /**
   * A domain is pinned as `EXCLUDE <d>, EXCLUDE *.<d>`, so pinning a public
   * suffix opens every site beneath it. `cloudfront.net` is the live example:
   * Duolingo's assets sit on a bare distribution id, and pinning the suffix
   * rather than that exact host would open every CloudFront-hosted site there
   * is. Wikipedia's `wikimedia.org` is the opposite lesson — leave it out and
   * every article renders with no pictures at all.
   */
  it("pins no public suffix, and keeps the closures whose absence breaks a site", () => {
    const suffixes = ["co.uk", "org.uk", "ac.uk", "com.au", "cloudfront.net"];
    for (const app of LEARNING_CATALOGUE) {
      for (const d of app.domains ?? []) {
        expect(suffixes, `${app.id} pins the suffix ${d}`).not.toContain(d);
      }
    }
    expect(
      LEARNING_CATALOGUE.find((a) => a.id === "wikipedia")?.domains,
    ).toContain("wikimedia.org");
    expect(
      LEARNING_CATALOGUE.find((a) => a.id === "bbc-bitesize")?.domains,
    ).toContain("bbci.co.uk");
  });

  it("a catalogue selection produces a valid clause body", () => {
    const g = learningToGrant(
      { enabled: true, apps: LEARNING_CATALOGUE },
      1782734400,
    );
    expect(g.v).toBe(1);
    expect(g.paused).toBeUndefined();
    expect(g.apps.length).toBe(LEARNING_CATALOGUE.length);
  });
});
