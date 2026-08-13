# Charter for GrapheneOS

A sibling target to [`linux/`](../linux) (the `charterd` warden): bringing Charter's
guardian-led, guardian-signed management to a **GrapheneOS** phone so it can be safely
used by a ward — screen-time scheduling and app-install control, on the same Charter
protocol and the same guardian approval flow.

> **Status: conceptual exploration only.** No code, no spec, no committed engineering.
> This folder currently holds the captured thinking from an early feasibility
> conversation. It exists so the idea has a home and can grow. Nothing here is a
> commitment to build.

## The objective (in one breath)

A ward's GrapheneOS phone where:

- **Screen time** is scheduled and enforced (per app, per the Charter model).
- **App installation is closed off** — the ward can't add apps from *any* channel
  (Play, F-Droid, Aurora, Accrescent, the GrapheneOS App Store, raw APK).
- The **guardian is the only door in**: apps arrive as guardian-approved, guardian-signed
  Charter grants — the same `request → guardian signs → verify → enact` loop the
  Linux warden already uses.
- No reliance on Google: GrapheneOS's sandboxed Play can't do Google child accounts /
  Family Link, so the ward runs an adult-shaped account or none. **Charter is the
  supervisor** — it doesn't need Google.

## The one-line feasibility verdict

Feasible at the **"cooperative / anti-casual"** tier (the same threat model `charterd`
already declares). You reuse the *decide* half of Charter and rebuild the *enforce*
half against Android's **Device Owner** APIs — and the enforce half is **smaller** on
a phone than on Linux, because Android gives you the app sandbox and verified boot for
free.

## Contents

- [`concept.md`](concept.md) — the full captured feasibility conversation: model
  mismatch, what carries over, the Device Owner architecture, the install-lockdown
  insight, the Duolingo / update-engine discussion, and the hard limits.
- [`spike.md`](spike.md) — the on-metal Device Owner capability sweep to run *before* any
  build: a fillable Go/No-Go checklist using an off-the-shelf DPC (TestDPC/OwnDroid), no
  Charter code required.
- [`ultracode-brief.md`](ultracode-brief.md) — the turnkey kickoff plan for when the build
  is greenlit ("ultracode GrapheneOS — build it all"): prerequisites/gates, the workflow
  phases (starting with a deep review of the hardened Linux impl), inputs, and the
  toolchain setup. Read this first at kickoff.

## See also

- [`../ubuntu-touch/`](../ubuntu-touch) — the same assessment for Ubuntu Touch (UBports),
  the Linux-native alternative phone target.
- [`../phone-targets.md`](../phone-targets.md) — GrapheneOS vs Ubuntu Touch side-by-side.

## Why factory reset isn't the threat

GrapheneOS deliberately has **no Factory Reset Protection**: a ward who knows the trick
can boot to recovery and wipe the device, and `DISALLOW_FACTORY_RESET` only blocks the
in-Settings path, not recovery. In an adversarial design that would be the fatal hole.
In a **trust-aligned** one it isn't the threat at all — it's the same accepted escape as
a ward booting **Tails from a USB stick** past the Linux warden. The tools exist to help a
cooperating family succeed, not to cage an adversary.

Two properties make that safe to stand behind:

- **The audit gap is the tell.** Charter logs screen time continuously, so a wiped phone
  that's still in a ward's hand shows up as *use with no time logged* — the guardian sees
  the audit stream go dark while the device is plainly still in use. Detection lives
  **guardian-side**, not on the phone (a wiped phone reports nothing); the signal is the
  *absence* of the expected heartbeat, the same way a laptop that stops checking in is.
- **A reset can't carry the charter forward.** Re-enrollment mints a **new device** — the
  guardian signs the fresh install as new, and the old charter and its grants cannot be
  migrated or replayed onto it. A wipe destroys the device identity grants were bound to;
  the only way back under Charter is through the guardian. Nobody resurrects a wiped phone
  by re-injecting the old signed charter to skip the door.

This is **detect-not-prevent**, and it's exactly the **cooperative / anti-casual** posture
Charter already owns — nothing philosophically changes.
