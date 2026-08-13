# MyCharter — streamlining the app: kill the repetition, segment the mega-list

**Date:** 2026-08-01
**Status:** design of record for the 0.2.x MyCharter UX pass

decented's report: *"the sections feel very similar, so you don't get a distinct
feeling that you're in different sections — you don't know where you are. It's
a waste of real estate and confusing. Limits is one long mega list; it needs to
be collapsible or segmented, maybe tabbed and segmented, double-tabbed: tabbed
by user, and tabbed by section within that."*

He is describing two separate faults that reinforce each other:

1. **The same facts are restated on every tab**, in five different visual
   dialects. Every screen opens with an avatar + a name + a status pill, so
   every screen looks like the last one.
2. **No screen has a shape of its own.** All five are a vertical run of white
   cards. Limits is the extreme case: nine always-expanded editors, ~2,000px
   of controls, one Save button at the very bottom.

---

## 1. Repetition audit (what is actually duplicated)

### 1.1 The ward identity block — rendered five ways, five times

| Screen | How "who am I looking at" is drawn |
|---|---|
| Home | `ChildCard` header: Avatar + name + `Allowed now`/`Locked` pill + chevron |
| Limits | sticky brand-red **segmented switcher** … *and then again* Avatar + name + "Set their rules below" + `Allowed now`/`Locked` pill |
| Family | `ChildCard` header: Avatar + name + "1 device connected" + liveness pill |
| Activity | scrollable **filter pills** ("Everyone", names) … and then `"{name} · this week"` card titles |
| Approvals | `section-label` with a 22px Avatar + name |

Three different mental models for the *same* action (choose a ward):
segmented control, filter pills, and a list of cards. Limits states the ward
**twice on one screen**, thirty pixels apart.

### 1.2 Duplicated content blocks

- **"Game & app logins — coming later"** ships twice: `ComingLater` on Home
  *and* `GameLoginsTeaser` inside every Family child card. Two dead cards.
- **Pending-request count** is rendered three times on Home alone: the tab-bar
  badge, the "N requests are waiting for you" line under the section label, and
  the per-card `PendingBlock` — which then repeats the whole list on Approvals.
- **"Go and set up a device"** is said in five places: the Onboarding card,
  Home's `NotSetUpBody`, the Limits page banner, the Limits missing-policy
  banner, and Family's setup flow.
- **Connectivity** is stated on Home (`DeviceLines`, per-device liveness pill)
  and again on Family (child liveness pill + per-device `On`/`Off` pill +
  "Managed by Charter · seen 3 min ago" + "1 device connected"). Family says
  it four ways in one card.
- **Limits' own summary paragraph** (`scheduleSummary · budgetSummary ·
  webSummary · appsSummary`) restates, in one dense run-on line, what the nine
  editors immediately below already show in full.

### 1.3 The Limits mega-list

One `Card` holding, always expanded, in this order: Daily schedule · Time
limit · Websites · Apps · When time's up · Hotspot · Lifeline · Learning ·
Time buckets — then a separate stack of per-app cards, then "add an app limit".
A single shared Save sits below all of it. Changing the daily limit means
scrolling past the lifeline editor to find Save.

---

## 2. The design

### 2.1 One switcher, learned once

A single `WardTabs` segmented control replaces the Limits switcher **and** the
Activity filter pills. Same component, same place (sticky, under the header),
same gesture. Activity gets an extra leading "Everyone" segment.

`WardHeading` (avatar + name + one status pill + optional trailing link) becomes
the single way a ward is titled inside a screen — used by Approvals groups and
Activity's weekly card. **Limits loses its second identity block entirely**: the
switcher carries the name, and the live status pill rides the right-hand end of
the section-tab strip, where it costs no vertical space. With one ward there is
no switcher, so the heading carries the name instead.

### 2.2 Each screen gets a distinct shape

The chrome is unified so the *content* can differ. The bodies become
deliberately unlike each other:

- **Home** — big numbers, progress bars, two action buttons. A dashboard.
- **Approvals** — full-bleed decision cards with Approve / Not now. A queue.
- **Limits** — a **settings list**: collapsed rows with a title, a summary and a
  chevron. Not cards. This is the single biggest "I know where I am" win.
- **Activity** — a chart plus a timeline. Neither cards nor rows.
- **Family** — people and devices; a roster.

The app header gains a one-line muted subtitle per tab (*"What's happening right
now" / "Decisions waiting for you" / "The rules you've set" / "What's happened" /
"People, devices and your key"*), and loses 4px of vertical padding to pay for it.

### 2.3 Limits: double tabs + accordions + a sticky save bar

```
┌ sticky ─────────────────────────────────────────┐
│  [ Robin ] [ Mia ]        ← ward tabs      │
│  [ Time ][ Apps & web ][ Safety ]   [Allowed now] │
└─────────────────────────────────────────────────┘
   Daily schedule      Allowed 5 days a week    ›
   Time limit          2h a day               • ›   ← • = changed
   App time limits     No app has its own limit ›
   Learning time       3 apps time-free         ›
   When time's up      Audio stops with screen  ›
┌ fixed above the tab bar (only when dirty) ──────┐
│  Changed: Time limit        [Discard] [Save]    │
└─────────────────────────────────────────────────┘
```

**Section groups**

| Tab | Sections |
|---|---|
| **Time** | Daily schedule (incl. "Off unless I open it" + "Pause") · Time limit · App time limits · Learning time · When time's up |
| **Apps & web** | Websites · Apps · *then* the per-app limit cards + "Add an app limit" |
| **Safety** | Lifeline · Hotspot |

Labels are the family's words, not ours. "Time buckets" became **App time
limits** (a parent asking for "Play is an hour a day" is setting one), "Content"
became **Apps & web**, and a collapsed row says what is true rather than
describing an absence — "Nothing is time-free", not "Learning time counts as
normal screen time".

**Rules**

- Sections are **closed by default**, so a tab is five short rows you can read
  at a glance. Each row's summary is the existing `*Summary()` helper (new ones
  written for buckets, learning and lifeline).
- A section with unsaved changes shows a **change dot**, and so does its tab —
  so an edit can never hide behind a closed row on a tab you aren't looking at.
  Rows are not force-opened: a section only becomes dirty while it is open, and
  a screen that re-opens what you just collapsed is fighting you.
- The Save bar is **fixed above the tab bar and only appears when dirty**. It
  names which sections changed, and tapping it opens one — switching tab first
  if the change is on a tab you can't see. Switching ward while dirty asks
  first; the editor is keyed by child, so it used to discard silently. This is what makes cross-tab editing safe:
  the save still spans every dimension in one signed clause (unchanged
  behaviour), so it must not live inside any one tab.
- Section-tab state lives in the `Limits` screen, not `DeviceLimits`, because
  the Apps & web tab also renders the per-app policy cards.

### 2.4 Link through instead of restating

New route: `#/activity/<childId>` (mirrors the existing `#/limits/<childId>`),
so any screen can hand off to a specific ward. A shared `useWardRoute(tab)` hook
serves both.

| From | To | Replaces |
|---|---|---|
| Home ward card header | `#/limits/<id>` | (already there) |
| Home device line | `#/family` | reading the device state twice |
| Home "N waiting" block | `#/approvals` | (already there) |
| Limits "no device" banner | button → `#/family` | two banners of prose |
| Approvals ward heading | "Their limits ›" | going to look the rules up |
| Activity weekly card | "Adjust limits ›" | — |
| Family child card | "Limits ›" + "Activity ›" | — |

### 2.5 Deletions

- Home `ComingLater` card (Family keeps the one teaser).
- Home's "N requests are waiting for you" line (badge + per-card block remain).
- Limits' second ward identity block.
- Limits' `DeviceLimits` title + dense summary paragraph (the accordions are
  the summary).
- One of Limits' two "no device set up" banners.
- Family's "1 device connected" line when the device list is right below it.

---

## 3. What must NOT be lost

Every control that exists today stays reachable, and the wire behaviour is
untouched:

- One Save = one signed clause carrying **every** changed dimension with a
  shared `issuedAt` — including a dimension whose only change is a per-device
  override. Unchanged.
- Per-device split toggles, the dormancy toggle, schedule pause, the fail-closed
  lifeline validation, the `LIFELINE_V2_MIN_VERSION_CODE` ship-order guard, the
  device-set-limits warning, the schedule window validation, and the
  confirm-with-signer banner all survive verbatim inside their sections.
- `buildSchedule`, `isDormant`, `toDormant`, `fromDormant`, `buildLifeline`,
  `buildListening`, `listeningSummary` keep their exports (tests import them).

## 4. Out of scope

Family's own internal split (children / devices / security) and the Guide.
Family is long, but it is a roster, not a mega-list of editors; segmenting it
is a separate change.
