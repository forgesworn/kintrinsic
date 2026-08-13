/**
 * Turning a URL a guardian typed into the domain closure their child's
 * educational window will be pinned to.
 *
 * The curated catalogue's closures are MEASURED against the live site (see
 * `data/learning_catalogue.ts`). A site the guardian adds themselves cannot be,
 * so this has to derive one — and the direction it errs in is the whole design.
 *
 * # Why not the registrable domain
 *
 * The obvious rule is "pin the registrable domain": type
 * `https://www.bbc.co.uk/bitesize`, pin `bbc.co.uk`. Doing that correctly needs
 * a public suffix list, because the number of labels varies —
 * `bbc.co.uk` is registrable, `co.uk` is a public suffix, and telling them
 * apart is not something you can do by counting dots. Get it wrong and you pin
 * `co.uk`, opening every commercial site in Britain inside a window that is
 * supposed to be one educational site.
 *
 * The PWA has no public-suffix dependency and its dependency list is
 * deliberately lean, so instead:
 *
 * **Pin the hostname they typed (minus a leading `www.`), and its subdomains.**
 *
 * | Typed                            | Pinned                              |
 * |----------------------------------|-------------------------------------|
 * | `https://www.khanacademy.org/`   | `khanacademy.org` (+ subdomains)    |
 * | `https://en.wikipedia.org/`      | `en.wikipedia.org` (+ subdomains)   |
 * | `https://bbc.co.uk/bitesize`     | `bbc.co.uk` (+ subdomains)          |
 *
 * This needs no list of any kind and **cannot be over-broad beyond the single
 * site the guardian named**. It is under-broad more often than the registrable
 * rule would be — a site whose images live on a different domain renders
 * without them, which is exactly what hand-typed Wikipedia does (its pictures
 * are all on `wikimedia.org`). That is the safe direction to fail in, it is
 * what the curated catalogue exists to fix, and the UI says so plainly.
 */

/**
 * Multi-part public suffixes, used ONLY as a safety net on the derived result —
 * never to derive it. Stripping a leading `www.` from an absurd input like
 * `www.co.uk` would otherwise pin a whole suffix. This list can only ever
 * REJECT a domain, so an entry missing from it can never widen a pin; the worst
 * case is a nonsense input being accepted, which is the guardian's own site.
 */
const PUBLIC_SUFFIXES = new Set([
  "co.uk",
  "org.uk",
  "ac.uk",
  "gov.uk",
  "sch.uk",
  "net.uk",
  "com.au",
  "edu.au",
  "net.au",
  "org.au",
  "co.nz",
  "ac.nz",
  "co.jp",
  "ne.jp",
  "or.jp",
  "co.za",
  "com.br",
  "com.mx",
  "co.in",
  "com.sg",
]);

export type SiteClosureError =
  | "empty"
  | "unparseable"
  | "not-a-hostname"
  | "public-suffix";

export type SiteClosureResult =
  | { ok: true; url: string; host: string; domains: string[] }
  | { ok: false; error: SiteClosureError };

/** Human-facing text for each rejection. */
export function siteClosureMessage(error: SiteClosureError): string {
  switch (error) {
    case "empty":
      return "Enter the web address of the site.";
    case "unparseable":
      return "That doesn’t look like a web address. Try something like www.example.org.";
    case "not-a-hostname":
      return "Use a site’s web address, not an IP address or a single word.";
    case "public-suffix":
      return "That address is too broad — it would open every site ending in it. Use the site’s own address.";
  }
}

/**
 * Derive the launch URL and pinned closure from what the guardian typed.
 *
 * Accepts input with or without a scheme (`bbc.co.uk/bitesize` works). The
 * returned `url` keeps their path — the window opens where they meant — while
 * `domains` is what the resolver is pinned to.
 */
export function siteClosureFor(input: string): SiteClosureResult {
  const raw = input.trim();
  if (!raw) return { ok: false, error: "empty" };

  // A bare `example.org/page` has no scheme; URL() would reject it.
  const withScheme = /^https?:\/\//i.test(raw) ? raw : `https://${raw}`;
  let parsed: URL;
  try {
    parsed = new URL(withScheme);
  } catch {
    return { ok: false, error: "unparseable" };
  }

  // A trailing dot is a legal FQDN and a distinct string to a resolver — the
  // same trailing-dot filter bypass the tethering proxy had to close.
  const host = parsed.hostname.toLowerCase().replace(/\.+$/, "");
  if (!host) return { ok: false, error: "unparseable" };

  // An IP literal or a single-label host is not a site we can pin meaningfully.
  const isIpv4 = /^\d{1,3}(\.\d{1,3}){3}$/.test(host);
  const isIpv6 = host.includes(":") || host.startsWith("[");
  if (isIpv4 || isIpv6 || !host.includes(".")) {
    return { ok: false, error: "not-a-hostname" };
  }
  if (host.split(".").some((l) => l === "")) {
    return { ok: false, error: "unparseable" };
  }

  // `www.` is the one label safe to drop: the pin covers subdomains, so pinning
  // the bare host reaches www AND the site's other subdomains, while pinning
  // `www.example.org` would miss `example.org` itself.
  const domain = host.startsWith("www.") ? host.slice(4) : host;

  if (!domain.includes(".") || PUBLIC_SUFFIXES.has(domain)) {
    return { ok: false, error: "public-suffix" };
  }

  return { ok: true, url: parsed.toString(), host, domains: [domain] };
}

/**
 * A `[a-z0-9-]` slug for a guardian-typed label, unique against the ids already
 * in use. The device keys the launcher filename, the manifest and the window
 * class off this, so it must be stable and collision-free.
 */
export function siteIdFor(label: string, taken: string[]): string {
  const base =
    label
      .toLowerCase()
      .replace(/[^a-z0-9]+/g, "-")
      .replace(/^-+|-+$/g, "")
      .slice(0, 32) || "site";
  if (!taken.includes(base)) return base;
  for (let n = 2; ; n++) {
    const candidate = `${base}-${n}`;
    if (!taken.includes(candidate)) return candidate;
  }
}
