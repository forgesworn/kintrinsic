# Kindred — brand & mark system design

*Agreed 2026-08-10, brainstormed with decented. Status: approved direction, pending
decented's review of this document.*

## What this is

The identity for the **Kindred suite** — ForgeSworn's family-facing collection of
parenting apps — and the specific mark for **Kintrinsic**, its first product.
Deliberately branded apart from ForgeSworn (privacy-tech, developer-facing) and
from Signet (whose wax-seal metaphor the old Charter identity borrowed): Kindred
is marketed to families, primarily non-technical.

## The suite

| App | What it is | The outcome it's named for |
|---|---|---|
| **Kintrinsic** | Screen time & web filtering | Intrinsic motivation — self-regulation |
| **Kindependence** | Consent-based family location (casual + emergency) | Independence — trusted freedom |
| **Kinclude** | Family-only messaging (constrained, no strangers) | Inclusion — belonging |
| **Kinterest** | Child's pocket money on a parent–child shared ledger | Interest — a real stake, both senses |

The naming pattern is the platform: **kin + the capability the child walks away
with**. Each app is scaffolding toward a capability, never surveillance. The
tracking app is named for the freedom it produces; that inversion is the brand.

**Platform story, one line:** *Kin keeps a fire; children leave carrying their
own light.*

## Decisions made in brainstorming

1. **The mark depicts "the becoming"** — guided growth turning into
   self-motivation; scaffolding that lets go. Not the outcome alone, not the
   parent–child relationship alone, and not an abstract lettermark (every app
   starts with K, so a letter system cannot differentiate the suite).
2. **Suite DNA first; Kintrinsic expresses it.** Proton-style: shared identity
   defined once, each product lands into it.
3. **Register: calm & literate.** Warm modern craft with a book-ish soul — the
   voice the copy already speaks ("a promise, not a leash"). Not techie, not
   candy-coloured, not Headspace-soft, not eco-organic.
4. **Umbrella name: Kindred.** Kin + kindled in one real word. decented reclaims
   the name from the `@forgesworn/kindred` dev library (kin/kith/ken contact
   verification), which he will rename — `kith` / `kith-kit` suggested, his call,
   tracked outside this spec.
5. **The old wax-seal identity returns to Signet**, where the signet/seal
   metaphor natively belongs. The *charter* concept (the signed family
   agreement) survives inside Kintrinsic's product language, because it
   truthfully describes the mechanism.

## The mark system: one flame — held, carried, gathered round, kept

*(Revised 2026-08-11 at the Task 4 checkpoint. The candle was rejected — a lone
lit candle reads votive/religious, a connotation this brand won't spend energy
fighting. The lantern and flame-ring were retired with the reassignments below.
Flame candidate A (teardrop with inner counter) won the favicon comparison and
is the DNA glyph, with one tweak: the inner counter drawn slightly larger
relative to the outer flame.)*

The shared DNA glyph is a **single small flame** — the child's own spark. Every
product mark is that same flame with a different **carrier**; the carrier is
what kin provides.

| Mark | Form | Says |
|---|---|---|
| **Kindred** (umbrella) | The bare flame | The spark itself; the family link on every splash screen |
| **Kintrinsic** | The budding flame — a small second flame splitting off the larger one's shoulder, with a real gap between them | Kin catches a spark and it becomes its own light — passed on, not just protected. *The two-hands pass-and-protect gesture (one hand above, one below, flame between) is promoted to hero-illustration use — large-scale contexts only (site hero, marketing pages), never at icon sizes.* |
| **Kindependence** | A campfire — flame over two crossed sticks | Base camp and signal fire: the warm fixed point you range out from, visible because someone chose to light it |
| **Kinclude** | Negative-space flame: the flame is the *gap* between two forms. **Confirmed: Variant 1** — two flames (taller + smaller) leaning together, the gap forming a third flame. (Variant 2, two facing quotation marks, parked.) | Two people's lights, and the space between them is itself warm; the conversation is the light |
| **Kinterest** | A glass jar holding the flame | Light kept, counted, saved, spent — and the jar is glass **because the ledger is shared**: both sides see the light |

Rules: the flame is drawn identically across all marks (same construction, same
proportions) — including as the negative-space silhouette, which must be the
DNA flame's exact outline. Carriers are simple geometric silhouettes,
woodcut-adjacent, never illustrative clutter. Hands are drawn near-single-stroke
(no fingers at small size), asymmetric — never the symmetrical cupped-hands
charity/insurance cliché, never a flat upright palm (Hamsa echo). Future Kin-
apps get a new carrier, never a new flame.

## Kintrinsic's mark

*(Revised 2026-08-11 at the Checkpoint 2 client review. The budding flame is
the mark; the pass-and-protect hands are promoted to hero-illustration status
— see the CHOSEN-2 comment at the top of `img/src/brand/preview.html`.)*

Kintrinsic's mark is **the budding flame**: the DNA flame with a second,
smaller flame — the DNA glyph's path data verbatim — splitting off the larger
one's shoulder, a real gap between them (no join). Phone test: "one flame
catching, becoming two." Calm-literate execution, woodcut-adjacent. This is
Kintrinsic's mark at every size, including the 16px favicon and app icon.

The pass-and-protect hands — one hand from above, one from below, the DNA
flame between them, mid-exchange, sheltered — remain in the system as the
**hero illustration**: large-scale contexts only (site hero banners,
marketing pages, app-store feature graphics), never at icon sizes. At 16–24px
the two-hands read collapses on rendered evidence (client review, Checkpoint
2), while the budding flame stays legible at every size tested; that evidence,
not theory, is what decided it.

## Visual language

- **Palette:** candlelight. Flame amber/gold as the accent; deep warm ink as the
  dark ground; the existing cream paper stays (continuity — the site's literate
  bones survive; migration is a re-warming, not a rip-out). Wax red is released
  back to Signet. Exact values chosen in execution against the contrast tests
  below.
- **Type:** keep the serif-display / humanist-body pairing already on the site.
- **Dark mode:** a candlelit room — the metaphor's native environment. Flame
  glyphs gain, not lose, in dark contexts.

## Hard tests (every mark must pass)

- Legible at a **16px favicon** and as Android adaptive + Linux hicolor icons.
- Works **one-colour** and in greyscale; both themes; WCAG AA contrast in situ.
- **No shields, eyes, locks, or magnifying glasses** anywhere in the system.
- Describable in five words over the phone.
- The four vessels distinguishable from each other at app-icon size.

## Implications for the existing site (kintrinsic.app)

Near-term (with the new mark): replace `seal.svg` and the hero clause-card seal
with the candle mark; shift accent palette from wax red to flame amber;
regenerate `img/og.png`. Charter-the-agreement language stays. The fuller
hearth-light rework of the site's decorative system (§ marks, clause-card
styling) is a **later pass, not a blocker**.

## Deliverables of the implementation phase

1. Flame DNA glyph — constructed SVG.
2. Kintrinsic candle mark: standalone SVG, wordmark lockup, favicon-optimised
   cut, app-icon composition (vessel + flame on warm dark ground).
3. Sketch-grade SVGs of the other three vessels (proof the system holds; not
   final art).
4. All candidates rendered **in context** — site nav, hero, favicon at actual
   size, app-icon frame, light and dark — for decented's selection.
5. Site integration of the chosen mark + palette shift + og image regeneration.

Out of scope: renaming the dev library; app-side icon shipping (charter repo is
migrating separately); the full site decorative rework.
