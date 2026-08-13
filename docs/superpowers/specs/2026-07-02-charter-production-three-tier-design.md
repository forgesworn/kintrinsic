# Charter to production — the three-tier family stack (design)

**Date:** 2026-07-02
**Status:** approved-in-principle (bring-up path + signer decision confirmed); implementation pending
**Author:** lead engineer (Claude) + decented (founder / CEO-CTO)
**Forcing function:** "Family Freedom Tech" talk, ~17 July 2026 — a *milestone*, not the deliverable.

## North star

Ship, to the world, a **production-working** family stack that a non-technical
home-educating family can actually run — not a demo. Three tiers, all genuinely
live:

1. **T1 — Charter on the laptop.** A guardian governs a child's account on Linux
   Mint: screen-time schedule + daily cap, app-install control, enforced (freeze
   + lock).
2. **T2 — Phone governs the laptop.** The guardian sets limits and approves/denies
   from their phone (MyCharter PWA), signed and delivered over the relay; the
   device publishes status back so the phone can show it.
3. **T3 — Phone governs a second phone.** The same wardship model extended to the
   child's GrapheneOS phone via Android Device Owner.

Positioning after it works: the **Home Ed community** (families taking
responsibility for their own children rather than the state) — many Linux users,
strong values alignment.

Non-negotiables: **normie-friendly** (no terminal required for the family) and
**production-ready**, in the shortest wall-clock we can manage. Token budget is not
a constraint.

## Estimating model (two clocks)

Per `CLAUDE.md`: price against *our* velocity, not generic-dev pace. Code
throughput is not the bottleneck; the bottlenecks are **serial hardware/test
rounds only decented can run** and **net-new platform toolchains**. Every tier is
tracked on two clocks:

| Tier | Code-ready | Hardware-verified & shipped |
|---|---|---|
| T1 laptop | already built (warden, ~390 tests, `.deb`) | ~1–2 live test rounds on real Mint — plausibly one evening |
| T2 phone→laptop | ~a focused session (self-signer + STATUS emission + real relay) | ~2–3 days, gated by live pairing rounds |
| T3 phone→phone | spike first (bench, 1–2 days, parallel), then net-new DO app | ~a week if the spike is green |

**All three ≈ 1 week; T1+T2 shippable days earlier.** Comfortably before the 17th.

## Strategic decisions

### D1 — Guardian signer: MyCharter self-signs (Signet is the upgrade seam)
The MyCharter PWA holds its own guardian BIP-340 key and signs clauses/grants
directly, via the injection seam `realSigner.ts` already exposes (a local-key
`GuardianOps` in place of the NIP-46 bunker `signetSigner.ts` provides). This is a
**real production signer**, not a demo trick: real key, real NIP-59 gift-wrap, the
laptop pins that one guardian pubkey exactly as designed. Zero Signet-repo work.

- **Signet is not cut** — it remains the "bring your own identity" upgrade on the
  existing seam, wired later on the Signet side.
- **Added scope this creates:** solid **key backup/recovery UX** in the PWA
  (recovery phrase / export) — required for production regardless.
- **Trust model unchanged:** the laptop still pins exactly one guardian pubkey;
  which key signs is all that changes.

### D2 — T1 bring-up is observe-mode-first on the real daily driver
The warden has never run live on real hardware, and it is designed to be hard to
escape. The daily driver is protected by a staged, reversible bring-up (see
Safety model). **Not** a VM-first path (VM can't exercise the real display grab /
VT lock); the real box is the target, de-risked.

## Tier detail

### T1 — laptop warden
**State:** complete + gated (mock + `--features real` core tests + `.deb`). The
subscribe→verify→enact→enforce loop, freeze (cgroup-v2 app-slice), on-display lock
(`charter-lock`, x11rb), VT lock, web filter, multi-child + device-only limits,
Signet-first per-child precedence, recovery rails — all built.

**Gap to done:** first live run on real Mint; fix real-syscall bugs (cgroup path,
display grab, VT ioctl); prove enforcement + recovery on hardware; normie install
(`.deb` double-click → "Charter Setup" → "Charter Screen Time").

**Definition of done:** on decented's Mint laptop, the child account is enforced
(schedule + cap → freeze + lock), the guardian can pause/recover graphically, and
`sudo systemctl stop charterd` always thaws. No terminal needed for the family.

### T2 — phone governs laptop
**State:** MyCharter PWA built (RealSigner + signetSigner NIP-46 + Policy→CLAUSE
gift-wrap + real device pairing UI); device ingests per-`(subject,kind)` clauses;
wire contract frozen. **Device STATUS feed (kind 31114) is contract-frozen but the
device does not yet emit it** — so the PWA can send limits but can't yet *show*
live state.

**Gap to done:** (a) local-key guardian signer in the PWA (D1) + key backup UX;
(b) real relay end-to-end (publish/subscribe over wss); (c) live pairing handshake
phone↔laptop (`bunker://` pin, device-code/QR — the flows exist, need live
proving); (d) **device STATUS emission** so the PWA shows each child's
time-left/lock state; (e) PWA deploy (auto on push to `main`).

**Definition of done:** from the phone, decented sets a limit / approves "ask for
more time"; it signs, rides the relay, the laptop enforces it, and the phone shows
the child's live status.

### T3 — phone governs phone (GrapheneOS Device Owner)
**State:** none built. Spike-gated by `graphene/spike.md` (T0–T11 Device-Owner
capability sweep on real hardware — a go/no-go gate that must pass *before* code).
Architecture captured in `graphene/concept.md` + `graphene/ultracode-brief.md`.

**Gap to done:** run the spike (bench, decented); if green, build the Device-Owner
enforcer (net-new Android artifact) reusing the OS-agnostic "decide" half; live
pairing + enforcement on the second phone.

**Definition of done:** the child's GrapheneOS phone is governed from decented's
phone on the same Charter model — install lockdown + screen-time gate — live.

## Safety model (T1 bring-up)

The dangerous hardening is **not** armed by a normal install:
- `.deb postinst` only registers + starts the daemon. No `fapolicyd` default-deny,
  no immutable files, no `$HOME` noexec — those are an explicit later hardening
  pass, not setup.
- **Rails already built:** `charter-setup` brick-guard (refuses to manage the sole
  admin → guardian keeps sudo); polkit lockdown bites only the `charter-managed`
  group (guardian unaffected); `charterd.service` `ExecStopPost=charter-recovery
  --thaw-all` (any stop — crash, `systemctl stop`, uninstall — thaws all slices +
  re-enables VT switching); graphical Recovery (pause/resume/off/thaw) behind the
  admin password; freeze target always the managed child's app-slice.
- **The one system-wide change** in standard setup: `charter-setup` enables
  `/tmp` + `/dev/shm` as noexec tmpfs. **Skipped for first run.**

**New work — `CHARTER_ENFORCE=observe` soft mode:** the daemon logs *"would
freeze/lock \<child\> now"* and applies **nothing** (no freeze, lock, or VT). There
is a pause flag today but no log-only mode. This makes the first real-hardware run
carry zero lock/freeze risk while proving display + cgroup targeting.

**Bring-up sequence (turnkey runbook to follow):**
1. Build observe mode; hand decented the runbook.
2. decented: Timeshift snapshot (instant rollback).
3. Install `.deb`; `charter-setup` on the **child** account; **skip noexec
   mounts**; run **observe mode**; keep a root terminal open.
4. Verify correct *decisions* against the real child session (zero risk).
5. Escalate: observe → freeze-only → verify recovery → enable lock → (later)
   hardening pass. Two steps forward, one back.

## Execution — three parallel tracks

- **Track A (lead engineer):** T1 real-hardware bring-up (observe mode → runbook →
  fix live bugs), then T2 (self-signer + key backup + STATUS emission + real
  relay), then T3 build once the spike is green.
- **Track B (decented, bench):** run the T3 spike now (overlaps everything); then be
  the hardware tester at each gate with turnkey asks.
- **Track C (the sysadmin):** PWA deploy (auto on push to `main`); Signet-side upgrade
  (D1 seam) if/when wanted.

**Gates needing decented (turnkey asks at each):** spike T0–T11 · Timeshift + observe
run · escalate-to-freeze · escalate-to-lock · T2 live pairing · T3 device flashing.

## Open [decide] items
- Key backup/recovery UX shape in the PWA (recovery phrase vs file export vs both).
- STATUS emission cadence (device-side) — align with the contract's `[decide]`.
- T3 build toolchain + language (Kotlin DO app) — settled by the spike + ultracode-brief.
- Whether the noexec/hardening pass ships in the family default or stays opt-in.

## Sequencing (critical path)
Spike (Track B, parallel) ∥ [ observe mode → T1 bring-up → T1 done ] → T2 →
(spike green) T3. T1+T2 ship to the world independent of T3.
