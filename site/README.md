# site/ — kintrinsic.app, Kintrinsic's front door

**This directory is the marketing site**, deployed from this repo by
`.github/workflows/deploy-site.yml`.

The public site for **Kintrinsic** (wardship for a child's Linux computer or
GrapheneOS phone — launched as **Charter**). Static HTML, no build step — open
any page in a browser to preview, or serve the folder
(`python3 -m http.server`).

The site is the product's marketing, its downloads, and its install support.
The parent app ships as an Android APK (bundling the console) and is also
served as a web app at `charter.mysignet.app` (see `apps/charter-app/DEPLOY.md`);
this site is where the APK and the Linux installer are linked from.

Multi-page, one shared stylesheet:

| File | Page |
|---|---|
| `index.html` | Home — marketing + what it does |
| `download.html` | Downloads, plus how to install the APK without fright |
| `setup.html` | Setup guide for the child's computer (the guaranteed path) |
| `setup-phone.html` | Setup guide for a child's GrapheneOS phone (the harder one) |
| `roadmap.html` | Honest map: shipped / newly-landed / coming |
| `faq.html` | The questions parents ask |
| `download/index.html` | Redirect from `/download` to `/download.html` |
| `downloads.json` | Current versions, URLs and SHA-256 per platform; read by `download.html` (hrefs in the markup are the no-JS fallback) |
| `styles.css` | Shared design system (wax-seal skin), light + dark |
| `seal.svg` | Favicon / logo |
| `img/` | `og.png` (link previews), the product screenshots, and `img/src/` (the og-card source) |
| `robots.txt`, `sitemap.xml` | Crawl basics (bump `lastmod` when a page changes) |

## The rename, and what still says "Charter"

Kintrinsic is the product; **a charter is still the thing a family writes** —
the agreement, alongside guardian / ward / warden in the lexicon. That's
deliberate: the concept keeps its name, the brand stops colliding with it.

The user-facing names are all Kintrinsic now — the apps, the installer, the
menu entries, this site. Only these keep the earlier name `charter`, as a
deliberate, permanent internal name (like Signal's package id):

- **`charter` / `charterd.service`** — the Linux command and daemon.
- **`org.forgesworn.charter` / `org.forgesworn.mycharter`** — the Android app ids.
- **`@forgesworn/charter`** — the npm package name.

Release artifacts (the `.deb` / `.apk`) are not in this repo. GitHub Releases
serve the Android APK, Blossom mirrors it, and the Linux `.deb` is currently
linked from Blossom; the download page gets its links from `downloads.json`.

The user-facing rebrand has shipped and those literals are intentionally kept.
If any shipped copy changes, recapture the screenshots (recipe below) and
regenerate `img/og.png` (`img/src/og-card.html`, rendered at exactly 1200×630).

**Branding assets are maintained privately:** all mark SVGs, the brand
workbench, the brand book, the living spec and the full decision record are
kept outside this (public) repo. This repo keeps only the shipped assets
(`seal.svg`, `img/og.png`) and `img/src/og-card.html` (the og-image source,
which uses the bud mark). The dated `docs/superpowers/` copies that used to
sit alongside them here have been removed.

Screenshots in `img/` are captured from the apps' own fixture data, never a
real family — `charter-console/ui/app.html` opened directly in a browser (its
`SAMPLE` fallback fires when no host answers), `charter-lock` via
`CHARTER_LOCK_RENDER_TO=` (renders offscreen, never locks anything), and the
parent app under `npm run dev` (which seeds a fictional "Sam") — capture from
`main`. None of those touch a running `charterd`. The four screenshots currently
in `img/` predate the new seal and still show the old "C"; the home page says so
until they are recaptured.

## Deploy

- Deploy: push to `main` → GitHub Actions rsyncs the repo to `/opt/charter-you/`
  on the Hetzner box (needs the `HETZNER_SSH_KEY` repo secret).
- Serving (one-time infra): DNS record for `kintrinsic.app` + a vhost pointing
  at `/opt/charter-you/`; a redirect from the old `charter.signet.you` keeps
  existing links alive.
- The deploy rsync uses `--delete-excluded` rather than plain `--delete`, so
  that adding an `--exclude` for any private/internal-only file actually
  removes it from the server rather than leaving a stale copy in place.
- The download page carries each app's version in the markup as a no-JS
  fallback; bump those when you cut a release (`downloads.json` still drives the
  live value).

## Cutting a release

The release itself (tag, CI draft, verify and publish) is described in
`docs/releasing.md`; `publish-release.mjs --from-draft` also writes the
channel's download manifest. For the site, after a release:

1. Check `site/downloads.json` matches the new manifest
   (`./scripts/sync-front-door-downloads.sh` copies it).
2. Bump the `data-ver` fallbacks and the `data-sha` fallback in
   `site/download.html`, and the file names in the verify commands there.
3. Commit `site/` and push `main`.
