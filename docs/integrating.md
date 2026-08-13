# Integrating Charter

**Audience:** anyone building a consumer app (game, browser app, anything with a sign-in flow) that wants to be wardship-aware.

**TL;DR:** integrate Sign in with Signet as you normally would, then handle one new structured deny shape on `sign_event` responses. Optionally subscribe to the audit channel for richer UX. That's the whole consumer-side surface.

---

## What "Charter integration" means

Charter doesn't change the way your app authenticates users — that's still Sign in with Signet (Nostr identity, NIP-46). What changes is that **a guardian can attach time / spend / content / comms clauses** to the dependant's signing authority. When a sign request arrives at the bunker outside the allowed window (or violates a future clause), the bunker refuses with a structured error.

Your job:

1. Recognise the Charter deny error shape.
2. Render a child-friendly "your Charter says X" screen.
3. (Optional) Subscribe to the audit channel for richer "back at 4pm" UX.

Steps 1 and 2 are mandatory for Charter compliance. Step 3 is polish.

## Read first

1. **[`spec/contract.md`](../spec/contract.md)** — the wire-level contract. Required reading. Defines clauses, deny shapes, audit shapes, error codes.
2. **[`README.md`](../README.md)** — what Charter is + the ForgeSworn stack context.
3. **[positioning doc](../../signet-plans/docs/plans/2026-05-08-charter-positioning.md)** — strategic context (why Charter exists as a product). Optional but useful for understanding the trajectory.

If you're integrating a partner project specifically, also see `docs/integrations/<your-project>.md` if one exists for project-specific notes.

## Steps for a consumer app

### Step 1 — Sign in with Signet (existing, no Charter awareness needed)

If your app already uses Sign in with Signet, no change. Continue to use the standard NIP-46 transport. If not, integrate Sign in with Signet first; Charter sits on top.

### Step 2 — Handle the Charter deny error

When you call `sign_event` (e.g. on session start, on credential present, on action sign), parse the response. The bunker refuses Charter-blocked requests with:

```
error: "charter:clause_blocked:<clause>:<reason>"
```

For v1, the only `<clause>` shipped is `schedule`, with `<reason>` ∈ `{outside_allowed_hours, paused}`. See `spec/contract.md` §Reserved error codes for the full grammar.

**Pseudo-code:**

```ts
const response = await bunker.signEvent(template);
if (response.error?.startsWith('charter:clause_blocked:')) {
  // Charter refused this sign. Render the deny screen.
  const [, , clause, reason] = response.error.split(':');
  showCharterDeny({ clause, reason });
  return;
}
if (response.error) {
  // Some other NIP-46 error — handle as you would today.
  return;
}
// signed event ready to use
```

**Render guidance:**

- For `schedule:outside_allowed_hours`: "Your gaming time is over for today" or similar. Don't expose timestamps unless the player asks (privacy posture — see contract §Privacy).
- For `schedule:paused`: "Gaming is paused — ask your parent."
- For unknown reasons (forward-compat): "Your Charter says this isn't allowed right now. Ask your parent."
- Parse defensively — never assume a fixed grammar; future clauses add new reason codes.

### Step 3 (optional) — Audit subscription

If you want to render "back at 4pm" or "you've used 90 min today" UX, subscribe to the audit channel. v1 emits kind-31000 gift-wrapped events to the guardian and to the dependant's paired-child clientPubkey. Consumer apps are reserved as a third recipient slot in v1.x but not yet emitted — once that lands, your app receives audit events for the dep's actions and can extract `next-allowed`, `schedule-source`, etc.

Until then, render the deny screen with the clause + reason only.

## Don'ts

- **Don't invent your own method names** like `examplegame_check_play_allowed` and intend to "rename to `charter_*` later." The contract is stable enough at v0.1 to integrate against; rename-after-shipped-consumer is the trap this whole sequencing avoids.
- **Don't bypass the bunker** by reading the dep's IDB directly. Always go through the NIP-46 transport — that's how the schedule, audit, and future clauses get enforced consistently.
- **Don't render timestamps you got from the deny error** — the deny string carries clause + reason only. Timestamps come via audit subscription where the guardian has explicitly granted visibility (privacy contract).
- **Don't pin to v0.x in production releases.** Wait for v1.0 of the contract before shipping to non-partner audiences. Wire shapes can still change in v0.x.
- **Don't draft your own NIP** for `charter_*` methods. Vendor-prefixed naming is intentional (matches the `heartwood_*` discipline).

## Acceptance checklist

You're Charter-compliant when:

- [ ] Sign in with Signet works as before.
- [ ] `sign_event` responses with `error: 'charter:clause_blocked:*'` are parsed and routed to a deny screen.
- [ ] The deny screen renders distinct copy for at least the two v1 reasons (`outside_allowed_hours`, `paused`).
- [ ] Unknown reason codes fall through to a generic "ask your parent" message (forward-compat).
- [ ] Your app does NOT cache or assume schedule data — every sign is a fresh check via the bunker.
- [ ] Your app does NOT render any clock-window data inferred from the deny response (privacy).

## What's live in Signet today

The canonical Charter implementation is the Signet bunker (`forgesworn/signet-app`). Always check `spec/contract.md` §Implementation status for what's actually shipping. As of contract v0.1:

- Schedule clause data model: live
- Sign-time enforcement (the deny error you'll see): live
- Cross-device sync: live
- Charter dashboard write-side methods (`charter_set_*`): not yet implemented (Phase 4 in Signet)
- Dashboard / guardian-side editor UI: not yet implemented (Phase 2 in Signet)

This means: you can integrate against the deny error today, but to actually exercise the path you'll need to manually plant a schedule via Signet's IDB devtools or wait for the Phase 2 editor to ship. Coordinate with the Signet team for testing.

## Status & contract version

You're integrating against **Charter Protocol v0.1** (pre-stable). See `spec/contract.md` §Stability and §Versioning for the breaking-change policy.

## Questions

- Spec ambiguity → open a discussion on `forgesworn/charter`
- Bunker bug → file on `forgesworn/signet-app-internal`
- Strategic question (clause priorities, etc.) → ask in the ForgeSworn channels
