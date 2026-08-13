import type { LearningAppSel } from "../domain/types";

/**
 * Curated learning apps with VERIFIED domain closures. The closure is the set
 * of registrable domains the on-device app window is resolver-pinned to —
 * anything outside it simply does not load in that window, so every entry
 * must be measured against the live site, never guessed.
 *
 * Khan Academy — measured 2026-07-17 with a real browser on the homepage AND
 * a playing lesson video (network capture):
 * - khanacademy.org        the site itself
 * - kastatic.org           Khan's static CDN (cdn.kastatic.org observed)
 * - kasandbox.org          exercise/sandboxed content (per Khan's own school
 *                          network allowlist doc; not exercised logged-out)
 * - youtube-nocookie.com   lesson videos are embedded via this player host
 *                          (observed: www.youtube-nocookie.com/embed/…)
 * - ytimg.com              player thumbnails/assets (i.ytimg.com observed)
 * - googlevideo.com        the actual video streams (rr*—*.googlevideo.com
 *                          observed while playing)
 * DELIBERATELY EXCLUDED: www.youtube.com — the embed's "Watch on YouTube"
 * escape link dies at the resolver, closing the walk-out-to-YouTube hole.
 * Also excluded: analytics/consent/marketing hosts (googletagmanager,
 * cookielaw/onetrust, apple marketing) — not needed to function, and a
 * child's pinned window is better without them.
 * Sign-in note: use a Khan username/password account — third-party SSO
 * (Google) is outside the pin by design.
 *
 * Wikipedia — measured 2026-08-02 on an image-heavy article
 * (en.wikipedia.org/wiki/Photosynthesis, ~30 images):
 * - wikipedia.org          the encyclopedia itself (all languages)
 * - wikimedia.org          EVERY image comes from upload.wikimedia.org, and
 *                          the login check from auth.wikimedia.org
 * This is the entry that best shows why closures are measured, not guessed:
 * pin Wikipedia to wikipedia.org alone and you get the articles with no
 * pictures at all, because the media lives on a different registrable domain.
 *
 * BBC Bitesize — measured 2026-08-02 on the homepage and an article
 * (bbc.co.uk/bitesize/articles/z24rfdm):
 * - bbc.co.uk              the site, its APIs (bag.api, idcta.api) and the
 *                          embedded media player (emp.bbc.co.uk)
 * - bbci.co.uk             all static assets and images (ichef.bbci.co.uk,
 *                          static.files.bbci.co.uk, nav.files.bbci.co.uk)
 * NOT EXERCISED: video clip playback. The player host is inside bbc.co.uk but
 * the streams themselves were never fetched in this measurement, so a clip may
 * fail to play until someone reports it and the stream host is measured. Under-
 * broad on purpose — that is the safe direction, and it is a catalogue edit to
 * fix, not a code change.
 *
 * Duolingo — measured 2026-08-02 on the logged-out home/learn page:
 * - duolingo.com           the app (www, plus its excess/zombie API hosts)
 * - d35aaqx5ub95lt.cloudfront.net
 *                          EVERY image, icon and audio asset. Note this is a
 *                          bare CloudFront distribution id, pinned as the exact
 *                          host — `cloudfront.net` must NEVER be pinned as a
 *                          registrable domain, since that would open every
 *                          CloudFront-hosted site on the internet. The flip
 *                          side is fragility: if Duolingo ever moves
 *                          distribution, the app goes blank and this entry
 *                          needs re-measuring.
 * - gstatic.com            fonts
 * - recaptcha.net          the sign-in captcha — without it you cannot log in
 *                          with an email account at all
 * DELIBERATELY EXCLUDED, as with Khan: accounts.google.com (SSO is outside the
 * pin by design — use an email account), and the analytics/consent hosts
 * (googletagmanager, cookielaw/onetrust).
 */
export const LEARNING_CATALOGUE: LearningAppSel[] = [
  {
    id: "khan-academy",
    label: "Khan Academy",
    kind: "site",
    url: "https://www.khanacademy.org/",
    domains: [
      "khanacademy.org",
      "kastatic.org",
      "kasandbox.org",
      "youtube-nocookie.com",
      "ytimg.com",
      "googlevideo.com",
    ],
  },
  {
    id: "wikipedia",
    label: "Wikipedia",
    kind: "site",
    url: "https://www.wikipedia.org/",
    domains: ["wikipedia.org", "wikimedia.org"],
  },
  {
    id: "bbc-bitesize",
    label: "BBC Bitesize",
    kind: "site",
    url: "https://www.bbc.co.uk/bitesize",
    domains: ["bbc.co.uk", "bbci.co.uk"],
  },
  {
    id: "duolingo",
    label: "Duolingo",
    kind: "site",
    url: "https://www.duolingo.com/learn",
    domains: [
      "duolingo.com",
      "d35aaqx5ub95lt.cloudfront.net",
      "gstatic.com",
      "recaptcha.net",
    ],
  },
];
