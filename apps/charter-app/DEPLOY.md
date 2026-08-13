# Deploying MyCharter (charter.mysignet.app)

MyCharter is a static **Progressive Web App** — same shape as signet-app
(React + Vite + `vite-plugin-pwa`). There is no server to run; you build a
static bundle and host the folder.

## Build

```sh
cd apps/charter-app
npm install
npm run build        # tsc --noEmit && vite build
```

Output: **`apps/charter-app/dist/`** — a self-contained static site:
`index.html`, `assets/*`, `manifest.webmanifest`, and the service worker
(`sw.js` + `workbox-*.js`).

## Host it the same way as signet-app

- Serve `dist/` as static files at **`https://charter.mysignet.app`**.
- **HTTPS is required** — the service worker (offline/installable) and the
  camera (QR scanning, later) only work over a secure origin.
- **No SPA rewrites needed.** The app uses **hash-based routing** (`/#/limits`,
  `/#/family`, …), so every route is served by the same `index.html`; a plain
  static host (or whatever pipeline already serves signet-app) is enough.
- Point the build pipeline at `apps/charter-app` with `npm ci && npm run build`
  and publish `dist/` — mirror signet-app's deploy step.

## Automated deploy (CI — same as signet-app)

`.github/workflows/deploy-charter-app.yml` mirrors signet-app's `deploy.yml`:
on push to `main` (when `apps/charter-app/**` changes) it runs typecheck +
test + build, then `rsync`s `dist/` to the Hetzner box. To enable it, the
repo/sysadmin must provide:

1. **Repo secrets `HETZNER_SSH_KEY` + `DEPLOY_HOST`** — a private key whose
   `deploy@` user can write the charter docroot (same secret name signet-app
   uses), and the deploy host's address.
2. **Confirm the docroot** — the workflow targets
   `deploy@$DEPLOY_HOST:/opt/charter-app/`. This MUST match the nginx vhost
   root the sysadmin set up for `charter.mysignet.app` (signet-app uses
   `/opt/signet-app/`). `--delete` keeps it a clean mirror, so the directory
   must be dedicated to MyCharter.
3. **Reach `main`** — it deploys from `main` (or a manual `workflow_dispatch`).

## Manual one-off deploy

If you'd rather push it by hand (e.g. before CI secrets are wired):

```sh
cd apps/charter-app && npm ci && npm run build
rsync -avz --delete -e "ssh -i <your_deploy_key>" \
  dist/ deploy@<deploy_host>:/opt/charter-app/   # confirm the docroot
```

## Security headers (nginx) — required

This origin holds the **guardian secret key**. That is the whole family's root
of trust: it signs every clause, grant and RELEASE for every device. A future
XSS, or one compromised npm package, is the difference between "someone
defaced a page" and "someone can forge policy for your children and unlock
every device you own" — so the headers below are not optional polish.

`index.html` already carries a `Content-Security-Policy` meta tag, so the app
is protected wherever it is served from. Serve these headers **as well**: a
real header cannot be injected past, arrives before the parser sees any
markup, and can carry directives a meta tag is not allowed to
(`frame-ancestors`, and the non-CSP headers below).

```nginx
# charter.mysignet.app
add_header Content-Security-Policy "default-src 'self'; script-src 'self'; style-src 'self' 'unsafe-inline'; img-src 'self' data: blob:; font-src 'self'; media-src 'self' blob:; connect-src 'self' wss:; worker-src 'self'; manifest-src 'self'; object-src 'none'; base-uri 'none'; form-action 'none'; frame-src 'none'; frame-ancestors 'none'" always;
add_header Cross-Origin-Opener-Policy "same-origin" always;
add_header Cross-Origin-Resource-Policy "same-origin" always;
add_header Referrer-Policy "no-referrer" always;
add_header X-Content-Type-Options "nosniff" always;
add_header X-Frame-Options "DENY" always;
add_header Strict-Transport-Security "max-age=31536000; includeSubDomains" always;
add_header Permissions-Policy "camera=(self), microphone=(), geolocation=(), interest-cohort=()" always;
```

Notes on the choices, so nobody "tidies" one away:

- **`frame-ancestors 'none'` / `X-Frame-Options: DENY`** — nothing may embed
  this app. Clickjacking an approval sheet is a way to get a guardian to sign
  something they never saw.
- **`Cross-Origin-Opener-Policy: same-origin`** — severs the opener
  relationship, so a page that launches this one cannot keep a handle on it.
- **`camera=(self)`** — the QR scanner needs it; nothing else does, and no
  third party ever does.
- **`connect-src … wss:`** stays open because relays are the guardian's own
  choice and cannot be enumerated here. Narrow it if the deployment ever
  fixes its relay set.
- **`style-src 'unsafe-inline'`** is deliberate: the app styles through React
  `style={{}}` props. Inline style cannot execute. Do **not** add
  `'unsafe-inline'` or `'unsafe-eval'` to `script-src` — that is the one line
  that makes the rest of this pointless.

After changing the vhost, verify from outside:

```sh
curl -sI https://charter.mysignet.app | grep -iE 'content-security|referrer|opener|frame|nosniff|strict-transport'
```

## Verify locally before shipping

```sh
npm run preview      # serves the built dist/ on http://localhost:4173
npm run typecheck    # tsc --noEmit
npm run test         # vitest
```

## Notes
- Runs today against a **mock signer + demo family** (localStorage), so it's
  fully clickable before the real Signet bunker exists. The real
  Signet/heartwood signer plugs in behind the `Signer` interface
  (`src/signer/Signer.ts`) — no UI rebuild.
- What Signet must implement for this to do real work:
  `internal/specs/2026-06-28-signet-app-changes-for-charter.md`.
