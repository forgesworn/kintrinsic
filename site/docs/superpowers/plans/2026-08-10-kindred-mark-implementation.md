# Kindred Mark System Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Build the Kindred "one flame, many vessels" mark system in SVG — flame DNA glyph, Kintrinsic candle mark (with lockup, favicon and app-icon cuts), sketch vessels for the other three apps — presented in context for decented's selection, then integrate the chosen mark into kintrinsic.app.

**Architecture:** All brand source lives in `img/src/brand/` (already excluded from deploy via the `img/src` rsync exclude). A single self-contained `preview.html` renders every candidate at real sizes, in site context, light and dark; it is the "test suite" — each task adds its candidates to the preview and verifies by screenshot at actual size. Final assets are copied out to the site only in the last task, after decented chooses.

**Tech Stack:** Hand-authored SVG, one static preview page (no build step), Playwright MCP for capture, Python/PIL for downscale inspection, the site's existing contrast-audit JS for palette gates.

**Spec:** `docs/superpowers/specs/2026-08-10-kindred-brand-design.md`

## Global Constraints

- The flame glyph is drawn **identically** in every mark: same construction, same proportions. Vessels vary; the flame never does.
- Register: calm & literate; geometric, woodcut-adjacent; no illustrative clutter.
- **Banned forms:** shields, eyes, locks, keyholes, magnifying glasses.
- Every mark must survive: 16px favicon, one-colour, greyscale, light and dark themes.
- Vessel silhouettes must be mutually distinguishable at 48px (app-icon size).
- Palette targets: flame amber/gold accent, deep warm ink ground, cream paper `#f9f8f4` retained. Any colour used for *text* must measure ≥ 4.5:1 on its ground (glyph-only colours need ≥ 3:1 against adjacent ground).
- Wax red (`#8f2a24`) appears in **no new asset**.
- SVGs: `viewBox="0 0 48 48"` master grid, integer-ish coordinates, no filters, no gradients in the core marks, `fill` from CSS `currentColor` where single-colour.
- Work on branch `kindred-mark`; do not push `main` until Task 5.

---

### Task 1: Preview harness + flame DNA glyph candidates

**Files:**
- Create: `img/src/brand/flame-a.svg`, `img/src/brand/flame-b.svg`, `img/src/brand/flame-c.svg`
- Create: `img/src/brand/preview.html`

**Interfaces:**
- Produces: three flame constructions, each a `<path>` (or two paths: flame + inner counter) on a 48×48 grid, occupying roughly x 14–34, y 6–30 so vessels can sit beneath in later tasks. Later tasks inline the *chosen* flame's path data verbatim — record each flame's path `d` strings at the top of `preview.html` in an HTML comment.
- Produces: `preview.html`, which every later task extends. Structure: one `<section>` per task, each candidate shown in a row of fixed-size cells — 96px, 48px, 24px, 16px — repeated on cream (`#f9f8f4`) and deep-ink (`#241b12`) grounds, plus a one-colour (black) and greyscale row. Pure static HTML + inline CSS; SVGs inlined via `<img>` to the sibling files AND one inline `<svg>` copy per candidate for the `currentColor` row.

- [ ] **Step 1: Create branch**

```bash
cd ~/charter-you && git checkout -b kindred-mark
```

- [ ] **Step 2: Author three flame constructions**

Three genuinely different geometries, not one shape tweaked. Starting skeletons (iterate by eye — coordinates are starting points, the *construction idea* per candidate is fixed):

`flame-a.svg` — teardrop flame, two arcs meeting in a point, with a lifted inner counter (the classic candle flame, asymmetric tip leaning right):

```svg
<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 48 48">
  <path fill="currentColor" d="M24 6c5 6.5 8 10.5 8 15.5a8 8 0 0 1-16 0C16 16.5 19 12.5 24 6Z"/>
  <path fill="#f9f8f4" d="M24 16c2.4 3.2 3.8 5.2 3.8 7.6a3.8 3.8 0 0 1-7.6 0c0-2.4 1.4-4.4 3.8-7.6Z"/>
</svg>
```

`flame-b.svg` — "spark leaning": a flame with a distinct flick to the tip, drawn as one continuous cut (no counter), more woodcut:

```svg
<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 48 48">
  <path fill="currentColor" d="M27 6c-1 4-6 6.5-8.5 10.5C16 20.5 16 24 18 27a8.5 8.5 0 0 0 14.5-6c0-3-1.5-5-2.5-8 2 .8 3 2 4 4C34.5 11.5 31 8 27 6Z"/>
</svg>
```

`flame-c.svg` — "the handed flame": two nested flame strokes, a smaller flame cradled inside a larger open one (kin + child's spark in one glyph):

```svg
<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 48 48">
  <path fill="none" stroke="currentColor" stroke-width="3" stroke-linecap="round"
        d="M24 7c5 6 7.5 10 7.5 14.5A7.5 7.5 0 0 1 20 28"/>
  <path fill="currentColor" d="M23 15c2.8 3.6 4.2 5.8 4.2 8.5a4.2 4.2 0 0 1-8.4 0c0-2.7 1.4-4.9 4.2-8.5Z"/>
</svg>
```

- [ ] **Step 3: Build `preview.html`**

Static page, section "Task 1 — flame DNA", the size/ground/one-colour grid described in Interfaces. Include a `<!-- PATHS ... -->` comment block recording each flame's `d` strings.

- [ ] **Step 4: Verify at real size**

```bash
cd ~/charter-you && setsid python3 -m http.server 8920 >/dev/null 2>&1 < /dev/null & disown
```

Open `http://localhost:8920/img/src/brand/preview.html` with the Playwright browser tools, screenshot the page, then read the screenshot and confirm: each flame still reads as a flame in the 16px cell, on both grounds, in one-colour. If any candidate mushes at 16px, simplify its geometry (fewer nodes, thicker counter) and re-check. This is the test cycle; do not proceed with a candidate that fails 16px.

- [ ] **Step 5: Commit**

```bash
git add img/src/brand && git commit -m "brand: three flame DNA candidates + preview harness"
```

---

### Task 2: Kintrinsic candle marks + lockup + favicon and app-icon cuts

**Files:**
- Create: `img/src/brand/kintrinsic-a.svg`, `kintrinsic-b.svg`, `kintrinsic-c.svg` (candle + the matching flame from Task 1: candle A carries flame A, etc.)
- Create: `img/src/brand/kintrinsic-lockup.svg` (mark + "Kintrinsic" wordmark)
- Create: `img/src/brand/kintrinsic-appicon.svg` (mark on warm-dark rounded square)
- Modify: `img/src/brand/preview.html` (add "Task 2" section)

**Interfaces:**
- Consumes: flame path `d` strings from the preview's PATHS comment — pasted verbatim, translated/scaled only as a whole group (`<g transform>`), never redrawn.
- Produces: candle vessel geometry (recorded in the PATHS comment): candle body ≈ x 19–29, y 30–44, with a wick line y 27–30; drip/collar optional per candidate. App-icon ground colour token recorded as `--kin-ink` (starting value `#241b12`) and flame colour `--kin-gold` (starting value `#e0a458`) in a `:root` block in `preview.html` — Task 5 copies these exact custom-property names into `styles.css`.

- [ ] **Step 1: Author the three candle marks**

Skeleton (candle for flame-a; body drawn as a simple slab with one melt notch — repeat the pattern for b and c with their own flames):

```svg
<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 48 48">
  <g><!-- flame-a paths pasted verbatim, transform="translate(0,-2)" --></g>
  <path fill="currentColor" d="M23.4 27h1.2v4h-1.2z"/><!-- wick -->
  <path fill="currentColor" d="M19 31h10v12a1.5 1.5 0 0 1-1.5 1.5h-7A1.5 1.5 0 0 1 19 43V31Z
       M19 31c0-1.2 1.6-2 3-1.4"/><!-- body + soft melt lip; iterate by eye -->
</svg>
```

- [ ] **Step 2: Author the lockup**

`kintrinsic-lockup.svg`: mark at left, wordmark "Kintrinsic" set in Georgia bold (the site's serif stack), tight tracking, baseline aligned to the candle's base. Wordmark as `<text>` is fine for preview; note in the PATHS comment that the shipped lockup must convert text to paths later (font availability), but conversion is NOT part of this plan.

- [ ] **Step 3: Author the app icon**

`kintrinsic-appicon.svg`: 48×48, `rx="11"` rounded square filled `var(--kin-ink)`, candle mark centred at 70% scale, flame in `var(--kin-gold)`, body in cream `#f9f8f4`.

- [ ] **Step 4: Extend preview + verify**

Add a "Task 2" section: the three candle marks through the same size grid; the app icon at 48px and 96px; the lockup on cream and ink; plus a **favicon strip** — each candidate inside a mock browser tab drawn at actual 16px next to 8 real-world favicons for comparison. Screenshot via Playwright at device-pixel-ratio 1 AND 2; read both; gates: flame+candle separable at 16px, app icon distinct and warm at 48px, lockup balanced. Iterate geometry until all three candidates pass or a candidate is consciously dropped (note why in the PATHS comment).

- [ ] **Step 5: Contrast gate**

In the Playwright browser, `evaluate` a contrast check on the preview page: `--kin-gold` on `--kin-ink` (needs ≥ 3:1 as a glyph colour) and candidate *text* accent `#8a5c0c` on `#f9f8f4` (needs ≥ 4.5:1 — this is the existing `--amber`, already AA-verified on this site). Record measured ratios in the PATHS comment. Adjust `--kin-gold` until it clears 3:1.

- [ ] **Step 6: Commit**

```bash
git add img/src/brand && git commit -m "brand: Kintrinsic candle candidates, lockup, favicon + app-icon cuts"
```

---

### Task 3: Vessel sketches — lantern, ring, jar

**Files:**
- Create: `img/src/brand/kindependence-sketch.svg`, `kinclude-sketch.svg`, `kinterest-sketch.svg`
- Modify: `img/src/brand/preview.html` (add "Task 3" section)

**Interfaces:**
- Consumes: the flame paths (verbatim, group-transformed) and `--kin-ink`/`--kin-gold` tokens.
- Produces: proof the system extends. Sketch grade — honest geometry, no polish pass.

- [ ] **Step 1: Author the three sketches**

One flame each, vessels only silhouette-deep:
- `kindependence-sketch.svg` — lantern: rounded-top rectangle outline, stroke 3, small hanging loop, flame centred inside.
- `kinclude-sketch.svg` — ring: five flames at 60% scale arranged on a circle (rotate copies of the flame group around centre; the ring of people, no faces, no figures).
- `kinterest-sketch.svg` — jar: mason-jar silhouette (shouldered rectangle + lid band), flame floating low inside like a kept firefly.

- [ ] **Step 2: Extend preview + verify distinguishability**

Add "Task 3" section: all four product marks (candle candidate A + three sketches) in a row at 48px and 24px on both grounds. Screenshot, read, gate: four silhouettes tell apart at 48px at a glance. If ring reads as "flower" or jar as "bottle of drink", adjust and re-check.

- [ ] **Step 3: Commit**

```bash
git add img/src/brand && git commit -m "brand: vessel sketches prove the system — lantern, ring, jar"
```

---

### Task 4: In-context render + decented's selection (CHECKPOINT — requires the user)

**Files:**
- Modify: `img/src/brand/preview.html` (add "Task 4" section)

**Interfaces:**
- Consumes: everything prior.
- Produces: decented's choice of flame/candle candidate, recorded at the top of `preview.html` as `<!-- CHOSEN: kintrinsic-X -->`. Task 5 is blocked until this exists.

- [ ] **Step 1: Build in-context section**

Reproduce, inside `preview.html` (copy the real markup/styles from the live files, swap the mark): the site nav bar with each candidate replacing the seal; the hero clause card with the candidate in the seal position; both light and dark theme blocks side by side.

- [ ] **Step 2: Capture the decision sheet**

Screenshot the full preview (light+dark, all sections) and send the images to decented with SendUserFile, with a one-paragraph summary of what differs between candidates A/B/C and any recommendation formed while iterating.

- [ ] **Step 3: STOP and wait**

Do not proceed to Task 5 without decented naming a candidate. When he does: write the `CHOSEN` comment, commit:

```bash
git add img/src/brand/preview.html && git commit -m "brand: record decented's chosen candidate"
```

---

### Task 5: Integrate the chosen mark into the site

**Files:**
- Modify: `seal.svg` (replaced by the chosen candle mark, keeping the filename — every page references `/seal.svg`)
- Modify: `index.html` (hero clause seal glyph: replace the `K` monogram `<span class="clause__seal">K</span>` styling usage with the mark — concretely: keep the span, but its content becomes an inline SVG of the flame; adjust `.clause__seal` in `styles.css` from wax circle to ink circle with gold flame)
- Modify: `styles.css` — add `--kin-ink` and `--kin-gold` (Task-2-verified values) and `--kin-accent` (text-safe amber: `#8a5c0c` light / `#dcaa5c` dark — both already AA-verified in this stylesheet as `--amber`). Then migrate by **renaming usages, not aliasing**: every `var(--wax)` used for text/links/accents becomes `var(--kin-accent)`; glyph/decorative wax uses become `var(--kin-gold)` or `var(--kin-ink)` as fits. Delete wax tokens that end up unused; if any wax usage is deliberately kept, list it in the commit message.
- Modify: `img/src/og-card.html` (candle mark replaces K seal; palette swap; regenerate `img/og.png` at exactly 1200×630 as done previously)
- Test: re-run the full existing verification pass (contrast/overflow/target audit across 6 pages × widths × themes) exactly as used earlier in this repo's history

**Interfaces:**
- Consumes: `CHOSEN` candidate SVG and the verified colour tokens.

- [ ] **Step 1: Swap assets** — `seal.svg`, clause seal, og-card; regenerate `og.png` (serve repo, Playwright 1200×630 capture, PIL save, confirm dimensions).

- [ ] **Step 2: Palette shift** — token edits in `styles.css` per Files above; grep for remaining `--wax` usages; each one either becomes `--kin-accent`/`--kin-gold` or is consciously kept (list any keeps in the commit message).

- [ ] **Step 3: Run the verification suite**

Serve the site on a fresh port; run the same Playwright contrast+overflow+target audit used at the rebrand (6 pages × [320, 390, 768, 1440] × [light, dark]); gate: zero failures. Fix and re-run until clean.

- [ ] **Step 4: Visual pass** — screenshot home light+dark at 1400px and 390px, read them, confirm the candlelight palette holds together (no orphaned wax-red elements, flame legible in nav).

- [ ] **Step 5: Commit, merge, push**

```bash
git add -A && git commit -m "brand: the lit candle — Kindred mark system lands on the site"
git checkout main && git merge kindred-mark && git push
```

Deploy fires on push; confirm with `gh run list --limit 1`.

---

## Amendment (2026-08-11): the revised board

decented's Task 4 checkpoint rejected the candle (religious read) and locked a
revised system (spec updated, same file, "one flame — held, carried, gathered
round, kept"). Task 6 below builds it and REPLACES the original Task 4's
candidate set; the original Task 4's in-context harness is reused. Task 5 then
integrates whatever decented picks from Task 6's sheet. Order: 6 → checkpoint → 5.

### Task 6: Revised mark set — the locked board

**Files:**
- Modify: `img/src/brand/flame-a.svg` (counter enlarged → this IS the DNA now)
- Create: `img/src/brand/kintrinsic-hands.svg`, `kintrinsic-bud.svg` (fallback variant)
- Create: `img/src/brand/kindependence-campfire.svg`
- Create: `img/src/brand/kinclude-negspace-flames.svg`, `kinclude-negspace-quotes.svg`
- Modify: `img/src/brand/kinterest-sketch.svg` (re-seat the revised flame; keep the de-padlocked body)
- Modify: `img/src/brand/preview.html` (new "Task 6" section + refreshed in-context rows using the new Kintrinsic candidates; PATHS comment updated — mark candle/lantern/ring entries RETIRED, do not delete history)

**Interfaces:**
- Consumes: flame-a construction (teardrop outer + evenodd counter, single path); tokens --kin-ink/--kin-gold with fallback syntax in standalone files.
- Produces: revised flame-a `d` as the sole DNA (recorded in PATHS comment; every live mark carries it verbatim, group transforms only); the six mark files above; decision-sheet captures for checkpoint 2.

- [ ] **Step 1: Revise the DNA.** Enlarge flame-a's inner counter relative to the outer flame (target: counter height ≈ 55–60% of outer height, up from ≈46%; iterate by eye). Re-run the 16px gate on both grounds + one-colour; the counter must stay open (not seal shut) at 16px. Record the new `d` in the PATHS comment; propagate verbatim into every live mark built in later steps.
- [ ] **Step 2: The hands (Kintrinsic primary).** Construction: two near-single-stroke hand forms — lower hand a shallow open cradle curve entering from bottom-left, upper hand a mirrored-but-offset curve entering from top-right, flame between them, clear air gaps between flame and both hands. HARD RULES: no fingers at any size (a single terminal thumb-notch per hand is the most detail allowed, and only if it survives 24px); never symmetrical cupping; never a flat upright palm. Gates: 16px legible as "something held/exchanged around a light"; 24px+ unmistakably two hands; no banned-form reads (check specifically: praying-hands, Hamsa, charity-cup).
- [ ] **Step 3: The bud (Kintrinsic fallback).** One DNA flame with a small second flame (≈40% scale, same construction) splitting off its upper shoulder, gap or thin join per what reads at 16px. Gate: must read as two flames (one emerging), not a deformed blob.
- [ ] **Step 4: The campfire (Kindependence).** DNA flame over two crossed stick strokes (chunky, stroke ≥ 3 at 48-grid; X kept low and wide so it can't read as a letter X or an error glyph at 16px). Gate: reads campfire at 24px, not "flame over letter X".
- [ ] **Step 5: The negative-space pair (Kinclude).** Variant 1: two DNA flames (taller + ~70% smaller) leaning toward each other, silhouettes fused/filled dark, the gap between them shaped to the DNA flame outline. Variant 2: two large facing quotation marks (serif comma-form, from the site's Georgia stack, drawn as paths not text), gap = DNA flame outline. Gates: the gap flame must be findable at 24px within 2 seconds (the "do you see the flame?" test — capture and judge); at 16px the mark may simplify to the flanks alone but must not read as random blobs.
- [ ] **Step 6: Re-seat the jar.** Swap the revised DNA flame into the kept jar body (transforms only). Re-run the 24px padlock check (gate table updated).
- [ ] **Step 7: Preview + gates + decision sheet.** New Task 6 section: full size grid for every mark above; refreshed in-context rows (nav, hero seal in wax-red continuity + ink/gold future palette) for BOTH Kintrinsic variants; app-icon compositions for hands, bud, campfire, both negspace variants, jar; favicon strip with revised flame-a, hands, bud. Run the banned-forms sweep across ALL marks at 24px and 16px. Capture decision-sheet-2-{light,dark,16px}.png to the session scratchpad.
- [ ] **Step 8: Commit** — `git add img/src/brand && git commit -m "brand: the locked board — hands, bud, campfire, negative-space pair, re-seated jar"`. Do not push.

**Checkpoint after Task 6 (controller):** send the sheets to decented; he picks Kintrinsic (hands vs bud) and Kinclude (flames vs quotes). Record as `<!-- CHOSEN-2: kintrinsic=X kinclude=Y -->` in preview.html. Task 5 then proceeds with the chosen Kintrinsic mark substituting every reference to "the chosen candle mark", and its Step 1 asset swap uses that mark's geometry for seal.svg, the clause seal, and og-card.
