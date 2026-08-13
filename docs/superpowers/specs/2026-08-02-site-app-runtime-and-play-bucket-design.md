# Chromium as a site-app runtime, and the Play bucket

**Date:** 2026-08-02
**Status:** design agreed, not built
**Origin:** decented, setting up his son's Linux laptop for real.

---

## 1. The scenario

One ward, one Mint laptop, four rules the family actually wants:

| Thing | Intended treatment |
|---|---|
| Khan Academy, Wikipedia, BBC Bitesize, Duolingo | **Free.** Never drains playtime, no cap. |
| Minecraft Java (official Mojang launcher) | Counts against playtime, and against a shared **Play** cap. |
| Firefox | The ward's browser. Counts against playtime, shares the Play cap. |
| Chromium | **Not a browser.** Only ever opens sanctioned educational site-apps. |
| Google Chrome | Removed from the machine. |

**Play allowance: 90 minutes a day, shared between Minecraft and Firefox.**
Educational sites are **uncapped** — genuinely unlimited, no `capMinutes`.

The consequence decented chose knowingly: when the 90 minutes are spent,
Minecraft and Firefox stop but the educational site-apps keep working, free.
The laptop becomes a homework machine rather than a brick. This is also why
add-your-own-site (§6) matters more than it appears — a school site he needs
becomes a site-app rather than a reason to hand the browser back.

## 2. The reframe that drives the design

The first framing was "block Chrome, carve out an exception for the Khan
window". decented's is better and inverts it: **Chromium is not a browser on this
machine, it is a runtime for sandboxed site-apps.** Firefox is the browser, and
Firefox is the one Charter already governs (`/etc/firefox/policies/policies.json`,
written root-owned and immutable).

A bare Chromium window is therefore not an exception to be policed — it is an
escape, and it is terminated. A blocklist with a hole in it becomes a
default-deny with a precisely-defined carve-out.

## 3. What exists today (verified, with evidence)

**Works already, no change needed:**

- Minecraft draining the day. Attribution walks the process tree
  (`focus.rs:474-499`), so the Java game spawned by `/usr/bin/minecraft-launcher`
  counts, not just the launcher. Deliberate — killing only the launcher while
  the game plays on is a limit that visibly does nothing.
- Firefox draining the day. Everything is screen time unless marked otherwise.
- The buckets machinery: clause tag `buckets`, store key 10, `AppBucket
  {id,label,apps,dailyMinutes}`, metered off the same single X probe as
  learning, spent bucket closes the bucket and never the device
  (`buckets.rs`, `usage.rs:176-183`, `runtime.rs:1510-1518`).
- Per-device app lists. `domain/deviceApps.ts` (commit `31d3734`) keys the
  reported inventory by device; a never-reported device gets an empty list, not
  its sibling's. **Unverified on hardware** — on the test list below.

**Broken or missing:**

1. **The learning enactor cannot find a browser.** `chromium_path()`
   (`enactors/learning_apps.rs:155-160`) checks only `/usr/bin/chromium` and
   `/usr/bin/chromium-browser`. The laptop has Google Chrome. Result:
   `SkippedNoChromium`, retried forever — Khan-as-free-time does **nothing**
   today.

2. **Blocking Chrome is inert.** Confirmed on a live machine: Google Chrome's
   `/usr/bin/google-chrome-stable` is a launcher script that re-execs the real
   binary at `/opt/google/chrome/chrome` under a spoofed argv0 (the reproduction
   detail is omitted from this narrative). The effect:

   Inventory reports `pkg=/usr/bin/google-chrome-stable`
   (`app_inventory.rs:48-58`), but `/proc/<pid>/exe` reads
   `/opt/google/chrome/chrome`. `pkg_matches_process` (`app_rules.rs:76-89`)
   compares full path (no match) then basename — `chrome` vs
   `google-chrome-stable` (no match). A guardian can tick "blocked" and get
   silence. Chromium survives this only by luck; its wrapper's basename happens
   to match.

3. **No sanction concept in the kill sweep.** `terminate_blocked_processes`
   (`runtime.rs:959-1016`) has no learning exemption, so "Chromium blocked, this
   window allowed" is not expressible.

4. **No `--user-data-dir` on the launcher.** Rendered Exec is
   `<chromium> --app=<url> --class=charter-<id> "--host-resolver-rules=<pin>"`
   (`learning_apps.rs:60-76`). If a same-binary session is already open, the
   `--app` launch hands off to it and the resolver pin is **silently ignored** —
   the thing that makes the window safe does not apply. Latent today, fatal once
   there are several site-apps (each would inherit whichever launched first).

5. **The pin value is never checked.** `focus::classify` (`focus.rs:64-70`)
   requires `--class=charter-<id>` and that *some* `--host-resolver-rules=`
   token exists — never that it is the right one. Today that is free-time theft.
   Under §4 it would become a full browser escape, so it must be fixed as part
   of this work, not after it.

6. **Sites are a hardcoded menu of one.** `LearningEditor` renders
   `LEARNING_CATALOGUE.map(...)` (`Limits.tsx:1635`); the catalogue
   (`data/learning_catalogue.ts`) holds exactly Khan Academy. Native apps come
   from the device inventory, but there is no way for a guardian to add a site.

7. **Chromium is entirely unmanaged by the web policy.** There is no
   `/etc/chromium/policies/managed` writer anywhere in the tree. Charter
   *requires* Chromium for site-apps and then governs nothing about it. §4 is
   the answer to this, not a Chrome policy renderer.

## 4. Component A — Chromium is a runtime, and it is enforced

**Rule.** While a managed child has **at least one site-app in force**, any
Chromium process owned by that child that is not a sanctioned site-app process
is SIGTERM'd by the existing sweep.

**Implied, not a separate toggle.** If the guardian had to also tick "block
Chromium", forgetting it leaves a fully unmanaged browser on the machine (gap
7). Turning on a site-app is what installs the runtime, so it is also what locks
it down.

**Native learning apps do not trigger it.** The rule keys off *site*-apps
specifically — a child whose only learning app is a native binary involves no
Chromium and gets no lockdown. Turn every site-app off and Chromium is an
ordinary browser again; the launchers are removed by the same reconcile that
created them (`learning_apps.rs:81-115`). That is the guardian's decision to
make and it is reversible from MyCharter, which is the point.

**It honours the enforcement mode.** Like every other effect, the lockdown runs
only under `mode.applies_effects()`; observe mode logs "would terminate" and
touches nothing (`runtime.rs:1580-1589`).

**Scope is already safe.** The sweep re-reads the owner uid from `/proc` and
only touches uids in `blocked_by_uid` (`runtime.rs:977-987`), which the caller
has already filtered to managed, lockable children. Root and every other
account — including the parent's — are untouchable. Charter never breaks
Chromium for anyone but a governed child who has site-apps switched on.

**Transparency.** MyCharter's Learning card states it in the guardian's words:
*"Firefox is their browser. While these are on, Chromium only opens these
sites."* Transparency is the Charter invariant; a silent behaviour change to a
browser the family installed is not acceptable.

### Sanctioned: the predicate

A Chromium **browser process** is sanctioned iff its `/proc/<pid>/cmdline` is
**exactly equal**, token for token, to the command line Charter renders for
`(uid, app-id)` for an app currently in force for that child.

A Chromium **child process** (renderer, GPU, zygote — their argv is Chromium's
own business and bears no resemblance) is sanctioned iff an ancestor is a
sanctioned browser process, reusing `ancestry::matches_with_ancestors`'s
bounded, cycle-safe walk.

**Exact equality, not shape-matching.** The tempting cheap version — "has a
`--class=charter-*` and a `--host-resolver-rules=`" — is what gap 5 already is,
and it fails to several forgeries:

| Forgery | Against shape-match | Against exact-match |
|---|---|---|
| `--class=charter-khan-academy --host-resolver-rules=MAP nothing` | escapes: unpinned browser, credited as free learning time | rejected |
| two `--host-resolver-rules` tokens (Chromium honours the last) | escapes | rejected |
| `--host-rules=...` added alongside a valid pin | escapes via a second host-mapping flag | rejected |
| any additional flag at all | must be individually anticipated and blocked | rejected by construction |

Exact equality is fail-closed by construction: an allowlist of one string, so
nothing has to be anticipated.

**What exact-match deliberately permits:** the ward can read the `.desktop` file
(world-readable in `/usr/share/applications`) and type the exact command
himself. He gets a Khan window. That is the point — **the resolver pin is the
security boundary; the class marker is only the meter.** Reproducing a sanctioned
command line yields a sanctioned sandbox and nothing more. Mixing Khan's class
with Wikipedia's pin yields a Wikipedia sandbox. There is no combination of
Charter-rendered pins that reaches an unpinned network.

**One matcher, two jobs.** The same predicate answers both "is this learning
time?" (`focus::classify`) and "should this be killed?" (the sweep). This is the
invariant the buckets work already established — different matchers would meter
one app and stop another. It is now also a security property: any drift between
them is a hole.

## 5. Component B — one profile per site-app

Each site-app runs with `--user-data-dir` pointing at a per-child, per-app
profile under the child's own home:

```
~/.local/share/charter/learn/<app-id>
```

Two reasons, both load-bearing:

- **Correctness.** Without it the second site-app launched hands off to the
  first and inherits its pin (gap 4).
- **Privacy.** The enactor writes **one** launcher per app across the union of
  every managed child (`runtime.rs:1682-1697`). A fixed system-wide profile path
  would mean two children on one laptop share a Khan login. Under the child's
  home, they cannot.

**Why a shim.** `.desktop` `Exec=` is not run through a shell, so `$HOME` does
not expand. The launcher therefore points at a small root-owned helper:

```
Exec=/usr/lib/charter/charter-learn <app-id>
```

`charter-learn` reads a root-owned manifest `/var/lib/charter/learn/<id>.json`
(url, pin, label), resolves the invoking user's home, and `exec`s Chromium with
the profile and pin. It is also the one place `charterd` can recompute
"what should this process's argv be for this (uid, id)" for §4's exact-match —
one renderer, used by both the launcher and the checker, so they cannot drift.

The profile directory is ward-writable (it holds cookies and his Khan login).
That is fine: it carries no policy, and the worst he can do is corrupt his own
session.

### Finding the runtime, and saying when it is missing

`SkippedNoChromium` is retried on every reconcile and reported nowhere. That is
precisely the bug that made Khan silently inert on this laptop (gap 1), and a
broader catalogue makes it likelier, not less. The device reports the condition
on STATUS and MyCharter shows it on the Learning card — *"Chromium isn't
installed on [device]. These sites won't open until it is."* A guardian ticking
a control and getting silence is the same class of defect as §7's inert block,
and it gets the same treatment: say so.

Discovery also gains `/usr/bin/google-chrome-stable` as a **last** fallback,
after both Chromium paths. Not for this laptop — Chrome is being removed — but a
family with only Chrome should get a working sandbox rather than nothing, and
every guarantee in §4 and §5 holds identically for it (same `--app`, `--class`,
`--host-resolver-rules`, `--user-data-dir`).

That fallback creates one interaction worth stating: if a family's runtime is
Chrome **and** the guardian also blocks Chrome through `appRules`, the sanction
predicate must exempt sanctioned processes from the **explicit block too**, not
only from §4's implied lockdown. Otherwise blocking the browser kills the
site-apps — the original problem in a new hat. One predicate, checked at the one
place that decides to kill (`terminate_blocked_processes`), covers both.

## 6. Component C — the catalogue, and adding your own

**Curated set.** Closures measured against the live site with a real browser,
the way Khan's six domains were captured (homepage plus a playing video). To
measure for this round: **Wikipedia, BBC Bitesize, Duolingo**. Khan is done.
Every entry's comment records what was exercised — measured, never guessed.

**Add-your-own.** `LearningEditor` gains a label + URL form producing an
ordinary `LearningApp` — `{id, label, kind:'site', url, domains}`. **No wire
change:** `LearningApp` already carries `url` and `domains`, and validation
already demands both for site apps (`learning.rs:81-114`). The catalogue is a
client-side convenience, not a wire concept.

### Deriving the pin without a public suffix list

The obvious rule — pin the *registrable domain* — needs a public suffix list to
be safe, and getting it wrong is a wide hole: `bbc.co.uk` mis-derived as `co.uk`
pins every UK commercial site. The PWA has no PSL dependency and its dependency
list is deliberately lean.

**Rule adopted instead: pin the typed hostname and its subdomains, plus the
`www.` counterpart.**

| Typed | Pinned |
|---|---|
| `https://www.khanacademy.org/` | `khanacademy.org`, `*.khanacademy.org` |
| `https://en.wikipedia.org/` | `en.wikipedia.org`, `*.en.wikipedia.org` |
| `https://bbc.co.uk/bitesize` | `bbc.co.uk`, `*.bbc.co.uk` |

This needs no PSL, and **cannot ever be over-broad beyond the single site the
guardian named**. It is under-broad more often than a registrable-domain rule
would be — a cross-domain CDN is missed, so Wikipedia typed by hand renders text
with no images, because those live on `upload.wikimedia.org`.

That is the correct failure direction and it is exactly why the curated set
exists. The copy says so honestly:

> Sites you add are locked to their own address. Some sites load pictures or
> videos from somewhere else and may look broken — tell us and we'll measure it
> properly.

## 7. Component D — make blocking Chrome work

`pkg_matches_process` gains **argv[0]** (first token of `/proc/<pid>/cmdline`) as
a third match source, alongside exe-path equality and basename equality. That is
what catches the `exec -a "$0"` wrapper case (gap 2): Chrome's argv[0] is
`/usr/bin/google-chrome-stable`, exactly what the inventory reports.

**This cannot create an evasion.** It is strictly *additional* matching — a
process matches if exe **or** argv[0] matches. A ward setting a misleading
argv[0] can only cause their own process to be killed, never spare one.

Kept even though Chrome is being removed from this laptop: a guardian ticking
"blocked" and getting silence is a silent-failure defect that will bite another
family.

## 8. Component E — the Play bucket

```
AppBucket {
  id: "play", label: "Play", dailyMinutes: 90,
  apps: ["/usr/bin/minecraft-launcher", "/usr/bin/firefox"]
}
```

Pure configuration — the machinery exists and needs no change. Both apps also
drain the general day, as designed. Ancestry covers the Java game.

Note `AppBucket.dailyMinutes` is a single number with no weekly shape, so
"different at weekends" is not expressible. Not requested for this round;
recorded as a known limit.

## 9. Wire contract

**No wire change.** Every component is device-side behaviour or client-side
convenience over clause bodies that already exist. `spec/contract.md`'s Learning
section gains prose recording that a site-app in force implies the Chromium
runtime is locked down for that child, and that sanction is exact-argv — a
documentation change, not a schema one.

## 10. Testing

**Unit / gate-green (headless, the four `linux/` gates):**

- `pkg_matches_process`: the exact Chrome shape — `pkg=/usr/bin/google-chrome-stable`,
  `exe=/opt/google/chrome/chrome`, `argv[0]=/usr/bin/google-chrome-stable`
  → matches. Plus a regression asserting exe-only and basename-only matching
  still work.
- Sanction predicate, one test per forgery row in §4's table, all rejected.
- Sanction predicate accepts the exact rendered line, and accepts a descendant
  by ancestry while rejecting an unrelated sibling Chromium.
- One renderer: assert the launcher's Exec and the checker's expected argv are
  produced by the same function for the same `(uid, id)`.
- Sanction survives an **explicit** `appRules` block of the runtime binary, not
  only the implied lockdown (§5's Chrome-as-runtime interaction).
- Profile isolation: two children, one app — different `--user-data-dir`.
- Domain derivation: the §6 table, plus explicit assertions that `bbc.co.uk`
  never yields `co.uk` and a two-label host never yields a bare TLD.
- Bucket accrual with Firefox and Minecraft both members: 90 minutes total
  across both spends the bucket; the day's budget also moves.

**Hardware round (decented, one visit — nothing below can be proven headless):**

1. `sudo apt remove google-chrome-stable && sudo apt install chromium`
   (confirm Mint ships a real `chromium` deb, not an Ubuntu snap stub).
2. Upgrade the `.deb`; confirm the four site-app menu entries appear.
3. Khan opens, videos play, "Watch on YouTube" dies at the resolver.
4. Wikipedia opens **with images** (proves the measured closure beat the
   hand-typed one).
5. Launch two site-apps in either order — each keeps its own pin (the gap-4
   regression).
6. `chromium` from a terminal or the menu → window dies within a tick.
7. Copy the exact Exec line from the `.desktop` and run it → a working, pinned
   Khan window. (Permitted by design; verifying it stays *pinned* is the point.)
8. Maths for 20 minutes → playtime unmoved, no lock.
9. Minecraft + Firefox to 90 minutes → both stop; site-apps still open and
   still free; the rest of the day intact.
10. Per-device app list: split a rule and confirm the laptop offers only laptop
    apps, no Android packages (the `31d3734` verification still outstanding).

## 11. Out of scope, deliberately

- **The closure-learning pass.** Site-app runs in a recording mode, logs the
  hosts it wanted, MyCharter asks *"Wikipedia also wants `upload.wikimedia.org`
  — allow?"*. The version that scales to any site correctly. A subsystem, not a
  feature — its own spec, once add-your-own has been used enough to show how
  often it falls short.
- **A Chromium managed-policy renderer.** §4 makes Chromium a runtime rather
  than a browser, which removes the need. Revisit only if Chromium ever becomes
  a general browser on a ward device.
- **Weekly-shaped bucket allowances** (§8).
- **Lock-except-learning.** A screen-time lock still locks site-apps too
  (`spec/contract.md:266-268`). Unchanged by this work; note that the Play
  bucket expiring is *not* a lock, so §1's "homework machine" outcome is
  unaffected.
- **Android.** Buckets and this runtime model are Linux-only for now; the phone
  ignores identities it does not have.
