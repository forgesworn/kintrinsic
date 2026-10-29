// The guardian app's side of *Guardian roots and working keys (v1)*
// (spec/contract.md): which key signs and seals what for each ward, the
// statements a guardian sends, presence confirmation, the guardian PIN and
// the family roster.
//
// Keys:
// - W, the working key, is this install's local guardian key. Every install
//   has one, and every inbound wrap is opened with it.
// - A local-only family has no separate root: the local key IS the pinned
//   root and signs directly (unchanged). `root()` then returns the local key.
// - With My Signet connected as root R (or a Heartwood behind My Signet, the
//   same bunker flow), R approves W once. From then on W signs and seals for
//   every ward that lists W under R and whose child passes the versioning
//   gate; any other ward still gets R's signature over NIP-46, as before.
// - During a root move to R2, each ward is judged by its own STATUS: one that
//   already pins R2 and lists W under it gets W; one still on R gets W under
//   R or R itself. Statements that need root authority wait until no ward is
//   half-way (see `requireNoMovedWards`).
// - Scan-to-pair under a Signet root is offered by R, with no approval, so a
//   ward that predates the section pins R exactly as before; W's approval
//   follows (`syncApprovals`). See docs/guardian-keys-open-questions.md,
//   "A working-key PAIR_OFFER to a ward that predates the section".
//
// Seal author per statement (contract, *Delivery and the two seal
// exceptions*):
// - approval: sealed by the W it names (proof of possession);
// - withdrawal, co-guardian, removal, move, cancel: signed by R, sealed by W
//   where the ward holds W bound to R, else by R itself;
// - objection: signed AND sealed by R (docs/guardian-keys-open-questions.md,
//   "Sealing an objection");
// - presence confirmation and roster: signed and sealed by W;
// - RELAYS with the guardian PIN: signed and sealed by the same key, the one
//   the ward's other events use.
//
// Every action builds its statement with the reference builders
// (`@forgesworn/charter/guardians`). Nothing here talks to React: the store
// injects state access, keys and a publisher, which keeps this testable.

import { finalizeEvent, getPublicKey, nip19, nip44, verifyEvent, type NostrEvent } from "nostr-tools";
import {
  buildApproval,
  buildCancel,
  buildCoGuardian,
  buildFamilyRoster,
  buildObjection,
  buildPresenceConfirm,
  buildRootMove,
  buildRootRemoval,
  buildWithdrawal,
  isDeviceConfirmed,
  markDeviceStatusSeen,
  mergeRoster,
  parseStatement,
  splitPendingD,
  MAX_LABEL_BYTES,
  type EventSigner,
  type LocalRoster,
  type LocalRosterChild,
  type RosterReason,
  type SignedEvent,
  type StatementType,
  type StatusGuardians,
} from "./sdk";
import { giftWrapSigned, type GuardianOps } from "../wire/giftwrap";
import { QueuedForHomeNetworkError } from "../signer/routePublish";
import { childReadiness, wardReadiness, type ChildReadiness, type GateDevice } from "./versionGate";
import { keyStanding } from "./statusGuardians";
import { parsePresenceCode, type PresenceCode } from "./presenceCode";
import { makeGuardianPinRecord, relaysWithPinTemplate } from "./pin";
import type { AnchoredRoot, GuardianKeysState, HeldRoster, InstallRoot, NextRoot, PresenceWard, RootVia } from "./state";

/** One paired ward, as this module sees it. */
export interface KeysDevice {
  machine: string;
  platform: "linux" | "android";
  name: string;
  /** The relays this ward's pairing uses. */
  relays: string[];
  /** From its latest STATUS read by this install; absent = never reported. */
  appVersionCode?: number;
  /** From its latest STATUS; absent = never reported, or a ward that predates the section. */
  guardians?: StatusGuardians;
  /** A STATUS from it has been read by this install. */
  statusSeen: boolean;
  /** Known only from another guardian's roster: never paired here. */
  fromRoster?: boolean;
}

export interface KeysChild {
  id: string;
  name: string;
  colour: string;
  /** The child's 64-hex roster subject; absent = not shared in rosters yet. */
  rosterId?: string;
  devices: KeysDevice[];
}

export interface GuardianKeysDeps {
  /** The persisted keys state and the family, read fresh on every call. */
  read(): { keys: GuardianKeysState; children: KeysChild[] };
  /** Apply an update to the persisted keys state. */
  write(update: (k: GuardianKeysState) => GuardianKeysState): void;
  /** W's secret key (the install's local guardian key). */
  workingKey(): Uint8Array;
  /**
   * R's operations: the NIP-46 bunker for a My Signet / Heartwood root, the
   * local key for a local-only family; null when no root is connected.
   */
  root(): GuardianOps | null;
  /** Publish one gift wrap to the given relays (and the home network, where the store routes it). */
  publish(relays: string[], wrap: NostrEvent): Promise<void>;
  /** Unix seconds. */
  now(): number;
  defaultRelays: readonly string[];
  /**
   * Rule 4: re-sign the child's current clause set with a live key. Called
   * before this install sends a departure; a failure aborts the departure.
   */
  resignClauses?(childId: string): Promise<void>;
}

/** What happened to one statement (or confirmation, PIN, roster) fan-out. */
export interface SendReport {
  /** The signed inner event that was sent. */
  event: SignedEvent;
  /** Machines (or working keys, for a roster) a relay or the home network took it for. */
  sent: string[];
  /** Held by the Android carrier for the home network. */
  held: string[];
  failed: { to: string; error: string }[];
}

/** Resend W's approval to a ward that doesn't list it at most this often. */
export const APPROVAL_RESEND_SECONDS = 6 * 60 * 60;

/** A pending statement with no `expiresAt` expires this long after `issuedAt` (contract, *Pending statements*). */
export const PENDING_LIFETIME_SECONDS = 7 * 24 * 60 * 60;

/**
 * A presence ward is forgotten this long after this install confirmed there,
 * if no roster from an anchored root has named it by then: an invitation
 * can't stay pending longer (OQ "Anchoring a joining parent's trust").
 */
export const PRESENCE_WARD_LIFETIME_SECONDS = PENDING_LIFETIME_SECONDS;

/** At most this many senders' rosters are held for a self-vouched ward's report. */
export const MAX_HELD_ROSTERS = 4;

/** A held roster is dropped this long after it arrived. */
export const HELD_ROSTER_LIFETIME_SECONDS = PENDING_LIFETIME_SECONDS;

const HEX64 = /^[0-9a-f]{64}$/;

/** A guardian identity typed or scanned: an `npub1…` or 64 hex. */
export function parseGuardianKey(input: string): string | null {
  const s = input.trim();
  if (/^[0-9a-fA-F]{64}$/.test(s)) return s.toLowerCase();
  try {
    const d = nip19.decode(s);
    if (d.type === "npub") return d.data;
    if (d.type === "nprofile") return d.data.pubkey;
  } catch {
    // not bech32
  }
  return null;
}

function opsFromSecret(sk: Uint8Array): GuardianOps {
  return {
    pubkey: getPublicKey(sk),
    signEvent: async (t) => finalizeEvent(t, sk),
    nip44Encrypt: async (recipient, plaintext) =>
      nip44.encrypt(plaintext, nip44.getConversationKey(sk, recipient)),
  };
}

/** An `EventSigner` for the reference builders from a `GuardianOps`. */
function signerOf(ops: GuardianOps): EventSigner {
  return async (t) => (await ops.signEvent({ ...t, tags: t.tags.map((x) => [...x]) })) as SignedEvent;
}

/** The event as plain JSON: what is persisted and put on the wire, no library markers. */
function plain(e: SignedEvent): SignedEvent {
  const { id, pubkey, created_at, kind, tags, content, sig } = e;
  return { id, pubkey, created_at, kind, tags: tags.map((t) => [...t]), content, sig };
}

function truncateLabel(label: string): string {
  const enc = new TextEncoder();
  let out = label.trim();
  while (enc.encode(out).length > MAX_LABEL_BYTES) out = out.slice(0, -1);
  return out;
}

export class GuardianKeys {
  constructor(private readonly deps: GuardianKeysDeps) {}

  /** > 0 while a rule 4 re-sign must use the root, not W (W is the key departing). */
  private rootSigning = 0;

  // ---------------------------------------------------------------------
  // Keys and the per-ward choice
  // ---------------------------------------------------------------------

  /** W's operations (the local key). */
  workingOps(): GuardianOps {
    return opsFromSecret(this.deps.workingKey());
  }

  /** W, hex. */
  workingPubkey(): string {
    return getPublicKey(this.deps.workingKey());
  }

  /** This install's root R, if My Signet or a Heartwood is connected as root. */
  signetRoot(): InstallRoot | undefined {
    return this.deps.read().keys.root;
  }

  /**
   * The key a ward pins for this install: R when a Signet root is connected,
   * otherwise the local key. What a pairing link or SAS must name.
   */
  rootPubkey(): string {
    return this.signetRoot()?.pubkey ?? this.workingPubkey();
  }

  private rootOps(): GuardianOps {
    const ops = this.deps.root();
    if (!ops) throw new Error("Connect your approval app first.");
    const want = this.rootPubkey();
    if (ops.pubkey !== want) {
      throw new Error("The connected approval app isn't this family's guardian identity.");
    }
    return ops;
  }

  private allDevices(): (KeysDevice & { childId: string })[] {
    return this.deps
      .read()
      .children.flatMap((c) => c.devices.map((d) => ({ ...d, childId: c.id })))
      .filter((d) => HEX64.test(d.machine));
  }

  private findDevice(machine: string): (KeysDevice & { childId: string }) | undefined {
    return this.allDevices().find((d) => d.machine === machine);
  }

  private gateOf(d: KeysDevice): GateDevice {
    return {
      machine: d.machine,
      platform: d.platform,
      appVersionCode: d.appVersionCode,
      statusSeen: d.statusSeen,
    };
  }

  /**
   * Does the versioning gate count this device? Not one known only from
   * another guardian's roster and never heard from here: nothing shows this
   * install's root is pinned there, so it can't hold this install back (it
   * isn't this install's to update), and it is shown as unconfirmed instead.
   */
  private gated(d: KeysDevice): boolean {
    return HEX64.test(d.machine) && (!d.fromRoster || d.statusSeen);
  }

  /** The versioning gate for one child: every ward this install has paired or heard from must be new enough. */
  childReadiness(childId: string): ChildReadiness {
    const child = this.deps.read().children.find((c) => c.id === childId);
    return childReadiness((child?.devices ?? []).filter((d) => this.gated(d)).map((d) => this.gateOf(d)));
  }

  /**
   * The gate across the family: co-guardians and root moves are offered only
   * when every ward of every child is ready. Children with no paired ward
   * (or only unconfirmed roster devices) don't block.
   */
  familyReadiness(): { ready: boolean; children: { childId: string; readiness: ChildReadiness }[] } {
    const children = this.deps
      .read()
      .children.filter((c) => c.devices.some((d) => this.gated(d)))
      .map((c) => ({ childId: c.id, readiness: this.childReadiness(c.id) }));
    return { ready: children.length > 0 && children.every((c) => c.readiness.ready), children };
  }

  /**
   * Which key signs ordinary events (CLAUSE, GRANT, RELEASE, USAGE_SYNC,
   * RELAYS) for this ward:
   * - "working": W, because the ward lists W under this install's root and
   *   every ward of the child passes the versioning gate, or because the
   *   ward pins W itself as a root (a local-only family);
   * - "root": R (over NIP-46 for a Signet root), as before the section.
   *
   * During a root move, a ward that already lists W under the new root R2
   * gets W too: it has moved, so R would be refused there (rule 8), and its
   * STATUS `guardians` already shows it implements the section.
   */
  keyFor(machine: string): "working" | "root" {
    const keys = this.deps.read().keys;
    if (!keys.root) return "root"; // local-only: root ops ARE the local key
    const device = this.findDevice(machine);
    if (!device?.guardians) return "root";
    const standing = keyStanding(device.guardians, this.workingPubkey());
    if (standing.as === "root") return "working";
    if (standing.as !== "working") return "root";
    if (this.rootSigning > 0) return "root";
    if (keys.nextRoot && standing.root === keys.nextRoot.pubkey) return "working";
    if (standing.root !== keys.root.pubkey) return "root";
    return this.childReadiness(device.childId).ready ? "working" : "root";
  }

  /**
   * Wards half-way through this install's root move: they pin R2 and no
   * longer R, so a statement R signs can't reach them.
   */
  private movedWards(): (KeysDevice & { childId: string })[] {
    const { keys } = this.deps.read();
    const next = keys.nextRoot;
    const root = keys.root;
    if (!next || !root) return [];
    return this.allDevices().filter(
      (d) =>
        d.guardians?.roots.some((r) => r.root === next.pubkey) &&
        !d.guardians.roots.some((r) => r.root === root.pubkey),
    );
  }

  /**
   * A statement signed by R would be dropped by every ward that has already
   * moved to R2, so root statements wait until the move is finished (or
   * restarted and finished). docs/guardian-keys-open-questions.md, "A root
   * move that lapses half-way".
   */
  private requireNoMovedWards(what: string): void {
    const moved = this.movedWards();
    if (moved.length === 0) return;
    const names = [...new Set(moved.map((d) => d.name))].join(", ");
    const lapsed = this.moveProgress()?.lapsed ?? false;
    throw new Error(
      lapsed
        ? `${names} already use${moved.length === 1 ? "s" : ""} your new guardian identity, but the move lapsed before your other devices switched. Finish it first (Guardians → Finish my identity move), then ${what}.`
        : `${names} already use${moved.length === 1 ? "s" : ""} your new guardian identity. Finish the move by confirming it in person at your other devices, then ${what}.`,
    );
  }

  /**
   * Wards this install paired itself or has heard from. A device known only
   * from another guardian's roster that has never reported here isn't
   * counted where "every device" must agree (leaving, finishing a move):
   * nothing shows this install's root is pinned there. OQ "A joining
   * parent's devices".
   */
  private ownWards(): (KeysDevice & { childId: string })[] {
    return this.allDevices().filter((d) => !d.fromRoster || d.statusSeen);
  }

  /** The move in progress, as the screens need it; null when there is none. */
  moveProgress(): { to: string; lapsed: boolean; expiresAt?: number; moved: string[] } | null {
    const { keys } = this.deps.read();
    const next = keys.nextRoot;
    if (!next) return null;
    const expiresAt = this.moveExpiresAt();
    const lapsed = next.cancelledAt !== undefined || (expiresAt !== undefined && this.deps.now() >= expiresAt);
    return {
      to: next.pubkey,
      lapsed,
      ...(expiresAt !== undefined ? { expiresAt } : {}),
      moved: [...new Set(this.movedWards().map((d) => d.name))],
    };
  }

  /** When the move stops being pending: the wards' STATUS `pending`, else its own 7 days. */
  private moveExpiresAt(): number | undefined {
    const { keys } = this.deps.read();
    const next = keys.nextRoot;
    if (!next) return undefined;
    const fromStatus = this.allDevices().flatMap((d) =>
      (d.guardians?.pending ?? []).filter((p) => p.d === next.moveD).map((p) => p.expiresAt),
    );
    const sentAt = keys.sent[next.moveD]?.issuedAt;
    return fromStatus.length > 0
      ? Math.max(...fromStatus)
      : (next.expiresAt ?? (sentAt !== undefined ? sentAt + PENDING_LIFETIME_SECONDS : undefined));
  }

  /** A family whose root is this phone's own key can't take part in presence or removal (OQ "Moving a phone-held root"). */
  private requireSignetRoot(what: string): InstallRoot {
    const root = this.signetRoot();
    if (!root) {
      throw new Error(
        `Your guardian identity is kept on this phone, so you can't ${what} here. Connect My Signet as your guardian identity first.`,
      );
    }
    return root;
  }

  /** The operations for `keyFor(machine)`. */
  opsFor(machine: string): GuardianOps {
    return this.keyFor(machine) === "working" ? this.workingOps() : this.rootOps();
  }

  /** Does `machine` hold W bound to R (so W may seal R's statements there)? */
  private holdsWorkingKey(machine: string, root: string): boolean {
    const g = this.findDevice(machine)?.guardians;
    if (!g) return false;
    const s = keyStanding(g, this.workingPubkey());
    return s.as === "working" && s.root === root;
  }

  // ---------------------------------------------------------------------
  // Connecting a root and delivering W's approval
  // ---------------------------------------------------------------------

  /**
   * Ask the connected root (My Signet, or a Heartwood behind it) to approve
   * this install's working key: one NIP-46 `sign_event` of a kind-31120
   * approval, `d` = `kintrinsic/device/<W>`, labelled with the install's
   * name. Stores {R, approval}. `expectRoot` (hex) refuses a different bunker.
   *
   * Refused when the bunker's key is W itself, which can't approve itself.
   */
  async approveThisInstall(opts: { via: RootVia; label: string; expectRoot?: string }): Promise<InstallRoot> {
    const ops = this.deps.root();
    if (!ops) throw new Error("Connect My Signet first.");
    if (opts.expectRoot && ops.pubkey !== opts.expectRoot) {
      throw new Error("That My Signet identity isn't the one you chose.");
    }
    const w = this.workingPubkey();
    if (ops.pubkey === w) throw new Error("This phone's own key can't approve itself.");
    const { keys: current } = this.deps.read();
    const differentIdentity = () =>
      new Error(
        "This phone is already approved by a different guardian identity. To change identity, move your guardian identity instead.",
      );
    if (current.root && current.root.pubkey !== ops.pubkey) {
      // A different identity is a root move (rule 11), never a re-approval.
      throw differentIdentity();
    }
    // Once the root was forgotten (My Signet disconnected), the devices still
    // say who their guardians are: an identity none of them lists would sign
    // for wards that drop it. With none reporting, the forgotten root is the
    // record of who they were paired to.
    const reported = this.reportedRoots();
    if (reported.length > 0 ? !reported.some((r) => r.root === ops.pubkey) : current.forgottenRoot !== undefined && current.forgottenRoot !== ops.pubkey) {
      throw differentIdentity();
    }
    const label = truncateLabel(opts.label) || "Kintrinsic";
    const approval = await buildApproval(signerOf(ops), w, { issuedAt: this.deps.now(), label });
    this.requireApproval(approval, ops.pubkey, w);
    const root: InstallRoot = { pubkey: ops.pubkey, approval: plain(approval), via: opts.via };
    this.deps.write((k) => {
      const { forgottenRoot: _f, ...rest } = k;
      void _f;
      return { ...rest, root, installLabel: label, approvalSentAt: {} };
    });
    return root;
  }

  /** A returned approval must be R's valid approval of W, whatever the bunker did. */
  private requireApproval(approval: SignedEvent, root: string, w: string): void {
    const parsed = parseStatement(approval);
    if (
      !parsed.ok ||
      parsed.statement.type !== "approval" ||
      parsed.statement.signer !== root ||
      parsed.statement.key !== w
    ) {
      throw new Error("My Signet returned something other than an approval of this phone.");
    }
  }

  /**
   * Send W's approval (sealed by W) to every ward that doesn't yet list W
   * under R, at most every `APPROVAL_RESEND_SECONDS` per ward unless `force`.
   * Wards that predate the section drop it as an unknown kind, harmlessly;
   * sending it anyway is how an upgraded ward first learns W.
   */
  async syncApprovals(opts: { force?: boolean; machines?: string[] } = {}): Promise<string[]> {
    const { keys } = this.deps.read();
    const root = keys.root;
    if (!root) return [];
    const now = this.deps.now();
    const moved = new Set(this.movedWards().map((d) => d.machine));
    const owed = this.allDevices().flatMap((d) => {
      if (opts.machines && !opts.machines.includes(d.machine)) return [];
      // A ward that has moved to R2 needs R2's approval of W; R's is dropped there.
      const bind = moved.has(d.machine) && keys.nextRoot ? keys.nextRoot : root;
      if (this.holdsWorkingKey(d.machine, bind.pubkey)) return [];
      const last = keys.approvalSentAt[d.machine];
      return opts.force || last === undefined || now - last >= APPROVAL_RESEND_SECONDS ? [{ d, approval: bind.approval }] : [];
    });
    if (owed.length === 0) return [];
    const w = this.workingOps();
    const sent: string[] = [];
    for (const { d, approval } of owed) {
      try {
        const wrap = await giftWrapSigned(approval, d.machine, w, now);
        await this.deps.publish(this.relaysFor(d), wrap);
        sent.push(d.machine);
      } catch (e) {
        if (e instanceof QueuedForHomeNetworkError) sent.push(d.machine);
      }
    }
    if (sent.length > 0) {
      this.deps.write((k) => {
        const approvalSentAt = { ...k.approvalSentAt };
        for (const m of sent) approvalSentAt[m] = now;
        return { ...k, approvalSentAt };
      });
    }
    return sent;
  }

  // ---------------------------------------------------------------------
  // Statements
  // ---------------------------------------------------------------------

  private relaysFor(d: KeysDevice): string[] {
    const set = new Set<string>(d.relays.length > 0 ? d.relays : this.deps.defaultRelays);
    for (const r of d.guardians?.relays ?? []) set.add(r);
    return [...set];
  }

  /**
   * Gift-wrap a signed statement to every paired ward. `seal` picks the seal
   * author per ward: "root" = R itself; "working-or-root" = W where the ward
   * holds W bound to R (exception 2), else R.
   */
  private async fanOut(event: SignedEvent, seal: "root" | "working-or-root"): Promise<SendReport> {
    const report: SendReport = { event, sent: [], held: [], failed: [] };
    const devices = this.allDevices();
    if (devices.length === 0) {
      throw new Error(
        Object.keys(this.deps.read().keys.presenceWards).length > 0
          ? "The other parent's phone hasn't shared the family with this one yet. Open Kintrinsic on their phone, then try again."
          : "No paired device to send this to yet.",
      );
    }
    const now = this.deps.now();
    // A ward half-way through this install's root move pins R2 and no longer
    // R: a statement R signs would be dropped there, so it isn't reported as sent.
    const root = this.deps.read().keys.root;
    const moved = new Set(root && event.pubkey === root.pubkey ? this.movedWards().map((d) => d.machine) : []);
    for (const d of devices) {
      if (moved.has(d.machine)) {
        report.failed.push({ to: d.machine, error: `${d.name} already uses your new guardian identity.` });
        continue;
      }
      try {
        const sealer =
          seal === "working-or-root" && this.holdsWorkingKey(d.machine, event.pubkey)
            ? this.workingOps()
            : this.rootOps();
        const wrap = await giftWrapSigned(event, d.machine, sealer, now);
        await this.deps.publish(this.relaysFor(d), wrap);
        report.sent.push(d.machine);
      } catch (e) {
        if (e instanceof QueuedForHomeNetworkError) report.held.push(d.machine);
        else report.failed.push({ to: d.machine, error: e instanceof Error ? e.message : String(e) });
      }
    }
    if (report.sent.length === 0 && report.held.length === 0) {
      throw new Error(report.failed[0]?.error ?? "This couldn't be sent.");
    }
    return report;
  }

  private remember(event: SignedEvent): void {
    const parsed = parseStatement(event);
    if (!parsed.ok) return;
    const st = parsed.statement;
    this.deps.write((k) => ({
      ...k,
      sent: { ...k.sent, [st.d]: { id: event.id, type: st.type as StatementType, issuedAt: st.issuedAt, key: st.key } },
    }));
  }

  private async signAsRoot(
    build: (signer: EventSigner) => Promise<SignedEvent>,
  ): Promise<SignedEvent> {
    return build(signerOf(this.rootOps()));
  }

  /**
   * The roots the family's wards report, merged (keys unioned per root).
   * Only devices of the family count: a presence ward's STATUS is signed by
   * a machine key that anyone can put in a presence code, so it can claim
   * any roots (OQ "Anchoring a joining parent's trust").
   */
  reportedRoots(): { root: string; keys: string[] }[] {
    const out = new Map<string, Set<string>>();
    const views: (StatusGuardians | undefined)[] = this.allDevices().map((d) => d.guardians);
    for (const g of views) {
      for (const r of g?.roots ?? []) {
        const set = out.get(r.root) ?? new Set<string>();
        for (const k of r.keys) set.add(k);
        out.set(r.root, set);
      }
    }
    return [...out].map(([root, keys]) => ({ root, keys: [...keys].sort() }));
  }

  private requireFamilyReady(what: string): void {
    const gate = this.familyReadiness();
    if (gate.ready) return;
    throw new Error(familyGateReason(gate, (m) => this.findDevice(m)?.name, what));
  }

  // ---------------------------------------------------------------------
  // Which other parents this install trusts
  // ---------------------------------------------------------------------

  /**
   * The roots whose working keys may send this install a roster and receive
   * one from it: this install's own root (and the root a move is going to),
   * the roots it has anchored (a scanned guardian code, an invitation), and
   * any root it invited before anchors were kept. A ward's STATUS never adds
   * one.
   */
  trustedRoots(): string[] {
    const { keys } = this.deps.read();
    const out = new Set<string>([this.rootPubkey()]);
    if (keys.nextRoot) out.add(keys.nextRoot.pubkey);
    for (const r of Object.keys(keys.anchoredRoots ?? {})) out.add(r);
    for (const st of Object.values(keys.sent)) if (st.type === "coguardian" && HEX64.test(st.key)) out.add(st.key);
    return [...out].sort();
  }

  /**
   * Trust the inviting parent's root: the guardian code (their npub) shown on
   * their phone, scanned or pasted on this one. Returns the root (hex).
   */
  anchorRoot(input: string): string {
    const key = parseGuardianKey(input.trim().replace(/^nostr:/i, ""));
    if (!key) throw new Error("That isn't a guardian code. It starts with npub1.");
    if (key === this.workingPubkey()) throw new Error("That's this phone's own key, not the other parent's guardian code.");
    if (key === this.signetRoot()?.pubkey) {
      throw new Error(
        "That's your own guardian code. Scan the code on the phone of the parent who invited you. (For a second phone of your own, skip this step.)",
      );
    }
    this.addAnchor(key, { anchoredAt: this.deps.now(), how: "scanned" });
    return key;
  }

  private addAnchor(root: string, anchor: AnchoredRoot): void {
    this.deps.write((k) => ({ ...k, anchoredRoots: { ...(k.anchoredRoots ?? {}), [root]: anchor } }));
  }

  /**
   * Re-sign every child's clauses with a live key before a departure (rule 4).
   * `asRoot`: W itself is departing, so the re-sign must come from R.
   */
  private async resignAll(opts: { asRoot?: boolean } = {}): Promise<void> {
    if (!this.deps.resignClauses) return;
    if (opts.asRoot) this.rootOps(); // R must be connected to re-sign for W
    if (opts.asRoot) this.rootSigning += 1;
    try {
      for (const c of this.deps.read().children) {
        if (c.devices.some((d) => HEX64.test(d.machine))) await this.deps.resignClauses(c.id);
      }
    } finally {
      if (opts.asRoot) this.rootSigning -= 1;
    }
  }

  /**
   * Invite another parent: a co-guardian statement for R2, signed by this
   * install's root, to every ward of every child. It takes effect at each
   * ward once the new parent confirms in person there (`confirmPresence`).
   * Offered only when every ward is new enough.
   */
  async inviteCoGuardian(r2: string, opts: { label?: string; expiresAt?: number } = {}): Promise<SendReport> {
    const key = parseGuardianKey(r2);
    if (!key) throw new Error("That isn't a guardian ID (npub).");
    if (key === this.rootPubkey() || key === this.workingPubkey()) {
      throw new Error("That's your own guardian ID.");
    }
    this.requireFamilyReady("invite another parent");
    this.requireNoMovedWards("invite another parent");
    const event = await this.signAsRoot((s) =>
      buildCoGuardian(s, key, {
        issuedAt: this.deps.now(),
        ...(opts.label ? { label: truncateLabel(opts.label) } : {}),
        ...(opts.expiresAt !== undefined ? { expiresAt: opts.expiresAt } : {}),
      }),
    );
    const report = await this.fanOut(event, "working-or-root");
    this.remember(event);
    // The parent this install invites is one it trusts to share the family.
    this.addAnchor(key, {
      anchoredAt: this.deps.now(),
      how: "invited",
      ...(opts.label ? { label: truncateLabel(opts.label) } : {}),
    });
    return report;
  }

  /**
   * Leave the family: a root removal of this install's own root, signed by
   * it. It takes effect at once on each ward, so it is refused unless every
   * ward that reports its guardians has another root (the last root leaves
   * only by unpairing), and when no ward reports them at all.
   *
   * Rule 4 isn't run: every key this install holds departs with it, so it has
   * no live key to re-sign with. The other parent's next clause of each kind
   * replaces what this install set.
   */
  async leaveFamily(): Promise<SendReport> {
    const me = this.rootPubkey();
    const wards = this.ownWards();
    if (wards.length === 0) {
      throw new Error(
        Object.keys(this.deps.read().keys.presenceWards).length > 0
          ? "The other parent's phone hasn't shared the family with this one yet. Open Kintrinsic on their phone, then try again."
          : "None of your devices has reported its guardians yet, so leaving can't be checked.",
      );
    }
    // Every ward must say who else looks after it: one that hasn't could
    // have this install as its only guardian, and would refuse (or, if it
    // predates the section, drop) the removal while the app said it left.
    const silent = wards.filter((d) => !d.guardians);
    if (silent.length > 0) {
      throw new Error(
        `${[...new Set(silent.map((d) => d.name))].join(", ")} ${silent.length === 1 ? "hasn't" : "haven't"} reported ${silent.length === 1 ? "its" : "their"} guardians yet, so leaving can't be checked.`,
      );
    }
    const alone = wards.filter((d) => !d.guardians!.roots.some((r) => r.root !== me));
    if (alone.length > 0) {
      throw new Error(`You're the only guardian on ${[...new Set(alone.map((d) => d.name))].join(", ")}. Unpair it instead.`);
    }
    this.requireNoMovedWards("leave");
    const event = await this.signAsRoot((s) => buildRootRemoval(s, me, { issuedAt: this.deps.now() }));
    const report = await this.fanOut(event, "working-or-root");
    this.remember(event);
    return report;
  }

  /**
   * Remove the other parent R2: a root removal signed by this install's root.
   * Each ward holds it pending for 72 hours and until this install confirms
   * in person there; R2 can object meanwhile. Re-signs the clauses first
   * (rule 4).
   */
  async removeCoGuardian(r2: string, opts: { expiresAt?: number } = {}): Promise<SendReport> {
    const key = parseGuardianKey(r2);
    if (!key) throw new Error("That isn't a guardian ID (npub).");
    if (key === this.rootPubkey()) throw new Error("To remove yourself, leave the family instead.");
    // Its presence confirmation must come from a working key bound to this
    // install's root; a phone-held root has none, so the ward would drop it.
    this.requireSignetRoot("remove the other parent");
    this.requireNoMovedWards("remove the other parent");
    await this.resignAll();
    const event = await this.signAsRoot((s) =>
      buildRootRemoval(s, key, {
        issuedAt: this.deps.now(),
        ...(opts.expiresAt !== undefined ? { expiresAt: opts.expiresAt } : {}),
      }),
    );
    const report = await this.fanOut(event, "working-or-root");
    this.remember(event);
    return report;
  }

  /**
   * Object to a pending removal of this install's root. Signed AND sealed by
   * the root itself (the objection is not in seal exception 2). Its `p` must
   * be the removal's signer, which STATUS doesn't carry: pass `remover`, or
   * it is taken as the one other root the wards report, and refused when
   * there is more than one.
   */
  async objectToRemoval(d: string, remover?: string): Promise<SendReport> {
    const target = splitPendingD(d);
    if (!target || target.type !== "removal") throw new Error("That isn't a pending removal.");
    const me = this.rootPubkey();
    if (target.key !== me) throw new Error("Only the parent being removed can object.");
    let p = remover ? parseGuardianKey(remover) : null;
    if (remover && !p) throw new Error("That isn't a guardian ID (npub).");
    if (!p) {
      const others = new Set<string>();
      for (const dev of this.allDevices()) {
        if (!dev.guardians?.pending.some((x) => x.d === d)) continue;
        for (const r of dev.guardians.roots) if (r.root !== me) others.add(r.root);
      }
      if (others.size !== 1) {
        throw new Error("Couldn't tell which parent asked for this removal.");
      }
      p = [...others][0];
    }
    const remov = p;
    const event = await this.signAsRoot((s) => buildObjection(s, d, remov, { issuedAt: this.deps.now() }));
    const report = await this.fanOut(event, "root");
    this.remember(event);
    return report;
  }

  /** Cancel a pending co-guardian, removal or move this install's root signed. */
  async cancelPending(d: string): Promise<SendReport> {
    if (!splitPendingD(d)) throw new Error("That isn't a pending co-guardian, removal or move.");
    const event = await this.signAsRoot((s) => buildCancel(s, d, { issuedAt: this.deps.now() }));
    const report = await this.fanOut(event, "working-or-root");
    this.remember(event);
    if (this.deps.read().keys.nextRoot?.moveD === d) {
      const at = this.deps.now();
      this.deps.write((k) => (k.nextRoot ? { ...k, nextRoot: { ...k.nextRoot, cancelledAt: at } } : k));
      this.dropLapsedMove();
    }
    return report;
  }

  /**
   * Withdraw a working key for good (a lost or replaced phone). Signed by its
   * binding root, which must be this install's root: sealed by this install's
   * W (another W of the same root), or by R itself when withdrawing this
   * install's own W. Re-signs the clauses first (rule 4): with R when it is
   * this install's own key, since W is the one departing.
   */
  async withdrawInstall(w: string): Promise<SendReport> {
    const key = parseGuardianKey(w);
    if (!key) throw new Error("That isn't a working key.");
    const me = this.rootPubkey();
    if (key === me) throw new Error("That's the guardian identity itself, not a phone's key.");
    const reported = this.reportedRoots();
    const boundTo = reported.find((r) => r.keys.includes(key))?.root;
    if (boundTo !== undefined && boundTo !== me) {
      throw new Error("That phone belongs to another parent; only they can withdraw it.");
    }
    this.requireNoMovedWards("withdraw that phone");
    const own = key === this.workingPubkey();
    // Rule 4: W itself departs when it is this install's own, so R re-signs.
    await this.resignAll({ asRoot: own });
    const event = await this.signAsRoot((s) => buildWithdrawal(s, key, { issuedAt: this.deps.now() }));
    const report = await this.fanOut(event, own ? "root" : "working-or-root");
    this.remember(event);
    return report;
  }

  /**
   * Move this install's root to a new identity R2 (`next`, the newly
   * connected bunker): R signs the move to every ward, then R2 approves W.
   * Each ward replaces R with R2 once this install confirms in person there
   * (`confirmPresence` carries R2's approval). Refused for a local-only
   * family, whose local key is the root and can't become a working key
   * (docs/guardian-keys-open-questions.md, "Moving a phone-held root").
   */
  async moveRoot(next: { ops: GuardianOps; bunkerUri: string; via: RootVia }): Promise<SendReport> {
    const root = this.signetRoot();
    if (!root) throw new Error("Moving a guardian identity held on this phone isn't available yet.");
    if (next.ops.pubkey === root.pubkey) throw new Error("That's already your guardian identity.");
    if (next.ops.pubkey === this.workingPubkey()) throw new Error("This phone's own key can't be the new identity.");
    const pending = this.deps.read().keys.nextRoot;
    if (pending && pending.pubkey !== next.ops.pubkey && this.movedWards().length > 0) {
      // Some wards already pin the identity of the move in progress; a move
      // R signs to somewhere else can't reach them. Restarting the same move
      // (the same identity) is how a lapsed one is finished.
      throw new Error("Some of your devices already use the guardian identity you were moving to. Finish that move first.");
    }
    this.requireFamilyReady("move your guardian identity");
    await this.resignAll();
    const issuedAt = this.deps.now();
    const event = await this.signAsRoot((s) => buildRootMove(s, next.ops.pubkey, { issuedAt }));
    const w = this.workingPubkey();
    const approval = await buildApproval(signerOf(next.ops), w, {
      issuedAt: this.deps.now(),
      label: this.deps.read().keys.installLabel ?? "Kintrinsic",
    });
    this.requireApproval(approval, next.ops.pubkey, w);
    const report = await this.fanOut(event, "working-or-root");
    const nextRoot: NextRoot = {
      pubkey: next.ops.pubkey,
      approval: plain(approval),
      via: next.via,
      bunkerUri: next.bunkerUri,
      moveD: event.tags.find((t) => t[0] === "d")?.[1] ?? "",
      moveId: event.id,
      expiresAt: issuedAt + PENDING_LIFETIME_SECONDS,
    };
    this.deps.write((k) => ({ ...k, nextRoot }));
    this.remember(event);
    return report;
  }

  /**
   * Once every ward that reports its guardians pins R2 and no longer R, the
   * move is done: R2 becomes this install's root. Returns the new root's
   * bunker link for the store to switch the signer to, or null.
   */
  settleRootMove(): string | null {
    const { keys } = this.deps.read();
    const next = keys.nextRoot;
    if (!next || !keys.root) return null;
    // Every ward counts, not only those that report: one paired under R and
    // not heard from since would otherwise be signed for by R2, which it
    // doesn't pin.
    const wards = this.ownWards();
    const done =
      wards.length > 0 &&
      wards.every(
        (d) =>
          !!d.guardians &&
          d.guardians.roots.some((r) => r.root === next.pubkey) &&
          !d.guardians.roots.some((r) => r.root === keys.root!.pubkey),
      );
    if (!done) {
      this.dropLapsedMove();
      return null;
    }
    this.deps.write((k) => {
      const { nextRoot: _drop, ...rest } = k;
      void _drop;
      return { ...rest, root: { pubkey: next.pubkey, approval: next.approval, via: next.via }, approvalSentAt: {} };
    });
    return next.bunkerUri;
  }

  /**
   * When the move was cancelled or has expired (by the wards' STATUS
   * `pending`, else its own 7 days) and no ward pins R2, forget it. If some
   * ward already moved, R2 is kept: those wards still take W under R2, and
   * restarting the move to the same identity finishes it.
   */
  private dropLapsedMove(): void {
    const { keys } = this.deps.read();
    const next = keys.nextRoot;
    if (!next) return;
    const reporting = this.allDevices().filter((d) => d.guardians);
    const expiresAt = this.moveExpiresAt();
    const lapsed = next.cancelledAt !== undefined || (expiresAt !== undefined && this.deps.now() >= expiresAt);
    if (!lapsed) return;
    if (reporting.some((d) => d.guardians!.roots.some((r) => r.root === next.pubkey))) return;
    this.deps.write((k) => {
      const { nextRoot: _drop, ...rest } = k;
      void _drop;
      return rest;
    });
  }

  /**
   * Forget the root (and any move) when this install stops using it: the
   * signer was disconnected. Nothing is sent.
   */
  forgetRoot(): void {
    const { keys } = this.deps.read();
    if (!keys.root && !keys.nextRoot) return;
    this.deps.write((k) => {
      const { root: _r, nextRoot: _n, ...rest } = k;
      void _n;
      return { ...rest, ...(_r ? { forgottenRoot: _r.pubkey } : {}), approvalSentAt: {} };
    });
  }

  /**
   * Drop an approval that no longer names this install's working key (a
   * key-only backup restored a different W). Wards would refuse it anyway:
   * an approval is sealed only by the W it names.
   */
  dropStaleApproval(): boolean {
    const { keys } = this.deps.read();
    const w = this.workingPubkey();
    const names = (a: SignedEvent | undefined) => {
      const p = a ? parseStatement(a) : null;
      return !!p && p.ok && p.statement.key === w;
    };
    const rootStale = !!keys.root && !names(keys.root.approval);
    const nextStale = !!keys.nextRoot && !names(keys.nextRoot.approval);
    if (!rootStale && !nextStale) return false;
    this.deps.write((k) => {
      const { root, nextRoot, ...rest } = k;
      return {
        ...rest,
        ...(root && !rootStale ? { root } : {}),
        ...(nextRoot && !nextStale && !rootStale ? { nextRoot } : {}),
        approvalSentAt: {},
      };
    });
    return true;
  }

  // ---------------------------------------------------------------------
  // Presence confirmation
  // ---------------------------------------------------------------------

  /**
   * Confirm a pending statement in person at a ward: parse the ward's
   * presence code, build the kind-31121 confirmation `{ v: 1, token,
   * statement, approval? }` signed and sealed by W, and send it to that
   * machine. For a co-guardian the approval is this install's root's (R2's)
   * approval of W; for a move, the new root's.
   */
  async confirmPresence(input: string | PresenceCode): Promise<SendReport> {
    const code = typeof input === "string" ? parsePresenceCode(input) : input;
    if (!code) throw new Error("That isn't a Kintrinsic presence code.");
    // Only a working key bound to a root may confirm; a phone-held root
    // signs as the root itself, and the ward would drop it.
    this.requireSignetRoot("confirm in person");
    const { keys } = this.deps.read();
    let approval: SignedEvent | undefined;
    if (code.kind === "coguardian") {
      if (!keys.root || keys.root.pubkey !== code.key) {
        throw new Error("This invitation is for a different guardian ID than the one connected here.");
      }
      approval = keys.root.approval;
    } else if (code.kind === "move") {
      if (!keys.nextRoot || keys.nextRoot.pubkey !== code.key) {
        throw new Error("This move is to a guardian ID this phone hasn't connected.");
      }
      approval = keys.nextRoot.approval;
    } else if (code.kind === "removal") {
      // Only the install that sent a removal is bound to its signer here;
      // the ward drops anyone else's confirmation (`confirmer_not_bound`).
      if (code.key === this.rootPubkey()) {
        throw new Error("This is a request to remove you. Object to it from Guardians instead.");
      }
      if (keys.sent[code.d]?.type !== "removal") {
        throw new Error("This removal wasn't started from this phone, so it can't be confirmed here.");
      }
    }
    const known = this.findDevice(code.machine);
    // A joining parent trusts the family only through the inviting parent's
    // guardian code: without it, nothing this ward later says, and no roster,
    // can be checked (OQ "Anchoring a joining parent's trust").
    const own = new Set([this.rootPubkey(), keys.nextRoot?.pubkey]);
    if (!known && code.kind === "coguardian" && !this.trustedRoots().some((r) => !own.has(r))) {
      throw new Error(
        "Scan the guardian code of the parent who invited you first (Guardians → Join a family), then confirm here.",
      );
    }
    const w = this.workingOps();
    const event = await buildPresenceConfirm(signerOf(w), {
      token: code.token,
      statement: code.statement,
      ...(approval ? { approval } : {}),
      createdAt: this.deps.now(),
    });
    // A joining install may not know this ward yet: its own defaults, plus
    // anything it does know. OQ: "Presence code carries no relays".
    const relays = [...new Set([...(known ? this.relaysFor(known) : []), ...this.deps.defaultRelays])];
    const report: SendReport = { event, sent: [], held: [], failed: [] };
    const wrap = await giftWrapSigned(event, code.machine, w, this.deps.now());
    try {
      await this.deps.publish(relays, wrap);
      report.sent.push(code.machine);
    } catch (e) {
      if (e instanceof QueuedForHomeNetworkError) report.held.push(code.machine);
      else throw e;
    }
    // This install was at that device: its STATUS may vouch for a roster's
    // sender from now on (`vouchedRoots`).
    const confirmedAt = this.deps.now();
    this.deps.write((k) => ({ ...k, presenceConfirmed: { ...(k.presenceConfirmed ?? {}), [code.machine]: confirmedAt } }));
    // A joining parent learns the ward's machine key here, before any roster:
    // remember it so the STATUS it now sends this install is admitted and
    // shown. It gets nothing else from this install (no clauses, PIN or
    // roster) until a roster from an anchored root names it.
    if (!known && code.kind === "coguardian") {
      const at = this.deps.now();
      this.deps.write((k) => ({
        ...k,
        presenceWards: { ...k.presenceWards, [code.machine]: { ...k.presenceWards[code.machine], confirmedAt: at } },
      }));
    }
    return report;
  }

  /** Is `machine` a ward confirmed in person here, not expired, that no child has yet? */
  isPresenceWard(machine: string): boolean {
    const p = Object.prototype.hasOwnProperty.call(this.deps.read().keys.presenceWards, machine)
      ? this.deps.read().keys.presenceWards[machine]
      : undefined;
    return !!p && !this.presenceExpired(p) && !this.findDevice(machine);
  }

  private presenceExpired(p: PresenceWard): boolean {
    return this.deps.now() >= p.confirmedAt + PRESENCE_WARD_LIFETIME_SECONDS;
  }

  /**
   * Wards confirmed in person here and waiting for a roster from an anchored
   * root to name them (oldest first), with what their STATUS said, for
   * display only.
   */
  waitingPresenceWards(): ({ machine: string } & PresenceWard)[] {
    return Object.entries(this.deps.read().keys.presenceWards)
      .filter(([m]) => this.isPresenceWard(m))
      .map(([machine, p]) => ({ machine, ...p }))
      .sort((a, b) => a.confirmedAt - b.confirmedAt);
  }

  /**
   * Keep what a presence ward's STATUS says about its guardians and version
   * (newest `ts` wins). Nothing else from it is recorded until a roster
   * names the device.
   */
  notePresenceStatus(status: { machine: string; ts: number; appVersionCode?: number; guardians?: StatusGuardians }): void {
    if (!this.isPresenceWard(status.machine)) return;
    const cur = this.deps.read().keys.presenceWards[status.machine];
    if (cur.ts !== undefined && status.ts < cur.ts) return;
    const next: PresenceWard = {
      confirmedAt: cur.confirmedAt,
      ts: status.ts,
      ...(status.appVersionCode !== undefined ? { appVersionCode: status.appVersionCode } : {}),
      ...(status.guardians ? { guardians: status.guardians } : {}),
    };
    if (JSON.stringify(next) === JSON.stringify(cur)) return;
    this.deps.write((k) => ({ ...k, presenceWards: { ...k.presenceWards, [status.machine]: next } }));
  }

  /** Forget presence wards a child now has (the roster named them), and expired ones. */
  prunePresenceWards(): void {
    const { keys } = this.deps.read();
    const known = new Set(this.allDevices().map((d) => d.machine));
    const drop = Object.entries(keys.presenceWards)
      .filter(([m, p]) => known.has(m) || this.presenceExpired(p))
      .map(([m]) => m);
    if (drop.length === 0) return;
    this.deps.write((k) => {
      const presenceWards = { ...k.presenceWards };
      for (const m of drop) delete presenceWards[m];
      return { ...k, presenceWards };
    });
  }

  // ---------------------------------------------------------------------
  // Android guardian PIN
  // ---------------------------------------------------------------------

  /**
   * Set the guardian PIN on Android wards (one child's, or all): a RELAYS
   * event with the ward's current relays and `guardianPin` (scrypt, 16-byte
   * salt), signed and sealed by the ward's usual key and gift-wrapped — never
   * a pasted code. A ward below the versioning threshold is skipped (it
   * refuses an unknown RELAYS field outright) and reported as failed.
   */
  async setGuardianPin(pin: string, opts: { childId?: string; machines?: readonly string[] } = {}): Promise<Omit<SendReport, "event">> {
    const record = await makeGuardianPinRecord(pin);
    const wards = this.allDevices().filter(
      (d) =>
        d.platform === "android" &&
        (!opts.childId || d.childId === opts.childId) &&
        (!opts.machines || opts.machines.includes(d.machine)),
    );
    if (wards.length === 0) throw new Error("No Android device to set a PIN on.");
    const report: Omit<SendReport, "event"> = { sent: [], held: [], failed: [] };
    const now = this.deps.now();
    for (const d of wards) {
      if (!wardReadiness(d.platform, d.appVersionCode, d.statusSeen).ready) {
        report.failed.push({ to: d.machine, error: `Update Kintrinsic on ${d.name} first.` });
        continue;
      }
      try {
        const relays = d.guardians?.relays.length ? d.guardians.relays : d.relays;
        const ops = this.opsFor(d.machine);
        const inner = (await ops.signEvent(relaysWithPinTemplate(d.machine, relays, record, now))) as SignedEvent;
        const wrap = await giftWrapSigned(inner, d.machine, ops, now);
        await this.deps.publish(this.relaysFor(d), wrap);
        report.sent.push(d.machine);
      } catch (e) {
        if (e instanceof QueuedForHomeNetworkError) report.held.push(d.machine);
        else report.failed.push({ to: d.machine, error: e instanceof Error ? e.message : String(e) });
      }
    }
    const done = [...report.sent, ...report.held];
    if (done.length > 0) {
      this.deps.write((k) => {
        const pinSentAt = { ...k.pinSentAt };
        for (const m of done) pinSentAt[m] = now;
        return { ...k, pinSentAt };
      });
    }
    return report;
  }

  // ---------------------------------------------------------------------
  // Family roster (31119), working key ↔ working key
  // ---------------------------------------------------------------------

  /**
   * This install's roster records: one per child with a `rosterId`, its
   * `issuedAt` bumped only when the name, colour or device list changed
   * since the last roster this install held for it.
   */
  private localRosterChildren(): LocalRosterChild[] {
    const { keys, children } = this.deps.read();
    const now = this.deps.now();
    const out: LocalRosterChild[] = [];
    for (const c of children) {
      if (!c.rosterId || !HEX64.test(c.rosterId)) continue;
      const devices = c.devices
        .filter((d) => HEX64.test(d.machine))
        .map((d) => ({ machine: d.machine, platform: d.platform, name: d.name, pairedHere: !d.fromRoster, statusSeen: d.statusSeen }))
        .sort((a, b) => (a.machine < b.machine ? -1 : a.machine > b.machine ? 1 : 0));
      const held = keys.roster.children.find((x) => x.subject === c.rosterId);
      const same =
        held &&
        held.name === c.name &&
        held.colour === c.colour &&
        JSON.stringify(held.devices.map((d) => [d.machine, d.platform, d.name])) ===
          JSON.stringify(devices.map((d) => [d.machine, d.platform, d.name]));
      out.push({
        subject: c.rosterId,
        issuedAt: same ? held.issuedAt : Math.max(now, (held?.issuedAt ?? 0) + 1),
        name: c.name,
        colour: c.colour,
        devices,
      });
    }
    return out;
  }

  /**
   * Send the family roster to every other working key that a ward this
   * install vouches for itself (`vouchedRoots`) lists under a trusted root
   * (this root's other installs, and anchored parents' installs), signed and sealed by W and carrying W's approval. An
   * install with no Signet root has no approval to carry and sends none (OQ
   * "A phone-held root has no approval for the roster").
   */
  async sendRoster(): Promise<Omit<SendReport, "event"> & { event?: SignedEvent }> {
    const root = this.signetRoot();
    if (!root) return { sent: [], held: [], failed: [] };
    const me = this.workingPubkey();
    const trusted = new Set(this.trustedRoots());
    const recipients = [
      ...new Set(
        // Not a presence ward's view: until a roster names it, it is a
        // machine key from a code, and the family goes nowhere on its word.
        this.vouchedRoots({ presenceWards: false })
          .filter((r) => trusted.has(r.root))
          .flatMap((r) => r.keys),
      ),
    ].filter((k) => k !== me);
    const children = this.localRosterChildren();
    // Record what was shared, so the next roster keeps the same issuedAt.
    this.deps.write((k) => {
      const kept = k.roster.children.filter((x) => !children.some((c) => c.subject === x.subject));
      return { ...k, roster: { children: [...kept, ...children] } };
    });
    if (recipients.length === 0 || children.length === 0) return { sent: [], held: [], failed: [] };
    const w = this.workingOps();
    const event = await buildFamilyRoster(signerOf(w), {
      children: children.map(({ subject, issuedAt, name, colour, devices }) => ({
        subject,
        issuedAt,
        name,
        colour,
        devices: devices.map(({ machine, platform, name: n }) => ({ machine, platform, name: n })),
      })),
      approval: root.approval,
      createdAt: this.deps.now(),
    });
    const relays = [...new Set(this.allDevices().flatMap((d) => this.relaysFor(d)).concat(this.deps.defaultRelays))];
    const report = { event, sent: [] as string[], held: [] as string[], failed: [] as { to: string; error: string }[] };
    for (const to of recipients) {
      try {
        const wrap = await giftWrapSigned(event, to, w, this.deps.now());
        await this.deps.publish(relays, wrap);
        report.sent.push(to);
      } catch (e) {
        report.failed.push({ to, error: e instanceof Error ? e.message : String(e) });
      }
    }
    return report;
  }

  /**
   * The roots and working keys that the wards this install vouches for itself
   * report: devices it paired, and devices it confirmed in person at
   * (including presence wards a roster hasn't named yet, unless
   * `presenceWards: false`). A device known only from a roster never
   * vouches, so a roster can't vouch for its own sender (OQ "Anchoring a
   * joining parent's trust").
   */
  vouchedRoots(opts: { presenceWards?: boolean } = {}): { root: string; keys: string[] }[] {
    const { keys } = this.deps.read();
    const confirmed = keys.presenceConfirmed ?? {};
    const views: (StatusGuardians | undefined)[] = this.allDevices()
      .filter((d) => !d.fromRoster || confirmed[d.machine] !== undefined)
      .map((d) => d.guardians);
    if (opts.presenceWards !== false) for (const p of this.waitingPresenceWards()) views.push(p.guardians);
    const out = new Map<string, Set<string>>();
    for (const g of views) {
      for (const r of g?.roots ?? []) {
        const set = out.get(r.root) ?? new Set<string>();
        for (const k of r.keys) set.add(k);
        out.set(r.root, set);
      }
    }
    return [...out].map(([root, ks]) => ({ root, keys: [...ks].sort() }));
  }

  /** Has any ward this install vouches for itself reported its guardians? */
  private anyVouchedReport(): boolean {
    const { keys } = this.deps.read();
    const confirmed = keys.presenceConfirmed ?? {};
    return (
      this.allDevices().some((d) => (!d.fromRoster || confirmed[d.machine] !== undefined) && !!d.guardians) ||
      this.waitingPresenceWards().some((p) => !!p.guardians)
    );
  }

  /**
   * Take in a roster another install sent (already unwrapped by
   * `unwrapRoster`), merged with `mergeRoster`. Accepted only when:
   * - the approval it carries is signed by a trusted root (`trustedRoots`)
   *   and names the sender; and
   * - a ward this install vouches for itself currently lists the sender
   *   under that root (`vouchedRoots`), so a withdrawn W, or a parent who has
   *   left or been removed, is refused.
   * While no such ward has reported at all (a joining install, a second
   * install whose device hasn't reported), a roster that passes every other
   * check is held, and re-checked by `releaseHeldRosters` once one reports.
   * Returns the subjects that changed. Every other refusal is final.
   */
  receiveRoster(inbound: { sealAuthor: string; inner: SignedEvent }):
    | { ok: true; updated: string[] }
    | { ok: false; reason: RosterReason | "own" | "root_not_trusted" | "held" } {
    if (inbound.inner.pubkey === this.workingPubkey()) return { ok: false, reason: "own" };
    const signer = rosterApprovalSigner(inbound.inner);
    if (signer !== null && !this.trustedRoots().includes(signer)) return { ok: false, reason: "root_not_trusted" };
    if (signer !== null && !this.anyVouchedReport()) {
      // Everything but the vouching is checked now, so only a roster that
      // would land is kept.
      const dry = mergeRoster(this.localRoster(), inbound, [{ root: signer, keys: [inbound.inner.pubkey] }], this.deps.now());
      if (!dry.ok) return dry;
      this.hold(inbound);
      return { ok: false, reason: "held" };
    }
    return this.mergeVouched(inbound);
  }

  private localRoster(): LocalRoster {
    const { keys } = this.deps.read();
    let local: LocalRoster = keys.roster;
    for (const d of this.allDevices()) if (d.statusSeen) local = markDeviceStatusSeen(local, d.machine);
    for (const [m, p] of Object.entries(keys.presenceWards)) if (p.ts !== undefined) local = markDeviceStatusSeen(local, m);
    return local;
  }

  private mergeVouched(inbound: { sealAuthor: string; inner: SignedEvent }):
    | { ok: true; updated: string[] }
    | { ok: false; reason: RosterReason } {
    const merged = mergeRoster(this.localRoster(), inbound, this.vouchedRoots(), this.deps.now());
    if (!merged.ok) return merged;
    this.deps.write((k) => ({ ...k, roster: merged.roster }));
    return { ok: true, updated: merged.updated };
  }

  /** Keep the newest roster per sender, a few senders at most (the oldest go first). */
  private hold(inbound: { sealAuthor: string; inner: SignedEvent }): void {
    const now = this.deps.now();
    this.deps.write((k) => {
      const held: Record<string, HeldRoster> = { ...(k.heldRosters ?? {}) };
      const sender = inbound.inner.pubkey;
      const cur = held[sender];
      if (cur && cur.inner.created_at > inbound.inner.created_at) return k;
      held[sender] = { sealAuthor: inbound.sealAuthor, inner: inbound.inner, heldAt: now };
      const order = Object.entries(held).sort((a, b) => b[1].heldAt - a[1].heldAt);
      return { ...k, heldRosters: Object.fromEntries(order.slice(0, MAX_HELD_ROSTERS)) };
    });
  }

  /**
   * Re-check held rosters once a ward this install vouches for itself has
   * reported: each is merged if that ward lists its sender, and dropped
   * either way (and when it has aged out). Returns the subjects that changed.
   */
  releaseHeldRosters(): string[] {
    const held = this.deps.read().keys.heldRosters ?? {};
    const entries = Object.entries(held);
    if (entries.length === 0) return [];
    const now = this.deps.now();
    const ready = this.anyVouchedReport();
    const drop: string[] = [];
    const updated = new Set<string>();
    for (const [sender, h] of entries.sort((a, b) => a[1].inner.created_at - b[1].inner.created_at)) {
      if (now >= h.heldAt + HELD_ROSTER_LIFETIME_SECONDS) {
        drop.push(sender);
        continue;
      }
      if (!ready) continue;
      drop.push(sender);
      if (!this.trustedRoots().includes(rosterApprovalSigner(h.inner) ?? "")) continue;
      const r = this.mergeVouched({ sealAuthor: h.sealAuthor, inner: h.inner });
      if (r.ok) for (const u of r.updated) updated.add(u);
    }
    if (drop.length > 0) {
      this.deps.write((k) => {
        const rest = { ...(k.heldRosters ?? {}) };
        for (const m of drop) delete rest[m];
        return { ...k, heldRosters: rest };
      });
    }
    return [...updated].sort();
  }

  /**
   * Other parents whose roster sharing has stopped because they appear to
   * have changed identity: a scanned or invited root that none of the wards
   * this install vouches for reports any more, while they report a root this
   * install doesn't trust. Shared only once their new guardian code is
   * scanned (OQ "A co-guardian's identity move stops the family list").
   */
  movedAwayRoots(): string[] {
    const views = this.vouchedRoots();
    if (views.length === 0) return [];
    const trusted = new Set(this.trustedRoots());
    if (!views.some((r) => !trusted.has(r.root))) return [];
    const own = new Set([this.rootPubkey(), this.deps.read().keys.nextRoot?.pubkey]);
    return Object.keys(this.deps.read().keys.anchoredRoots ?? {})
      .filter((r) => !own.has(r) && !views.some((v) => v.root === r))
      .sort();
  }

  /** Roster devices not yet confirmed here (never paired here, no STATUS yet). */
  unconfirmedRosterDevices(): { subject: string; machine: string; name: string; platform: string }[] {
    const { keys, children } = this.deps.read();
    const out = new Map<string, { subject: string; machine: string; name: string; platform: string }>();
    // Adopted into the family: known only from a roster and not heard from.
    for (const c of children) {
      for (const d of c.devices) {
        if (d.fromRoster && !d.statusSeen && HEX64.test(d.machine)) {
          out.set(d.machine, { subject: c.rosterId ?? "", machine: d.machine, name: d.name, platform: d.platform });
        }
      }
    }
    // Held in the merged roster only (not adopted, or adopted elsewhere).
    const known = new Set(children.flatMap((c) => c.devices.map((d) => d.machine)));
    for (const c of keys.roster.children) {
      for (const d of c.devices) {
        if (!isDeviceConfirmed(d) && !known.has(d.machine) && !out.has(d.machine)) {
          out.set(d.machine, { subject: c.subject, machine: d.machine, name: d.name, platform: d.platform });
        }
      }
    }
    return [...out.values()];
  }
}

/**
 * The root that signed the approval a roster carries, before anything else
 * is checked; null when there is none to read (`mergeRoster` refuses it).
 */
function rosterApprovalSigner(inner: SignedEvent): string | null {
  try {
    const c = JSON.parse(inner.content) as { approval?: unknown };
    const pa = parseStatement(c?.approval);
    return pa.ok && pa.statement.type === "approval" && pa.statement.key === inner.pubkey ? pa.statement.signer : null;
  } catch {
    return null;
  }
}

/**
 * One line for why the family isn't ready for guardian keys: devices that
 * haven't checked in, and devices that need an update, each named.
 */
export function familyGateReason(
  gate: { children: { readiness: ChildReadiness }[] },
  nameOf: (machine: string) => string | undefined,
  what: string,
): string {
  if (gate.children.length === 0) return `Pair a device first, then ${what}.`;
  const waiting = gate.children.flatMap((c) => c.readiness.wards.filter((w) => !w.readiness.ready));
  const names = (ws: typeof waiting) => [...new Set(ws.map((w) => nameOf(w.machine) ?? "a device"))];
  const silent = names(waiting.filter((w) => !w.readiness.ready && w.readiness.reason === "silent"));
  const old = names(waiting.filter((w) => !w.readiness.ready && w.readiness.reason !== "silent"));
  if (silent.length === 0) return `Update Kintrinsic on ${old.join(", ")} first, then ${what}.`;
  const heard = `${silent.join(", ")} ${silent.length === 1 ? "hasn't" : "haven't"} checked in with this phone yet`;
  if (old.length === 0) {
    return `${heard}. Open Kintrinsic on ${silent.length === 1 ? "it" : "them"}, connected to the internet, then ${what}.`;
  }
  return `${heard}, and ${old.join(", ")} ${old.length === 1 ? "needs" : "need"} a Kintrinsic update. Then ${what}.`;
}

/**
 * Open a gift wrap addressed to W and return its seal author and signed inner
 * event, or null. The seal's signature is verified, so the seal author is
 * authenticated; the inner event's own signature is left to the caller's
 * check (`mergeRoster` verifies it).
 */
export function unwrapToWorkingKey(
  wrap: NostrEvent,
  workingSk: Uint8Array,
): { sealAuthor: string; inner: SignedEvent } | null {
  try {
    const seal = JSON.parse(
      nip44.decrypt(wrap.content, nip44.getConversationKey(workingSk, wrap.pubkey)),
    ) as NostrEvent;
    if (seal.kind !== 13 || !verifyEvent(seal)) return null;
    const inner = JSON.parse(
      nip44.decrypt(seal.content, nip44.getConversationKey(workingSk, seal.pubkey)),
    ) as SignedEvent;
    return { sealAuthor: seal.pubkey, inner };
  } catch {
    return null;
  }
}
