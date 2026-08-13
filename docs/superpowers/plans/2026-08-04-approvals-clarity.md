# Approvals Clarity Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: superpowers:subagent-driven-development. Steps use `- [ ]` for tracking.

**Goal:** Guardian notifications name the ward and the device; a decision takes one tap and never asks you to "approve" a denial; a pile of repeat asks can be dismissed.

**Architecture:** Three independent slices. (A) a roster pushed PWA → carrier over the existing JS bridge so `Notifier` can resolve `machine` → names. (B) the signer's confirm gate learns which decision it is, and skips entirely for a locally-signed decision. (C) a local-only dismiss on the approvals list that signs nothing.

**Tech Stack:** Kotlin (carrier app), TypeScript/Preact (MyCharter PWA).

**Spec:** `docs/superpowers/specs/2026-08-04-approvals-clarity-design.md` — read it; it carries the reasoning this plan assumes.

## Global Constraints

- **The CODE is the authority, not this plan.** Anchors here (`file:line`) were read on 2026-08-04 and may have drifted. Where this plan and the code disagree, follow the code and say so in your report. Do not transcribe a snippet from this plan without checking it compiles against the real signatures.
- **Never fabricate a user-facing value.** An unresolved `machine` renders today's generic wording — never a guessed name.
- **The roster is a convenience, never a dependency.** Every notification must still fire, with full information minus the names, when the roster is empty, stale, or malformed.
- **Do not change the ward's side of an ask.** No new wire message, no new ward state, no change to `denyRequest`'s behaviour or wording.
- **Android + TypeScript only.** Do NOT touch `linux/`, `core/`, or `android/app/` (the ward app). This work is `android/carrier/` and `apps/charter-app/` only.
- **No new dependencies.**

**Gates.** The PWA is a separate project from the repo root — the root `vitest.config.ts` excludes `apps/**`.
- Carrier: `cd android && ./gradlew :carrier:testDebugUnitTest --rerun-tasks && ./gradlew :carrier:assembleDebug` (source `~/Android/env.sh` if gradle needs the SDK env)
- PWA: `cd apps/charter-app && npm test && npm run lint && npm run build`

**Baselines at branch point:** PWA 883 tests / 78 files, 0 lint errors / 35 warnings. Establish the carrier baseline yourself before changing it and record it.

---

### Task A: Name the ward and the device in carrier notifications

**Files:**
- Modify: `android/carrier/src/main/kotlin/org/forgesworn/mycharter/web/CarrierBridge.kt` (new `@JavascriptInterface` roster setter beside `provision`)
- Modify: the carrier's provision store (find it via `store.provision()` in `CarrierService.kt:168`) — persist the roster the same way provision is persisted
- Modify: `android/carrier/src/main/kotlin/org/forgesworn/mycharter/service/Notifier.kt` — `notifyOverride` (~:67-91) and `notifyRequest` (~:93-120)
- Modify: `android/carrier/src/main/kotlin/org/forgesworn/mycharter/service/CarrierService.kt:176-197` — pass `machine` into `notifyRequest` (it currently does not)
- Modify: `apps/charter-app/src/` — wherever the PWA already calls `provision()` on the bridge; push the roster from the same place
- Test: carrier unit tests for the pure lookup; PWA tests for roster construction

**Interfaces produced:**
- A pure Kotlin `fun describeWard(machine: String, roster: …): WardName?` (or equivalent) returning the child name + device label, or null when unknown. Keep it free of `Context` so it is JVM-testable.
- A pure TS function building the roster array from `Child[]`.

**Behaviour:**
- Roster entry = `{ machine (device pubkey), childName, deviceLabel }`.
- `notifyOverride` text with a hit: names the child and the device, keeping today's warm register and the `scope`/`minutes` facts. Without a hit: **exactly today's string**.
- `notifyRequest` same treatment. Note it does not currently receive `machine` — thread it through from the verdict (`classify.rs:42` confirms requests carry it).
- The notification KEY must stay keyed by machine so a second override still replaces rather than stacks (`Notifier.kt` ~:89 comment explains why).

- [ ] **Step 1:** Establish and record the carrier test baseline.
- [ ] **Step 2:** Write failing tests for the pure lookup: a known machine resolves to name+device; an unknown one returns null; an empty/malformed roster returns null and does not throw.
- [ ] **Step 3:** Implement the roster store + bridge method + lookup. Persist beside provision so it survives a restart.
- [ ] **Step 4:** Wire both notification builders. Verify by test that the no-hit path produces the ORIGINAL strings verbatim.
- [ ] **Step 5:** Push the roster from the PWA at the same point it provisions, and whenever children/devices change. Test the roster builder: a child with two devices yields two entries; a child with none yields none; unpaired devices are still included (a notification can arrive from a device mid-unpair).
- [ ] **Step 6:** Run both gates. Commit.

---

### Task B: One tap to decide, and never "approve" a denial

**Files:**
- Modify: `apps/charter-app/src/signer/mockSigner.ts:26-30` (`ConfirmGate` context type)
- Modify: `apps/charter-app/src/domain/signPrompt.ts` (whole file, 26 lines — read it first)
- Modify: `apps/charter-app/src/store/store.tsx:554` (`confirmGate`) and wherever `pendingSignature` is set/typed
- Modify: `apps/charter-app/src/signer/realSigner.ts:125` and `mockSigner.ts` — the `autoSign || !confirmGate` early return is where the skip belongs
- Modify: `apps/charter-app/src/components/SignSheet.tsx` if the copy needs it
- Test: `signPrompt` tests; a store/signer test proving a local decision does not raise the sheet

**Behaviour:**
1. The gate context carries WHICH decision: extend `action: "clause" | "decision"` so approve and deny are distinguishable. Choose the shape that reads best (a third variant, or a sibling field) and justify it in your report.
2. **Local signer + decision ⇒ no sheet at all.** Resolve as if confirmed. The list button was the confirmation.
3. **External signer + decision ⇒ sheet still shows** (the Signet round trip is real), but a denial must never render the word "Approve". Write copy that reads correctly for a no.
4. **`action: "clause"` is completely unchanged** — same sheet, same copy, same behaviour. Prove it with a test.
5. Auto-sign on ⇒ unchanged (already skips).

**Determining "local":** `signPrompt.ts:10` uses `const LOCAL_LABEL = "This phone"` compared against `signerLabel`. Check whether a more robust signal exists (the signer's `kind`, per `selectSigner.ts:29`) and prefer it — a string compare against a display label is fragile. Justify your choice.

- [ ] **Step 1:** Write failing tests: local+approve ⇒ no sheet; local+deny ⇒ no sheet; external+deny ⇒ sheet whose CTA contains no "Approve"; clause ⇒ sheet unchanged.
- [ ] **Step 2:** Extend the gate context and `signPrompt`.
- [ ] **Step 3:** Implement the skip at the signer's gate call, not in the Approvals screen — every decision path must benefit, not just the one button.
- [ ] **Step 4:** Run gates. Commit.

---

### Task C: Dismiss a request without answering it

**Files:**
- Modify: `apps/charter-app/src/store/store.tsx` (new `dismissRequest(id)` beside `denyRequest`, ~:1899 / exported ~:2134)
- Modify: `apps/charter-app/src/screens/Approvals.tsx` (the request `Card`, ~:570-604)
- Test: store test for dismiss semantics; screen test that the control exists and is distinct from Not now

**Behaviour:**
- `dismissRequest(id)` removes the request from local state ONLY. No signer call, no clause, no relay traffic, no change to ward state. It must work with no signer connected and offline.
- It must be idempotent and must persist (a dismissed request must not reappear on reload — check how requests are stored and ensure dismissal survives; if requests are re-hydrated from the relay, a dismissed-ids set is needed).
- **UI:** a small ✕ in the card's top-right corner. `aria-label="Dismiss"`. Visually quiet — this is the lightest of the three actions and must never compete with Approve or Not now.
- No confirmation dialog. It is reversible in effect (the ward can ask again) and signs nothing.
- Available on every request.

- [ ] **Step 1:** Write failing tests: dismiss removes exactly one request; leaves others; does not call the signer; survives a reload; is idempotent.
- [ ] **Step 2:** Implement `dismissRequest` + persistence.
- [ ] **Step 3:** Add the ✕ control. Keep Approve/Not now untouched.
- [ ] **Step 4:** Run gates. Commit.

---

### Task D: Full gates, merge, push

- [ ] Both gate suites clean; `git diff --stat main -- linux/ core/ android/app/` empty.
- [ ] Merge to `main`, re-run BOTH gates on the merged result, push.
- [ ] Note in the report that the carrier APK needs a real signed release before the notification change reaches decented's phone — a PWA deploy alone does not ship carrier Kotlin.
