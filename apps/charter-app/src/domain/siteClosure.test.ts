import { describe, expect, it } from "vitest";
import { siteClosureFor, siteIdFor } from "./siteClosure";

/** The domains a successful derivation pins, or null if it was rejected. */
function pinned(input: string): string[] | null {
  const r = siteClosureFor(input);
  return r.ok ? r.domains : null;
}

describe("siteClosureFor", () => {
  it("pins the typed host and its subdomains", () => {
    expect(pinned("https://en.wikipedia.org/")).toEqual(["en.wikipedia.org"]);
    expect(pinned("https://khanacademy.org")).toEqual(["khanacademy.org"]);
  });

  it("drops a leading www so the bare site and its other subdomains are reached", () => {
    // Pinning `www.wikipedia.org` would miss `en.wikipedia.org` AND the bare
    // domain; pinning `wikipedia.org` reaches www through the subdomain rule.
    expect(pinned("https://www.wikipedia.org/")).toEqual(["wikipedia.org"]);
    expect(pinned("https://www.bbc.co.uk/bitesize")).toEqual(["bbc.co.uk"]);
  });

  it("accepts an address with no scheme, the way a guardian types one", () => {
    expect(pinned("bbc.co.uk/bitesize")).toEqual(["bbc.co.uk"]);
    expect(pinned("  www.khanacademy.org  ")).toEqual(["khanacademy.org"]);
  });

  it("keeps the path so the window opens where they meant", () => {
    const r = siteClosureFor("bbc.co.uk/bitesize");
    expect(r.ok && r.url).toBe("https://bbc.co.uk/bitesize");
  });

  // ---- the hole this rule exists to avoid -----------------------------

  /**
   * A registrable-domain rule needs a public suffix list to know that
   * `bbc.co.uk` is a site and `co.uk` is a suffix. Getting it wrong opens
   * every commercial site in Britain inside a window meant for one. This rule
   * never derives a suffix in the first place.
   */
  it("never widens a multi-part domain to its public suffix", () => {
    expect(pinned("https://www.bbc.co.uk/bitesize")).toEqual(["bbc.co.uk"]);
    expect(pinned("https://someschool.sch.uk")).toEqual(["someschool.sch.uk"]);
    expect(pinned("https://university.ac.uk")).toEqual(["university.ac.uk"]);
    expect(pinned("https://shop.com.au")).toEqual(["shop.com.au"]);
    for (const d of pinned("https://www.bbc.co.uk") ?? []) {
      expect(d).not.toBe("co.uk");
    }
  });

  it("never yields a bare TLD from a two-label host", () => {
    expect(pinned("https://example.org")).toEqual(["example.org"]);
    expect(pinned("https://www.example.org")).toEqual(["example.org"]);
  });

  /** The safety net: stripping `www.` must not leave a whole suffix pinned. */
  it("rejects an address that would pin a public suffix outright", () => {
    expect(siteClosureFor("https://www.co.uk")).toEqual({
      ok: false,
      error: "public-suffix",
    });
    expect(siteClosureFor("https://co.uk")).toEqual({
      ok: false,
      error: "public-suffix",
    });
  });

  // ---- rejections ------------------------------------------------------

  it("rejects empty, unparseable and non-hostname input", () => {
    expect(siteClosureFor("")).toEqual({ ok: false, error: "empty" });
    expect(siteClosureFor("   ")).toEqual({ ok: false, error: "empty" });
    expect(siteClosureFor("localhost")).toEqual({
      ok: false,
      error: "not-a-hostname",
    });
    expect(siteClosureFor("http://192.168.0.1/")).toEqual({
      ok: false,
      error: "not-a-hostname",
    });
    expect(siteClosureFor("not a url at all")).toEqual({
      ok: false,
      error: "unparseable",
    });
  });

  /**
   * A trailing dot is a legal FQDN and a DISTINCT string to a resolver — the
   * same trailing-dot bypass the tethering proxy had to close. It must
   * normalise, not sneak past as a separate domain.
   */
  it("normalises a trailing dot rather than pinning it separately", () => {
    expect(pinned("https://example.org./")).toEqual(["example.org"]);
    expect(pinned("https://www.example.org./")).toEqual(["example.org"]);
  });

  it("normalises case", () => {
    expect(pinned("https://WWW.Example.ORG/")).toEqual(["example.org"]);
  });
});

describe("siteIdFor", () => {
  it("slugifies a label to [a-z0-9-]", () => {
    expect(siteIdFor("BBC Bitesize", [])).toBe("bbc-bitesize");
    expect(siteIdFor("Sam's Maths!", [])).toBe("sam-s-maths");
  });

  it("never collides with an id already in use", () => {
    expect(siteIdFor("Wikipedia", ["wikipedia"])).toBe("wikipedia-2");
    expect(siteIdFor("Wikipedia", ["wikipedia", "wikipedia-2"])).toBe(
      "wikipedia-3",
    );
  });

  it("falls back rather than yielding an empty id", () => {
    // The id is the launcher filename and the window class on the device; an
    // empty one would produce `charter-learn-.desktop` and fail validation.
    expect(siteIdFor("!!!", [])).toBe("site");
    expect(siteIdFor("", [])).toBe("site");
  });
});

describe("siteIdFor against reserved ids", () => {
  it("steps past an id the caller reserves (the learning catalogue's)", () => {
    expect(siteIdFor("Wikipedia", [])).toBe("wikipedia");
    const id = siteIdFor("Wikipedia", ["wikipedia"]);
    expect(id).not.toBe("wikipedia");
    expect(id.startsWith("wikipedia")).toBe(true);
  });
});
