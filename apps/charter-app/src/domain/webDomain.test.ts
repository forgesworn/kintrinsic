import { describe, expect, it } from "vitest";
import { normalizeWebDomain } from "./webDomain";
import { contentToGrant } from "../wire/clause";

const ok = (s: string) => {
  const r = normalizeWebDomain(s);
  return r.ok ? r.domain : `ERR: ${r.message}`;
};

describe("normalizeWebDomain", () => {
  it("keeps a plain domain", () => {
    expect(ok("youtube.com")).toBe("youtube.com");
    expect(ok("  News.BBC.co.uk ")).toBe("news.bbc.co.uk");
  });

  it("reduces a pasted address to the host the filter can match", () => {
    expect(ok("https://www.youtube.com/watch?v=abc#t=1")).toBe("youtube.com");
    expect(ok("http://user:pw@example.org:8080/path")).toBe("example.org");
    expect(ok("*.tiktok.com")).toBe("tiktok.com");
    expect(ok(".roblox.com.")).toBe("roblox.com");
    expect(ok("m.facebook.com/home")).toBe("m.facebook.com");
  });

  it("never strips www down to a bare ending", () => {
    expect(ok("www.com")).toBe("www.com");
  });

  it("carries a non-ASCII name as punycode", () => {
    expect(ok("bücher.de")).toBe("xn--bcher-kva.de");
  });

  it("refuses what could only ever enforce nothing", () => {
    for (const bad of ["youtube", "you tube.com", "-bad.com", "a..b.com", "192.168.1.1", "", "https://"]) {
      expect(normalizeWebDomain(bad).ok, bad).toBe(false);
    }
  });
});

describe("the content clause", () => {
  it("canonicalises entries saved before the editor vetted them, dropping none", () => {
    const g = contentToGrant(
      {
        enabled: true,
        posture: "blocklist",
        ageTier: "young",
        allow: ["Wikipedia.org", "https://www.wikipedia.org/wiki/Cat"],
        block: ["https://www.youtube.com/watch?v=1", "*.tiktok.com", "not a domain"],
        youtube: "off",
        safeSearch: true,
      },
      1_700_000_000,
    );
    expect(g.parentAllow).toEqual(["wikipedia.org"]);
    expect(g.parentDeny).toEqual(["youtube.com", "tiktok.com", "not a domain"]);
  });
});
