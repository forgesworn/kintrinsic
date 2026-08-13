# Charter Local-Key Guardian Signer — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Give the MyCharter PWA its own local BIP-340 guardian key so it signs and gift-wraps CLAUSEs directly (no external bunker), closing the last T2 code gap.

**Architecture:** `RealSigner` already takes an injected `GuardianOps`. We add a second builder, `createLocalSigner`, that implements those ops from a browser-held key — a sibling to the existing `createSignetSigner`. The store selects it via a new `SignerKind: "local"`. A "Pair this laptop" screen exports the guardian pubkey as the `bunker://…?kind=charter` string the laptop pins. Nothing downstream (gift-wrap, relay, STATUS) changes; charterd is untouched.

**Tech Stack:** TypeScript, React 18, `nostr-tools` ^2.23, Vitest (jsdom), Vite.

## Global Constraints

- Spec: `docs/superpowers/specs/2026-07-03-charter-local-guardian-signer-design.md`.
- No new npm dependencies. QR is **out of scope** (no lib; the laptop is the ingesting side — lead with the copyable `bunker://` string + `npub`).
- No seed phrase, no passphrase, no key export, no server account (spec non-goals). Recovery = re-pair.
- The guardian secret key lives in its **own** localStorage key `charter.guardian.key.v1`, persisted as an `nsec…` (via `nip19`), **never** bundled into `charter.state.v2`.
- Guardian relays for the pairing URI = `DEFAULT_RELAYS` from `src/signer/config.ts` (`["wss://relay.trotters.cc"]`).
- Test command: `npm test` (`vitest run`). Typecheck: `npm run typecheck`. Run from `apps/charter-app/`.
- Match existing file style: factory function `createXSigner(config)` returning a `RealSigner`; tests use `describe/it` from `vitest`.

## File Structure

- Create `src/signer/guardianKey.ts` — get-or-create/clear the persisted guardian secret key. One responsibility: key custody.
- Create `src/signer/guardianKey.test.ts`.
- Create `src/signer/guardianPairing.ts` — build the guardian `bunker://…` URI + `npub` for display. Pure formatting.
- Create `src/signer/guardianPairing.test.ts`.
- Create `src/signer/localSigner.ts` — `createLocalSigner`, the local-key `GuardianOps` builder. Sibling of `signetSigner.ts`.
- Create `src/signer/localSigner.test.ts`.
- Create `src/signer/selectSigner.ts` — pure `selectSigner(state, deps)` used by the store's `buildSigner`.
- Create `src/signer/selectSigner.test.ts`.
- Modify `src/domain/types.ts` — add `"local"` to the `SignerKind` union.
- Modify `src/signer/realSigner.ts` — `labelFor("local") → "This phone"`.
- Modify `src/store/store.tsx` — delegate `buildSigner` to `selectSigner`; add `enableLocalSigner` action + mount reconnect.
- Modify `src/screens/Family.tsx` — "Set up on this phone" entry + "Pair this laptop" section.

---

### Task 1: Guardian key store

**Files:**
- Create: `apps/charter-app/src/signer/guardianKey.ts`
- Test: `apps/charter-app/src/signer/guardianKey.test.ts`

**Interfaces:**
- Produces: `loadOrCreateGuardianKey(): Uint8Array`, `guardianPubkeyHex(): string`, `clearGuardianKey(): void`.

- [ ] **Step 1: Write the failing test**

```ts
// apps/charter-app/src/signer/guardianKey.test.ts
import { afterEach, describe, expect, it } from "vitest";
import { getPublicKey } from "nostr-tools";
import { clearGuardianKey, guardianPubkeyHex, loadOrCreateGuardianKey } from "./guardianKey";

afterEach(() => clearGuardianKey());

describe("guardianKey", () => {
  it("persists one key — same pubkey across calls", () => {
    const a = getPublicKey(loadOrCreateGuardianKey());
    expect(guardianPubkeyHex()).toBe(a);
  });

  it("regenerates a different key after clear (the revocation property)", () => {
    const first = guardianPubkeyHex();
    clearGuardianKey();
    const second = guardianPubkeyHex();
    expect(second).not.toBe(first);
    expect(second).toHaveLength(64);
  });
});
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cd apps/charter-app && npx vitest run src/signer/guardianKey.test.ts`
Expected: FAIL — cannot resolve `./guardianKey`.

- [ ] **Step 3: Write minimal implementation**

```ts
// apps/charter-app/src/signer/guardianKey.ts
import { generateSecretKey, getPublicKey, nip19 } from "nostr-tools";

// The guardian secret key lives on its OWN localStorage key — never bundled into
// the app-state blob. Persisted as an `nsec…` so it's a recognisable Nostr key.
const KEY = "charter.guardian.key.v1";

/** Load the persisted guardian secret key, generating + persisting one on first use. */
export function loadOrCreateGuardianKey(): Uint8Array {
  const existing = readKey();
  if (existing) return existing;
  const sk = generateSecretKey();
  localStorage.setItem(KEY, nip19.nsecEncode(sk));
  return sk;
}

/** The guardian pubkey (hex) for the persisted key, creating it if needed. */
export function guardianPubkeyHex(): string {
  return getPublicKey(loadOrCreateGuardianKey());
}

/** Forget the guardian key — used on deliberate re-pair / identity reset. */
export function clearGuardianKey(): void {
  localStorage.removeItem(KEY);
}

function readKey(): Uint8Array | null {
  try {
    const raw = localStorage.getItem(KEY);
    if (!raw) return null;
    const decoded = nip19.decode(raw);
    return decoded.type === "nsec" ? decoded.data : null;
  } catch {
    return null; // corrupt / unavailable storage → treat as no key
  }
}
```

- [ ] **Step 4: Run test to verify it passes**

Run: `cd apps/charter-app && npx vitest run src/signer/guardianKey.test.ts`
Expected: PASS (2 tests).

- [ ] **Step 5: Commit**

```bash
git add apps/charter-app/src/signer/guardianKey.ts apps/charter-app/src/signer/guardianKey.test.ts
git commit -m "feat(mycharter): persisted local guardian key store (get-or-create/clear)"
```

---

### Task 2: Guardian pairing URI builder

**Files:**
- Create: `apps/charter-app/src/signer/guardianPairing.ts`
- Test: `apps/charter-app/src/signer/guardianPairing.test.ts`

**Interfaces:**
- Produces: `guardianBunkerUri(pubkeyHex: string, relays: string[]): string`, `guardianNpub(pubkeyHex: string): string`.

- [ ] **Step 1: Write the failing test**

```ts
// apps/charter-app/src/signer/guardianPairing.test.ts
import { describe, expect, it } from "vitest";
import { parseBunkerInput } from "nostr-tools/nip46";
import { guardianBunkerUri, guardianNpub } from "./guardianPairing";

const PK = "a".repeat(64);

describe("guardianBunkerUri", () => {
  it("round-trips the pubkey + relay and carries kind=charter", async () => {
    const uri = guardianBunkerUri(PK, ["wss://relay.trotters.cc"]);
    expect(uri.startsWith("bunker://")).toBe(true);
    expect(uri).toContain("kind=charter");
    const pointer = await parseBunkerInput(uri);
    expect(pointer?.pubkey).toBe(PK);
    expect(pointer?.relays).toContain("wss://relay.trotters.cc");
  });
});

describe("guardianNpub", () => {
  it("encodes the hex pubkey as an npub", () => {
    expect(guardianNpub(PK)).toMatch(/^npub1/);
  });
});
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cd apps/charter-app && npx vitest run src/signer/guardianPairing.test.ts`
Expected: FAIL — cannot resolve `./guardianPairing`.

- [ ] **Step 3: Write minimal implementation**

```ts
// apps/charter-app/src/signer/guardianPairing.ts
import { nip19 } from "nostr-tools";

/**
 * The guardian identity the laptop pins at setup:
 *   `bunker://<guardian-pubkey>?relay=wss://…&kind=charter`
 * (`spec/contract.md`, "Pairing — bunker:// grammar"). The laptop only PINS this
 * pubkey + relay; it never dials a bunker — so a local-key PWA advertises its own
 * pubkey here exactly as a Signet bunker would.
 */
export function guardianBunkerUri(pubkeyHex: string, relays: string[]): string {
  const relayParams = relays.map((r) => `relay=${encodeURIComponent(r)}`).join("&");
  return `bunker://${pubkeyHex}?${relayParams}&kind=charter`;
}

/** The same guardian key as an `npub…`, for human-readable display. */
export function guardianNpub(pubkeyHex: string): string {
  return nip19.npubEncode(pubkeyHex);
}
```

- [ ] **Step 4: Run test to verify it passes**

Run: `cd apps/charter-app && npx vitest run src/signer/guardianPairing.test.ts`
Expected: PASS (2 tests).

- [ ] **Step 5: Commit**

```bash
git add apps/charter-app/src/signer/guardianPairing.ts apps/charter-app/src/signer/guardianPairing.test.ts
git commit -m "feat(mycharter): guardian bunker:// pairing-URI + npub builders"
```

---

### Task 3: `createLocalSigner` (the local-key GuardianOps)

**Files:**
- Create: `apps/charter-app/src/signer/localSigner.ts`
- Test: `apps/charter-app/src/signer/localSigner.test.ts`
- Modify: `apps/charter-app/src/domain/types.ts` (add `"local"` to `SignerKind`)
- Modify: `apps/charter-app/src/signer/realSigner.ts` (`labelFor("local")`)

**Interfaces:**
- Consumes: `loadOrCreateGuardianKey` (Task 1); `RealSigner`, `ChildTarget` (existing); `GuardianOps` (existing).
- Produces: `createLocalSigner(config: LocalSignerConfig): RealSigner` where `LocalSignerConfig = { resolveChild: (childId: string) => ChildTarget | undefined; now?: () => number; pool?: SimplePool; loadKey?: () => Uint8Array }`.

- [ ] **Step 1: Add `"local"` to the SignerKind union and the RealSigner label**

In `src/domain/types.ts`, change the `SignerState.kind` field:

```ts
  kind: "none" | "signet" | "heartwood" | "local";
```

In `src/signer/realSigner.ts`, extend `labelFor`:

```ts
function labelFor(kind: SignerKind): string | undefined {
  if (kind === "signet") return "Signet";
  if (kind === "heartwood") return "Heartwood";
  if (kind === "local") return "This phone";
  return undefined;
}
```

- [ ] **Step 2: Write the failing test**

```ts
// apps/charter-app/src/signer/localSigner.test.ts
import { describe, expect, it } from "vitest";
import { generateSecretKey, getPublicKey, type NostrEvent, type SimplePool } from "nostr-tools";
import type { Policy } from "../domain/types";
import { unwrapClause } from "../wire/giftwrap";
import { createLocalSigner } from "./localSigner";
import type { ChildTarget } from "./realSigner";

const GSK = generateSecretKey();
const GUARDIAN = getPublicKey(GSK);
const DEV = generateSecretKey();
const DPK = getPublicKey(DEV);
const SUBJECT = getPublicKey(generateSecretKey());
const RELAYS = ["wss://relay.trotters.cc"];

const policy: Policy = {
  id: "p1",
  scope: { kind: "device" },
  schedule: { tz: "Europe/London", weekly: { mon: [{ start: "16:00", end: "18:00" }] } },
  budget: { tz: "Europe/London", dailyMinutes: 90 },
};

function makeSigner(published: { relays: string[]; event: NostrEvent }[], target: ChildTarget) {
  // Minimal SimplePool stub: capture the publish synchronously, return no promises.
  const pool = {
    publish: (relays: string[], event: NostrEvent) => {
      published.push({ relays, event });
      return [];
    },
  } as unknown as SimplePool;
  return createLocalSigner({ resolveChild: () => target, now: () => 1_700_000_000, loadKey: () => GSK, pool });
}

describe("createLocalSigner", () => {
  it("connects offline with the local label", async () => {
    const signer = makeSigner([], { subject: SUBJECT, devicePubkeys: [DPK], relays: RELAYS });
    const st = await signer.connect("local");
    expect(st).toMatchObject({ connected: true, kind: "local", label: "This phone" });
  });

  it("signs clauses with the local guardian key and gift-wraps them to the device", async () => {
    const published: { relays: string[]; event: NostrEvent }[] = [];
    const signer = makeSigner(published, { subject: SUBJECT, devicePubkeys: [DPK], relays: RELAYS });
    await signer.connect("local");

    await signer.signClause("child_sam", policy);

    expect(published).toHaveLength(2); // schedule + budget
    for (const p of published) expect(p.event.kind).toBe(1059);
    const unwrapped = published.map((p) => unwrapClause(p.event, DEV, GUARDIAN));
    expect(unwrapped.every((u) => u.valid && u.payload.subject === SUBJECT)).toBe(true);
    expect(unwrapped.map((u) => u.payload.kind).sort()).toEqual(["budget", "schedule"]);
  });
});
```

- [ ] **Step 3: Run test to verify it fails**

Run: `cd apps/charter-app && npx vitest run src/signer/localSigner.test.ts`
Expected: FAIL — cannot resolve `./localSigner`.

- [ ] **Step 4: Write minimal implementation**

```ts
// apps/charter-app/src/signer/localSigner.ts
import {
  SimplePool,
  finalizeEvent,
  getPublicKey,
  nip44,
  type EventTemplate,
  type NostrEvent,
} from "nostr-tools";
import type { GuardianOps } from "../wire/giftwrap";
import { RealSigner, type ChildTarget } from "./realSigner";
import { loadOrCreateGuardianKey } from "./guardianKey";

export interface LocalSignerConfig {
  /** Resolve a child id → subject + device pubkeys + relays. */
  resolveChild: (childId: string) => ChildTarget | undefined;
  now?: () => number;
  /** Override the relay pool (tests). */
  pool?: SimplePool;
  /** Override the guardian key (tests). Defaults to the persisted browser key. */
  loadKey?: () => Uint8Array;
}

/**
 * Build a guardian signer backed by a LOCAL BIP-340 key held in the browser —
 * MyCharter self-signs, no external bunker. It implements the same GuardianOps
 * the gift-wrap needs directly with nostr-tools; the laptop pins this key's
 * pubkey. This is the sibling of `signetSigner.ts` (which fulfils the same seam
 * over NIP-46). Orchestration + crypto live in realSigner.ts / wire/giftwrap.ts.
 */
export function createLocalSigner(config: LocalSignerConfig): RealSigner {
  const pool = config.pool ?? new SimplePool();
  const loadKey = config.loadKey ?? loadOrCreateGuardianKey;

  const connectGuardian = async (): Promise<GuardianOps> => {
    const sk = loadKey();
    const pubkey = getPublicKey(sk);
    return {
      pubkey,
      signEvent: async (t: EventTemplate): Promise<NostrEvent> => finalizeEvent(t, sk),
      nip44Encrypt: async (recipient: string, plaintext: string): Promise<string> =>
        nip44.encrypt(plaintext, nip44.getConversationKey(sk, recipient)),
    };
  };

  return new RealSigner({
    connectGuardian,
    publish: async (relays: string[], event: NostrEvent) => {
      // Best-effort fan-out; don't hard-fail if one relay is down (mirrors Signet).
      await Promise.allSettled(pool.publish(relays, event));
    },
    resolveChild: config.resolveChild,
    now: config.now ?? (() => Math.floor(Date.now() / 1000)),
  });
}
```

- [ ] **Step 5: Run tests to verify they pass**

Run: `cd apps/charter-app && npx vitest run src/signer/localSigner.test.ts`
Expected: PASS (2 tests).

- [ ] **Step 6: Typecheck (the union change touches switches)**

Run: `cd apps/charter-app && npm run typecheck`
Expected: no errors. (If a `switch (kind)` warns on the new member, it already has a `default`; no change needed.)

- [ ] **Step 7: Commit**

```bash
git add apps/charter-app/src/signer/localSigner.ts apps/charter-app/src/signer/localSigner.test.ts apps/charter-app/src/domain/types.ts apps/charter-app/src/signer/realSigner.ts
git commit -m "feat(mycharter): createLocalSigner — local-key guardian signer (self-sign)"
```

---

### Task 4: Store selection + `enableLocalSigner` action

**Files:**
- Create: `apps/charter-app/src/signer/selectSigner.ts`
- Test: `apps/charter-app/src/signer/selectSigner.test.ts`
- Modify: `apps/charter-app/src/store/store.tsx`

**Interfaces:**
- Consumes: `createLocalSigner` (Task 3), `createSignetSigner`, `createMockSigner`, `ConfirmGate`, `ChildTarget`.
- Produces: `selectSigner(signerState: SignerState, deps: { resolveChild: (id: string) => ChildTarget | undefined; confirmGate: ConfirmGate }): Signer`; store action `enableLocalSigner(): Promise<void>`.

- [ ] **Step 1: Write the failing test for `selectSigner`**

```ts
// apps/charter-app/src/signer/selectSigner.test.ts
import { afterEach, describe, expect, it } from "vitest";
import { clearGuardianKey } from "./guardianKey";
import { selectSigner } from "./selectSigner";

const deps = { resolveChild: () => undefined, confirmGate: () => true };

afterEach(() => clearGuardianKey());

describe("selectSigner", () => {
  it("selects a working local signer for kind 'local' (connects offline)", async () => {
    const s = selectSigner({ connected: false, kind: "local", autoSign: false }, deps);
    const st = await s.connect("local");
    expect(st).toMatchObject({ connected: true, kind: "local", label: "This phone" });
  });

  it("selects the Signet signer when a bunkerUri is present", () => {
    const s = selectSigner(
      { connected: false, kind: "signet", autoSign: false, bunkerUri: "bunker://deadbeef?relay=wss://r" },
      deps,
    );
    expect(s.status()).toMatchObject({ connected: false, kind: "none" }); // RealSigner pre-connect
  });

  it("falls back to the mock signer otherwise", () => {
    const s = selectSigner({ connected: false, kind: "none", autoSign: true }, deps);
    expect(s.status()).toMatchObject({ autoSign: true });
  });
});
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cd apps/charter-app && npx vitest run src/signer/selectSigner.test.ts`
Expected: FAIL — cannot resolve `./selectSigner`.

- [ ] **Step 3: Write `selectSigner`**

```ts
// apps/charter-app/src/signer/selectSigner.ts
import type { SignerState } from "../domain/types";
import type { ChildTarget } from "./realSigner";
import type { Signer } from "./Signer";
import { createMockSigner, type ConfirmGate } from "./mockSigner";
import { createSignetSigner } from "./signetSigner";
import { createLocalSigner } from "./localSigner";

export interface SelectSignerDeps {
  resolveChild: (childId: string) => ChildTarget | undefined;
  confirmGate: ConfirmGate;
}

/**
 * Pick the concrete Signer for a persisted signer state:
 *   kind "local"  → the local-key self-signer (this phone holds the key)
 *   bunkerUri set  → the Signet NIP-46 bunker signer
 *   otherwise      → the local mock (pre-setup default)
 * `kind` "local" is checked FIRST so it never carries a bunkerUri.
 */
export function selectSigner(signerState: SignerState, deps: SelectSignerDeps): Signer {
  if (signerState.kind === "local") {
    return createLocalSigner({ resolveChild: deps.resolveChild });
  }
  if (signerState.bunkerUri) {
    return createSignetSigner({
      getBunkerUri: async () => signerState.bunkerUri!,
      resolveChild: deps.resolveChild,
    });
  }
  return createMockSigner({ initial: signerState, confirmGate: deps.confirmGate });
}
```

- [ ] **Step 4: Run test to verify it passes**

Run: `cd apps/charter-app && npx vitest run src/signer/selectSigner.test.ts`
Expected: PASS (3 tests).

- [ ] **Step 5: Delegate the store's `buildSigner` to `selectSigner`**

In `src/store/store.tsx`, add the import near the other signer imports:

```ts
import { selectSigner } from "../signer/selectSigner";
```

Replace the `buildSigner` body (currently the `signerState.bunkerUri ? createSignetSigner(...) : createMockSigner(...)` ternary) with:

```ts
  const buildSigner = useCallback(
    (signerState: SignerState): Signer => selectSigner(signerState, { resolveChild, confirmGate }),
    [resolveChild, confirmGate],
  );
```

Remove the now-unused `createSignetSigner` / `createMockSigner` imports **only if** no other reference remains (grep first: `grep -n "createSignetSigner\|createMockSigner" src/store/store.tsx`). `SignerCancelled` / `ConfirmGate` imports stay.

- [ ] **Step 6: Add the `enableLocalSigner` action + mount reconnect**

In `src/store/store.tsx`, add the action alongside `pairSignet` (after the `connectSigner` definition):

```ts
  // Turn on the local-key signer: this phone holds the guardian key and signs
  // clauses directly (no external bunker). Idempotent — safe to call again.
  const enableLocalSigner = useCallback(async () => {
    const next: SignerState = { ...stateRef.current.signer, kind: "local", bunkerUri: undefined };
    signer.current = buildSigner(next);
    await signer.current.connect("local");
    dispatch({ type: "SET_SIGNER", signer: signer.current.status() });
  }, [buildSigner]);
```

Add a mount effect so a persisted local signer re-connects on reload (after the `signer.current` is first built):

```ts
  // On reload, a persisted local signer must re-open (loads the browser key — no
  // network). The Signet path reconnects on its own pairing action.
  useEffect(() => {
    if (state.signer.kind === "local" && !signer.current!.status().connected) {
      signer.current!.connect("local").then(syncSigner).catch(() => {});
    }
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);
```

Add `enableLocalSigner` to the `CharterContextValue` interface (near `pairSignet`):

```ts
  /** Turn on the local-key signer (this phone holds the guardian key). */
  enableLocalSigner: () => Promise<void>;
```

And include `enableLocalSigner` in BOTH the `useMemo` value object and its dependency array (alongside `pairSignet`).

- [ ] **Step 7: Typecheck + full test run**

Run: `cd apps/charter-app && npm run typecheck && npm test`
Expected: typecheck clean; all tests pass (existing + the new signer tests).

- [ ] **Step 8: Commit**

```bash
git add apps/charter-app/src/signer/selectSigner.ts apps/charter-app/src/signer/selectSigner.test.ts apps/charter-app/src/store/store.tsx
git commit -m "feat(mycharter): wire the local signer into the store (select + enable + reconnect)"
```

---

### Task 5: "Set up on this phone" + "Pair this laptop" UI

**Files:**
- Modify: `apps/charter-app/src/screens/Family.tsx`

**Interfaces:**
- Consumes: `enableLocalSigner` (Task 4) from the store; `guardianBunkerUri`, `guardianNpub` (Task 2); `guardianPubkeyHex` (Task 1); `DEFAULT_RELAYS` (existing).

- [ ] **Step 1: Add the imports**

At the top of `src/screens/Family.tsx`, add:

```ts
import { guardianPubkeyHex } from "../signer/guardianKey";
import { guardianBunkerUri, guardianNpub } from "../signer/guardianPairing";
import { DEFAULT_RELAYS } from "../signer/config";
```

Pull `enableLocalSigner` from the store hook where `connectSigner` is already destructured.

- [ ] **Step 2: Add a "Set up on this phone" primary option**

In the `if (!signer.connected)` card's non-`pairing` branch (the `<div className="stack">` with the "Set up with Signet" / "Set up with Heartwood" buttons), add as the FIRST button:

```tsx
            <Button block disabled={busy} onClick={() => run(enableLocalSigner)}>
              Set up on this phone
            </Button>
```

Update the helper line under the buttons to reflect that this phone can hold the key:

```tsx
        <p className="card-sub" style={{ marginTop: 12 }}>
          Simplest: “Set up on this phone” keeps the guardian key right here — no
          extra app. You can also set limits first; they’ll save now and apply
          once approval is on.
        </p>
```

- [ ] **Step 3: Add the "Pair this laptop" section for the local signer**

In the connected view (the `return (<Card> … "Parent approval" …)` block), add, shown only when `signer.kind === "local"`, a section that surfaces the pairing string. Add a copy handler in the component body (mirrors `Guide.tsx`'s pattern):

```tsx
  async function copyText(text: string) {
    try {
      await navigator.clipboard.writeText(text);
    } catch {
      const ta = document.createElement("textarea");
      ta.value = text;
      document.body.appendChild(ta);
      ta.select();
      document.execCommand("copy");
      document.body.removeChild(ta);
    }
  }
```

And render (inside the connected `<Card>`, after the title row):

```tsx
      {signer.kind === "local" && (() => {
        const pk = guardianPubkeyHex();
        const bunker = guardianBunkerUri(pk, DEFAULT_RELAYS);
        return (
          <div className="stack" style={{ marginTop: 12 }}>
            <p className="card-sub" style={{ margin: 0 }}>
              Pair a laptop: on the computer open “Charter Setup”, choose “Pair a
              guardian”, and paste this. It links that laptop to this phone.
            </p>
            <code style={{ wordBreak: "break-all", fontSize: 12, opacity: 0.85 }}>{bunker}</code>
            <Button block onClick={() => run(() => copyText(bunker))}>
              Copy pairing link
            </Button>
            <p className="card-sub" style={{ margin: 0, opacity: 0.7 }}>
              Your guardian ID: {guardianNpub(pk).slice(0, 20)}…
            </p>
          </div>
        );
      })()}
```

- [ ] **Step 4: Typecheck**

Run: `cd apps/charter-app && npm run typecheck`
Expected: no errors.

- [ ] **Step 5: Manual verification (browser)**

Run: `cd apps/charter-app && npm run dev`, open the app, go to **Family**.
Verify, in order:
1. Before setup, the approval card shows **"Set up on this phone"** as the first option.
2. Tapping it flips the card to connected with label **"This phone"** and no error.
3. A **"Pair this laptop"** section shows a `bunker://…?kind=charter` string and **Copy pairing link** places it on the clipboard.
4. Set/change a limit for a child → no crash (it signs + publishes best-effort).
5. **Reload** the page → still connected as "This phone" (mount reconnect), pairing section still shown (re-runnable).

- [ ] **Step 6: Commit**

```bash
git add apps/charter-app/src/screens/Family.tsx
git commit -m "feat(mycharter): set up the guardian on this phone + pair-this-laptop screen"
```

---

## Self-Review

**Spec coverage:**
- Local-key `GuardianOps` builder → Task 3. ✓
- Key persistence (own storage key, nsec) → Task 1. ✓
- `bunker://…?kind=charter` export → Task 2 + Task 5. ✓
- `SignerKind: "local"` + store wiring → Task 3 (type) + Task 4 (select/enable/reconnect). ✓
- "Pair this laptop" screen → Task 5. ✓
- Re-runnable pairing → Task 5 Step 5.5 (reload/re-show) + Task 1's clear→regenerate (revocation). ✓
- Recovery = re-pair; not fail-closed for child → design property, no code (verified in charterd). ✓
- Zero device-side / gift-wrap / relay / STATUS changes → honored (no such files touched). ✓
- Non-goals (no seed phrase, passphrase, export, server, QR) → none added. ✓

**Placeholder scan:** every code step contains full code; every run step has an exact command + expected result. No TBDs.

**Type consistency:** `createLocalSigner(config)` / `LocalSignerConfig` used identically in Tasks 3 & 4; `selectSigner(state, deps)` signature matches its store call; `guardianBunkerUri` / `guardianNpub` / `guardianPubkeyHex` names consistent across Tasks 1, 2, 5; `SignerKind` `"local"` added once (Task 3 Step 1) and consumed everywhere after.

**Scope:** single subsystem (the PWA guardian signer), one focused plan. No decomposition needed.
