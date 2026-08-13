# site/ — kintrinsic.app, Kintrinsic's front door

**This directory is the marketing site**, copied (working tree, not history)
from the retired `forgesworn/charter-you` repo on 2026-08-11 so everything
lives in one repo for the eventual public flip. The old repo keeps the messy
site history (and keeps deploying until this repo's `deploy-site.yml` takes
over at the rename-branch merge); it gets archived at cutover.

The public site for **Kintrinsic** (wardship for a child's Linux computer or
GrapheneOS phone — launched as **Charter**; this repo led the rename). Static
HTML, no build step — open any page in a browser to preview, or serve the
folder (`python3 -m http.server`).

The site is the product's marketing, its downloads, and its install support.
There is **no PWA**: the parent app ships as an APK (plus the Linux installer),
and this site is where both live.

Multi-page, one shared stylesheet:

| File | Page |
|---|---|
| `index.html` | Home — marketing + what it does |
| `download.html` | Downloads, plus how to install the APK without fright |
| `setup.html` | Setup guide for the child's computer (the guaranteed path) |
| `setup-phone.html` | Setup guide for a child's GrapheneOS phone (the harder one) |
| `roadmap.html` | Honest map: shipped / newly-landed / coming |
| `faq.html` | The questions parents ask |
| `styles.css` | Shared design system (wax-seal skin), light + dark |
| `seal.svg` | Favicon / logo |
| `img/` | `og.png` (link previews, source in `img/src/`) and the product screenshots |
| `robots.txt`, `sitemap.xml` | Crawl basics |

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

Release artifacts (the `.deb` / `.apk`) live on Blossom (content-addressed),
not in this repo; the download page links out to them via `downloads.json`.

The user-facing rebrand has shipped and those literals are intentionally kept.
If any shipped copy changes, recapture the screenshots (recipe below) and
regenerate `img/og.png` (`img/src/og-card.html`, rendered at exactly 1200×630).

**Branding lives in `forgesworn/kindred-internal`** (private): all mark SVGs,
the brand workbench, the brand book, the living spec and the full decision
record moved there 2026-08-11. This repo keeps only the shipped assets
(`seal.svg`, `img/og.png`) and `img/src/og-card.html` (the og-image source,
which uses the bud mark). The `docs/superpowers/` copies here are dated
history; the living spec is kindred-internal's.

Screenshots in `img/` are captured from the apps' own fixture data, never a
real family — `charter-console/ui/app.html` opened directly in a browser (its
`SAMPLE` fallback fires when no host answers), `charter-lock` via
`CHARTER_LOCK_RENDER_TO=` (renders offscreen, never locks anything), and the
parent app under `npm run dev` (which seeds a fictional "Sam") — capture from
the `rename/kintrinsic` branch so the wordmark reads Kintrinsic. None of those
touch a running `charterd`.

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

## Cutting a release (D2 order)

1. Bump the version: `android/app/build.gradle.kts` (ward) /
   `android/carrier/build.gradle.kts` (carrier) / `linux/Cargo.toml` (deb).
2. Run the publish script(s) — `android/scripts/publish-apk.sh`,
   `android/scripts/publish-carrier-apk.sh`, `./scripts/publish-deb.sh`. Each
   now ALSO uploads the artifact to the Blossom mirrors and announces it as a
   signed kind-30063 event on the release relays
   (`scripts/release/publish-release.mjs`; key: `~/.charter-release/`, see
   `android/keystore/README.md`). `CHARTER_RELEASE_EVENT_SKIP=1` skips that
   step for offline builds.
3. `./scripts/sync-front-door-downloads.sh`, bump the `data-ver` fallbacks in
   `site/download.html`.
4. Commit `apps/charter-app/public/` + `site/` and push main (both deploys
   fire). The origin JSON feeds are the UNSIGNED FALLBACK only — clients
   prefer the relay events; the feeds (and this step's urgency) retire at D3
   once the fleet has been seen updating via relays + Blossom.
