// The real (Signet-backed) Signer — orchestrates the parent's rule change into
// signed, gift-wrapped CLAUSEs published to the child's device(s). It implements
// the same `Signer` seam the screens already use, so no UI changes are needed.
//
// Transport is INJECTED (connectGuardian / publish): in production those are the
// NIP-46 bunker + a relay pool (see ./signetSigner.ts); in tests they're local
// keys + a capture. So this orchestration — the load-bearing part — is fully
// unit-tested without a live bunker or relay.

import type { NostrEvent } from "nostr-tools";
import type { Policy, SignerKind, SignerState } from "../domain/types";
import { appRulesToGrant, policyToClauses, updateToGrant } from "../wire/clause";
import { effectivePolicyForDevice } from "../domain/effectivePolicy";
import type { ClausePayload, GrantGift, GrantStandDown, UpdateManifest } from "../wire/types";
import {
  giftWrapClause,
  giftWrapGrant,
  giftWrapPairOffer,
  giftWrapRelease,
  giftWrapUsageSync,
  type GuardianOps,
} from "../wire/giftwrap";
import type { UsageSyncPayload } from "../wire/usageSync";
import {
  buildAppOpenGrant,
  buildInstallApkGrant,
  buildTimeExtendGrant,
  clampGrantMinutes,
  endOfDayUnix,
  DEFAULT_STANDDOWN_GRACE_SECS,
} from "../wire/grant";
import type { DecisionContext, Signer } from "./Signer";
import { SignerCancelled, type ConfirmGate } from "./mockSigner";

/** One deliverable device: its domain id, and the key its wraps are sealed to. */
export interface TargetDevice {
  /** `Device.id` — what per-device rule overrides are keyed by. */
  id: string;
  /** The device's own pubkey hex (the gift-wrap recipient). */
  pubkey: string;
}

/** Where one child's clauses must be delivered. */
export interface ChildTarget {
  /** The child's dependant pubkey hex (== contract `subject`); null = single-child default. */
  subject: string | null;
  /** The child's display name. Errors and confirm sheets render it — a
   *  guardian reads "pair Sam's phone first", never `pair child_lx3k9_4's`. */
  name?: string;
  /**
   * Every paired, deliverable device. Carries the id as well as the key because
   * a child's charter can be split per device (Half A) — resolving an override
   * needs to know WHICH device a wrap is being sealed for, not just its key.
   */
  devices: TargetDevice[];
  /** Relays to publish the gift-wraps to. */
  relays: string[];
}

export interface RealSignerDeps {
  /** Establish a guardian session (prod: NIP-46 bunker connect). */
  connectGuardian: (kind: SignerKind) => Promise<GuardianOps>;
  /** Publish a finalized event to the given relays. */
  publish: (relays: string[], event: NostrEvent) => Promise<void>;
  /** Resolve a child id to its subject + device pubkeys + relays. */
  resolveChild: (childId: string) => ChildTarget | undefined;
  /**
   * The child's current policy list. App-scope policies aggregate into ONE
   * `appRules` clause (replace-the-set, like `learning`), so signing one app
   * rule needs the child's WHOLE app-scope set — not just the policy being
   * saved. The just-saved policy is merged over its stale copy here. Absent =
   * only the saved policy is carried (device-scope saves never consult this).
   */
  childPolicies?: (childId: string) => Policy[];
  /** Current unix seconds — the clause `issuedAt`. */
  now: () => number;
  initial?: SignerState;
  /**
   * When set, an in-app confirm step gates each authorization while auto-sign is
   * off — the local "this phone" signer. Absent = no in-app gate (e.g. Signet,
   * whose bunker approves out-of-band).
   */
  confirmGate?: ConfirmGate;
}

function labelFor(kind: SignerKind): string | undefined {
  if (kind === "signet") return "Signet";
  if (kind === "heartwood") return "Heartwood";
  if (kind === "local") return "This phone";
  return undefined;
}

export class RealSigner implements Signer {
  private state: SignerState;
  private guardian: GuardianOps | null = null;
  /** The last clause `issuedAt` this signer handed out — see `nextIssuedAt`. */
  private lastIssuedAt = RealSigner.loadLastIssuedAt();

  private static readonly ISSUED_AT_KEY = "charter.signer.lastIssuedAt";

  private static loadLastIssuedAt(): number {
    try {
      const n = Number(localStorage.getItem(RealSigner.ISSUED_AT_KEY));
      return Number.isFinite(n) && n > 0 ? n : 0;
    } catch {
      return 0;
    }
  }

  /**
   * The `issuedAt` for the next clause: now, but STRICTLY above every one
   * signed before. The device's per-(subject, kind) rollback floor DROPS a
   * clause whose `issuedAt` does not exceed the standing one's, and `now()` is
   * whole seconds — so two clauses of a kind inside one second silently lost
   * the second: a lift that left the ward locked till midnight, a second
   * "give 5 minutes" the log said had landed, the `apps` clause that actually
   * opens an approved app. Stand-down used to carry its own per-session bump;
   * one clock for every clause covers them all, and it is persisted so a
   * reload inside the same second can't step back under it. It only ever runs
   * ahead by as many seconds as clauses were signed in a burst; the device has
   * no future-skew gate (its own clock may be dead), so that costs nothing.
   */
  private nextIssuedAt(): number {
    const issuedAt = Math.max(this.deps.now(), this.lastIssuedAt + 1);
    this.lastIssuedAt = issuedAt;
    try {
      localStorage.setItem(RealSigner.ISSUED_AT_KEY, String(issuedAt));
    } catch {
      // Private mode / quota — the in-memory floor still holds this session.
    }
    return issuedAt;
  }

  constructor(private deps: RealSignerDeps) {
    this.state = deps.initial ?? { connected: false, kind: "none", autoSign: false };
  }

  status(): SignerState {
    return this.state;
  }

  async connect(kind: SignerKind): Promise<SignerState> {
    this.guardian = await this.deps.connectGuardian(kind);
    this.state = { ...this.state, connected: true, kind, label: labelFor(kind) };
    return this.state;
  }

  async disconnect(): Promise<void> {
    this.guardian = null;
    this.state = { connected: false, kind: "none", autoSign: this.state.autoSign };
  }

  async setAutoSign(on: boolean): Promise<void> {
    this.state = { ...this.state, autoSign: on };
  }

  /**
   * In-app confirm step while auto-sign is off. No-op when no gate is wired.
   *
   * A `"decision"` on the LOCAL signer (`this.state.kind === "local"`) skips
   * the gate too — the Approve/Not now press on the request list already IS
   * the confirmation, so a second sheet adds nothing. `"clause"` (a rule
   * edit) always still goes through the gate: a bulk change is a different
   * act from a one-tap answer, and "nothing changes until you confirm" is
   * genuinely worth saying for it, local signer included.
   */
  private async authorize(
    action: "clause" | "decision",
    detail: string,
    decision?: "approved" | "denied",
  ): Promise<void> {
    const skipGate = action === "decision" && this.state.kind === "local";
    if (this.state.autoSign || !this.deps.confirmGate || skipGate) return;
    const ok = await this.deps.confirmGate({
      action,
      decision,
      signerLabel: this.state.label ?? "your approval app",
      signerKind: this.state.kind,
      detail,
    });
    if (!ok) throw new SignerCancelled();
  }

  /**
   * Refuse to report success when a clause has nowhere to go.
   *
   * Every guardian-initiated publish below is `for (const device of
   * target.devices) { … }` followed by an unconditional `return { ok: true }`.
   * With an empty device list — or no relay to reach one — the loop body simply
   * never runs, nothing is signed, nothing travels, and the caller is told it
   * worked. `GiveTime` then prints "15 minutes sent to <ward>" over a wire that
   * carried nothing, which is precisely what its own comment forbids ("Never
   * claim minutes travelled when nothing was signed"). A guardian who is told
   * the minutes landed has no reason to look again, so the failure is invisible
   * on both ends at once — the ward stays locked and the parent is certain they
   * unlocked them.
   *
   * Throwing is deliberate: these paths already surface `err.message` to the
   * guardian, so a refusal reaches a human instead of a console. Automated
   * feeds (`publishUsageSyncs`) are left alone — they are reflection, not a
   * promise to anybody, and an empty list there is normal.
   *
   * Guards the ONE-OFF, delivery-only acts — a gift, a maintenance window, an
   * update push — whose whole value is arriving now. Deliberately NOT
   * `signClause`: a charter is durable state that legitimately exists before
   * any phone is paired (set the rules, then pair), and `signClauseIfConnected`
   * only applies the change locally once signing resolves, so refusing there
   * would stop a guardian saving rules at all for a not-yet-paired ward. Two
   * tests pin that down; they were right and a blanket guard was wrong.
   */
  private requireDeliverable(target: ChildTarget, childId: string): void {
    const who = target.name ?? childId;
    if (target.devices.length === 0) {
      throw new Error(
        `No paired device to send this to yet — pair ${who}'s phone first, then try again.`,
      );
    }
    if (target.relays.length === 0) {
      throw new Error(
        `No relay is set up to reach ${who}'s device, so this couldn't be sent.`,
      );
    }
  }

  async signClause(childId: string, policy: Policy): Promise<{ ok: true }> {
    const guardian = this.guardian;
    if (!guardian) throw new Error("signer not connected");
    const target = this.deps.resolveChild(childId);
    if (!target) throw new Error(`unknown child: ${childId}`);

    await this.authorize("clause", `Rule change for ${target.name ?? childId} (${policy.id})`);

    const issuedAt = this.nextIssuedAt();
    if (policy.scope.kind === "app") {
      // App-scope rebuilds the child's ONE aggregate `appRules` clause, which
      // is the same set for every device — per-app rules are not splittable.
      const clauses = this.appRulesClauses(childId, policy, target.subject, issuedAt);
      for (const clause of clauses) {
        for (const device of target.devices) {
          const wrap = await giftWrapClause(clause, device.pubkey, guardian, issuedAt);
          await this.deps.publish(target.relays, wrap);
        }
      }
      return { ok: true };
    }

    // Device scope: each device gets the charter as it applies to IT — the base
    // with that device's own overrides laid over the top. With nothing split
    // this is the identical clause for everyone, byte for byte, exactly as
    // before per-device rules existed. `effectivePolicyForDevice` also strips
    // the override map, so no device ever learns its siblings' rules.
    for (const device of target.devices) {
      const effective = effectivePolicyForDevice(policy, device.id);
      const clauses = policyToClauses(target.subject, effective, issuedAt);
      for (const clause of clauses) {
        const wrap = await giftWrapClause(clause, device.pubkey, guardian, issuedAt);
        await this.deps.publish(target.relays, wrap);
      }
    }
    return { ok: true };
  }

  /**
   * Publish each device its consolidated cross-device usage view (USAGE_SYNC,
   * 31115 — B3). Each device gets a DISTINCT payload (its own usage excluded).
   * Deliberately NOT confirm-gated: this is the automated reflection feed the
   * store re-publishes as usage flows in, not a policy change — prompting for
   * it would train everyone to click through the gate that matters.
   */
  async publishUsageSyncs(
    childId: string,
    payloadsByDevice: Record<string, UsageSyncPayload>,
  ): Promise<{ ok: true }> {
    const guardian = this.guardian;
    if (!guardian) throw new Error("signer not connected");
    const target = this.deps.resolveChild(childId);
    if (!target) throw new Error(`unknown child: ${childId}`);
    const createdAt = this.deps.now();
    for (const [devicePubkey, payload] of Object.entries(payloadsByDevice)) {
      if (!target.devices.some((d) => d.pubkey === devicePubkey)) continue;
      const wrap = await giftWrapUsageSync(payload, devicePubkey, guardian, createdAt);
      await this.deps.publish(target.relays, wrap);
    }
    return { ok: true };
  }

  /**
   * Open a maintenance window: for a short, signed, EXPIRING span the ward
   * stands down its install lock so a cabled phone can be repaired. Exists
   * because our own install lock plus a broken self-updater once left a phone
   * with no remote recovery at all — the rescue must not depend on the thing
   * that broke.
   */
  async signMaintenanceClause(childId: string, minutes: number): Promise<{ ok: true }> {
    const guardian = this.guardian;
    if (!guardian) throw new Error("signer not connected");
    const target = this.deps.resolveChild(childId);
    if (!target) throw new Error(`unknown child: ${childId}`);
    this.requireDeliverable(target, childId);
    await this.authorize(
      "clause",
      minutes <= 0
        ? `Close the install window on ${target.name ?? childId}'s phone now`
        : `Allow installs on ${target.name ?? childId}'s phone for ${minutes} minutes`,
    );
    const issuedAt = this.nextIssuedAt();
    // Absolute expiry, computed once: a duration would restart on every
    // re-read of the stored clause and the window would never shut.
    //
    // `minutes <= 0` is how a guardian CLOSES a window early: the expiry lands
    // at (or before) issue, so the ward's `is_open` — which shuts on
    // `untilUnix <= now` — reads it closed on the very next tick, and the
    // store's monotonic `issuedAt` floor makes this newer clause supersede the
    // open one. The same shape lifting a stand-down uses. Nothing special is
    // needed on the phone, which is exactly why it is safe: a guardian ending
    // a loosening early must never depend on new ward code.
    const body = { v: 1 as const, untilUnix: issuedAt + minutes * 60, issuedAt };
    const clause: ClausePayload = { v: 1, kind: "maintenance", issuedAt, body };
    if (target.subject) clause.subject = target.subject;
    for (const device of target.devices) {
      const wrap = await giftWrapClause(clause, device.pubkey, guardian, issuedAt);
      await this.deps.publish(target.relays, wrap);
    }
    return { ok: true };
  }

  /**
   * Give time nobody asked for. Answering an ask was the only way to add
   * minutes — a GRANT must echo a pending request — so with nothing
   * outstanding, or after a "no" you've thought better of, the only lever was
   * rewriting the schedule (decented, 2026-07-26). This is a plain signed clause
   * instead, additive and dead at end of day.
   */
  async signGiftClause(
    childId: string,
    minutes: number,
    tz?: string,
    groupId?: string,
  ): Promise<{ ok: true }> {
    const guardian = this.guardian;
    if (!guardian) throw new Error("signer not connected");
    const target = this.deps.resolveChild(childId);
    if (!target) throw new Error(`unknown child: ${childId}`);
    this.requireDeliverable(target, childId);
    await this.authorize("clause", `Give ${minutes} more minutes to ${target.name ?? childId}`);
    const issuedAt = this.nextIssuedAt();
    // Each gift needs its own identity: the device's extension ledger applies
    // an id exactly once, which is what stops a clause it re-reads every tick
    // from topping the ward up forever — and what lets a SECOND gift add again.
    const bytes = new Uint8Array(8);
    crypto.getRandomValues(bytes);
    const id = `${issuedAt}-${Array.from(bytes, (b) => b.toString(16).padStart(2, "0")).join("")}`;
    const body: GrantGift = {
      v: 1,
      issuedAt,
      id,
      minutes: clampGrantMinutes(minutes),
      // End of day in the CHILD's tz, exactly like a granted ask's exp — never
      // the guardian phone's midnight, which can land on the wrong side of the
      // device's own day roll.
      expiresAt: endOfDayUnix(issuedAt, tz),
    };
    // Named times: top up THAT group's own extension pool rather than the
    // whole-device one. Additive (`v: 1` unchanged) — an old ward just ignores
    // the key and lands the gift as device time anyway (errs generous).
    if (groupId) body.groupId = groupId;
    const clause: ClausePayload = { v: 1, kind: "gift", issuedAt, body };
    if (target.subject) clause.subject = target.subject;
    for (const device of target.devices) {
      const wrap = await giftWrapClause(clause, device.pubkey, guardian, issuedAt);
      await this.deps.publish(target.relays, wrap);
    }
    return { ok: true };
  }

  /**
   * Call — or lift — a stand-down: "finish up now", then done for today.
   *
   * The mirror of `signGiftClause`, and deliberately built the same way: a fresh
   * `id` per stand-down, and `expiresAt` at end of the WARD's day so a
   * stand-down the guardian forgets about lapses instead of quietly becoming
   * permanent. Never the guardian phone's midnight — that can fall the wrong
   * side of the device's own day roll.
   *
   * Lifting publishes a stand-down that has ALREADY expired. That is not a
   * trick: the device's rule is "the standing clause with the highest issuedAt
   * wins, and an expired one does not stand", so one code path both calls and
   * clears, and a lift can never be reordered behind the call it lifts.
   */
  async signStandDownClause(
    childId: string,
    opts?: { lift?: boolean; graceSecs?: number; tz?: string },
  ): Promise<{ ok: true }> {
    const guardian = this.guardian;
    if (!guardian) throw new Error("signer not connected");
    const target = this.deps.resolveChild(childId);
    if (!target) throw new Error(`unknown child: ${childId}`);
    this.requireDeliverable(target, childId);
    const lift = opts?.lift === true;
    const who = target.name ?? childId;
    await this.authorize(
      "clause",
      lift ? `Allow ${who} back on` : `Ask ${who} to finish up now`,
    );
    // Strictly above the last stand-down — a lift signed in the same second
    // as the call must supersede it (see `nextIssuedAt`).
    const issuedAt = this.nextIssuedAt();
    const bytes = new Uint8Array(8);
    crypto.getRandomValues(bytes);
    const id = `${issuedAt}-${Array.from(bytes, (b) => b.toString(16).padStart(2, "0")).join("")}`;
    const body: GrantStandDown = {
      v: 1,
      issuedAt,
      id,
      // A lift is an already-dead stand-down; `issuedAt` still climbs, so it
      // supersedes whatever stands. Stamped a full DAY into the past, not
      // "now": a warden comparing the expiry only against its own slower
      // clock must still read it as expired — `expiresAt: issuedAt` re-locked
      // a ward whose device ran behind the guardian's. (Wardens with the
      // clock-independent `expiresAt <= issuedAt` rule don't need this; the
      // ones already shipped do.)
      expiresAt: lift ? issuedAt - 86_400 : endOfDayUnix(issuedAt, opts?.tz),
      graceSecs: opts?.graceSecs ?? DEFAULT_STANDDOWN_GRACE_SECS,
    };
    const clause: ClausePayload = { v: 1, kind: "standdown", issuedAt, body };
    if (target.subject) clause.subject = target.subject;
    for (const device of target.devices) {
      const wrap = await giftWrapClause(clause, device.pubkey, guardian, issuedAt);
      await this.deps.publish(target.relays, wrap);
    }
    return { ok: true };
  }

  async signUpdateClause(
    childId: string,
    manifest: UpdateManifest,
    url: string,
  ): Promise<{ ok: true }> {
    const guardian = this.guardian;
    if (!guardian) throw new Error("signer not connected");
    const target = this.deps.resolveChild(childId);
    if (!target) throw new Error(`unknown child: ${childId}`);
    this.requireDeliverable(target, childId);

    await this.authorize(
      "clause",
      `Update Kintrinsic to ${manifest.versionName} on ${target.name ?? childId}'s devices`,
    );

    const issuedAt = this.nextIssuedAt();
    const body = updateToGrant(manifest, url);
    const clause: ClausePayload = { v: 1, kind: "update", issuedAt, body };
    if (target.subject) clause.subject = target.subject;
    for (const device of target.devices) {
      const wrap = await giftWrapClause(clause, device.pubkey, guardian, issuedAt);
      await this.deps.publish(target.relays, wrap);
    }
    return { ok: true };
  }

  /**
   * App-scope: aggregate ALL of the child's app-scope policies into ONE
   * `appRules` clause (replace-the-set). The just-saved `saved` policy is
   * authoritative for its own id — it's merged over the (still stale, pre-
   * commit) copy in the child's policy list — so the device receives the
   * parent's true current per-app intent for every app at once.
   */
  private appRulesClauses(
    childId: string,
    saved: Policy,
    subject: string | null,
    issuedAt: number,
  ): ClausePayload[] {
    const others = (this.deps.childPolicies?.(childId) ?? []).filter(
      (p) => p.scope.kind === "app" && p.id !== saved.id,
    );
    const body = appRulesToGrant([...others, saved], issuedAt);
    const clause: ClausePayload = { v: 1, kind: "apprules", issuedAt, body };
    if (subject) clause.subject = subject;
    return [clause];
  }

  async releaseDevice(
    machinePubkey: string,
    relays: string[],
    label?: string,
  ): Promise<{ ok: true }> {
    const guardian = this.guardian;
    if (!guardian) throw new Error("signer not connected");
    // Only a real 64-hex machine key is a deliverable target.
    if (!/^[0-9a-f]{64}$/.test(machinePubkey)) return { ok: true };
    // Name the device when we know it (S9). "Release device ab12cd34…" tells
    // a parent nothing they could disagree with; "Disconnect Rook's phone —
    // it stops following your rules" tells them exactly what they are about
    // to undo.
    await this.authorize(
      "clause",
      label
        ? `Disconnect ${label} — it stops following your rules`
        : `Release device ${machinePubkey.slice(0, 8)}…`,
    );
    const wrap = await giftWrapRelease(machinePubkey, guardian, this.deps.now());
    await this.deps.publish(relays, wrap);
    return { ok: true };
  }

  async sendPairOffer(
    machinePubkey: string,
    token: string,
    relays: string[],
  ): Promise<{ ok: true }> {
    const guardian = this.guardian;
    if (!guardian) throw new Error("signer not connected");
    // Only a real 64-hex machine key is a deliverable target.
    if (!/^[0-9a-f]{64}$/.test(machinePubkey)) return { ok: true };
    if (!/^[0-9a-f]{32}$/.test(token)) return { ok: true };
    await this.authorize("clause", `Connect to computer ${machinePubkey.slice(0, 8)}…`);
    const wrap = await giftWrapPairOffer(
      machinePubkey,
      token,
      relays,
      guardian,
      this.deps.now(),
    );
    await this.deps.publish(relays, wrap);
    return { ok: true };
  }

  async signDecision(
    requestId: string,
    decision: "approved" | "denied",
    ctx?: DecisionContext,
  ): Promise<{ ok: true }> {
    // The Signet bunker has no NIP-46 decision flow yet (its approvals happen
    // out-of-band, and GRANT signing over NIP-46 is a later seam) — explicit,
    // never a silent no-op. The confirm-gated signer (local) is the real path.
    if (!this.deps.confirmGate) {
      throw new Error("remote GRANT signing is not implemented yet");
    }
    await this.authorize("decision", `${decision} request ${requestId}`, decision);

    // install.apk: build + gift-wrap the InstallApk GRANT pinning the catalog's
    // signer digest (ctx.install carries it — never the phone's request). The
    // device re-checks signing continuity against the staged bytes before it
    // installs. Answered as its own path since it echoes no time/limit fields.
    const install = ctx?.install;
    if (install) {
      const guardian = this.guardian;
      if (!guardian) throw new Error("signer not connected");
      const target = this.deps.resolveChild(ctx!.childId);
      if (!target) throw new Error(`unknown child: ${ctx!.childId}`);
      const ts = this.deps.now();
      // Allow pins the catalog cert (buildInstallApkGrant throws if it's
      // missing — the security anchor); deny needs none and carries the sentinel.
      const payload = buildInstallApkGrant({
        reqId: install.reqId,
        nonce: install.nonce,
        decision: decision === "approved" ? "allow" : "deny",
        packageName: install.packageName,
        signerCertSha256: install.signerCertSha256,
        versionCode: install.versionCode,
        source: install.source,
        ts,
      });
      const wrap = await giftWrapGrant(payload, install.machine, guardian, ts);
      await this.deps.publish(target.relays, wrap);
      return { ok: true };
    }

    // app.open: the plain Decision echo (see `wire/grant.ts`'s
    // `buildAppOpenGrant`). The actual permission — the AppHold — travels as
    // a SEPARATE `apps`-clause save the caller (the store) makes alongside
    // this; there is nothing to enact from this GRANT's params.
    const appOpen = ctx?.appOpen;
    if (appOpen) {
      const guardian = this.guardian;
      if (!guardian) throw new Error("signer not connected");
      const target = this.deps.resolveChild(ctx!.childId);
      if (!target) throw new Error(`unknown child: ${ctx!.childId}`);
      const ts = this.deps.now();
      const payload = buildAppOpenGrant({
        reqId: appOpen.reqId,
        nonce: appOpen.nonce,
        decision: decision === "approved" ? "allow" : "deny",
        pkg: appOpen.pkg,
        minutesGranted: ctx?.minutesGranted ?? 0,
        ts,
      });
      const wrap = await giftWrapGrant(payload, appOpen.machine, guardian, ts);
      await this.deps.publish(target.relays, wrap);
      return { ok: true };
    }

    // No wire correlation = a simulated/demo request: there is no reqId/nonce
    // to echo and no machine to deliver to, so the decision records locally
    // only — the demo path keeps working with zero relay traffic.
    const wire = ctx?.wire;
    if (!wire) return { ok: true };

    const guardian = this.guardian;
    if (!guardian) throw new Error("signer not connected");
    const target = this.deps.resolveChild(ctx.childId);
    if (!target) throw new Error(`unknown child: ${ctx.childId}`);
    // Refuse rather than emit a phone-local `exp`: without ANY clause tz the
    // grant can overshoot the device's end-of-day cap and be silently
    // rejected (see pickExtendTz). This fires on real state divergence — a
    // normally-managed child carries a schedule or budget tz, and even a
    // named-times-only ward (neither of those) still has the buckets
    // clause's own tz as the last-resort fallback (C-1, hardware round
    // 2026-08-03) — so `undefined` here means truly nothing was ever saved.
    if (!ctx.tz) {
      throw new Error(
        "can't send this decision — this child's time zone is unknown. Re-open their limits to re-sync, then try again.",
      );
    }

    // Build the GRANT: reqId/nonce/limitHit echoed verbatim, minutes clamped
    // onto the wire, exp = end-of-day in `ctx.tz` clamped to the device's OWN
    // validity cap (`scheduleTz`/`budgetTz` — never buckets; see
    // `deviceExtendCapEod`) so a buckets-only ward's grant is never one the
    // device silently drops (review round 2, hardware round 2026-08-03).
    const ts = this.deps.now();
    const payload = buildTimeExtendGrant({
      reqId: wire.reqId,
      nonce: wire.nonce,
      decision: decision === "approved" ? "allow" : "deny",
      minutesGranted: ctx.minutesGranted ?? 0,
      limitHit: wire.limitHit,
      // Named times: echoed verbatim, exactly like limitHit itself, on both
      // allow and deny — absent stays absent for the whole-device dimensions.
      bucketId: wire.bucketId,
      ts,
      tz: ctx.tz,
      scheduleTz: ctx.scheduleTz,
      budgetTz: ctx.budgetTz,
    });
    // The GRANT answers exactly ONE device — the machine that brokered the ask.
    const wrap = await giftWrapGrant(payload, wire.machine, guardian, ts);
    await this.deps.publish(target.relays, wrap);
    return { ok: true };
  }
}
