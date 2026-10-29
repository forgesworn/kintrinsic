# Guardian app: UX overhaul (design)

Status: proposal, 2026-10-29. Presentation only: the store (`src/store/`),
wire, signer and keys code are untouched. Every `useCharter()` function keeps
a home (see the checklist at the end).

The redesign was built on the guardian-keys branch and ships ahead of it. The
parts that belong to guardian keys (other guardians, join a family, the
Guardians sub-screens and their `#/settings/guardians` routes) arrive with
that feature; until then Settings has no Guardians section.

## Why

The app is organised by *kind of thing* (Approvals, Limits, Activity,
Family) rather than by *child*. One ward appears on all five tabs, each with
a different slice of them, and five different ward pickers that don't share
a selection. Limits then nests three more levels (Time / Apps & web /
Safety, then an accordion, then a per-device split) and has two save models.
Device operations sit in dense rows on Family, and some that look per-device
are ward-wide (Allow installs, Update Kintrinsic). A guardian who isn't the
one who built it can't find their way round.

Concrete layout bug: Give time expands inline inside the ward card, so its
`+` lands under "Finish now" and "Give 30 min" wraps to three lines. That
comes from the structure, not a one-off: any inline expansion inside a
two-column card collides.

## The jobs, by how often

| Job | How often | Taps now | Target |
|---|---|---|---|
| Answer an ask (more time, open app, install) | daily | 2 (Approvals tab, approve) | **1** from the child page or the notification |
| Give extra time | daily | 3 (Give time, stepper, Give) | **2** (More time, pick +15/+30/+1h) |
| Stop for today / allow back on | daily | 2 | **2** (Stop now, confirm) |
| See status: locked? time left? devices OK? | daily | 0 | **0** (child page) |
| See their week / what happened | weekly | 1 | **1** (child page) |
| Change a rule (schedule, limit, websites, apps…) | weekly | 3–6 | **3** (child, rule, Save) |
| Block / hold one app | occasionally | 5–6 | **3** (child, Apps, ⏱) |
| Device op (disconnect, unlock code, debugging, relays, update) | rare | 2–4 | **3** (child, device, action) |
| Set up a device / add a child | rare | 2–6 | same flows, started from the child / Settings |
| Parent approval, key backup, other guardians, join a family | rare | 2–4 + long scroll | **2** (Settings, item) |

## The new shape: three surfaces

### 1. Opening screen: the first child, not a Home

Once the family has a child there is no Home page. The app opens on the first
child's page (the order is set in Settings, "Children and family devices"),
and the per-child count badges in the bottom bar say who has asks waiting, so
a separate "what needs me now" list was redundant. A notification for an ask
opens the first child with something waiting.

- Family-wide notices sit at the top of each child page, each only while it
  holds: getting started (until a computer is connected), key backup, an
  unclaimed phone, a removal against me, and what became of this child's
  recent answers (home-network delivery). They are not filed away in Settings.
- Recent activity (the family-wide list) is a row in Settings, under
  "Children and family devices".
- Home survives only as the getting-started page for a family with no
  children yet: the welcome card, the recovery nudges and the app download
  nudge. Then the bar has a Home button too, so it stays reachable.
- The bottom bar (below) is the way between the top-level pages, so those
  have no header back arrow. Pages one level down keep one, and Android back
  works because every page is a hash route.

### The bottom bar: the children, Settings

The bottom is where a thumb reaches, so the main way between pages is a
fixed bar there: ⚙ Settings pinned right, and one button per child (avatar in
their colour, name, a count badge when they have asks waiting) in the family's
order. With no children it is 🏠 Home and Settings. Two children and
Settings fit at 360 px; beyond that the middle strip scrolls sideways, with a
fade on the side that has more, and the current child is scrolled into view.
The current page is marked with `aria-current` and the brand colour. Some
families treat a shared family device as a "child", and others have many
children, so nothing assumes a small number.

It shows on the child page, child settings, Settings and its sub-pages, and
Activity (and on Home, with no children). It is hidden on the full-screen editors (rule, device and
add-device pages, which have their own pinned actions), on set-up and on the
recovery screen. A child's page and child settings, and Settings are
top-level: no back arrow.

### 2. Child page (`#/ward/<id>`): the daily things

Stacked sections with headings. No sub-tabs. Only what is looked at often:

1. **Now**: status, More time, Stop now / Allow back on.
2. **Waiting for you**: this child's asks.
3. **This week**: the chart and the last few events, then "All activity ›".

A ⚙ in the header (top right, "{name}'s settings") and a full-width row at the
bottom ("{name}'s settings ›", "Rules, devices, name") both open the child's
settings.

### 2b. Child settings (`#/ward/<id>/settings`): set once, changed rarely

The bar's ⚙ is the household's settings, so each child has their own page for
the rest. A top-level page (no back arrow), titled "{name}'s settings", with
the child current in the bar:

1. **Rules**, one list in three plain groups:
   - *Time*: Daily schedule · Time limit · Named times · When time's up
   - *Apps & web*: Always available · Websites · Apps (the "single out a
     game or app" cards move inside Apps)
   - *Safety*: Lifeline · Hotspot

   Each row shows its summary and opens a **full-screen editor page**
   (`#/ward/<id>/rule/<section>`) with **Save / Cancel pinned at the bottom**.
   This is the only save model. The per-device split stays inside the editor.
2. **Devices**: one row per device (name, health). Tap opens the device page
   (`#/ward/<id>/device/<deviceId>`, titled with the device's name, which is
   that device's own settings): Reconnect / Disconnect / Remove, offline
   Unlock code, USB debugging, Relays, Install update. Then "Set up another
   device", which starts the existing wizard for *this* child.
3. **Child-wide device settings**, labelled as covering all of this child's
   devices: Allow installs (window), Update Kintrinsic.
4. **Manage**: Rename / colour, Remove child.

Leaving a rule editor, a device page or add-device lands back here.

### 3. Settings (⚙): the household, rarely touched

Parent approval (signer, auto-sign) · Back up / restore guardian key ·
Other guardians and Join a family (with guardian keys: the Guardians
sub-screens, unchanged inside) · Children and family devices · Setup guide · Get / update the
Kintrinsic app · App version.

**Children and family devices** lists the children with ↑ and ↓ buttons
(48 px, disabled at the ends) beside the link to each child's page, then "Add
a child". The order here is the order of the buttons in the bottom bar. A
shared family device can be a "child" here. The order is guardian-side
display only: `moveChild` swaps a child with their neighbour in
`state.children` and persists it like any other change, with no clause, no
wire message and nothing sent to a device. Nothing else depends on the order
beyond "first child" defaults (`limits` with no id, Onboarding) and display.

## One sheet for every lever

Give time, Stop now and the amount control on time asks all use one bottom
sheet, built on the existing `.sheet` pattern (SignSheet, AppHoldSheet):

```
┌ Give Sam more time ──────────────┐
│  [+15 min] [+30 min] [+1 hour]   │  ← each gives straight away
│  Or:   (−)   45 min   (+)        │
│  Add it to: [Whole day ▾]        │
│  [        Give 45 min         ]  │
│  Extra time for today only.      │
└──────────────────────────────────┘
```

It is full-width with big targets and nothing beside it, so nothing can
overlap. An ask opens it pre-filled with the amount asked for, and its button
reads "Approve 15 min".

## Routes (nothing dead)

| Old | New |
|---|---|
| `#/home`, `#/`, any unknown route, a child who no longer exists | the first child's page (Home only with no children) |
| `#/approvals` (carrier notification) | the first child with an ask waiting (else the first child), scrolled to Waiting for you |
| `#/approvals/<childId>` | child page, Waiting for you |
| `#/ward/<childId>/settings` | child settings (new) |
| `#/limits/<childId>`, `#/limits` | child settings, Rules (first child if none) |
| `#/activity`, `#/activity/<childId>` | the activity list (all, or one child), as a page |
| `#/family` | Settings |
| `#/family/guardians/<sub>` | `#/settings/guardians/<sub>` (old kept as redirect; with guardian keys) |
| `#/setup` | unchanged |

`domain/wardRoute.ts` (`goToWard`) and the carrier's `MainActivity.kt`
targets keep working through the redirects. No APK change is needed.

## Out of scope

- Visual language (`theme.css` colours and type): this is a structure change,
  not a restyle. Spacing and touch-target fixes are in scope.
- Wire, signer, guardian-keys logic. The store gains only `moveChild`
  (local order).
- New features beyond the bottom bar, child settings and ordering.

## Build plan

Each phase lands green (lint, typecheck, tests) and the old screens keep
working until their replacement lands.

1. **Shell, Home, child page, lever sheet.** New router with redirects, Home,
   child page sections 1–3 and 5–7 (rules rows link to the old Limits for
   now), the sheet replacing inline GiveTime/FinishNow and the Approvals
   stepper.
2. **Rule editors as pages.** `DeviceLimits` renders one `sections` entry
   per page with a bottom Save bar. Per-app cards move under Apps, and the
   Limits tab goes.
3. **Settings and device pages.** Family is split: ward and device parts go
   to the child and device pages, household parts to Settings.
4. **Remove the tab bar and old screens, test sweep.** Screen tests are
   updated in the phase that changes them.

The plan is written here. Build agents get exclusive files per phase, and
the diff gets an independent review before merge.

Clocks: **code-ready** is about one focused session. **Hardware-verified**
needs one test round on the Pixel 8 (guardian), with both wards live: answer
an ask from a notification, give time, stop now, change a schedule, open a
device page and generate an unlock code. Then one round with the second
guardian, because the real test is whether they can do the daily jobs
unaided.

## Feature-preservation checklist

| `useCharter()` | Now | New home |
|---|---|---|
| `giveTime` | Home card, inline | lever sheet (Home card, child page) |
| `standDown` / lift | Home Finish now / Allow back on | Stop now / Allow back on (Home card, child page) |
| `approveRequest`, `denyRequest`, `dismissRequest`, `setRequestChosenMinutes` | Approvals | Home Waiting + child page Waiting; amount via sheet |
| `approveAppOpen` | Approvals 30 min/1 h/Rest of today | same chips, inline on the ask |
| `savePolicy` (schedule, limit, named times, when time's up, always available, websites, apps, holds, lifeline, hotspot, per-device overrides) | Limits sub-tabs and accordions | rule editor pages |
| `saveAppRule`, `removePolicy`, `addAppLimit` | Limits "Single out a game or app" | Apps editor page |
| `addChild` | Family | Settings and Home empty state |
| `editChild`, `removeChild` | Family child card | child settings, Manage |
| `moveChild` | none (new) | Settings, Children and family devices |
| `addDevice`, `confirmPairing`, `beginPhonePairing`, `cancelPhonePairing`, `sendPairOffer` | Family SetupComputer wizard | child settings "Set up another device" (same wizard) |
| `reconnectDevice`, `unpairDevice`, `removeDevice` | Family device row | device page |
| offline Unlock | Family device row | device page |
| `setDeviceDebugging` | Family device row | device page |
| `createRelayCode`, `recordDeviceRelays` | Family device row | device page |
| `sendLinuxUpdate` | Family device row | device page |
| `sendCharterUpdate`, `openMaintenanceWindow` | Family device row (ward-wide in fact) | child settings, child-wide device settings |
| `claimDevice` / unclaimed phones | Family banner | Home banner, then pick a child |
| `enableLocalSigner`, `pairSignet`, `disconnectSigner`, `setAutoSign` | Family Parent approval | Settings, Parent approval |
| `restoreGuardianKey`, key backup | Family (twice), GuardianRecovery | Settings, Key backup; nudge on Home; GuardianRecovery unchanged |
| `guardianKeys.*` (invite, confirm presence, PIN, move, lost phone, leave, remove, roster) | Family › Guardians | Settings › Other guardians (sub-screens unchanged; with guardian keys) |
| Join a family | Family | Settings (with guardian keys) |
| `resolveSignature` (SignSheet) | global | global, unchanged |
| `refreshNow` (pull to refresh) | every tab | Home and child page |
| usage history / weekly chart | Activity, Home "Their week" | child page This week; family activity list |
| AppVersion, CarrierDownload | Family footer, Home | Settings (CarrierDownload nudge stays on Home until installed) |
| Setup guide | Home onboarding, Family blurb | Home onboarding, Settings |

## Later (not in this change)

- **A shared device that takes on whoever is using it.** A family tablet is
  off by default and today is set up as if it were a child of its own. With
  children of different ages, a guardian wants to hand it to one of them and
  have *their* rules apply for that session. This changes the assumption that
  each device belongs to one ward. It reaches the wire contract (something that
  names which ward is active on a device) and enforcement on Android and Linux,
  so it needs a design of its own, not a screen change.
