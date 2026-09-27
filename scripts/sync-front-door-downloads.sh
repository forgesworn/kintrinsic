#!/usr/bin/env bash
# Point the front-door download page (kintrinsic.app, the site/ directory) at
# the current release artifacts. Artifacts are NOT copied into the repo: the
# publish scripts put them on their GitHub Release (the primary host) and
# mirror them to Blossom. This writes downloads.json — which download.html
# reads to set the download buttons — AND rewrites the page's no-JS fallback
# hrefs, version labels and structured-data downloadUrl to match, so nothing
# is left to hand-edit.
#
# Per platform the button is the GitHub Release download (the first GitHub
# entry of the manifest's `urls`), and the secondary "mirror" link is the
# verified Blossom address (the manifest's `url`). A manifest written before
# GitHub Releases has no `urls`: the button is then the Blossom URL and the
# mirror link is hidden.
#
# Run AFTER the publish scripts. Then commit + push this repo. This script
# pushes nothing and copies no binaries.
set -euo pipefail
cd "$(dirname "$0")/.."          # charter repo root
PUB=apps/charter-app/public
CY="${CHARTER_YOU_DIR:-./site}"

[ -d "$CY" ] || { echo "front-door site dir not found at $CY (set CHARTER_YOU_DIR)"; exit 1; }

# Take the *verified* URLs straight from the manifests the publish scripts
# wrote — never reconstruct them, so a partial-mirror release can't produce a
# dead front-door link.
node --input-type=module -e '
  import { readFileSync, writeFileSync } from "node:fs";
  import { isGithubReleaseUrl } from "./scripts/release/release-helpers.mjs";
  const [pub, site] = process.argv.slice(1);
  const HEX64 = /^[0-9a-f]{64}$/;

  function platform(file, shaKey) {
    const m = JSON.parse(readFileSync(`${pub}/${file}`, "utf8"));
    const sha = m[shaKey];
    const legacy = m.url;
    if (typeof m.versionName !== "string" || !m.versionName || !HEX64.test(sha ?? "") ||
        typeof legacy !== "string" || !legacy.startsWith("https://")) {
      throw new Error(`${file} is missing version/url/sha — run the publish scripts first`);
    }
    const urls = Array.isArray(m.urls) ? m.urls : [];
    const github = urls.find((u) => isGithubReleaseUrl(u));
    const out = { version: m.versionName, url: github ?? legacy, sha256: sha };
    if (github && legacy !== github) out.mirror = legacy;
    return out;
  }

  const dl = {
    android: platform("mycharter-apk.json", "apkSha256"),
    linux: platform("charter-deb.json", "sha256"),
  };
  writeFileSync(`${site}/downloads.json`, JSON.stringify(dl, null, 2) + "\n");

  // The no-JS fallback: the same values baked into the markup.
  const page = `${site}/download.html`;
  let html = readFileSync(page, "utf8");
  const sub = (re, fn, what) => {
    let n = 0;
    html = html.replace(re, (...m) => { n++; return fn(...m); });
    if (n === 0) throw new Error(`download.html: no ${what} found — markup changed?`);
  };
  for (const [key, d] of Object.entries(dl)) {
    sub(new RegExp(`(<a\\b[^>]*?\\bhref=")[^"]*("[^>]*\\bdata-dl="${key}")`, "g"),
      (_, a, b) => a + d.url + b, `data-dl="${key}" button`);
    sub(new RegExp(`(<a\\b[^>]*?\\bhref=")[^"]*("[^>]*\\bdata-dl-mirror="${key}")`, "g"),
      (_, a, b) => a + (d.mirror ?? d.url) + b, `data-dl-mirror="${key}" link`);
    sub(new RegExp(`(<span\\b[^>]*\\bdata-mirror="${key}")( hidden)?>`, "g"),
      (_, a) => a + (d.mirror ? "" : " hidden") + ">", `data-mirror="${key}" wrapper`);
    sub(new RegExp(`(<span data-ver="${key}">)[^<]*(</span>)`, "g"),
      (_, a, b) => a + d.version + b, `data-ver="${key}" label`);
  }
  sub(/("downloadUrl": ")[^"]*(")/g, (_, a, b) => a + dl.linux.url + b, "structured-data downloadUrl");
  writeFileSync(page, html);

  for (const [key, d] of Object.entries(dl)) {
    console.log(`${key.padEnd(8)} ${d.version}  ${d.url}` + (d.mirror ? `\n${" ".repeat(9)}mirror  ${d.mirror}` : ""));
  }
' "$PUB" "$CY"

echo
echo "downloads.json and download.html updated. Commit site/ and push."
echo "Verify the buttons resolve first: curl -sIL <url> | grep -i '^http'"
