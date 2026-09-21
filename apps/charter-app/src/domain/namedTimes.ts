// Named times — ONE guardian model that replaces the separate Learning and
// App-time-limits (buckets) sections (2026-08 spec). A "named time" is a
// group of apps with exactly one policy:
//
//   free      — time-free, like the old Learning section (a merged `learning`
//               clause; group MEMBERSHIP is guardian bookkeeping — see
//               `FreeGroupRecord` — because the wire clause itself is just a
//               flat app union with no group boundary).
//   counted   — a shared daily/weekly allowance, like the old buckets
//               (`buckets` clause, one bucket per counted group, verbatim).
//   onRequest — blocked, but askable ("Play only if you ask first"); compiles
//               into the `apps` clause's `blocked`+`askFirst` pair.
//
// An app belongs to AT MOST ONE group — `moveApp` is what keeps that
// invariant true across an edit session (pick an app already elsewhere and it
// MOVES, never duplicates).
//
// Two whole-axis switches survive independently of group membership: a family
// can pause "Free times" or "Counted times" for a while (the old Learning/
// buckets sections' own master toggle) without losing any group's
// configuration — `learningEnabled`/`bucketsEnabled` on `NamedTimesDraft`
// carry exactly `LearningPolicy.enabled`/`BucketsPolicy.enabled` losslessly
// through decompose∘compile. There is no equivalent axis for on-request: the
// `apps` clause's own `enabled` is the classic Apps section's master switch,
// and named times never owns it (it only ever ratchets it ON when an
// on-request pkg needs it — see `applyAppsFragment`).
//
// Group NAMES: a counted group's name lives on the wire (`AppBucketRule.label`)
// and round-trips exactly. A free group's name (and which apps belong to
// WHICH free group, when there is more than one) has no wire representation
// at all — the `learning` clause is one flat app union — so it is persisted
// guardian-side as `FreeGroupRecord[]` (the `deviceOverrides` precedent:
// bookkeeping that rides a save inertly and is never read by
// `policyToClauses`). `decompose` reconciles the saved grouping against the
// wire's current app union: an app the wire still carries but no saved group
// claims (drift — e.g. an old pre-freeGroups save) joins the FIRST group.
// An on-request group's identity is NOT persisted this way (out of this
// pass's scope): `decompose` reconstructs at most one on-request group from
// the flat `askFirst` list, generically labelled.

import type {
  AppBucketRule,
  AppsPolicy,
  BucketsPolicy,
  LearningAppSel,
  LearningPolicy,
} from "./types";
import { LEARNING_CATALOGUE } from "../data/learning_catalogue";
// Runtime import (not type-only) — safe despite `launchSignatures.ts`
// importing `GroupPolicy` back from this file, because that import is
// `import type` and erased at compile time; there is no runtime cycle.
import { isCmdlineIdentity } from "./launchSignatures";

export type GroupPolicy = "free" | "counted" | "onRequest";

/** One named time, as the editor holds it — the shape `groupsToClauses`
 *  compiles down to the three clause fragments a single save signs. */
export interface NamedGroup {
  id: string;
  label: string;
  /** On-device identities (Android package id, Linux exec path/flatpak id) —
   *  and, for a `free` group only, learning-app ids too (catalogue entries or
   *  guardian-added sites), resolved back to their full shape by
   *  `groupsToClauses` via its `prior` pool. */
  apps: string[];
  policy: GroupPolicy;
  /** `counted` only. At least one of daily/weekly is required to save. */
  dailyMinutes?: number;
  weeklyMinutes?: number;
}

/** A free group's guardian-side persistence record — see the file doc.
 *  Mirrors `NamedGroup`'s free-relevant fields exactly (no policy/minutes,
 *  which don't apply to a free group). */
export interface FreeGroupRecord {
  id: string;
  label: string;
  apps: string[];
}

/** The three saved dimensions a Named-times save touches, as they stood
 *  before this edit — the round-trip source `decompose` reconstructs groups
 *  from, and the passthrough source `groupsToClauses` borrows fields
 *  (capMinutes, tz, weekStart, richer LearningAppSel detail) that live on the
 *  wire but not on a NamedGroup. */
export interface PriorClauseState {
  learning?: LearningPolicy;
  buckets?: BucketsPolicy;
  apps?: AppsPolicy;
}

/** Everything the editor holds: the groups, plus the two whole-axis switches
 *  (see the file doc) that survive independently of any single group. */
export interface NamedTimesDraft {
  groups: NamedGroup[];
  learningEnabled: boolean;
  bucketsEnabled: boolean;
}

/** Mirrors the device's fail-safe bucket cap (`charter-schedule::MAX_BUCKETS`,
 *  also `wire/status.ts`'s `MAX_GROUPS`) — the most counted named times a
 *  clause carries. Enforced here (editor/domain validation, generous error
 *  message) rather than left to `bucketsToGrant` silently dropping the tail. */
export const MAX_BUCKETS = 12;

/** Emission-side cap on one group's app list. Generous — mirrors nothing on
 *  the device today, but a runaway list (every app on the device, picked by
 *  accident) should read as a mistake to fix, not silently truncate on save. */
export const MAX_BUCKET_APPS = 64;
/** The longest name a counted named time may carry — the device's
 *  `GrantBuckets::is_valid` and the wire's `validBucket` both drop a longer one. */
export const MAX_BUCKET_LABEL = 32;

/**
 * Reserved synthetic group ids, used ONLY when `decompose` has to invent a
 * container with no saved record to draw an id from (a pre-`freeGroups` save,
 * or the always-unnamed on-request axis). Both start with a hyphen, which
 * `namedTimeSlug` can never produce (it always trims a leading/trailing
 * hyphen off whatever it builds) — so a guardian-created group can never
 * collide with one of these, even a bucket literally labelled "Free" (found
 * in review: an unprefixed `"free"` sentinel DID collide with exactly that).
 */
export const RESERVED_FREE_ID = "-free";
export const RESERVED_ON_REQUEST_ID = "-on-request";
export const ON_REQUEST_LABEL = "On request";

function dedupe(list: string[]): string[] {
  return [...new Set(list.map((s) => s.trim()).filter(Boolean))];
}

/**
 * Every learning-app identity `groupsToClauses` can resolve a free group's
 * plain id against: the curated catalogue (always freshest — a saved copy of
 * a catalogue entry never shadows a since-updated domain pin), plus whatever
 * fuller shapes the caller already knows (the previously-saved
 * `learning.apps`, and — for an entry created THIS session, before its first
 * save — whatever the editor has been told about it).
 *
 * A native entry is ALSO indexed by its `exec` path/pkg, not just its `id`:
 * legacy entries (saved by the pre-named-times Learning section) keyed native
 * apps by a slugified LABEL (`learnSlug`) with the real identity in `exec`,
 * where every native entry named times itself creates keys BY that identity
 * directly (`id === exec === pkg`). Indexing both ways means a legacy entry's
 * nicer label survives being looked up by its pkg (see `decompose`, which
 * normalises group membership to `exec`/pkg either way — the "dedupe by pkg"
 * fix: without this a legacy slug-id and its own pkg would resolve as two
 * different, half-duplicate apps).
 *
 * Honesty note: because catalogue always wins, a stale saved domain pin gets
 * silently refreshed the next time ANYTHING in named times is saved — both
 * `compiled` and `savedCompiled` resolve the same id through this same pool,
 * so the upgrade never itself shows up as a "change" needing the guardian's
 * attention (see `Limits.tsx`'s dirty-compare). That is deliberate — a pin
 * update is not a rule change — but it does mean the editor can display a
 * fresher domain list than what is currently signed on the wire, with
 * nothing marked dirty to say so, until the next unrelated save carries it
 * along.
 */
export function learningAppPool(known?: LearningAppSel[]): Map<string, LearningAppSel> {
  const m = new Map<string, LearningAppSel>();
  for (const a of known ?? []) {
    m.set(a.id, a);
    if (a.kind === "native" && a.exec) m.set(a.exec, a);
  }
  for (const c of LEARNING_CATALOGUE) m.set(c.id, c);
  return m;
}

function resolveLearningApp(id: string, pool: Map<string, LearningAppSel>): LearningAppSel {
  // Unknown id (a bare pkg the guardian picked from "programs on their
  // computer", never a site) — a plain native entry, same shape `learnSlug`
  // callers have always produced.
  return pool.get(id) ?? { id, label: id, kind: "native", exec: id };
}

/** The identity a NAMED-TIMES group membership entry resolves to for a
 *  learning-app: a native entry's real device identity (`exec`, falling back
 *  to `id` only if somehow absent) — never a legacy label-slug — so group
 *  membership always matches what `available` (the device's reported apps)
 *  and `moveApp` key on. A site entry's `id` IS its identity (catalogue ids
 *  and guardian-added site ids have no separate device pkg). */
function learningMembershipKey(a: LearningAppSel): string {
  return a.kind === "native" ? (a.exec ?? a.id) : a.id;
}

/** What one `groupsToClauses` compile produces — the exact fragments a save
 *  signs, plus the guardian-side bookkeeping that never reaches the wire. */
export interface CompiledClauses {
  learning: LearningPolicy;
  buckets: BucketsPolicy;
  /** `blockedAdd` — pkgs an on-request group requires to be blocked (the
   *  caller unions these with whatever it already blocks for reasons outside
   *  named times, e.g. the classic Apps section's manual list — see
   *  `applyAppsFragment`). `askFirst` is the on-request set's exact wire
   *  value: named times fully owns that dimension, nothing else writes it. */
  apps: { blockedAdd: string[]; askFirst: string[] };
  /** Free groups' guardian-side persistence — see the file doc. Written even
   *  when unchanged, so a save always carries the current truth forward. */
  freeGroups: FreeGroupRecord[];
}

/**
 * Compile the draft into the three clause fragments one Named-times save
 * signs. Pure: no id generation, no clock, no device I/O — everything it
 * needs beyond the draft itself comes from `prior`.
 *
 * `learning.enabled`/`buckets.enabled` are `draft.learningEnabled`/
 * `draft.bucketsEnabled` VERBATIM — never derived from "does a group of that
 * policy exist". A family who pauses Counted times for the holidays keeps
 * every counted group's configuration; only the enforcement bit flips.
 */
export function groupsToClauses(draft: NamedTimesDraft, prior: PriorClauseState): CompiledClauses {
  const { groups, learningEnabled, bucketsEnabled } = draft;
  const free = groups.filter((g) => g.policy === "free");
  const counted = groups.filter((g) => g.policy === "counted").slice(0, MAX_BUCKETS);
  const onRequest = groups.filter((g) => g.policy === "onRequest");

  const freeGroups: FreeGroupRecord[] = free.map((g) => ({
    id: g.id,
    label: g.label,
    apps: dedupe(g.apps),
  }));

  // --- buckets (counted groups) ------------------------------------------
  const bucketRules: AppBucketRule[] = counted.map((g) => {
    const out: AppBucketRule = {
      id: g.id,
      label: g.label,
      apps: dedupe(g.apps).slice(0, MAX_BUCKET_APPS),
    };
    if (g.dailyMinutes != null) out.dailyMinutes = g.dailyMinutes;
    if (g.weeklyMinutes != null) out.weeklyMinutes = g.weeklyMinutes;
    return out;
  });
  const buckets: BucketsPolicy = {
    enabled: bucketsEnabled,
    tz: prior.buckets?.tz || "UTC",
    buckets: bucketRules,
  };
  if (prior.buckets?.weekStart) buckets.weekStart = prior.buckets.weekStart;

  // --- apps (on-request groups) -------------------------------------------
  // On-request means BOTH blocked and askable — the two lists are the same
  // set by construction, so the ⊆ invariant `appsToGrant` enforces on the way
  // to the wire holds trivially here too.
  //
  // Same defence-in-depth strip as `learning` below: an on-request
  // `cmdline:` identity would render the raw needle on the ward's own tray
  // ("Ask to open cmdline:...") and let an approved hold still get swept
  // dead (the identity stays blocked while only the launcher pkg lifts).
  const askFirst = dedupe(onRequest.flatMap((g) => g.apps)).filter((id) => !isCmdlineIdentity(id));
  const blockedAdd = [...askFirst];

  // --- learning (free groups' apps, PLUS costing sites named elsewhere) ---
  // No merged-total cap here: each free group's OWN size is what
  // `namedTimesError` caps before save (MAX_BUCKET_APPS per group); the union
  // of several validated groups has no separate wire-side limit.
  const pool = learningAppPool(prior.learning?.apps);
  // Defence in depth (review finding, F1/New-2): a `cmdline:` launch-
  // signature identity may only ever live in a Counted group (see
  // `domain/launchSignatures.ts`'s `shouldAttachSignature` — Free grants it
  // as free time off a root-owned exe while argv, all it can ever match,
  // stays ward-written). The picker gate stops a NEW one from being
  // attached here, but this strip closes the class regardless of
  // provenance — an already-saved policy from before that gate existed
  // must not keep re-emitting one on every future save either.
  const freeAppIds = dedupe(free.flatMap((g) => g.apps)).filter((id) => !isCmdlineIdentity(id));
  // A `site:<id>` identity inside a Counted or On-request group names a site
  // app that COSTS rather than being free (2026-08-06 "named costs" design,
  // §4.3). `learning` is still the ONLY clause that defines a site's pinned,
  // resolver-locked window — without an entry here the bucket/askFirst
  // identity above names a window that does not exist to meter — so it is
  // materialised there too, marked `free: false` so it is never mistaken for
  // the old "site in `learning` ⇒ free" default.
  //
  // Read from the WIRE-EMITTED lists (`bucketRules`/`askFirst`), not the raw
  // group apps: a site id truncated off a bucket by `MAX_BUCKET_APPS` must
  // never leave a phantom `learning.apps` entry nothing on the wire actually
  // references.
  const costingSiteIds = dedupe(
    [...bucketRules.flatMap((b) => b.apps), ...askFirst]
      .map(siteIdOf)
      .filter((id): id is string => id != null),
  );
  const learningApps: LearningAppSel[] = [];
  const seenLearningIds = new Set<string>();
  for (const id of freeAppIds) {
    if (seenLearningIds.has(id)) continue;
    seenLearningIds.add(id);
    learningApps.push(resolveLearningApp(id, pool));
  }
  for (const id of costingSiteIds) {
    // By construction (`moveApp`'s "at most one group" invariant) a site
    // cannot legitimately be both free AND costing at once — this guard is
    // defence in depth against a hand-built/legacy draft that violates it,
    // same spirit as the `isCmdlineIdentity` strips above: free wins rather
    // than silently emitting a duplicate id.
    if (seenLearningIds.has(id)) continue;
    seenLearningIds.add(id);
    learningApps.push({ ...resolveLearningApp(id, pool), free: false });
  }
  const learning: LearningPolicy = {
    enabled: learningEnabled,
    apps: learningApps,
  };
  if (prior.learning?.capMinutes != null) learning.capMinutes = prior.learning.capMinutes;

  return { learning, buckets, apps: { blockedAdd, askFirst }, freeGroups };
}

/**
 * Reconstruct the draft a saved policy implies.
 *
 * Free groups: if `savedFreeGroups` is non-empty, it is authoritative —
 * each saved group's app list is filtered down to identities the wire STILL
 * actually carries (an app removed some other way shouldn't linger), and any
 * wire app claimed by NO saved group (drift) joins the first one, so nothing
 * silently vanishes. This reconciliation runs whether or not `clauses.learning`
 * is even defined: `freeGroups` can be signed on its own (an empty free group
 * created before any app is chosen never dirties `learning`, so a save never
 * even touches it), so a group's SAVED MEMBERSHIP must not depend on a wire
 * clause that may not exist yet (found in review — an earlier version gated
 * this whole block on `learning` being truthy, which silently dropped a
 * freeGroups-only save on the very next recompute).
 *
 * `savedFreeGroups` being `undefined` (never saved with this branch's code at
 * all — a true pre-`freeGroups` family) is DIFFERENT from it being `[]`
 * (explicitly saved as "no free groups", e.g. the guardian just switched
 * their only free group to Counted): `undefined` falls back to a single
 * synthetic container — id `RESERVED_FREE_ID` — holding every free app,
 * exactly the old Learning section's shape, so a genuinely legacy family
 * still sees its apps. An explicit `[]` is trusted outright: zero free
 * groups means zero, even though `learning` itself is still a defined
 * (enabled, empty) object — collapsing that distinction (found in review) is
 * what made a guardian's own "no free groups any more" edit read as
 * permanently dirty, because a phantom empty container kept reappearing on
 * every recompute. The one case `[]` does NOT fully trust: a wire app no
 * saved group (even an empty list of them) accounts for, which still gets a
 * synthetic container — dropping a real app silently is worse than a stray
 * empty card.
 *
 * Counted groups: one per saved bucket, verbatim — the wire already carries
 * full fidelity here.
 *
 * On-request: at most one group, reconstructed from the flat `askFirst`
 * list — out of this pass's scope to persist multiple (see file doc).
 */
export function decompose(
  clauses: PriorClauseState,
  savedFreeGroups?: FreeGroupRecord[],
): NamedTimesDraft {
  const groups: NamedGroup[] = [];
  const learning = clauses.learning;
  // NOT gated on `if (learning)` — `freeGroups` can be signed on its OWN
  // (learningDirty is false, and so `learning` is never even part of the
  // save, whenever a free group's edit doesn't touch its app union — e.g.
  // creating one before adding any app). A family whose FIRST-EVER save
  // creates an empty free group therefore has `clauses.learning === undefined`
  // while `savedFreeGroups` already has an entry — gating this block on
  // `learning` existing lost that group on the very next recompute (found in
  // review: it read permanently dirty after a successful save, and vanished
  // on a cold reload). `wireIds` degrades to `[]` when `learning` is absent,
  // which is exactly correct: there is no wire app union yet to reconcile
  // against, only the guardian's own saved grouping.
  //
  // `free === false` entries are excluded here (2026-08-06 "named costs"):
  // those are sites `groupsToClauses` materialised into `learning.apps` only
  // to define their pinned window — their MEMBERSHIP lives in a Counted/
  // On-request group's own `apps[]` (a `site:<id>` identity), reconstructed
  // below from `buckets`/`askFirst` verbatim. Without this filter a costing
  // site would be double-counted as "drift" no free group claims and get
  // pulled into a synthetic free container — silently making a paid site
  // free again on the very next decompose.
  const wireIds = dedupe(
    (learning?.apps ?? []).filter((a) => a.free !== false).map(learningMembershipKey),
  );
  if (savedFreeGroups && savedFreeGroups.length > 0) {
    const claimed = new Set<string>();
    const rebuilt = savedFreeGroups.map((g) => {
      const apps = g.apps.filter((id) => wireIds.includes(id));
      for (const id of apps) claimed.add(id);
      return { id: g.id, label: g.label, apps, policy: "free" as const };
    });
    const leftover = wireIds.filter((id) => !claimed.has(id));
    if (leftover.length > 0) {
      if (rebuilt.length > 0) rebuilt[0].apps = [...rebuilt[0].apps, ...leftover];
      else rebuilt.push({ id: RESERVED_FREE_ID, label: "Free time", apps: leftover, policy: "free" });
    }
    groups.push(...rebuilt);
  } else if (savedFreeGroups === undefined) {
    // Legacy, never saved with this branch's code at all — mirror the old
    // Learning section, which always showed once `learning` existed, even
    // with nothing chosen yet. Gated on `learning` existing (unlike the
    // authoritative branch above) because there is nothing else to anchor a
    // synthetic container to for a family that has truly never touched
    // either dimension.
    if (learning) groups.push({ id: RESERVED_FREE_ID, label: "Free time", apps: wireIds, policy: "free" });
  } else if (wireIds.length > 0) {
    // `savedFreeGroups` explicitly `[]` (authoritative "zero free groups"),
    // but the wire still has an app no saved group accounts for (drift) —
    // never lose it silently.
    groups.push({ id: RESERVED_FREE_ID, label: "Free time", apps: wireIds, policy: "free" });
  }

  for (const b of clauses.buckets?.buckets ?? []) {
    const g: NamedGroup = { id: b.id, label: b.label, apps: [...b.apps], policy: "counted" };
    if (b.dailyMinutes != null) g.dailyMinutes = b.dailyMinutes;
    if (b.weeklyMinutes != null) g.weeklyMinutes = b.weeklyMinutes;
    groups.push(g);
  }

  const askFirst = clauses.apps?.askFirst ?? [];
  if (askFirst.length > 0) {
    groups.push({ id: RESERVED_ON_REQUEST_ID, label: ON_REQUEST_LABEL, apps: [...askFirst], policy: "onRequest" });
  }

  return {
    groups,
    // Never saved (dimension untouched) defaults ON — a family's first
    // group should visibly do something the moment they create it, matching
    // the new group-first UI. Once saved even once (true OR false), that
    // exact value is what a pause survives across reloads for (see the C2
    // review finding this was added to fix).
    learningEnabled: clauses.learning?.enabled ?? true,
    bucketsEnabled: clauses.buckets?.enabled ?? true,
  };
}

/** The result of one `moveApp` — the new group list, and (when the app was
 *  already living in a different group) which one it came from, so the
 *  editor can offer an inline "moved from X — undo". */
export interface MoveResult {
  groups: NamedGroup[];
  movedFrom?: string;
}

/**
 * Move `pkg` into `toGroupId`, keeping the "at most one group" invariant: if
 * the app is already in a DIFFERENT group it is removed from there first
 * (never duplicated); if it's already in the target, this is a no-op; if it
 * isn't anywhere yet, it's simply added.
 */
export function moveApp(groups: NamedGroup[], pkg: string, toGroupId: string): MoveResult {
  const target = groups.find((g) => g.id === toGroupId);
  if (!target) throw new Error(`moveApp: no group "${toGroupId}" to move "${pkg}" into`);
  if (target.apps.includes(pkg)) return { groups };

  const origin = groups.find((g) => g.id !== toGroupId && g.apps.includes(pkg));
  const next = groups.map((g) => {
    if (g.id === toGroupId) return { ...g, apps: [...g.apps, pkg] };
    if (origin && g.id === origin.id) return { ...g, apps: g.apps.filter((p) => p !== pkg) };
    return g;
  });
  return { groups: next, movedFrom: origin?.id };
}

/**
 * Client-side mirror of the fail-closed rejections a save would otherwise hit
 * silently downstream (`bucketsNameError`'s pattern, generalised): a name
 * every group needs, an axis every counted group needs, and the two
 * emission-side caps (RIDER from Task 10's review — `bucketsToGrant` doesn't
 * reject an over-long set today, so the editor is what keeps a guardian from
 * ever finding out the hard way that the tail was silently dropped).
 */
export function namedTimesError(groups: NamedGroup[]): string | null {
  if (groups.some((g) => !g.label.trim())) {
    return "Give every named time a name — an unnamed one can’t be sent to the device.";
  }
  const counted = groups.filter((g) => g.policy === "counted");
  // `validBucket` (wire) and the device's `GrantBuckets::is_valid` both drop a
  // counted group whose name is over 32 — individually and silently, so the
  // guardian's screen kept showing a limit that never reached the device.
  const longName = counted.find((g) => g.label.trim().length > MAX_BUCKET_LABEL);
  if (longName) {
    return `“${longName.label.trim()}” is too long a name — keep it to ${MAX_BUCKET_LABEL} characters or it can’t be sent to the device.`;
  }
  if (counted.length > MAX_BUCKETS) {
    return `Up to ${MAX_BUCKETS} counted named times — combine or remove one before saving.`;
  }
  const overfull = groups.find((g) => g.apps.length > MAX_BUCKET_APPS);
  if (overfull) {
    return `“${overfull.label}” has more than ${MAX_BUCKET_APPS} apps — split it into another named time.`;
  }
  const noAxis = counted.find((g) => g.dailyMinutes == null && g.weeklyMinutes == null);
  if (noAxis) {
    return `“${noAxis.label}” needs a daily or weekly limit — add one, or switch it to Free or On request.`;
  }
  return null;
}

/**
 * Layer an on-request compile fragment onto the app policy some OTHER editor
 * (the classic Apps section's manual block/allow list, or one device's split
 * override of it) already owns, without either one stepping on the other.
 * `prior` here is the CURRENT draft — not necessarily the saved policy — so
 * calling this again after the guardian drops an on-request group correctly
 * lifts exactly what named times added: anything in `prior.blocked` that
 * ISN'T in `prior.askFirst` is a manual block, untouched by named times, and
 * always carried forward; anything that IS in `prior.askFirst` was named
 * times' doing, and is replaced wholesale by the fragment's current
 * `askFirst`/`blockedAdd`. Idempotent and additive, so it is safe to apply to
 * BOTH the base apps policy and every device's own split override — see
 * `Limits.tsx`'s `effectiveOverrides`.
 *
 * `holds` is carried through whenever the key is PRESENT on `prior` at all —
 * `prior?.holds !== undefined`, not `.length` — so an override that legitimately
 * settled on an explicit `holds: []` (every hold lifted; `putHold` produces
 * exactly this shape, see `domain/appHolds.ts`) round-trips byte-identical
 * instead of silently losing the key. Dropping it on an empty array made this
 * function non-identity on such an override: `effectiveOverrides` compared its
 * output back against the raw override every render and read permanently
 * dirty for a device that had nothing left to say about holds (found in
 * review).
 *
 * `allowed` is carried through VERBATIM, deliberately never stripped of
 * `fragment.askFirst` pkgs here — found in review (N1): `prior` is not always
 * the pre-save draft. `Limits.tsx` signs this function's own output, and the
 * NEXT render's `draftApps`/`savedApps` re-seed from THAT saved policy — so
 * `prior` crosses the save boundary and becomes the persisted truth. A strip
 * here would therefore be destructive, not idempotent: it would permanently
 * delete Minecraft from the guardian's stored allowlist, and deleting the
 * on-request group later would never bring it back (there is nothing left to
 * restore it FROM — the persisted `allowed` no longer has it). The allowlist
 * fail-open this guards against is closed on the WIRE instead — see
 * `appsToGrant`, which strips askFirst pkgs from `allowed` only in the bytes
 * it emits, leaving the stored policy (and this function) untouched.
 */
export function applyAppsFragment(
  prior: AppsPolicy | undefined,
  fragment: { blockedAdd: string[]; askFirst: string[] },
): AppsPolicy {
  const priorAskFirst = new Set(prior?.askFirst ?? []);
  const manualBlocked = (prior?.blocked ?? []).filter((p) => !priorAskFirst.has(p));
  const out: AppsPolicy = {
    enabled: (prior?.enabled ?? false) || fragment.askFirst.length > 0,
    posture: prior?.posture ?? "blocklist",
    blocked: dedupe([...manualBlocked, ...fragment.blockedAdd]),
    allowed: prior?.allowed ? [...prior.allowed] : [],
  };
  if (prior?.holds !== undefined) out.holds = prior.holds.map((h) => ({ ...h }));
  if (fragment.askFirst.length) out.askFirst = dedupe(fragment.askFirst);
  // "Remove from device" is carried through UNTOUCHED and named times never
  // writes it: hiding an app is a different decision from what a named time
  // costs, and this function rebuilds the policy from scratch, so anything it
  // forgets to copy is silently deleted on the next save (the `allowed` bug
  // above, in a different dimension). Absent when empty, like `askFirst`, so
  // an unchanged policy stays byte-identical.
  if (prior?.hidden?.length) out.hidden = [...prior.hidden];
  return out;
}

/**
 * The view the classic Apps section should actually show and edit: every pkg
 * named times owns (currently in `managedPkgs`, i.e. `compiled.apps.askFirst`)
 * removed from the editable lists. Without this, a pkg named times added
 * appeared as an ordinary removable pill there — tapping it looked like it
 * worked (the local draft changed) but `applyAppsFragment` re-added it from
 * the still-live on-request group on the very next render, so the "removal"
 * was a silent no-op and the row never even showed as dirty (found in
 * review). The fix is presentation-only: whatever this returns is safe to
 * hand straight to `AppsEditor`, and its `onChange` output is safe to write
 * straight back to the draft — the managed pkgs are re-added downstream by
 * `applyAppsFragment` regardless of whether the draft itself still lists
 * them, so dropping them from what THIS editor can see costs nothing.
 */
export function stripManagedApps(apps: AppsPolicy, managedPkgs: Set<string>): AppsPolicy {
  if (managedPkgs.size === 0) return apps;
  const out: AppsPolicy = {
    ...apps,
    blocked: apps.blocked.filter((p) => !managedPkgs.has(p)),
    allowed: apps.allowed.filter((p) => !managedPkgs.has(p)),
  };
  if (apps.askFirst?.length) {
    const askFirst = apps.askFirst.filter((p) => !managedPkgs.has(p));
    if (askFirst.length) out.askFirst = askFirst;
    else delete out.askFirst;
  }
  return out;
}

/** A stable [a-z0-9-] slug for a new named time — mirrors the old
 *  `bucketSlug`'s rules (the wire's bucket-id shape), used for every group
 *  regardless of policy so an app never needs re-slugging when its policy
 *  changes. Can never produce a leading hyphen (see `RESERVED_FREE_ID`). */
export function namedTimeSlug(label: string, taken: string[]): string {
  const base =
    label.trim().toLowerCase().replace(/[^a-z0-9]+/g, "-").replace(/^-|-$/g, "") || "named-time";
  let id = base.slice(0, 40);
  let n = 2;
  while (taken.includes(id)) id = `${base.slice(0, 36)}-${n++}`;
  return id;
}

/** Suggestion chips for the "+ Named time" create flow — plain editable
 *  text, no apps and no policy attached. Suggestions teach the concept
 *  ("time can have a name"); the family decides what it means. */
export const NAMED_TIME_SUGGESTIONS = ["Play", "Learning", "Social", "Creative"] as const;

/** The `site:` identity form — one of the guardian's OWN site apps, named by
 *  its learning-clause id. Mirrors `charter_schedule::is_site_id`. */
export const SITE_ID_PREFIX = "site:";

/** Wrap a site app's catalogue id into the identity a costing group names. */
export function siteIdentity(id: string): string {
  return `${SITE_ID_PREFIX}${id}`;
}

/** The site-app id a `site:` identity names, or null if it isn't one. */
export function siteIdOf(identity: string): string | null {
  if (!identity.startsWith(SITE_ID_PREFIX)) return null;
  const id = identity.slice(SITE_ID_PREFIX.length).trim();
  return id === "" ? null : id;
}

/** An identity the DEVICE can actually match — as opposed to a bare
 *  learning-catalogue slug, which matches nothing the device ever reports.
 *  Mirrors the filter the old Learning section's native-app picker applied.
 *
 *  Three forms qualify: an exec path (`/usr/bin/firefox`), a flatpak/Android
 *  id (`org.kde.gcompris`, `com.mojang.Minecraft`), and a `site:` identity.
 *
 *  `site:` is the addition that lets a website carry a POLICY. Until it
 *  existed, a site could only ever be free — one clause both defined the
 *  pinned window and made it free — so `partitionOnPolicyChange` dropped
 *  every site the moment its group stopped being free, and "YouTube is half
 *  an hour a day" was unsayable. The bare catalogue slug (`khan-academy`)
 *  still isn't device-shaped and still gets dropped: it names an entry in the
 *  guardian's own list, not a thing running on a machine. */
export function isDeviceShaped(id: string): boolean {
  return id.startsWith("/") || id.includes(".") || siteIdOf(id) !== null;
}

/**
 * What survives when a group switches AWAY from `free`: only device-shaped
 * app ids carry over (Counted/On-request only ever match a real pkg — see
 * `isDeviceShaped`). Catalogue/custom-site ids are dropped and returned
 * separately, so the caller can say plainly what happened rather than let
 * them vanish silently (the exact trap the old buckets editor's doc warned
 * about: a rule keyed by an id nothing on the device will ever match).
 */
export function partitionOnPolicyChange(apps: string[]): { kept: string[]; dropped: string[] } {
  const kept: string[] = [];
  const dropped: string[] = [];
  for (const id of apps) (isDeviceShaped(id) ? kept : dropped).push(id);
  return { kept, dropped };
}
