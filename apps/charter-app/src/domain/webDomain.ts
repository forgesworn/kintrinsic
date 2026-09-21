// What a parent types into "Blocked sites" → the bare domain the ward's DNS
// filter can actually match. The lists are passed through VERBATIM to the DNS
// plan (`charter-webpolicy` `DnsFilterPlan`), where a rule only ever matches a
// hostname — so `https://www.youtube.com/watch`, `*.youtube.com` or `youtube`
// used to be accepted, signed, shown back as a working chip, and block nothing.

export type WebDomainResult = { ok: true; domain: string } | { ok: false; message: string };

const LABEL = /^[a-z0-9]([a-z0-9-]{0,61}[a-z0-9])?$/;

export function normalizeWebDomain(input: string): WebDomainResult {
  let s = input.trim().toLowerCase();
  if (!s) return { ok: false, message: "Type a site, like youtube.com." };
  // A pasted address: keep only the host.
  s = s.replace(/^[a-z][a-z0-9+.-]*:\/\//, "");
  s = s.replace(/^[^/?#]*@/, ""); // user:pass@
  s = s.split(/[/?#]/, 1)[0];
  s = s.replace(/^(\*\.)+/, "").replace(/^\.+/, ""); // "*.site.com" means the site
  s = s.replace(/:\d+$/, "").replace(/\.+$/, "");
  // Non-ASCII names go to the punycode form a DNS query actually carries.
  if ([...s].some((c) => c.charCodeAt(0) > 127)) {
    try {
      s = new URL(`http://${s}`).hostname;
    } catch {
      return { ok: false, message: "That doesn't look like a site address." };
    }
  }
  const labels = s.split(".");
  if (labels.length < 2) {
    return { ok: false, message: `Add the ending too — like ${s || "youtube"}.com.` };
  }
  if (s.length > 253 || !labels.every((l) => LABEL.test(l))) {
    return { ok: false, message: "That doesn't look like a site address — try something like youtube.com." };
  }
  if (/^\d+$/.test(labels[labels.length - 1])) {
    return { ok: false, message: "Use the site's name, not a number address — the filter works on names." };
  }
  // A rule on `youtube.com` already covers `www.youtube.com`; the reverse is
  // not true, and the parent means the site. Never strip down to a bare ending.
  if (labels[0] === "www" && labels.length > 2) s = labels.slice(1).join(".");
  return { ok: true, domain: s };
}
