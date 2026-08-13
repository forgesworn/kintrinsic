# Charter Protocol — Contract v0.1

**Status:** draft (pre-v1)
**Date:** 2026-05-08 (roles amended 2026-07-22 — face/vault split)
**Scope:** the wire-level contract between the four Charter-aware roles. Clauses, methods, deny responses, audit events, authorisation. Implementation lives elsewhere: Signet's bunker (`signet-app`) is the canonical enforcement implementation.

---

## What this document is

The Charter protocol is the wire surface that:

- A **guardian app / Charter dashboard** writes against (to issue / amend / revoke clauses for a dependant).
- A **consumer app** (game, browser app, anything that signs in via a ward's Signet) reads against (sign requests get refused with structured deny responses; audit events surface decisions back to the guardian).
- A **bunker** enforces (Signet's NIP-46 server is the v1 implementation; other bunkers can implement the same contract).

This document specifies **what data crosses the wire and what shape it has**. It does not specify UI, internal data layout, or implementation language.

If you're building a game and want it to be Charter-aware, this is the document to read. If you're building a wardship dashboard (a guardian's Charter dashboard), this is also the document. The Charter consumer SDK (when one exists) will be a thin wrapper over this contract.

## Stability

- **v0.x:** unstable; field names + shapes can change. Do not ship consumer apps against v0.x outside of co-developed integration partners (e.g. ExampleGame as the marquee P1 integration).
- **v1.0:** stable. Wire breaking changes after v1.0 require a `v` field bump and migration guidance.

The schema-version field on every clause carries the version (currently `v: 1` for all clauses).

## Roles

| Role | Meaning |
|------|---------|
| **Guardian surface** | The app the guardian *converses with*: the one inbox where every ask lands (time extensions, app/install requests, sign-in exceptions) and where clauses are authored. The canonical implementation is the **Charter console** — the MyCharter web app (`apps/charter-app`, charter.mysignet.app) plus its native carrier APK (`android/carrier`) for reliable wake. Other surfaces can implement the same wire. |
| **Vault** | The custody + signing engine: holds the guardian (and dependants') keys and signs the surface's rulings — **silently**; the vault never prompts for wardship decisions. **Signet** is the vault (embeddable inside the Charter app for Charter-only families, standalone for Nostr-native families). During bootstrap, MyCharter's local browser key fulfils this role; custody hands up to Signet (see the 2026-07-22 face/vault design). |
| **Bunker** | The NIP-46 server (Signet's, in the vault) that enforces clauses at sign-time. |
| **Consumer app** | A game (or any app) that the ward signs into using their Charter-managed Signet identity. Reads structured deny responses, enforces its own in-app clauses when Charter-aware, and emits asks it cannot decide to the guardian surface. |

> **One surface, one vault (design decision 2026-07-22).** The full rationale —
> four-layer reversibility, the never-prompts rule, embeddable packaging — is in
> `docs/superpowers/specs/2026-07-22-approvals-architecture-face-and-vault-design.md`.
> Earlier drafts of this contract named Signet as "the guardian app" that
> approves; that role is now split as above. Nothing on the wire changes: asks
> are gift-wraps addressed to the guardian pubkey, whoever renders them.

> **Wardship lexicon.** Charter's relationship model is *wardship*: the adult is
> the **guardian**, the minor is the **ward**. This wire contract keeps the
> protocol / data-model terms **`dependant`** and — in the multi-child model —
> **`child`** as the on-wire name for the ward, i.e. *a ward == a `dependant` ==
> the `subject` pubkey*. The code identifiers (`dependantId`, `subject`,
> `multi_child`, `per-child`) stay byte-stable across repos; only the
> human-facing vocabulary is guardian/ward.

## Clauses

A **Charter** is the bundle of bounded permissions a guardian grants to a dependant for a specific interaction context. The unit of permission is a **clause**.

Charter v1 ships one clause; the others are reserved namespaces:

| Clause | Status | Purpose |
|--------|--------|---------|
| `schedule` | **v1 implemented** | When the dependant may sign — weekly windows + per-date overrides + paused state. |
| `budget` | **v1 implemented (device-enforced)** | How much per-day / per-week time is allowed. Enforced by `charterd`, not at bunker sign-time. |
| `spend` | reserved | Spend caps + approval thresholds. |
| `content` | reserved | Rating cap + category filters. |
| `comms` | reserved | Friend allowlist + voice/DM permissions. |

Clauses can attach at two scopes:

- **Per-origin:** on a `RememberedGrant` for a specific (dependant, scope, origin) tuple. "Tom can play Roblox 4-8pm".
- **Dep-default:** on the dependant record itself, applied to ANY sign request that doesn't have a per-origin override. "Tom can't game after 9pm, period."

Per-origin and dep-default of the same clause type compose by **intersection**: per-origin can never extend the dep-default's allowed envelope. (See Sign-time enforcement.)

## Clause data shapes

### Schedule (v1)

```ts
interface GrantSchedule {
  v: 1;
  /** IANA timezone identifier (e.g. "Europe/London"). Required.
   *  Stamped at issue time; stays consistent across devices regardless
   *  of the receiving device's local tz. Both clock-times AND
   *  override-date keys are interpreted in this tz. */
  tz: string;
  /** When set, blocks ALL signs while the schedule is active. */
  paused?: boolean;
  /** Recurring weekly windows. Empty / missing = no weekly windows. */
  weekly: WeeklySchedule;
  /** Per-date overrides (`YYYY-MM-DD` keys, in `tz`).
   *  Replace the weekly entry for that date entirely. Empty array =
   *  blocked all day. Omit the date key to fall through to weekly. */
  overrides?: Record<string, ScheduleWindow[]>;
  /** Unix seconds — when the guardian issued the schedule. Used for LWW
   *  during cross-device sync (see Sync). Carried in audit events. */
  issuedAt: number;
}

interface WeeklySchedule {
  mon?: ScheduleWindow[];
  tue?: ScheduleWindow[];
  wed?: ScheduleWindow[];
  thu?: ScheduleWindow[];
  fri?: ScheduleWindow[];
  sat?: ScheduleWindow[];
  sun?: ScheduleWindow[];
}

interface ScheduleWindow {
  /** "HH:MM" (00-23, 00-59) in the schedule's `tz`. */
  start: string;
  /** "HH:MM" — must satisfy `start < end` strictly. No midnight
   *  crossings (split into two day-bound windows). */
  end: string;
}
```

**Empty-vs-paused:** `weekly = {}` and no overrides means a no-op (always allowed; matches absence of schedule). `paused: true` means everything blocked. The two states are distinct.

**Validation invariants:**
- `tz` must round-trip through `Intl.DateTimeFormat` without RangeError.
- `start` and `end` must match `^([01]\d|2[0-3]):([0-5]\d)$`.
- `start < end` strictly. Zero-duration / inverted windows are invalid.
- Override date keys must match `^\d{4}-\d{2}-\d{2}$` and parse as a real calendar date.

### Budget (v1 — device-enforced)

The `budget` clause is **device-enforced**: the bunker validates + stores it like
`schedule`, but (unlike a sign-time clause) it does **not** emit a sign-time
`clause_blocked:budget` deny — the bunker holds no usage state. Charter for Linux
(`charterd`) caches the signed clause, accounts **active** session usage, and
enforces `effective = min(window-left, quota-left)`.

```ts
interface GrantBudget {
  v: 1;
  /** IANA timezone — day/week resets happen at local midnight in this tz. */
  tz: string;
  /** Per-day cap in minutes. Absent = no daily cap. */
  dailyMinutes?: number | null;
  /** Per-week cap in minutes. Absent = no weekly cap. */
  weeklyMinutes?: number | null;
  /** Week boundary for the weekly reset. Default 'mon'. */
  weekStart?: 'sun' | 'mon';
  /** When true, blocks all time (quota = 0). Distinct from absent. */
  paused?: boolean;
  /** Soft tombstone — guardian revoked the budget (no constraint). */
  revoked?: boolean;
  /** Unix seconds — LWW + rollback protection (monotonic per kind). */
  issuedAt: number;
}
```

**Reset semantics:** daily at 00:00 local-tz; weekly at 00:00 local-tz on
`weekStart`. `quota_left = min over present caps of (cap − used)`, saturating ≥0;
absent caps unconstrained; `paused` ⇒ 0; `revoked` ⇒ no constraint. Usage counts
**Active** time only (idle / locked / suspended / charter-frozen excluded) and
survives a daemon restart without refilling.

`charter_set_budget` / `charter_set_default_budget` mirror the schedule methods
`(dependantId, [...scope keys], GrantBudget | null)`; the device authenticates
the signed clause against the pinned guardian + monotonic `issuedAt` before
caching. A shared `budget_vectors.json` pins the arithmetic both repos load.

### Content (v1 — device-enforced)

The `content` clause is **device-enforced**, like `budget`: it does not emit a
sign-time `clause_blocked:content` deny — there is no per-request check at the
bunker. Charter for Linux (`charterd`: Firefox policy + DNS) and Android (a
DO-pinned DNS-filter `VpnService`) each cache the signed clause and enforce it
locally against the device's DNS / browser traffic. Unlike `schedule` and
`budget`, `content` is **machine-wide** — it is not routed per-child on
multi-child devices (see Device-broker transport, below).

```ts
interface GrantContent {
  v: 1;
  /** IANA timezone identifier. Optional — carried for parity with the other
   *  clauses; content enforcement is not itself time-of-day dependent. */
  tz?: string;
  /** Base posture: `allowlist` (only `parentAllow` + curator-admitted
   *  domains resolve) or `blocklist` (everything resolves except
   *  `blockCategories` + `parentDeny`). */
  posture: 'allowlist' | 'blocklist';
  /** Coarse, parent-set policy selector. NOT derived from a date of birth. */
  ageTier: 'young' | 'older';
  /** Curator pubkeys (hex) whose signed domain lists are a trusted input to
   *  an allowlist posture. Empty / missing = no curator lists subscribed. */
  curators?: string[];
  /** Curators required to jointly admit a domain into an allowlist.
   *  Default 2, floored at 1. */
  quorumN?: number;
  /** Category blocks (blocklist posture). Free-form category identifiers. */
  blockCategories?: string[];
  /** Google/Bing SafeSearch enforcement. Defaults ON when unspecified. */
  safeSearch?: boolean;
  /** YouTube restricted-mode level. Default `off`. */
  youtubeRestrict?: 'off' | 'moderate' | 'strict';
  /** Explicit per-domain overrides layered on top of posture + curator
   *  lists / category blocks. */
  parentAllow?: string[];
  parentDeny?: string[];
  /** When true, blocks ALL web (fail-closed) — checked BEFORE `revoked`, so
   *  a clause with both set stays locked. Distinct from absent. */
  paused?: boolean;
  /** Soft tombstone — guardian revoked the content clause: web goes
   *  unrestricted (no constraint), unless `paused` also wins. */
  revoked?: boolean;
  /** Unix seconds — LWW + rollback protection (monotonic per kind). */
  issuedAt: number;
}
```

`content` has **store_key 3** in the per-clause-kind rollback-protected clause
store. The full `ClauseKind` set (wire tag → store key) is `schedule` (1),
`budget` (2), `content` (3), `apps` (4), `learning` (5), `apprules` (6),
`tethering` (7), `update` (8), `lifeline` (9), `buckets` (10),
`maintenance` (11), `gift` (12), `standdown` (13), `listening` (14),
`alwaysavailable` (15). Three device-local, non-clause slots share the same
rollback-protected store and never cross the wire: `100` (cached USAGE_SYNC),
`101` (the stand-down grace pin) and `102` (the install-window span). It is
parsed and evaluated by the
shared, pure `charter-content` crate (`GrantContent` → `EffectiveWebPolicy`,
no I/O, no clock) that both the Linux and Android enforcers call.

### Learning (v1 — device-enforced)

The `learning` clause designates **time-free learning apps**: focused time in
one credits a separate *learning* bucket instead of draining the screen-time
budget (homework must not cost leisure time). Device-enforced like `budget`;
the bunker validates + stores only. Attribution is by OS-side identity of the
**foreground window** (never window titles — those are page-controlled): site
apps are materialised on-device as resolver-pinned Chromium app windows whose
launcher cmdline carries `--class=charter-<id>` + the domain pin; native apps
match a root-owned executable path or flatpak id — EXCEPT a flatpak/bare-name
identity the device's own app inventory flags `userInstalled`, which never
credits the free bucket regardless of ownership (see `AppRef.userInstalled` in
the Apps section: the exe-ownership check alone cannot distinguish a genuine
system flatpak from a `flatpak install --user` impostor of the same app id).
Fail-closed: any attribution failure, absent/paused clause, or spent cap
charges the screen bucket.

```ts
type LearningAppKind = 'site' | 'native';

interface LearningApp {
  /** [a-z0-9-] slug — launcher filename + attribution marker. */
  id: string;
  label: string;
  kind: LearningAppKind;
  /** site: VERIFIED registrable-domain closure the app window may resolve
   *  (subdomains implied). Measured against the live site, never guessed. */
  domains?: string[];
  /** site: launch URL. */
  url?: string;
  /** native: absolute exec path, or flatpak app id. */
  exec?: string;
  /** native, ward-writable path (their own project): guardian-vouched —
   *  attribution honours it but the identity is advisory, not enforced. */
  trusted?: boolean;
}

interface GrantLearning {
  v: 1;
  apps: LearningApp[];
  /** Daily learning cap in minutes; absent = uncapped. Past the cap,
   *  learning time charges the screen bucket — learning apps never lock. */
  capMinutes?: number;
  /** Lift the whole policy (everything charges screen again). */
  paused?: boolean;
  issuedAt: number;
}
```

**Validation (fail-closed):** site apps require `url` + non-empty `domains`;
native require `exec`; ids must be [a-z0-9-] slugs. STATUS gains optional
`learningTodaySecs` (absent when no clause is in force — pre-learning payloads
stay byte-identical). **Boundary (v1):** the learning bucket does not survive a
screen-time lock — once the ward is locked, the whole session is locked,
learning apps included. "Lock-except-learning" is a possible v2.
`learning_clause_vectors.json` pins producer (TS) and consumer (Rust) — the
same file asserts both sides.

**The site-app runtime (device behaviour, Linux — no wire effect).** Charter
installs a Chromium-family browser on a ward's machine so site apps can run
resolver-pinned. That makes it a **runtime, not a browser**: Firefox is the
ward's browser and the only one Charter can govern (`policies.json`); there is
no Chromium managed-policy renderer. So **while a child has any site app in
force, an unsanctioned Chromium-family process of theirs is terminated** by the
same sweep that enforces `appRules`. This is implied by the clause rather than
being a separate control — a "block Chromium too" toggle is one a guardian can
forget, and forgetting it leaves an ungoverned browser beside a governed one.
Native learning apps do not trigger it; withdrawing every site app lifts it.

**Sanctioned** means the process's argument **multiset** equals exactly what the
device rendered for that app — same tokens, no extras, nothing missing — with
`--user-data-dir` checked for presence and uniqueness only. Multiset rather than
sequence so a browser reordering its own argv cannot un-sanction a child's
homework window mid-lesson. Renderer/GPU children inherit their browser's
sanction by ancestry. The **resolver pin is the boundary; the class marker is
only the meter**: a ward who retypes a sanctioned command line by hand gets the
same sandbox, which is why an approximate match must never be accepted — a
genuine marker beside a permissive pin would otherwise be an unpinned browser
whose time was free. A sanctioned window also survives an **explicit** `appRules`
block naming the runtime binary, so a family whose runtime is Chrome and who
also block Chrome do not thereby kill their child's educational windows.

STATUS gains optional `siteRuntimeMissing: true` — set only when site apps are
in force and no runtime is installed, so nothing can open. Reported because the
alternative is silence: the device retries forever, and a guardian who ticks a
site and sees nothing happen has no way to find out why.

### Time buckets (v1/v2 — device-enforced)

The `buckets` clause (wire tag `"buckets"`, store key `10`) gives a NAMED SET
of apps its own **daily and/or weekly allowance** — "Play is an hour a day",
"Play is five hours a week", or both at once. It generalises the `learning`
bucket: where learning time is FREE (it never drains the day), a bucket here
is CAPPED. Replace-the-set like `appRules`, so it reuses the per-(subject,
kind) monotonic `issuedAt` rollback protection with no per-bucket store
keying.

**Spending a bucket closes THAT BUCKET, never the device.** The ward keeps the
rest of their day — homework, messages, whatever they were half-way through —
and only the bucket's own apps stop. That is the difference between "games are
done for today" and "your computer is confiscated", and it is why this is not
expressed as a second device lock. When a bucket carries both a daily and a
weekly cap, they are **independently binding** — never summed or averaged —
so whichever axis exhausts first closes the bucket.

Members carry the same identity vocabulary as `appRule.pkg`, so a bucket
listing a laptop's games never matches anything on a phone — "limit this on his
laptop only" needs no per-device setting.

```ts
interface AppBucket {
  /** Stable [a-z0-9-] slug — the key the device's day/week counters live under. */
  id: string;
  /** The name the family uses for it ("Play"). */
  label: string;
  /** On-device identities: Android package id, or Linux exec path / flatpak id. */
  apps: string[];
  /** The bucket's daily allowance, in minutes (1..=1440). At least one of
   *  dailyMinutes/weeklyMinutes is required; both may be set. */
  dailyMinutes?: number;
  /** The bucket's weekly allowance, in minutes (1..=10080). */
  weeklyMinutes?: number;
}

interface GrantBuckets {
  v: 1 | 2;
  buckets: AppBucket[];
  /** Lift every bucket (nothing is capped) without deleting the set. */
  paused?: boolean;
  /** Week-start day for the weekly axis: 'sun' | 'mon'. Absent = 'mon' —
   *  mirrors GrantBudget.weekStart. */
  weekStart?: 'sun' | 'mon';
  /** IANA tz the day/week boundary is computed in — same discipline as `budget`. */
  tz: string;
  issuedAt: number;
}
```

**Versioning rule (compat) — `v: 2` adds the weekly axis.** `dailyMinutes`
went from required to optional; `weeklyMinutes` (1..=10080) is new; **at least
one of the two is required per bucket**. The device accepts both `v: 1` and
`v: 2` bodies. **Producer rule:** MyCharter emits `v: 1` whenever every bucket
in the set has a `dailyMinutes` value (so an unchanged all-daily set stays
byte-identical to what a pre-weekly ward already understands); it emits `v: 2`
the moment **any** bucket in the set is **weekly-only**. A pre-weekly ward does
not recognise `v: 2` and — per the buckets fail-open doctrine above — caps
**nothing** until it updates: honest and never confiscating, and surfaced in
the editor's parity note. **Week convention is identical to the `budget`
clause**: the same `week_key_of` calendar-day walk-back to `weekStart`
(default `mon`), computed in the enforcement tz.

**Attribution** is by the **foreground window**, reusing the `learning`
clause's probe and the `appRules` identity matcher — so what is metered is
exactly what is stopped. An app merely running in the background spends
nothing. Bucket time is ALSO ordinary screen time: an hour of Minecraft is an
hour of Play *and* an hour of the day's budget. The device now credits a
week-keyed meter (`bucket_week_secs`) beside the existing day meter
(`bucket_today_secs`), zeroed on the week roll — one attribution, two ledgers.

**Group extension pool ("ask for more Play time" / a gift to a group).** A
granted `time.extend` or `gift` naming this bucket (see below) lands in a
per-group, today-only additive pool: `ExtensionLedger::apply_bucket(now,
reqId, minutes, bucketId)`, read back via `bucket_extra_secs(now, bucketId)`.
It shares the ledger's one idempotent-by-`reqId` applied-list with the
whole-device `schedule`/`budget` pools (so a single `reqId` can never land in
both), but is keyed by bucket id in its own map — a sibling pool, never a
variant of the whole-device `Dimension` enum. The **effective spent** the
device evaluates per axis is `spent − extra`: the extra **lifts BOTH the
daily and weekly wall today** —
granting "Play" 20 extra minutes gives 20 minutes of headroom against whichever
axis (or both) is currently binding, not a separate third pool.

**Fail-safe direction (deliberately opposite to `learning`):** a paused,
malformed or unparseable `buckets` body caps **nothing**. `learning` fails to
`Screen` because an error must never make time *free*; a cap fails to *open*
because an error must never *confiscate* an app the ward is entitled to.
Neither lets an error quietly favour the enforcement side. Validation is
fail-closed on both ends: slug ids, unique per clause, 1–32 char label, 1–64
apps, `dailyMinutes` in 1..=1440, `weeklyMinutes` in 1..=10080, at least one of
the two present, `v == 1 || v == 2`.

**Privacy boundary:** enforcement needs the DEVICE to count per-app time
locally; it does not need to report it. STATUS may carry per-BUCKET seconds
("Play: 45m of 60m") and **never** per-app minutes — per-app detail is where
reflection tips into monitoring (see the per-device design memo). STATUS gains
optional `groups?: [{ id, daySecs, weekSecs }]` (spent seconds, day- and
week-keyed, capped at 12 entries — mirrors `MAX_BUCKETS`) so the guardian's app
can show progress on both axes ("Play: 45m of 60m today · 2h10 of 5h this
week"). Absent until a `buckets` clause is in force; a payload somehow
claiming more than the cap is **truncated on parse, not rejected** — losing an
otherwise-trustworthy STATUS over a malformed tail would hide real numbers the
guardian is owed.

A weekly-only bucket, wire example (MyCharter emits `v: 2` because the set has
no `dailyMinutes` at all):

```jsonc
{
  "v": 2,
  "buckets": [
    { "id": "play", "label": "Play", "apps": ["com.mojang.minecraftpe"], "weeklyMinutes": 300 }
  ],
  "weekStart": "mon",
  "tz": "Europe/London",
  "issuedAt": 1732550400
}
```

And the matching STATUS excerpt:

```jsonc
{ "groups": [ { "id": "play", "daySecs": 1320, "weekSecs": 4800 } ] }
```

### Gifted time (v1 — device-enforced)

The `gift` clause (wire tag `"gift"`, store key `12`) is minutes the guardian
gave **without being asked**. It exists because a GRANT must echo a pending
REQUEST (`verify_grant` rejects any grant whose `reqId`/`nonce` does not bind to
one), so answering an ask was the ONLY way to add time: with nothing
outstanding — or having said no and thought better of it — a guardian's only
lever was editing the `schedule`, i.e. rewriting a standing rule to settle a
one-off.

```ts
interface GiftBody {
  v: 1;
  issuedAt: number;
  /** Unique per gift — the device ledger applies each id exactly once. */
  id: string;
  minutes: number;
  /** Unix seconds; end of day in the CHILD's tz. Dead after this. */
  expiresAt: number;
  /** The named-times bucket this gift tops up. Absent = the whole-device
   *  schedule/budget pool, exactly as before named times existed. */
  groupId?: string;
}
```

**Additive, not replace-the-state** — the one clause that is. A gift lands in
the same today-only `ExtensionLedger` a granted `time.extend` does (to the
enforcer the two are the same thing, so the lock math, the time-left readout
and STATUS need no new cases), and that ledger is idempotent by id. The clause
is re-read every tick, so `id` — not the clause slot — is what keeps one gift
to one application; a SECOND gift is a new id and therefore genuinely adds
again. Rollback protection is unchanged: the per-(subject, kind) monotonic
`issuedAt` floor still refuses a replayed older gift.

**Which pool** is the DEVICE's call, never the guardian's: minutes charged to
the budget pool while a closed window is what is holding the ward out would
never unlock anything (the same M7 routing the ward's own ask uses). Locked on
`schedule` ⇒ the schedule pool; otherwise the budget pool.

**Fail-safe direction (as `maintenance`, opposite to `buckets`):** this clause
LOOSENS enforcement, so every uncertainty resolves to *no extra time* — wrong
`v`, an empty `id` (nothing to deduplicate on, so it would top up forever),
`minutes` of 0 or above `MAX_GIFT_MINUTES` (1440), or an `expiresAt` at or
before now. `expiresAt` is exclusive, and it is what stops a phone that spent
the evening offline waking tomorrow to yesterday's half hour.

**`groupId` — give time to a named bucket.** Present ⇒ the minutes land in
that bucket's own per-group pool — `ExtensionLedger::apply_bucket(now, reqId,
minutes, bucketId)`, keyed separately from (never a variant of) the
whole-device `Dimension::Schedule`/`Dimension::Budget` pools; absent ⇒ exactly
today's behaviour, unchanged. Stays `v: 1` — the field is additive and the body
does not deny unknown keys, so **an old ward simply ignores `groupId` and
lands the gift as device time anyway**. That is documented and deliberate: it
errs generous
(never "Mum gave me time and nothing happened"), and the gap closes on its own
the moment the ward updates — worth noting in the guardian's parity copy, not
worth gating the whole gift on.

### Stand-down — "finish up now" (v1 — device-enforced)

The `standdown` clause (wire tag `"standdown"`, store key `13`) is the guardian
saying **finish up now, then you're done for today**. It is the inverse of
`gift`, and the two are deliberately symmetric: where a gift ADDS minutes, a
stand-down CAPS them.

```ts
interface StandDownBody {
  v: 1;
  issuedAt: number;
  /** Unique per stand-down — the device pins first-sight against this. */
  id: string;
  /** Unix seconds; end of day in the WARD's tz. The midnight lapse. */
  expiresAt: number;
  /** Seconds of warning owed before the lock lands. Device clamps to 1..=600. */
  graceSecs: number;
}
```

**Both wardens enforce it** (Android 0.5.0+, `charterd` 0.4.0+). An OLDER warden
takes the clause, ignores it, and keeps reporting the ward as allowed — so a
family mid-upgrade can see a locked phone beside an "allowed" laptop. The grace
rule is shared (`charter_spine::standdown`) rather than reimplemented per
platform, because first-sight pinning is easy to get subtly wrong in ways that
only show up as a lock that never lands.

**Not a second kind of lock.** A stand-down caps the ward's remaining time at
`graceSecs` and then holds it at zero, so everything downstream is the machinery
that already exists: the 60-second warning already emitted by the enforcer fires
as the warning, the time-left readout counts down so the ward *watches* the
grace elapse rather than being told about it, and at zero the ordinary lock
appears — with the learning exemption, the lifeline and break-glass all
unchanged. A separate lock path is where a ward gets trapped, so there isn't
one. `LockReason` gains `StandDown` (STATUS `lockReason: "standdown"`) purely so
the ward is told *which* wall she met: a spent budget waits for tomorrow, a
closed window waits for the window, but a stand-down waits for a **person**.

**Replace-the-state, and lifting is the same clause.** "Allow back on" publishes
a stand-down whose `expiresAt` is **at or before its own `issuedAt`**. One code
path calls and clears, and because the per-(subject, kind) monotonic `issuedAt`
floor orders them, a lift can never be reordered behind the call it lifts. The
device recognises a lift by that relation — `expiresAt <= issuedAt` never
stands — NOT by comparing `expiresAt` with its own clock: the relation compares
the guardian's clock with itself, so a device running minutes behind her cannot
misread the message that frees the ward as a fresh stand-down (and, past the
grace, lock her with it).

**The lapse is device-held.** A stand-down claiming to stand longer than
`MAX_STANDDOWN_LIFE_SECS` (36 h) past its own `issuedAt` does not stand at all.
The honest maximum is "the rest of today" in the ward's tz; anything beyond it
is a client bug, and doubt fails open — never into a week-long lock.

**Precedence: the cap is applied AFTER both extension pools.** That ordering is
the entire reason a `gift` cannot quietly undo a deliberate stand-down — no
special case, just the order of operations. An unbounded charter (no clauses at
all) is capped too, so a ward with no rules is still stoppable.

**The grace clock belongs to the DEVICE, not the guardian.** The lock instant is
deliberately absent from the wire. The device pins the moment it FIRST saw a
given `id` and counts from there, persisted in device-local store key `101`
(`{"id":…,"startedAt":…}` — not a clause; nothing signs it and it never crosses
the wire). Were the instant stamped `issuedAt + graceSecs`, relay transit would
eat into the warning and a phone that spent an hour offline would reconnect and
cut the ward off with none at all. Persisting it is the symmetric requirement: an
in-memory pin would restart the grace on every boot, so the lock would never
land.

**Fail-safe direction (OPPOSITE to `gift`, and this is the point):** this clause
TIGHTENS enforcement, so every uncertainty resolves to *no lock* — wrong `v`, an
empty `id` (nothing to pin first-sight against), or an `expiresAt` at or before
now. The governing principle is that doubt must never invent a state harsher OR
looser than what the guardian signed; both directions land on the standing
charter. This direction matters more than the gift's: a malformed clause that
locks a ward out of her own phone while her guardian believes she is fine is a
worse failure than one that leaves her alone, and the guardian can see it did not
apply and simply try again. `graceSecs` is CLAMPED rather than refused (0 ⇒ 60,
above 600 ⇒ 600) — zero would cut her off mid-sentence and an absurd value would
mean the stand-down never arrives, and neither is worth discarding the clause
over.

### Apps (v1 — device-enforced)

The `apps` clause (wire tag `"apps"`, store key `4`) is the **standing per-app
policy** for one child: `blocklist` blocks `blocked`; `allowlist` runs ONLY
`allowed` (everything else suspended). It is independent of the time lock — a
blocked app stays blocked during allowed screen time. `paused` lifts the whole
clause (the guardian surface's "off").

```ts
type HoldState = 'allowed' | 'blocked';

/** A time-boxed departure from the standing lists, for ONE app. */
interface AppHold {
  /** On-device identity — the same vocabulary as AppRule.pkg. */
  pkg: string;
  state: HoldState;
  /** ABSOLUTE unix seconds. The hold ends AT this instant. */
  untilUnix: number;
}

interface GrantApps {
  v: 1;
  posture: 'blocklist' | 'allowlist';
  blocked?: string[];
  allowed?: string[];
  paused?: boolean;
  /** Time-boxed overrides of the lists above; omitted when empty. */
  holds?: AppHold[];
  /** Presentation hint: apps the guardian surface should offer an "ask to
   *  open" affordance for, instead of a flat block. Every listed pkg MUST
   *  also be in `blocked`. */
  askFirst?: string[];
  issuedAt: number;
}
```

**Holds** exist because a guardian who wants a one-off ("let him on the browser
for an hour") otherwise has to edit a standing RULE and remember to put it back.
`untilUnix` is absolute rather than a duration for the same reason
`maintenance.untilUnix` is: a duration restarts every time the stored clause is
re-read, and the window never shuts. The expiry is enforced by the ward's own
device — never by a timer on the guardian's surface, which the OS freezes the
moment it is backgrounded.

**Resolution (fail-closed), recomputed every tick:** the device enforces
`effective_at(now)`, not the clause as authored —

1. drop holds with `untilUnix <= now`;
2. drop holds with `untilUnix > issuedAt + 24h` (`MAX_APP_HOLD_SECS`) — the
   fail-closed reading of an over-long LOOSENING is to refuse it;
3. drop past the 64th hold (`MAX_APP_HOLDS`);
4. apply the survivors in clause order (last writer wins per `pkg`): under
   `blocklist`, `allowed` removes from `blocked` and `blocked` adds to it; under
   `allowlist`, `allowed` adds to `allowed` and `blocked` removes from it;
5. `paused` beats every hold.

Enforcement downstream then reads posture + lists exactly as before. Because
resolution is level-triggered from an absolute instant, a reboot inside a hold
does not extend it by a second, and a device switched off for a whole hold comes
back to its standing rule with nothing to undo.

`holds` is **additive at `v: 1`**: a warden that predates it ignores the field
and keeps enforcing the standing lists rather than rejecting the clause. Guardian
surfaces version-gate the feature instead (MyCharter: `wardenSupport.appHold`,
ward Charter ≥ versionCode 36).

**`askFirst` — on-request apps (named times).** A **pure presentation hint**,
also additive at `v: 1`: apps the guardian surface should render as "Ask to
open" rather than a flat "Blocked". **Every `askFirst` pkg MUST also appear in
`blocked`** — enforcement reads `blocked` only and never consults `askFirst`,
so an on-request app is genuinely closed until asked-for and granted on every
warden, old or new. An old warden that has never heard of `askFirst` ignores
the field and keeps enforcing `blocked` exactly as before: blocked, no ask
button, fail **closed** — never fail open. An updated warden additionally
offers the ask affordance for exactly the same set, wired to the new `app.open`
op below.

```jsonc
{
  "v": 1,
  "posture": "blocklist",
  "blocked": ["com.mojang.minecraftpe"],
  "askFirst": ["com.mojang.minecraftpe"],
  "issuedAt": 1732550400
}
```

**Ward-facing:** an app that starts working and later stops working with nothing
on the phone explaining either moment is the silent change Charter's transparency
invariant forbids. Both edges are announced to the ward, and the end wording is
read from the resolved policy rather than assumed from the hold's direction.

**Linux (charterd ≥ 0.7.1).** The same resolved clause feeds the per-app
SIGTERM sweep alongside `appRules` and spent buckets — the shared
`GrantApps::effective_at` does the resolving, so a hold ends at the same
instant on every platform. Two Linux-specific rules:

- **`allowlist` is scoped to the machine's own app inventory** (the identities
  STATUS reports and the guardian ticked): every inventory app not on the
  resolved `allowed` list is blocked, and ONLY inventory apps. Android's
  "everything else" is the enumerable launchable-package set; on Linux the
  sweep kills processes, so the set must be enumerable too or an allowlist
  would take down the session itself. An empty inventory blocks nothing.
- **Sanctioned learning windows survive any blocked pkg** (the existing
  `sweep_decision` rule), so an allowlist that omits the browser cannot kill a
  child's pinned homework window.

Before 0.7.1 charterd stored this clause and never read it — the standing
lists and holds alike were Android-only. Guardian surfaces gate on charterd
0.7.1 / versionCode 701.

**The inventory itself, and `AppRef.userInstalled` (charterd ≥ 0.7.5, honest
attribution).** STATUS's `apps` (`AppRef[]`) is built by scanning `.desktop`
launcher dirs for `pkg` (resolved exec path or flatpak app id) + `label`,
skipping hidden/non-Application/Charter's-own/no-`Exec` entries. Two dir sets
are scanned, and every entry carries which one it came from:

```ts
interface AppRef {
  pkg: string;
  label: string;
  /** true only for an entry from a ward-writable scan dir; absent (root-owned) otherwise. */
  userInstalled?: boolean;
}
```

- **Root-owned** (unflagged): `/usr/share/applications`,
  `/var/lib/flatpak/exports/share/applications` (system flatpaks).
- **Ward-writable** (`userInstalled: true`): the managed user's own
  `~/.local/share/applications` and
  `~/.local/share/flatpak/exports/share/applications` (`flatpak install
  --user`) — so the guardian can SEE a ward-installed launcher (Prism,
  MultiMC, any `--user` flatpak) exist at all, which the pre-0.7.5 inventory
  never scanned. Flagging is what makes showing a ward-writable entry safe:
  the guardian is told plainly it came from the ward's own writable area, not
  presented as if it were an inventory the ward has no way to have altered.
  When the identical resolved identity appears from both a root dir and a
  managed user's dir, the root-owned entry wins (unflagged) — a ward cannot
  launder a root identity's trust by dropping their own copy of the same
  `.desktop` file into their writable dir.

**The `userInstalled` asymmetry (extends §Learning's ownership rule).** A
`learning` app whose identity is flagged `userInstalled` in the inventory
**never credits the free learning bucket**, even when its running process
looks enforcement-grade. This closes a hole the exe-ownership check (below)
cannot: a real system flatpak and a `flatpak install --user` impostor of the
*same* app id land on the identical `app-flatpak-<id>-<pid>.scope` cgroup and
the identical root-owned `/usr/bin/bwrap` — nothing about the *running
process* tells them apart. The inventory's own knowledge (which dir the
`.desktop` entry came from) is the only signal that can. **Capping and
blocking are UNAFFECTED** — restricting a user-installed identity is
self-harm only, never a way to make time free, so `appRules`/`buckets`
enforcement reads the same `pkg` exactly as before.

A guardian-vouched (`trusted: true`) learning app is **exempt from this gate**
— the same exemption the ownership check already gives it. A parent who runs
`flatpak install --user` for their child and then deliberately ticks that
exact app as learning has vouched for it themselves; charging it to the
screen budget anyway would be the "learning never worked and said nothing"
failure class, reproduced by the very control meant to prevent it.

**An UNKNOWN inventory is not an EMPTY one.** Until the device's inventory
scan has answered at least once, this gate has nothing to consult — and a
ward can arrange exactly that with no privilege (a hung FUSE mount at
`~/.local/share/applications` wedges the scan, which times out and backs
off). While the inventory is unknown, the learning arm that DEPENDS on it
fails **closed** to screen time — learning's documented fail direction —
rather than reading the absent set as "nothing here is user-installed" and
handing the impostor free time. The `trusted` exemption and the arms gated by
exe ownership never consulted the inventory and are unaffected.

### App rules (v1 — device-enforced)

The `appRules` clause (wire tag `"apprules"`, store key `6`) governs **specific
apps/games** (as opposed to the whole-device `schedule`). One clause carries the
child's FULL per-app rule set — replace-the-set semantics like `learning`, so it
reuses the per-(subject, kind) monotonic `issuedAt` rollback protection with no
per-app store keying. Each rule blocks an app outright and/or constrains it to
its own allowed hours (a `GrantSchedule`, so the same tz-aware evaluator governs
it, on top of the device schedule).

```ts
interface AppRule {
  /** On-device identity: Android package id, a Linux exec path / flatpak id,
   *  or a Linux `cmdline:` id (see "Identity vocabulary" below). */
  pkg: string;
  /** Display name (guardian-facing); advisory. */
  label?: string;
  /** Blocked outright — may never open, regardless of schedule. */
  blocked: boolean;
  /** Optional allowed-hours; usable only INSIDE these windows. */
  schedule?: GrantSchedule;
}

interface GrantAppRules {
  v: 1;
  rules: AppRule[];
  issuedAt: number;
}
```

**Enforcement (fail-safe):** the device evaluates each rule at every tick —
`blocked` wins outright; otherwise an app with a per-app schedule is usable only
when that schedule is Open/Unbounded, and Blocked when Locked or the schedule
won't parse (never fail-open). Blocked/out-of-window apps are suspended (Android
`DevicePolicyManager`) or frozen (Linux). Apps with no rule are unaffected by
this clause.

**Identity vocabulary (the `pkg` string, Linux ≥ 0.7.5 / versionCode 705).**
`pkg` is a bare string in one of three forms — no wire-shape change, the
`AppRule`/`AppBucket` interfaces above are unchanged:

| Form | Example | Matches |
|---|---|---|
| Flatpak app id | `org.prismlauncher.PrismLauncher` | the process's cgroup scope |
| Exec path | `/usr/bin/minecraft-launcher` | the resolved exe or argv0, full path or basename |
| `cmdline:<substring>` | `cmdline:net.minecraft.client.main.Main` | a substring of any token in the process's own command line |

`cmdline:` names the **game, not the launcher**. Minecraft's real process is a
JVM, not the launcher that started it — `org.prismlauncher.PrismLauncher` or
`/usr/bin/minecraft-launcher` only ever matches whichever launcher happens to
be installed, and a ward can rename the binary, install a different launcher
(Prism, MultiMC, ATLauncher, Modrinth), or run `java -jar` directly with none
at all. `cmdline:net.minecraft.client.main.Main` matches the JVM's main class
regardless of how it was started, covering all of those at once and every
launcher that does not yet exist. Evading it means patching the jar — a
different order of effort than a rename.

The substring after the prefix must be **at least 8 characters** once
whitespace is stripped, matched case-sensitively against each NUL-split
`cmdline` token and the joined line. Shorter than that, it fails to recognise
the identity at all (`None`, no match) rather than under-matching — a needle a
handful of characters long would match almost any process on the machine,
turning a rule meant to name one game into an accidental blanket one. That is
a deliberate CAP on the vocabulary, not an enforcement decision: recognising
the form is pure, matching a live process against the needle is the
enforcer's job, same as the other two forms.

**Compatibility.** A pre-0.7.5 charterd does not know this form exists — it
compares `pkg` as a literal string against an exe path or a flatpak cgroup
scope, so a `cmdline:` entry never matches anything, on EITHER side. In a
`blocked`/capped list that is a loosening (the entry silently does nothing),
so it fails **open** there — the same direction every other version-gated
capability is required to fail. MyCharter version-gates offering the form at
all behind capability `cmdlineIdentity` (linux ≥ versionCode 705), exactly like
`wardenSupport.appHold` gates holds on old wardens.

### Tethering (v1 — device-enforced, Android)

The `tethering` clause (wire tag `"tethering"`, store key `7`) governs the ward's
**hotspot/tethering** posture. It is **default-blocked**: a phone hotspot hands
tethered devices raw upstream internet, kernel-forwarded around the on-device
DNS/VPN filter (verified on Android 13–16 and GrapheneOS), so it is a clean
bypass unless closed. Replace-the-state, per-(subject, kind) monotonic `issuedAt`.

```ts
type TetherAllow = "none" | "raw" | "filtered";

interface GrantTethering {
  v: 1;
  issuedAt: number;
  allow: TetherAllow;
  /** Unix seconds when the allowance ends (ends AT this instant). Absent =
   *  until a superseding clause revokes it. Meaningless (and omitted) on `none`. */
  until?: number;
}
```

**Enforcement (fail-safe, level-triggered per tick):**
- `none` / no clause / a malformed clause ⇒ **blocked**: `DISALLOW_CONFIG_TETHERING`
  (locks Wi-Fi + USB + Bluetooth tethering; re-applying it auto-terminates a live
  hotspot — OS behavior). This is in the Device Owner baseline.
- `raw` ⇒ tethering config freed for the granted window. Tethered devices get
  UNFILTERED internet; the ward's own device stays filtered. The guardian UI must
  state this plainly.
- `filtered` ⇒ **Charter Hotspot**: the system Wi-Fi hotspot is locked
  (`DISALLOW_WIFI_TETHERING`, which leaves the app's own local-only AP legal),
  and Charter hosts a local-only AP whose guests egress through an in-app
  filtering proxy applying the SAME policy as the DNS filter (`content` clause).
  Guests that don't use the proxy get no route (fail-closed). The proxy grants
  guests the **public internet only**: the address each tunnel actually connects
  to (post-DNS) is re-vetted, refusing loopback/link-local/RFC1918/ULA/CGNAT —
  a rebinding name or raw private IP can never reach the phone or the home LAN. No competing
  parental-control product filters shared connections.

A grant with `until` ends AT that instant — the next tick re-applies the block
(auto-killing any live session). `until` recomputed each tick, like `appRules`;
no alarms.

### Listening — audio at the lock (v1 — device-enforced, Android)

The `listening` clause (wire tag `"listening"`, store key `14`) is the family's
agreed answer to a small, recurring cruelty: the lock landing mid-chapter. It
names the apps that may keep **playing** when the screen shades, and for how
long. The screen still locks; nothing new may start.

```ts
type ListeningMode = 'stop' | 'continue' | 'grace';

interface GrantListening {
  v: 1;
  issuedAt: number;
  /** `stop` = the lock takes everything (the behaviour before this clause).
   *  `continue` = a story may finish, for as long as the lock stands.
   *  `grace` = a story may finish, within `graceMinutes`. */
  mode: ListeningMode;
  /** `grace` only. Device refuses 0 or > MAX_LISTENING_GRACE_MINUTES (240). */
  graceMinutes?: number;
  /** Packages that count as listening. Empty ⇒ nothing is exempt. */
  apps?: string[];
}
```

**Audio must actually be playing.** The verdict takes the platform's own
"is audio playing" signal as a required input, not a courtesy: without it a
named app keeps itself alive through every lock by playing silence. The
guardian's list says *which* apps may; producing sound says *when*.

**The grace clock is the DEVICE's**, pinned on first sight of the lock (the
same discipline as `standdown`, and for the same reason: a reboot must not
restart it). It is measured as **elapsed since the lock**, never as
`end − now`, so a clock that jumps backwards cannot mint a longer grace than
the guardian granted.

**Fail-safe direction (CLOSED, opposite to `lifeline`'s break-glass):** this
clause LOOSENS enforcement — it keeps an app alive past a lock — so a wrong
`v`, an unlisted package, a `grace` with no or an absurd `graceMinutes`, or no
audio all resolve to `stop`. Break-glass fails the other way because the harm
there is a ward stranded with no way out; the harm here is a story ending
early.

**Platform scope: Android only.** `charterd` stores the clause and does not
read it — a Linux lock freezes the whole user slice, so audio stops regardless.
A guardian setting this for a laptop-only ward is authoring a rule that machine
cannot keep; the mode is deliberately indistinguishable from `stop` on the
wire, so a guardian surface must version-gate rather than infer.

### Always available — apps open at any hour (v1 — device-enforced, Android)

The `alwaysavailable` clause (wire tag `"alwaysavailable"`, store key `15`) is
the family's answer to the apps a lock shouldn't reach at all: an audiobook
player at 2am, a messaging app at a sleepover. It names packages that may
launch and keep running through a lock, standing or until an absolute
instant.

```ts
interface AlwaysAvailableEntry {
  pkg: string;
  /** Unix seconds; the grant ends AT this instant. Absent ⇒ standing, no end. */
  untilUnix?: number;
}

interface GrantAlwaysAvailable {
  v: 1;
  issuedAt: number;
  /** Empty ⇒ nothing is exempt. */
  apps: AlwaysAvailableEntry[];
}
```

**`untilUnix` is an ABSOLUTE instant, never a duration** — mirrors `AppHold`.
A duration restarts every time the stored clause is re-read, and the grant
would never end.

**The exemption is narrow on purpose, and only in one direction.** A package
in `apps` is exempt from a `schedule` or `budget` lock ONLY. What unites those
two, and separates them from the reasons below, is not "the device reached
this on its own clock" — a device set to **dormant** (`schedule.paused`,
"off unless a guardian opens it") reaches `schedule` too, with no clock
involved at all. What unites them is that both are **DEFAULT-STATE locks**:
the ordinary shape of the device when nobody has just done anything to it — a
closed window, a spent budget, a device that is off unless and until someone
turns it on. The always-available naming IS the standing exception to that
default state, chosen once and left in force, exactly as its name says:
**always** available means available through the default, not just through a
schedule window's edges. It is **never** exempt from a `standdown`: that is
an ACTIVE guardian intervention — stop, right now — and if an app could sit
outside a moment like that the button would stop meaning anything (the
lifeline still reaches her either way, so nothing is lost by holding the line
there). It is **never** exempt from a malformed charter, either: an
untrustworthy charter is not a state Charter can distinguish from any other,
so nothing it names can be trusted to be a safe app. And it never overrides a
**standing block in the `apps` clause** — the guardian's outright "no" always
wins over her own "except this one, sometimes". The device subtracts the
always-available set from the LOCK-DRIVEN posture only, never from the
ordinary allow/block list.

Founder's ruling (2026-08-04), verbatim in substance: *"if that tablet is off
until a request has been accepted or approved, or I just say it's now on — if
that is set up, so a specific app is always available, then that's the way it
should be."* A dormant device is not a form of enforcement to except an app
FROM; it is simply the default the naming was always meant to stand outside
of.

**Fail-safe direction (CLOSED, same as `listening`):** this clause LOOSENS
enforcement, so a wrong `v`, a lock reason outside `schedule`/`budget`, or an
expired entry all resolve to "not exempt" — the behaviour before this clause
existed. An expired entry is inert, not poison: it drops out and the rest of
the list stands.

**Platform scope: Android only.** `charterd` stores the clause and does not
read it — a Linux lock freezes the whole user slice, so an exempted app is
frozen along with everything else regardless of what the clause says; there is
no per-process carve-out to freeze around. A guardian setting this for a
laptop-only ward is authoring a rule that machine cannot keep.

### Maintenance — the install window (v1 — device-enforced, Android)

The `maintenance` clause (wire tag `"maintenance"`, store key `11`) opens the
ward's device for installs for a bounded window. It exists because the Device
Owner baseline holds `DISALLOW_INSTALL_APPS` **and**
`DISALLOW_INSTALL_UNKNOWN_SOURCES` permanently, and on GrapheneOS sandboxed
Play is itself an "unknown source" — so lifting one alone silently does
nothing.

```ts
interface GrantMaintenance {
  v: 1;
  issuedAt: number;
  /** ABSOLUTE unix seconds — the window ends AT this instant. */
  untilUnix: number;
}
```

`untilUnix` is absolute for the same reason `apps.holds` is: a duration
restarts every time the stored clause is re-read, and the window never shuts.
Level-triggered per tick, so a reboot inside a window neither extends nor
cancels it. The device also keeps a **device-local span record** (store key
`102`, `{"startedAt":…,"endedAt"?:…}` — not a clause, nothing signs it, it
never crosses the wire) so the ward's own notice and the guardian's account of
the window survive the clause being superseded; closing a window early would
otherwise destroy the record of when it began.

**Fail-safe direction (CLOSED):** this clause loosens enforcement, so doubt
resolves to a shut window — the restrictions are re-applied every tick from the
baseline, and only a live, well-formed clause holds them off.

### Update — self-update (v1 — device-enforced, Android)

The `update` clause (wire tag `"update"`, store key `8`) tells a ward's device
to run a given version of its own Charter app or newer. A ward phone refuses
adb installs by construction (`DISALLOW_INSTALL_APPS`), so **self-update is the
only upgrade path** — which is exactly why it is a clause rather than an
out-of-band chore.

```ts
interface GrantUpdateApp {
  v: 1;
  packageName: string;      // must be the app's OWN package
  versionCode: number;      // positive; level-triggered (already-newer = no-op)
  versionName: string;
  url: string;              // https:// only
  apkSha256: Hex32;         // checked by the stager, before install
  signerCertSha256: Hex32;  // signing-cert continuity, checked before commit
}
```

**Both digests are load-bearing and checked at different moments:** the archive
sha256 in the stager (are these the bytes we were promised?) and the signing
certificate in the installer (is this the same publisher?). A signer change is
refused rather than installed — an update that could re-sign the warden would
be a complete bypass of everything else in this document.

Level-triggered: a device already at or past `versionCode` does nothing, so the
clause is safe to leave standing. Attempts and the last error are retained for
STATUS rather than retried silently forever — a device that cannot update needs
to be able to say so.

**Platform scope: Android only.** Charter for Linux updates through `apt` /
the published `.deb`, so `charterd` stores this clause and never reads it.

### Enforcement parity — which warden reads which clause

Every clause is **authorable for any ward**, but the two wardens do not read the
same set. A clause a device stores and never reads is the worst failure mode in
this system: the guardian is told nothing, the ward notices nothing, and the
rule simply does not exist. Guardian surfaces MUST version- and platform-gate
against this table rather than assume parity (MyCharter: `wardenSupport`).

| Clause | Linux `charterd` | Android ward | Note |
|---|---|---|---|
| `schedule` (1) | enforced | enforced | |
| `budget` (2) | enforced | enforced | |
| `content` (3) | **partial** — Firefox `policies.json` only | enforced (DNS-filter `VpnService`) | Linux renders a DNS plan it does not yet apply, so SafeSearch / YouTube-restrict do not take effect there, and only Firefox is governed |
| `apps` (4) | enforced (≥ 0.7.1) | enforced | Linux `allowlist` is scoped to the machine's `.desktop` inventory — see the Apps section |
| `learning` (5) | enforced | **stored, inert** | |
| `apprules` (6) | enforced | enforced | |
| `tethering` (7) | n/a (no hotspot to govern) | enforced | |
| `update` (8) | n/a (`apt` / `.deb`) | enforced | |
| `lifeline` (9) | **stored, inert** | enforced | no call/torch/break-glass on the Linux lock |
| `buckets` (10) | enforced | enforced | |
| `maintenance` (11) | n/a (no install lock) | enforced | |
| `gift` (12) | enforced | enforced | |
| `standdown` (13) | enforced | enforced | |
| `listening` (14) | **stored, inert** | enforced | a Linux lock freezes the slice, so audio stops regardless |
| `alwaysavailable` (15) | **stored, inert** | enforced (≥ versionCode 39) | a Linux lock freezes the whole user slice, so an exempted app is frozen with everything else regardless of what the clause says |

"**stored, inert**" means the clause is authenticated against the pinned
guardian and cached exactly as any other — rollback protection and all — and
then never consulted. It is a gap to close, not a design; "n/a" is a genuine
platform difference with nothing to enforce.

### Device brokering — `time.extend` (v1, frozen)

Charter for Linux brokers a child "ask for more time" request. The guardian
approves it by **signing a gift-wrapped GRANT** (the device-brokering surface,
not a sign-time clause). Frozen wire shape:

```ts
interface TimeExtendRequestParams { minutesRequested: number /* u16, 1..=1440 */; reason?: string /* ≤280, private */; limitHit: 'schedule' | 'budget' | 'bucket'; bucketId?: string; }
interface TimeExtendGrantParams   { minutesGranted: number   /* u16, 0..=1440 */; limitHit: 'schedule' | 'budget' | 'bucket'; bucketId?: string; }
```

- `MAX_EXTEND_MINUTES = 1440` (24h); minutes are **`u16`** on both sides.
- The guardian may grant **less** than requested; `minutesGranted: 0` is a
  no-op. The **grant echoes `limitHit`** so the device's params-echo check is
  exact.
- The grant `exp` is **today-only additive** and idempotent by `reqId`; the
  device discards it on the day roll. The extension offsets only the
  dimension named by `limitHit`. The device's actual validity cap is
  end-of-day in its **enforcement tz** — the schedule clause's tz, else the
  budget clause's tz, else UTC when NEITHER clause exists
  (`charter-spine::enforcer_runtime::time_extend_eod`, the LATER of the two
  local midnights when both a schedule and a budget clause exist) — and a
  grant whose `exp` lands more than 300s past that cap is rejected
  (`EnactError::Terminal`), silently from the guardian app's point of view
  (the grant still publishes; the device just never enacts it). The
  **buckets clause's tz is NOT consulted** for this cap, even for a
  `bucket`-hit extension on a named-times-only ward that has neither a
  schedule nor a budget — the guardian side clamps its signed `exp` to this
  same rule before publishing (`wire/grant.ts`'s `deviceExtendCapEod`) so it
  never signs a grant the device would silently drop. A future ward release
  may teach the device to consult the buckets clause's own tz for this cap
  too (matching how it already keys the bucket's own extension-pool
  rollover by it); once that ships, the guardian-side clamp can relax to
  match.
- The child-authored `reason` rides **only** the E2E gift-wrapped request and
  **never** appears in audit tags or local logs.
- **`bucket` — a named-times group hit its allowance instead of the
  whole-device wall.** `LimitHit` gains `'bucket'`; when it is used, `bucketId`
  names the specific bucket and is **echoed verbatim** from the request into
  the grant, exactly like `limitHit` itself. `bucketId` is absent for the
  whole-device dimensions (`schedule`/`budget`), and absent bytes from a
  pre-buckets device or guardian still parse as unset — nothing new to
  ship-order. Device-side, a granted `bucket` extension calls
  `ExtensionLedger::apply_bucket(now, reqId, minutes, bucketId)` — a separate,
  bucket-id-keyed pool (read back via `bucket_extra_secs(now, bucketId)`), not
  a variant of `Dimension` and never touching the device's `schedule`/`budget`
  pools (see "Group extension pool" above). The PWA is a deployed web app, so
  the guardian side understands `'bucket'` the moment it ships; there is no
  stale-guardian window to design around.

A `bucket`-hit extend, request/grant pair:

```jsonc
// REQUEST params
{ "minutesRequested": 20, "reason": "just finishing this build", "limitHit": "bucket", "bucketId": "play" }
// GRANT params (guardian granted less than asked)
{ "minutesGranted": 15, "limitHit": "bucket", "bucketId": "play" }
```

### App-open ask — `app.open` (v1)

A new brokered op, `app.open`: a child-authored ask to open, or keep open past
a hold's end, an app the `apps` clause presently gates via `blocked`/
`askFirst` (named times' "On request" policy). It rides the **identical**
REQUEST envelope and transport as `install.*`/`exec.allow`/`time.extend`
(`reqId`/`nonce`/`subject`/`machine`/`ts`), so nothing about brokering itself
is new — only the params shape and what answering it means.

```ts
interface AppOpenRequestParams {
  /** On-device app identity — same vocabulary as appRules/buckets. */
  pkg: string;
  /** Display-only, bounded like `reason` (mirrors InstallApkRequestParams). */
  label?: string;
  /** u16, 1..=1440 when present. */
  minutesRequested?: number;
  /** ≤280 chars, private — never appears in audit/logs. */
  reason?: string;
}
```

**`app.open`'s GRANT is a minimal answer SIGNAL, shaped exactly like
`time.extend`'s** (fixed 2026-08-03, review round 1 — the original design
gave it no grant params at all, which meant an ALLOW could never verify:
`verify_grant`'s `Decision::Allow` arm unconditionally parses op-specific
params before any signature/id check runs, so "no params to parse" and "this
grant is forged" were indistinguishable, and a real allow left the ward's
pending-ask record stuck `Pending` forever — see `charter-proto`'s
`AppOpenGrantParams` and `charter-spine`'s `AppOpenEnactor`, a deliberate
no-op enactor registered purely so the lifecycle reaches `Enacted`). The
guardian's one-tap answer is two separate things:

1. The GRANT itself — `{ pkg, minutesGranted }`. `pkg` is the TRUSTED
   SIGNER's own echo of the REQUEST's `pkg`, by convention — same as every
   other op's params on this wire: `verify_grant` binds `reqId`/`nonce`/`op`
   to the pending request but never cross-checks `pkg` (or any op's params)
   against it, and the ward keeps no copy of a pending request's params to
   compare against. The signed GRANT is the sole authority. `minutesGranted`
   is a u16 0..=1440 (`0` on a deny). The GRANT closes the loop in the ward's
   UI exactly like any other op's grant, but **enacts nothing on its own** —
   there is no OS effect tied to receiving it.
2. **Only on allow:** the guardian ALSO re-signs the standing `apps` clause
   with a time-boxed `AppHold { pkg, state: 'allowed', untilUnix }` — the
   identical machinery already shipped for "allow Vanadium for an hour". This
   is what actually opens the app; the GRANT above is answer-only. A deny
   needs no clause change at all; the app is already `blocked`, and stays
   that way.

```ts
interface AppOpenGrantParams {
  /** Echoed verbatim from the REQUEST. */
  pkg: string;
  /** u16, 0..=1440. `0` on a deny — never parsed as a duration in that case. */
  minutesGranted: number;
}
```

A REQUEST carrying `app.open`:

```jsonc
{
  "v": 1,
  "op": "app.open",
  "reqId": "3f9c1a7e2b6d4c8a9f0e1d2c3b4a59683f9c1a7e2b6d4c8a9f0e1d2c3b4a5968",
  "nonce": "8a12f0e3c9b7d6a54321fedcba0987658a12f0e3c9b7d6a54321fedcba098765",
  "subject": "…",
  "machine": "…",
  "ts": 1732550400,
  "params": {
    "pkg": "com.mojang.minecraftpe",
    "label": "Minecraft",
    "minutesRequested": 30,
    "reason": "almost done building"
  }
}
```

Its GRANT, on allow:

```jsonc
{
  "v": 1,
  "op": "app.open",
  "reqId": "3f9c1a7e2b6d4c8a9f0e1d2c3b4a59683f9c1a7e2b6d4c8a9f0e1d2c3b4a5968",
  "nonce": "8a12f0e3c9b7d6a54321fedcba0987658a12f0e3c9b7d6a54321fedcba098765",
  "decision": "allow",
  "ts": 1732550400,
  "exp": 1732636800,
  "params": { "pkg": "com.mojang.minecraftpe", "minutesGranted": 30 }
}
```

### Device-broker transport (v1)

> **Coordination status (updated 2026-07-22).** `charterd` (Charter for Linux)
> has **frozen** the transport below and pins it with `charter-proto` golden
> vectors. The guardian half is **built and live in MyCharter** (local-key
> vault mode): request intake (`subscribeRequests`), the Approvals inbox, the
> local guardian signer, and the native carrier APK's wake channel. A
> Signet-bunker vault mode (the guardian surface asks Signet to decrypt/sign
> over NIP-46 instead of the local key) is the designed hand-up and is **not
> yet built** in signet-app. Items tagged **[decide]** are genuine cross-repo
> choices that need Signet sign-off before the Signet-side half ships.

**Nostr kinds (single-sourced; no divergent numbers).** Inner rumor kinds
`CHARTER_DEVICE_REQUEST = 31111`, `GRANT = 31112`, `CLAUSE = 31113`,
`AUDIT = 31000`; the outbound `CHARTER_DEVICE_STATUS = 31114` and the reserved
inbound `CHARTER_DEVICE_USAGE_SYNC = 31115` (both contract-frozen but
device-deferred — see the STATUS and consolidation subsections below) round out
the namespace. Every charter-device event carries the marker tag
`["t", "charter-device"]`. Delivery is **NIP-59 gift-wrapped** (`SEAL = 13`,
`GIFT_WRAP = 1059`) for private, durable store-and-forward.

**Signed-inner-event gift-wrap (load-bearing).** A guardian→machine **GRANT
(31112)** and **CLAUSE (31113)** are **fully-signed NIP-01 events** (valid
`id` + BIP-340 `sig` by the **pinned guardian key**), and are then *sealed*
(13) and *wrapped* (1059) — they are **NOT** bare NIP-59 rumors. The device
re-derives the inner `id`, verifies the inner schnorr `sig`, and checks the
author == pinned guardian **before** acting (a GRANT) or caching (a CLAUSE). A
hostile relay can delay/drop but can **never forge** a grant or a clause. The
gift-wrap spread MUST preserve the inner `sig`.

**Envelopes** (the inner event `content`, JSON, `camelCase`; bytes pinned by
`charter-proto` golden vectors — 32-byte ids/nonces are lowercase hex):

```ts
// REQUEST (31111, machine → guardian)
interface RequestPayload { v: 1; op: 'install.flatpak'|'exec.allow'|'time.extend'|'app.open';
  reqId: Hex32; nonce: Hex32; subject: Hex32; machine: Hex32; ts: number; params: OpParams; }
// GRANT (31112, guardian → machine, inner event signed by the pinned guardian)
interface GrantPayload   { v: 1; op: …; reqId: Hex32; nonce: Hex32; decision: 'allow'|'deny';
  ts: number; exp: number /* exp > ts; device accepts within ±300s skew */; params: OpParams; }
// CLAUSE (31113, guardian → machine, inner event signed) — body is GrantSchedule | GrantBudget
interface ClausePayload  { kind: 'schedule'|'budget'; subject?: Hex32 /* the child this clause targets (== Signet dependantId); absent = the pairing's sole subject_pubkey, back-compat */; issuedAt: number; body: object; }
```

The GRANT echoes the request's `reqId`+`nonce` verbatim and (for install/exec)
the **exact** approved `params`; only `time.extend`'s and `app.open`'s
`minutesGranted` may be clamped ≤ requested (`0` on a deny). Enact authority
is the **signed grant**, never the request — except `app.open`, whose GRANT
enacts nothing at all (see "App-open ask" above): its `{pkg, minutesGranted}`
is an answer signal only, and the guardian's ALSO-signed `apps` clause
carrying an `AppHold` is what actually opens the app.

**Per-child CLAUSE — optional `subject` (additive, back-compat).** A CLAUSE MAY
carry an optional `subject: Hex32` naming the child it targets (== the Signet
`dependantId`; the same opaque, PII-free pubkey as `RequestPayload.subject`).
**Absent** `subject` means the pairing's sole `subject_pubkey` — the single-child
default, which keeps every existing `charter-proto` golden vector valid. With
`subject` present, one pairing carries independent policy for many children. Two
consequences follow:

- **Rollback protection is per-`(subject, kind)`.** The monotonic `issuedAt`
  high-water mark is tracked independently for each child and clause kind, so a
  hostile relay can neither revert one child's clause nor replay another child's.
- **A clause for an unbound subject is authenticated + cached but inert.** If the
  named `subject` is not yet bound to a local account, the clause is still verified
  against the pinned guardian and cached — it simply has no effect until the admin
  creates the binding, at which point it applies. Caching (not rejecting) avoids a
  binding-ordering race; the clause is guardian-signed, so it is authentic
  regardless of arrival order.

The **trust root is unchanged**: there is still exactly **one pinned guardian**,
and every CLAUSE is its fully-signed inner event. Per-child keying changes *which
subject* a clause addresses, never *who* may author it.

**Pairing — `bunker://` grammar.** `charterd`/`charter-cli`/`charter-setup` pin
the guardian from `bunker://<guardian-pubkey>?relay=wss://…&kind=charter`:
a valid 64-hex guardian pubkey, ≥1 `wss://` relay, and `kind=charter`.
**[decide]** Whether `kind=charter` rides the `bunker://` URI itself or the
app-initiated `nostrconnect://` connect-metadata (and whether `ws://localhost`
is allowed for local dev) is a Signet-coordinated choice; both the daemon
(`pin_from_connect`) and the CLI validator move in lockstep when it settles.
First-run pairing is admin-only; re-pairing requires admin or the existing
guardian's signed authorization (never the managed user).

**Device audit (31000).** `charterd` emits a **machine-authored**,
gift-wrapped-to-guardian audit per decision: tags
`["t","charter-device"]`, `["outcome", …]`, `["op", …]`; **content is always
empty**; no child content, exec source path, `time.extend` reason, or DOB ever
appears. The outcome vocabulary is `enacted | denied | failed | locked | thawed
| override`.

**Break-glass override event (frozen 2026-07-24; design memo
`docs/superpowers/specs/2026-07-24-lifeline-incall-and-break-glass.md`).**
When the ward activates the emergency override, the device emits the standard
device audit with tags `["t","charter-device"]`, `["outcome","override"]`,
`["op","unlock.breakglass"]`, `["scope","calls"|"full"]`,
`["durationSecs","<n>"]` — content empty, no reason text, no PII. The
override itself **never waits on the wire**: offline, the event is journaled
durably and emitted on reconnect. Guardian side: the carrier notifies
immediately on any `outcome=override` audit; MyCharter renders it in Activity
and the week. Configuration rides the **lifeline clause** as an additive
optional field — `breakGlass?: { enabled: boolean, scope: "calls"|"full",
durationMinutes: number }` — and the lifeline `numbers` cap grows **1..3 →
1..5** plus an optional platform-sourced emergency entry
(`emergencyServices?: boolean`; the device resolves the region-correct number
itself, never a wire-carried digit string). Device-side acceptance of the
wider shapes MUST ship before MyCharter authors them (fail-closed validation
would otherwise render no lifeline at all).

The clause also carries **`torch?: boolean`** — offer the ward a torch on the
shade. A locked phone is still a light: a child walking home in the dark should
not have to use the emergency unlock just to see, and teaching them that the
break-glass button is for ordinary things devalues it. Off when absent, so
pre-torch payloads stay byte-identical. Unlike the wider `numbers` shapes this
needs **no ship-order guard**: the body does not deny unknown fields, so an
older ward simply ignores it rather than rejecting the clause. The device hides
the affordance entirely on hardware with no flash — never a dead control — and
the torch deliberately keeps burning if the shade goes away (a light that
snuffed itself out on unlock would be a nasty surprise halfway down a lane).
**[decide]** This kind/tagging differs from signet-app's existing guardian-
authored audit feed (`["t","audit"]`); reconcile to either a distinct device-
audit kind or a shared `["t","audit"]` + device-outcome vocabulary, and decide
whether to also wrap audits to the **dependant** when `audit_transparency` is
set (the device parses but does not yet act on that flag).

### Device STATUS feed — `CHARTER_DEVICE_STATUS = 31114` (v1 wire; emission deferred)

> **Coordination status.** The STATUS wire below is **frozen** in this contract,
> but **device emission is deferred** — `charterd` does not yet emit it (it needs
> the guardian/PWA half to exist). One feed, **two** consumers: the MyCharter PWA
> renders it for the guardian, and the multi-device consolidation aggregator (next
> subsection) sums it. Items tagged **[decide]** are genuine cross-repo choices
> that need Signet sign-off before either side ships.

A **machine-authored**, gift-wrapped-to-guardian status event, **one per child
per device** (the inner event `content`, JSON, `camelCase`):

```ts
// STATUS (31114, machine → guardian, gift-wrapped)
interface StatusPayload {
  v: 1;
  subject: Hex32;            // which child
  machine: Hex32;            // which device (== Pairing.machine) — the aggregation key
  ts: number;
  dayKey: string;            // 'YYYY-MM-DD' in the budget clause's tz (the daily reset boundary)
  weekKey?: string;          // ISO week key in the budget tz, when a weekly cap is set
  usedTodaySecs: number;     // THIS device's raw contribution today  → aggregation input
  usedWeekSecs?: number;     // THIS device's raw contribution this week
  windowLeftSecs: number;    // device's local view: schedule window left → display
  quotaLeftSecs: number;     // device's local view: budget quota left    → display
  effectiveSecs: number;     // min(window, quota) on this device         → display
  locked: boolean;
  lockReason?: 'schedule' | 'budget' | 'malformed' | 'standdown';
  source: 'guardian' | 'device-only' | 'unconstrained';   // which policy is in force for this child now
}
```

**A consumer MUST NOT reject a STATUS it cannot fully name.** An unrecognised
`lockReason` (or `source`) has to DEGRADE — drop the field, keep the report —
never invalidate the payload. `lockReason` is display-only, and the rest of a
heartbeat is still perfectly good without it. MyCharter got this wrong until
2026-07-27: it returned null on any unknown reason, so a phone on newer firmware
reporting a word that build had never heard of vanished from the guardian's view
entirely — no last-seen, no time-left — and read as a **dead phone** rather than
a new vocabulary. That made every future clause kind a deployment trap, since
devices self-update on their own schedule and will always be able to run ahead of
a guardian's app.

The three `source` values match the device's `PolicySource` exactly: `guardian`
(a paired guardian's per-child clauses), `device-only` (the local fallback), and
`unconstrained` (a bound child the guardian hasn't set *and* with no device-only
limits yet — the absent-clause semantics named in the Authority section below).

**`unrecognisedTodaySecs?: number` (charterd ≥ 0.7.5, honest attribution).**
Foreground-screen seconds today that are **not vouched for** AND **not matched
by any identity any of the child's clauses currently name** (`learning`,
`buckets`, `appRules`, the standing `apps` clause — the process's ancestors
walked too, the same way `bucket_id_for` already does, so a guardian-governed
launcher and the game it spawns are judged as one thing).

**Not vouched for is exactly ONE thing:** the exe's owner is a **known,
non-root uid** — the ward could have written or replaced that binary. That
owner uid is read by `stat`ing the kernel's magic symlink `/proc/<pid>/exe`
itself, never the path string it renders to (see the mount-namespace forgery
below).

**SCOPE — what this field does NOT count, and why (binding).** A
**ward-authored payload run by a root-owned interpreter** (`java -jar
~/x.jar`, `python3 ~/game.py`) is **deliberately not counted**. Such a rule
was specified and withdrawn twice; the shape *"a root-owned program with the
ward's own file in its argv"* is not decidable from outside, because it is
equally the shape of a repacked jar and of **a system app opening the child's
own document**. Both separating rules attempted produced false accusations on
a real machine: a suffix + exec-bit test flagged `xed ~/homework.py` and
`evince ~/essay.pdf`; an interpreter allowlist keyed on the exe basename
flagged `drawing ~/art.png`, because the reference distro ships **130
`#!/usr/bin/python*` launchers in `/usr/bin` alone** (581 shebang launchers in
total, including the image editor and the desktop's own tools), every one of
which resolves `/proc/<pid>/exe` to the interpreter. The governing principle
wins: *absence of evidence must never become a finding* — a false "Charter
couldn't identify 3h" about a child is worse than missing a bypass.

So this field's scope is **software running from a ward-owned executable**,
which covers the realistic routes (a renamed binary, a home-installed
launcher and its bundled JRE, an AppImage) and not the skilled one (a
repacked jar under the system JVM). **A consumer must not present this number
as a complete account of unvouched software** — it is a floor, not a total.
Deliberate under-reporting in the safe direction is the design.

**Unknown ownership NEVER accrues.** A failed `stat` is not evidence: a
process that exits between the probe and the stat, or a `/proc/<pid>/exe` the
warden could never read, yields no owner and must never become a finding. Same
fail direction as a failed probe.

**Sandboxed apps have KNOWN ownership (normative; corrected for charterd ≥
0.7.5's magic-symlink rule).** A warden that reads ownership off the *rendered*
exe path sees nothing for a sandboxed app — a flatpak's `/proc/<pid>/exe`
renders `/app/bin/<foo>`, which exists inside the sandbox's own mount namespace
and not on the host — and so treats every flatpak as "unknown". A warden that
stats the magic symlink (as it MUST, per the rule below) resolves the real
inode through any namespace and therefore knows the owner. The resulting
verdicts are:

| sandboxed app | owner uid | `unrecognisedTodaySecs` |
|---|---|---|
| **system** flatpak (`/var/lib/flatpak`, root-owned) | `0` | never accrues |
| **`flatpak install --user`** (under the ward's own home, ward-owned) | the ward's | **accrues** unless a clause names it |

The second row is deliberate: a ward-installed sandboxed app the guardian has
not named is exactly the software this field reports. Naming it in any clause
(`learning`, `buckets`, `appRules`, `apps`) silences it, as for any other
ward-owned executable. Verified live against a real system flatpak and against
a ward-owned binary behind a namespace-only path.

**Suppression trusts KERNEL-RESOLVED IDENTITY ONLY.** Of everything an
identity matcher can read, only `/proc/<pid>/exe` comes from the kernel;
**argv0, the rest of argv, and the cgroup path are all written by the ward —
and stay ward-written when the exe is root-owned**, because the ward launches
the process and hands it both. Wide matching over all of them is right where a
match *restricts* — a forged field only gets the ward's own process killed, so
enforcement keeps every arm — and wrong where a match *spares*, because each
ward-writable field is then an off switch the ward holds. Each forgery below
was verified live against a governed identity, each earning no free time and
stopped by nothing, each previously silencing this counter:

<!-- The verbatim reproduction commands are omitted from this narrative (they
     are a paste-ready recipe); each row describes the technique and the field
     it forges. All were verified live and are closed by the single rule below. -->

| forgery (technique) | ward-writable field |
|---|---|
| copy a governed binary's name onto another binary | the basename of `exe` |
| launch with a spoofed argv0 | argv0 |
| wrap the process in a self-named unprivileged user scope | the cgroup path |
| put a `cmdline:` needle in your own argv | argv |
| launder any of the above through a root-owned ancestor inside the forged scope | argv + cgroup, past a root-exe gate |

The last row is why an earlier revision — "believe the `cmdline:`/flatpak
arms when the exe is root-owned" — was inert exactly where it had to bite:
the ward writes the argv and names the scope of the root-owned programs they
launch. So suppression is ONE rule: an exact governed match on the
kernel-resolved `exe` path — the process's own, or an ancestor's own (each
ancestor judged by its exe path alone, or being *started by* a forgery is the
way around it). A governed `cmdline:` identity (the system JVM running
Minecraft) is indistinguishable by construction from the pasted needle, so it
cannot suppress here; that supported path stays un-accused one level up — a
live bucket hit or a learning credit settles the same seconds, never both
credited and unrecognised.

**Ownership must be read from the magic symlink, not the rendered path.**
`/proc/<pid>/exe` is a *magic* symlink: `readlink` renders a path string
reconstructed for the reader's own mount namespace, while a `stat` **through**
the link is resolved by the kernel to the executable's real inode. On a
distro with unprivileged user namespaces enabled — the reference platform — an
unprivileged user-namespace bind mount (verbatim command kept out of the
public repo) needs no privilege and leaves the link *rendering* as
`/usr/bin/wc`, a genuine root-owned distro binary, to every reader outside the
namespace. Measured live: the rendered path stat'd **uid 0**, the magic
symlink **uid 1000**. Since root ownership gates free learning time (§Learning
above), the site-app runtime check, and this counter, a warden implementation
MUST take the owner uid from `stat("/proc/<pid>/exe")` and MUST NOT stat the
readlink result. The rendered string stays correct for identity and display.
The same rule covers a second case: a binary unlinked after launch renders
`<path> (deleted)`, which no longer resolves, so a path-based lookup returns
"unknown" — and "unknown never accrues" would turn one `rm` into permanent
silence.

Day-keyed on the same boundary as
`usedTodaySecs`/`learningTodaySecs`; **absent when zero or unknown**, so a
device that has never seen anything unrecognised stays byte-identical to a
pre-0.7.5 payload. The ward sees the same number on their own surfaces —
Charter's transparency invariant does not carve out an exception for what it
cannot identify.

**Why NOT "absent from the device's inventory" (the original design, revised
after review against a real machine).** That reading is a false accusation
waiting to happen: Charter's own lock shade (a `.desktop` `Exec=` naming one
binary while a different one actually runs), a browser installed as a
wrapper script (`Exec=` names the wrapper; the real binary an `exec` away is
a different path), a multi-process suite (LibreOffice's real process is
`soffice.bin`, never what any `.desktop` names), and anything installed as a
snap or under `/usr/local` (never scanned by the inventory at all) would ALL
have accrued hours of "Charter couldn't identify this" about a child's
completely ordinary afternoon — worse than the bypass this feature exists to
name. It was also forgeable in the SUPPRESSING direction: a single
ward-writable `.desktop` entry naming `~/.local/bin/java` would zero the
counter forever. The definition above is a POSITIVE signal instead — *"your
child spent time in software Charter can't vouch for"* — and it does not
depend on the inventory staying complete. Nothing about its suppression side
is closed "by construction": a ward cannot make their own binary root-owned,
but they can trivially make it *share a name* with a governed one, wear a
governed argv0, sit in a hand-made `app-flatpak-*` scope, or quote a
`cmdline:` needle. It is closed by the kernel-resolved-only rule above —
by refusing to read what the ward can write.

**PRIVACY RULE — binding.** `unrecognisedTodaySecs` is **one aggregate
number, and only ever that**. STATUS carries no per-app usage today by
design, and this field is no exception: naming *which* unrecognised program
ran — a path, a class name, a label, anything that would let the guardian
single out one running thing on the machine — would be per-app behavioural
reporting of a child, exactly the kind of ambient surveillance Charter
refuses to do. The guardian learns *"Charter couldn't identify this much
screen time,"* which is grounds for a conversation, not a dossier; joined
against `apps`'s `userInstalled` entries (immediately above) they can draw
their own conclusion without Charter narrating the session for them. No
future revision of this field may add a path/pkg/label sibling — a new,
separately-justified field would be required, and it would have to clear the
same bar.

**`enforcementGap?: { unexplainedBoots: number; lastNoticedAt: number }`
(ward ≥ 2026-08-07, S1).** Boots this device went through with **no warden
running**. Absent — never a zero — when there have been none.

A warden cannot witness its own absence. Android safe mode disables every
third-party package, a Device Owner included, so a safe-mode session produces
no tick, no lock, no app suspension and no report; the phone comes back
enforcing as though the holiday never happened. `DISALLOW_SAFE_BOOT` closes
the door, and this counts anyone who finds another one. What the platform
keeps regardless is its own boot counter, so the warden records the count it
last ran under and treats daylight beyond the expected +1 as boots it missed.
`lastNoticedAt` is when it *noticed*, never when the gap began — nothing was
running then to see it, and a start time would be a guess.

Three cases are **not** a gap and MUST NOT be reported as one: the same count
again (a service restart inside one boot), exactly one more (the ordinary
reboot being run through right now), and a count that went **backwards** (the
platform reset it — a wipe or a restore — so the baseline is retaken rather
than reporting hundreds of unwarded boots). A device whose platform keeps no
boot counter reports nothing: an absent counter must never manufacture an
accusation.

The counter is **monotone for the life of the install** — there is no
acknowledge-and-clear, because a counter a ward can reset by waiting is not a
counter. It is a **floor and a fact, not an accusation**: a flash, a restore
or a platform-level force-stop lands here too, and a guardian surface MUST
present it as "the warden was not running for N boots" and leave the reading
to the family. Numbers only, per the privacy rule below — it never names what
ran while nothing was watching.

**Privacy.** Numbers + enums only — **no** child content, exec source path,
`time.extend` reason, name, or DOB ever appears. Delivery is E2E NIP-59
gift-wrapped to the guardian. `source` tells the PWA whether the guardian's
clauses or the device-only fallback is currently in force for that child;
`machine` + `dayKey` + the raw `usedTodaySecs` / `usedWeekSecs` are exactly the
inputs the consolidation aggregator needs (next subsection).

**[decide]** (Signet sign-off): the emission cadence; whether to also gift-wrap
the status to the **dependant** when `audit_transparency` is set (the device
parses but does not yet act on that flag); the exact field set.

### Scan-to-pair — `CHARTER_DEVICE_PAIR_OFFER = 31117` (guardian → machine)

An **unpaired** ward shows a QR carrying its machine key **and a one-time
token**:

```
charter://pair?m=<machine-hex-64>&t=<token-hex-32>
```

The guardian's phone scans it and replies with a PAIR_OFFER, gift-wrapped to
that machine key:

```jsonc
// PAIR_OFFER (31117, guardian → machine, inner event signed, gift-wrapped)
{
  "token": "<32 lowercase hex>",   // echoed from the ward's screen
  "relays": ["wss://…"],           // where the guardian will publish clauses
  "ts": 1730000000                 // seconds; the device bounds staleness on it
}
```

**Why this is safe, and the one rule that makes it so.** The payload names **no
guardian pubkey**. The device pins the **authenticated seal author** — the key
that actually sealed the wrap. A forged payload can therefore only ever
nominate its own sender, which is the whole point. *Never* pin a key read out
of the payload.

The token is the proof of **physical presence**: knowing it means having looked
at the ward's screen, the same trust basis as typing on the machine. Therefore:

- 32 lowercase hex (16 random bytes), compared in **constant time**.
- **Single use** — spent the moment a pin succeeds.
- **600-second TTL**, enforced against both the stored mint time and the
  offer's own `ts`; a future-dated `ts` is refused as clock-skew forgery.
- Stored **0600, root-only**. If the ward's own account could read it, the
  child could pair the laptop to a phone they control and grant themselves
  unlimited time. Minting is privileged (`charter-pair-invite`, pkexec'd);
  `49-charter.rules` denies the managed child pkexec.
- **A contested token pins nobody.** Two distinct keys presenting the same
  token means it leaked; the device refuses both rather than coin-flip on who
  owns the ward. The parent simply shows a fresh code.

The unpaired ward opens **only** this door, and only while a live token exists
— with no QR on screen it never even queries the relay.

**The offline path remains normative.** Scanning needs both devices online;
`bunker://` pasted into `charter-pair` is the no-internet fallback and is
unchanged. Its `--subject` is **optional**: MyCharter has no dependant pubkey
to give, so absent one the device mints a subject locally, exactly as the scan
path does.

### Multi-device usage consolidation — `CHARTER_DEVICE_USAGE_SYNC = 31115` (reserved; contract-complete)

> **Coordination status (updated 2026-07-24, B3a).** IMPLEMENTED on the device
> side: **both wardens** (Linux `charterd` + Android JNI, via the shared spine
> broker) now ingest `USAGE_SYNC`, verify it against the pinned guardian
> (`charter-verify::verify_usage_sync` — same signed-inner-event gift-wrap as a
> CLAUSE, monotonic by `ts`), cache it per subject, and enforce the **pooled**
> budget (`cap − |own ∪ elsewhere|` under the union rule, scalar fallback).
> Both wardens also emit `activeMinutesToday` in STATUS. **No new trust root.**
> The guardian-side aggregator/publisher is ALSO live (B3b, same day):
> MyCharter builds each device its elsewhere view from the STATUS feed and
> publishes guardian-signed USAGE_SYNC wraps, change-gated with per-subject
> strictly-monotonic `ts`. Items tagged
> **[decide]** still want Signet sign-off; the bitmap layout is frozen by
> cross-stack vectors (`charter-testkit/vectors/usage_sync/`).

**TRUST BOUNDARY — binding (S10, review 2026-08-07).** The figures consolidated
here are **self-reported by the devices**. A machine's signature on its STATUS
authenticates *who is reporting*; **nothing authenticates that the number is
true**, and everything downstream — the aggregator, the USAGE_SYNC payload, the
wardens' pooled enforcement, the guardian's weekly picture — inherits that.

A ward who extracts one device's machine key can under-report that device and
so enlarge their share of the pool on every SIBLING device. The precondition is
full compromise of a device they already control, which by itself defeats
enforcement *on* that device, so the delta is **cross-device only**. That is the
honest size of it — real, bounded, and not a reason to distrust the ordinary
case.

There is no fix at this layer and none is planned: a device is the only witness
to its own screen time, and a guardian's phone cannot see a laptop's foreground
window. The intended direction is to make dishonesty **visible** rather than
impossible — per-`(device, day)` monotonic counters, so a reported total can
never go *down*, plus a regression flag the guardian is shown. That is a
reflection surface, in keeping with the rest of Charter, not a lock. Any future
change here MUST NOT be described as making the numbers trustworthy.

**Keystone — identity is device-independent.** A child is **one** subject pubkey;
N devices each bind a local account to that subject. The guardian sends **one**
budget clause per subject and every device caches it — so the **cap** is shared
for free. Only **usage** must be consolidated.

**Aggregation model.** The guardian's backend (Signet) sums each subject's
`usedTodaySecs` / `usedWeekSecs` across all that subject's devices (from the
STATUS feed above) per `(subject, dayKey)` / `(subject, weekKey)`, and returns
each device a **spent-elsewhere** value — the sum of all *other* devices' usage
(this device excluded, so there is no double-count). It is delivered as a new
reserved inbound event, **guardian-signed** (the same pinned authority as a
CLAUSE — no new trust root):

```ts
// USAGE_SYNC (31115, guardian → machine, inner event signed, gift-wrapped)
interface UsageSyncPayload {
  v: 1;
  subject: Hex32;
  ts: number;
  dayKey: string;
  spentElsewhereTodaySecs: number;    // Σ usage of all OTHER devices today
  weekKey?: string;
  spentElsewhereWeekSecs?: number;    // Σ usage of all OTHER devices this week
  /**
   * UNION-RULE EXTENSION (additive, optional — see below): the union of all
   * OTHER devices' active minutes today, as a base64url 1440-bit bitmap
   * (bit i = minute i of dayKey local time was active on ≥1 other device).
   */
  elsewhereMinutesToday?: string;
}
```

**Union-rule extension (product-decided 2026-07-24; wire additive).** Summing
scalars double-counts *simultaneous* use (the child gaming on the laptop while
chatting on the phone would burn budget twice). The decided semantics: a
child's pooled usage is the **union of active minutes across their devices,
counted once**. To carry it: (1) each device's STATUS gains an optional
`activeMinutesToday` base64url 1440-bit bitmap alongside `usedTodaySecs`
(same self-reported trust class, no content — activity on/off per local
minute); (2) USAGE_SYNC gains the optional `elsewhereMinutesToday` above (the
guardian-side union of the *other* devices' bitmaps); (3) an enforcer that has
both computes `pooledUsed = |ownMinutes ∪ elsewhereMinutes|` and uses it in
place of `spentElsewhere + localUsed` (which remains the fallback when either
bitmap is absent — degrading to the scalar model, never failing). Bitmaps are
per-`dayKey` in the device's local tz, same as `usedTodaySecs`; the weekly
figure sums the daily unions. Freshness/monotonicity/offline rules are
unchanged — a stale or missing elsewhere-bitmap only *under*-counts the pool.

**Device enforcement extension (pure, additive layer).** Budget enforcement moves
from

```
quota_left = min over present caps of (cap − used)
```

to

```
quota_left = min over present caps of (cap − spentElsewhere − localUsed)   // saturating ≥ 0
```

where `spentElsewhere` **excludes this device** (no double-count) and `localUsed`
stays the device's own live usage ledger and keeps accruing between syncs. This is
one optional additive input to the enforcer (a `ConsolidatedUsage { daily, weekly }`)
— it does **not** rewrite the existing budget arithmetic.

**Durability / freshness.** `spentElsewhere` is durable per `(subject, dayKey)` /
`(subject, weekKey)`, **monotonic by `ts`** (a fresher `ts` supersedes; a stale
relay replay with an older `ts` is rejected), and implicitly **resets to 0 when
the period key rolls** (new day/week → 0 until that period's first sync). No `exp`
is needed.

**Offline + reconcile (the online constraint, made honest).** Offline, a device
enforces `cap − stale_spentElsewhere − localUsed` — siblings' *new* offline usage
is invisible, so a **bounded cross-device over-spend is possible** during an
offline window. On reconnect every device reports its full local usage via STATUS,
the aggregator re-sums and pushes fresh spent-elsewhere; if the consolidated total
now exceeds the cap, every device's `quota_left` saturates to 0 and **all lock**.
Over-spend is thus *caught and corrected on reconnect* — matching the
fail-safe-offline philosophy: offline you get per-device cap enforcement, not
consolidated.

**Trust boundary.** Usage is **device-self-reported**. `charterd` runs as root and
the managed child does not control it, so within-family device self-reports are
trusted (the anti-casual threat model). A replay of an old *lower* spent-elsewhere
(which would grant more time) is resisted by the `ts`-monotonic rule.

**[decide]** (Signet sign-off): the kind number `31115`; **guardian-signed** vs a
separately-pinned aggregator key (**recommend guardian-signed** — reuses the
pinned authority, no new trust root); the sync cadence; the offline fail-direction
— reset-to-0 vs **persist-last-known** (**recommend persist-last-known**); an
optional value-monotonic-within-period hardening on top of `ts`-monotonic; the
precise daily/weekly period-key derivation; and for the union-rule extension, the
bitmap encoding (base64url of 180 bytes proposed) + whether a weekly bitmap set
is carried or the weekly figure is derived guardian-side (**recommend derive**).

### Authority + identity model (per-child, multi-device)

- **One pinned guardian** signs **every** CLAUSE and every USAGE_SYNC. Per-child
  and per-device keying changes *which subject / period* an event addresses, never
  the trust root — there is always exactly one pinned guardian authority.
- A **child = a subject pubkey**, independent of any device; an admin binds local
  account(s) to that subject (the privileged pairing step). **Device-only** limits
  are the **fallback**: an unpaired device, an offline gap, or a child the guardian
  has never set. **No-local-authority** holds: only root (the guardian acting
  locally) or the pinned guardian may set policy — **never** the managed child.
- **Whole-child precedence.** For a bound child with **≥1** guardian clause, the
  guardian owns **both** dimensions (an absent dimension = *unconstrained*, the
  guardian's intent); device-only applies **only** to children the guardian has
  never touched; otherwise unconstrained (the existing absent-clause semantics). A
  malformed guardian **schedule** fail-safe **locks that child**; a malformed
  guardian **budget** fail-**opens** (no cap) — the schedule remains the fail-safe
  gate. Both behaviors are scoped per child, so one child's bad clause can never
  affect a sibling.

## NIP-46 methods

Charter methods are vendor-prefixed `charter_*`. They live alongside the standard NIP-46 methods (`sign_event`, `get_public_key`, `nip44_encrypt`, etc.) on the same `kind: 24133` transport.

### Authorisation

`charter_*` methods may only be called by trusted-app pairings whose `kind` field is `'charter'`. The bunker rejects calls from `kind: 'app'` (or absent) pairings with:

```json
{
  "id": "<request id>",
  "result": "",
  "error": "charter:not_authorised"
}
```

Charter pairings are established the same way as regular trusted-app pairings (signet-app-internal#165) but the connecting app advertises `kind: 'charter'` in its `nostrconnect://` connect-metadata. The guardian sees and approves the elevated authority on the pair confirmation screen ("Pair Charter? This app will manage time, spend and content rules for {dep}").

Existing pairings (those without a `kind` field) are treated as `'app'` — auto-promotion is explicitly rejected.

### `charter_set_schedule`

Set or clear the per-origin schedule on a specific grant.

**Request:**

```json
{
  "id": "<request id>",
  "method": "charter_set_schedule",
  "params": [
    "<dependantId hex>",
    "<scope>",
    "<origin>",
    "<schedule JSON | null>"
  ]
}
```

- `dependantId`: 64-char hex pubkey.
- `scope`: scope string from Signet's scope inference (e.g. `"sign-in"`).
- `origin`: full origin string (e.g. `"https://roblox.com"`).
- The 4th param is either a JSON-encoded `GrantSchedule` or the literal `null` (= clear the schedule).

**Behaviour:** the bunker validates the schedule shape (rejecting on validation error), looks up the matching grant (creating one with `decision: 'allow', decidedAt: now` if none exists), saves the schedule field, and audits the policy change.

**Response (success):**

```json
{
  "id": "<request id>",
  "result": "{\"ok\":true}",
  "error": ""
}
```

**Response (validation error):**

```json
{
  "id": "<request id>",
  "result": "",
  "error": "charter:invalid_schedule"
}
```

The error message includes a human-readable hint after the colon — implementations MAY include `:` followed by a short description (e.g. `charter:invalid_schedule:end must be after start`).

### `charter_set_default_schedule`

Set or clear the dep-level default schedule.

**Request:**

```json
{
  "id": "<request id>",
  "method": "charter_set_default_schedule",
  "params": [
    "<dependantId hex>",
    "<schedule JSON | null>"
  ]
}
```

**Response:** same shape as `charter_set_schedule`.

### Future methods (reserved)

`charter_set_budget`, `charter_set_spend`, `charter_set_content`, `charter_set_comms` follow the same pattern: `(dependantId, [...scope keys], clause | null)`. Reserved in the namespace; not callable in v1.

`charter_get_schedule` / `charter_get_default_schedule` — read-side methods. Reserved; v1 dashboards read schedules via the existing dependant + grants sync rather than per-call lookup.

## Sign-time enforcement (consumer-facing)

Consumer apps (games) don't call `charter_*` methods directly. They call the standard NIP-46 `sign_event` and parse the response. When a clause blocks the sign, the bunker returns a structured error.

### Effective-schedule resolution

When a sign request `(dependantId, scope, origin)` arrives, the bunker computes the effective schedule:

1. `perOrigin = grant?.schedule` for the (dependant, scope, origin) tuple
2. `depDefault = dependant.defaultSchedule`
3. `effective = intersect(perOrigin, depDefault)` — pairwise window intersection. If either side has `paused: true`, the result is paused. If either is undefined, the other passes through.
4. If `effective` is undefined OR `isWithinSchedule(effective, now)` returns `{ allowed: true }` → proceed with the existing sign flow.
5. Otherwise → respond with the structured deny below.

This applies to sign requests whether or not a grant exists. Step 2 (dep-default) is what closes the unknown-origin bypass — a fresh sign-in to a never-seen-before origin during locked hours is silently refused (not enqueued for guardian approval at midnight).

Explicit deny grants (`grant.decision === 'deny'`) take precedence over schedule — a denied origin stays denied regardless of clock.

### Deny response shape

```json
{
  "id": "<request id>",
  "result": "",
  "error": "charter:clause_blocked:schedule:outside_allowed_hours"
}
```

The error format is **colon-delimited** with three meaningful segments after the namespace:

```
charter:clause_blocked:<clause>:<reason>
```

| Segment | v1 values |
|---------|-----------|
| Namespace | always `charter` |
| Action | `clause_blocked` for clause-driven denies; `not_authorised` / `invalid_schedule` / `not_found` etc. for method-call errors. |
| Clause | `schedule` (v1). Reserved: `budget`, `spend`, `content`, `comms`. |
| Reason | `outside_allowed_hours` (v1 schedule). `paused` for the `paused: true` case. Reserved per-clause as new clauses ship. |

### Structured data on deny (optional, behind a per-call hint)

The error string conveys the structured reason. Some consumers want richer metadata — e.g. "back at 4pm". v1 does NOT include this in the standard response (`error` is a string-only field per NIP-46). Implementations MAY emit additional metadata via:

- An audit event the consumer's clientPubkey is gift-wrapped to receive (see Audit), OR
- A future `charter_query_clause_status` companion call (reserved; not in v1).

**Why no inline `data` field in v1:** the NIP-46 response shape (`{id, result, error}`) is fixed, and adding non-standard fields fragments tooling. Pushing structured metadata into a side-channel (audit subscription) keeps the wire response standard.

### Privacy in deny responses

The deny error string carries the clause + reason but **not** the next-allowed time, schedule shape, or any timestamp. A curious or malicious game cannot infer the child's sleep window from the response itself. Consumers wanting "back at 4pm" UX must subscribe to the audit channel (see Audit) where the guardian has explicitly granted visibility.

## Audit events

The bunker emits a kind-31000 audit event for every clause-driven decision (allow under clause, blocked by clause). Audit events are **gift-wrapped (NIP-59)** to:

1. The guardian's pubkey (canonical, always emitted).
2. The dependant's paired-child client pubkey, if one exists AND the dep's `auditVisibility` resolves to visible per signet-app-internal#90 v2.
3. **(reserved for v1.x)** the consumer app's pubkey, if the dep's grant for this origin opts in. Not implemented in v1.

### Audit event shape

The unsigned rumor (kind 31000) before gift-wrap:

```json
{
  "kind": 31000,
  "pubkey": "<guardian pubkey>",
  "created_at": <unix seconds>,
  "tags": [
    ["t", "audit"],
    ["d", "<dependantId>:<ms timestamp>"],
    ["k", "<event kind being signed>"],
    ["outcome", "clause-blocked"],
    ["origin", "<origin>"],
    ["clause", "schedule"],
    ["reason", "outside-allowed-hours"],
    ["schedule-source", "per-origin" | "dep-default" | "intersection"],
    ["schedule-issued", "<unix seconds>"],
    ["next-allowed", "<unix seconds>"]
  ],
  "content": ""
}
```

The `clause` and `reason` tags compose: `clause` identifies which Charter clause refused (one of the reserved clause names), `reason` identifies the specific cause within that clause. The same naming structure appears in the wire error string (`charter:clause_blocked:<clause>:<reason>`).

**Privacy contract:** `content` is always empty. Tags carry only routing + classification + guardian-only metadata. The dependant's content (the template they were trying to sign) never appears in the audit log.

### Outcomes

| `outcome` tag value | Meaning |
|---------------------|---------|
| `approved` | Guardian prompt approved. |
| `denied` | Guardian prompt denied. |
| `auto-approved` | Allowed under existing grant or auto-stage policy. |
| `auto-denied` | Refused under existing deny grant. |
| `clause-blocked` | Refused by a Charter clause (v1: schedule). |
| `ceremony-complete` | Lifecycle: independence ceremony completed. |

The `clause` tag and downstream metadata tags only appear on `clause-blocked` outcomes.

## Cross-device sync

Charter clauses sync via the existing #143 grants-sync rail (per-origin) and #121 dependant-sync rail (dep-default). The sync wires now carry the `schedule` and `defaultSchedule` fields respectively.

**LWW posture:** schedule freshness is independent of the underlying record's freshness. A grant's `decidedAt` and a schedule's `issuedAt` are independently merged — the newer schedule wins regardless of which side has the newer grant decision. Remote breaks ties on equal `issuedAt`.

**Validation on receive:** sync receivers MUST validate inbound schedules (same invariants as `validateSchedule`). Malformed schedules are dropped without rejecting the rest of the record.

## Cross-stack constant parity (§5.6)

These magic numbers ride the wire between the guardian PWA (TypeScript) and the
wardens (Rust). If the two stacks disagree on any one, valid messages are
**silently rejected** — a wrap outside the jitter window, a grant outside the
skew window, a kind that doesn't match. They are pinned by a parity test on
each stack (`core/crates/charter-spine/tests/contract_constants.rs` and
`apps/charter-app/src/wire/contractConstants.test.ts`); change a value here and
you change it in both, or a red test tells you which stack drifted.

| Constant | Value | Rust | PWA |
|---|---|---|---|
| Wrap freshness jitter | `172800` (2 days) | `nip59::MAX_JITTER_SECS` | `MAX_WRAP_JITTER_SECS` |
| Grant freshness skew | `300` s | `verify::FRESHNESS_SKEW_SECS` | (device-side only) |
| Max `time.extend` minutes | `1440` | `MAX_EXTEND_MINUTES` | `MAX_EXTEND_MINUTES` |
| Max child `reason`/`label` len | `280` | `MAX_REASON_LEN` | `MAX_REASON_LEN` |
| Warn thresholds | `600` / `60` s | `enforcer` (10-/1-min) | (device-side only) |
| STATUS heartbeat | `60` s | warden `STATUS_HEARTBEAT_SECS` | (device-side only) |
| Kinds | 1059/13/31111/31112/31113/31114/31000/31116/31117/30100 | `primitives::kinds` | `wire/*` |
| Marker tag | `["t","charter-device"]` | `kinds::marker_tag()` | `MARKER` |

## Versioning

- Clauses carry `v: 1` today. Future bumps signal incompatible shape changes.
- The contract version (this document) is `v0.1` — pre-stable.
- The NIP-46 method namespace is `charter_*`; we do not draft an upstream NIP until the namespace stabilises (matches the `heartwood_*` discipline).

## Reserved error codes

| Error | Meaning |
|-------|---------|
| `charter:not_authorised` | Caller's pairing is not `kind: 'charter'`. |
| `charter:invalid_schedule` | Schedule failed `validateSchedule`. |
| `charter:not_found` | Dependant or grant referenced does not exist. |
| `charter:clause_blocked:schedule:outside_allowed_hours` | Sign refused outside schedule windows. |
| `charter:clause_blocked:schedule:paused` | Sign refused while schedule is paused. |
| `charter:method_not_supported` | Unknown `charter_*` method. |

Future codes follow `charter:<action>` or `charter:clause_blocked:<clause>:<reason>`.

## Charter-aware consumer apps (the in-app layer) — v0 draft

> **Status.** Drafted 2026-07-23 against the face/vault design (D6: three
> enforcement layers, one adjudication point). ExampleGame is the marquee
> template implementation — this section is written so ANY app can adopt it,
> never as a private first-party hookup. Shapes marked **frozen** reuse wire
> machinery already proven on-metal; items tagged **[decide]** need sign-off
> (Signet's and/or the template app's) before either side ships.

A **Charter-aware app** participates in the wardship at the layer only it can
enforce — its own insides (session length, feature gates, mods, spend). The
division of labour is fixed by design D6:

- The app **enforces silently** from standing clauses it holds.
- When it hits an ask its clauses cannot decide, it **emits a REQUEST** to the
  guardian surface — the same one inbox as device time asks — and honours the
  signed GRANT (or its absence) that comes back.
- The app **never grows a second voice** to the guardian: no in-app parent
  PIN, no email-a-code, no separate notification channel. One inbox, one
  brand asking questions.

### Ask/grant wire (frozen shapes, reused machinery)

Consumer asks ride the existing device-brokering kinds — REQUEST 31111 /
GRANT 31112, gift-wrapped (1059), guardian-pubkey-addressed — with the same
`RequestPayload` envelope (`v/op/reqId/nonce/subject/machine/ts/params`) and
the same params-echo rule on grants. New `op` values (namespace `app.*`):

```ts
// The generic in-app ask. `askKind` scopes the request inside the app;
// the guardian surface renders `summary` verbatim (≤120 chars, ward-visible
// language, no markup).
interface AppAskRequestParams {
  appId: string;          // the app's stable id (Android pkg / origin) — must
                          // match the identity the transport authenticated
  askKind: string;        // app-defined, e.g. "mod.install", "feature.enable",
                          // "session.extend"
  summary: string;        // "Robin asks to install the Dragons mod"
  detail?: object;        // app-defined payload the grant echoes verbatim
}
interface AppAskGrantParams {
  appId: string;          // echo
  askKind: string;        // echo
  detail?: object;        // echo — the app enacts EXACTLY what was approved
  standing?: boolean;     // guardian chose "…and make this a rule" (D7); the
                          // app MAY cache the outcome for identical asks, and
                          // the surface ALSO authors the matching clause —
                          // the clause is authoritative, the flag is a hint
}
```

- `op: "app.ask"` for the lot — the surface routes/renders on `askKind`.
  **[decide]** one generic op vs. an enumerated op per ask family
  (`app.mod.install`, …): generic keeps the device/proto layer closed to
  app-specific churn (preferred); enumerated gives typed params. Proposal:
  generic, with `askKind` conventions documented per template app.
- Deny is the existing signed 0-value grant pattern (an answered "not now"),
  never silence; unanswered asks expire client-side like `time.extend`.

### In-app clauses (standing policy the app enforces)

**[decide]** (surface + template app): the in-app rule set rides either
(a) the existing per-app `apprules` clause grown an `scopes` map, or (b) a new
`appscopes` clause kind (store_key from the next free number at freeze time).
Proposal: **(b)** — `apprules` is device-enforced (suspension) and its
consumer is the warden; in-app scopes are app-enforced and their consumer is
the app. Distinct enforcement points, distinct clauses (mirrors the
`schedule`-vs-`budget` split).

```ts
// Proposal sketch — freezes only at [decide] sign-off.
interface GrantAppScopes {
  v: 1;
  appId: string;
  // App-defined scope rules; the app is the only interpreter. Examples:
  // { "mods": {"posture": "allowlist", "allowed": ["dragons"]},
  //   "chat":  {"enabled": false},
  //   "session": {"dailyMinutes": 45} }
  scopes: Record<string, object>;
  issuedAt: number;
}
```

### Transport & identity **[decide — the load-bearing choice]**

How does the app's ask reach the relay, and what key signs/wraps it?

- **(T1) Via the ward's Signet session (proposed).** The app is signed in
  through the ward's Signet (NIP-46); it calls a `charter_app_ask` method on
  that session; the **bunker** wraps/publishes the REQUEST to the guardian
  pubkey and returns the GRANT outcome over the same session. Trust story:
  the bunker authenticates the app session (origin == `appId`), so a foreign
  app cannot impersonate ExampleGame; the app needs no Nostr plumbing of its
  own; clause delivery (`appscopes`) rides the same session state. Cost: a
  new NIP-46 method needs Signet sign-off (same coordination lane as
  `charter_set_schedule`).
- **(T2) App-held machine key, direct relay publish.** The app mints a
  keypair and speaks the device-broker transport itself (what `charterd`
  does). No Signet dependency, works for non-Nostr apps — but attribution is
  self-claimed (`appId` unverifiable), pairing UX must be built per app, and
  it bypasses the bunker's enforcement seat. Kept as the documented fallback
  for apps outside the Signet ecosystem; not the template path.

Proposal: **T1 primary** — it is exactly the "Signet enforces, Charter
adjudicates" split, and the sign-in session is the only place `appId`
attribution is real.

### Audit

Every app ask + outcome emits the standard audit event (kind 31000) with
`clause: "appscopes"` and the existing outcome vocabulary (`approved` /
`denied` / `auto-approved` / `expired`). The ward-visibility rule follows
the audit section above unchanged; `summary` MAY appear in audit content,
`detail` MUST NOT (it is app-private, like `reason` on `time.extend`).

### What the template app must demonstrate (ExampleGame acceptance list)

1. Reads an `appscopes` clause and enforces at least two scope kinds silently.
2. Emits an `app.ask` on an undecidable gate; renders "asked / granted /
   not now" states honestly in-game (ward-warm copy, no dark patterns).
3. Enacts a grant exactly (`detail` echo), including a deny.
4. Handles offline: cached clauses keep enforcing; asks queue or fail
   honestly; no fail-open on relay loss.
5. Never prompts the guardian itself (the one-voice rule).

## Implementation status

| Component | Status |
|-----------|--------|
| Schedule clause data model | Implemented in Signet (`forgesworn/signet-app` `7e88c98`) |
| Sign-time enforcement | Implemented (`16401fa`) |
| Cross-device sync | Implemented (`55ddad7`) |
| Deny error string format | Aligned to this contract (commit follows). Both `charter:clause_blocked:schedule:outside_allowed_hours` and `:paused` reasons emitted. |
| Audit event tags | Aligned to this contract (commit follows). `clause` + `reason` tag separation applied. |
| `charter_set_schedule` NIP-46 method | Not yet implemented. Phase 4 of the Signet schedule rollout. Blocked on encryption-key threading inside `useBunkerServer`. |
| `charter_set_default_schedule` NIP-46 method | Not yet implemented. Same phase. |
| Authorisation gate (`kind: 'charter'`) | Type field added (`19d538a`); enforcement gate ships with phase 4 method dispatch. |
| Per-child CLAUSE `subject` (kind 31113) | **Contract-frozen + device-consumed this session.** `charterd` ingests per-`(subject, kind)` clauses and resolves whole-child precedence (guardian over device-only); absent `subject` stays the single-child default, golden vectors unchanged. |
| Device STATUS feed (kind 31114) | **Contract-frozen + device-emits.** `charterd` publishes per guardian-bound child, gift-wrapped to the guardian (throttled: displayable state-change + 60s heartbeat), numbers/enums only. The PWA-consume half + live pairing exercise remain; `usedWeekSecs`/`weekKey` are aggregator-deferred. |
| Multi-device USAGE_SYNC (kind 31115) | **Contract-frozen but device-deferred.** Consolidation protocol fully specified; reserved — no device code this session. |
| Guardian surface (MyCharter + carrier APK) | **Live (2026-07-22).** Request intake, Approvals inbox, local-key signing/GRANTs, clause authoring incl. `tethering`/`update`/`lifeline`; the carrier APK holds the relay socket for reliable wake. Signet-bunker vault mode: designed, not yet built. |
| `lifeline` clause (store_key 9) | **Shipped device+surface (2026-07-22).** Guardian numbers callable from the ward's lock screen (spec D9); fail-closed body validation both ends. |
| `standdown` clause (store_key 13) | **Android PROVEN on hardware 2026-07-27** (warning + lockout on a real ward's phone); **Linux code-complete** in charterd 0.4.0, not yet run on a laptop.|

## References

- Charter positioning — `forgesworn/signet-plans/docs/plans/2026-05-08-charter-positioning.md`
- Signet schedule design (clause-internal implementation) — `forgesworn/signet-app/docs/superpowers/specs/2026-05-08-per-origin-schedule-design.md`
- NIP-46 — Nostr Connect (the underlying transport)
- NIP-59 — Gift wrap (used for audit events)
- signet-app-internal#90 v2 — audit-log dual-address gift-wrap
- signet-app-internal#143 — per-origin grants + sync
- signet-app-internal#165 — trusted-app pairings (`kind` field added in `19d538a`)
- signet-app-internal#172 — Charter clause #1 implementation tracker (this work)
