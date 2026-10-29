// The upgrade path for a family already on My Signet: after a reload the
// signer is paired (a persisted bunker link) but not connected, and this
// install has no approved working key yet. "Approve this phone" must re-open
// the bunker and ask it once, rather than dead-end on "connect first".
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { createRoot, type Root } from "react-dom/client";
import { act } from "react";
import { finalizeEvent, generateSecretKey, getPublicKey, nip19, nip44, type EventTemplate } from "nostr-tools";
import { resetGuardianKeyCacheForTests } from "../signer/guardianKey";

const rSk = generateSecretKey();
const R = getPublicKey(rSk);
const bunker = vi.hoisted(() => ({ connects: 0 }));

vi.mock("nostr-tools/nip46", () => ({
  parseBunkerInput: async () => ({ pubkey: "00".repeat(32), relays: ["wss://bunker.example"], secret: null }),
  BunkerSigner: {
    fromBunker: () => ({
      connect: async () => {
        bunker.connects += 1;
      },
      getPublicKey: async () => R,
      signEvent: async (t: EventTemplate) => finalizeEvent(t, rSk),
      nip44Encrypt: async (to: string, pt: string) => nip44.encrypt(pt, nip44.getConversationKey(rSk, to)),
    }),
  },
}));

import { CharterProvider, STORAGE_KEY, useCharter, type CharterContextValue } from "./store";

const wSk = generateSecretKey();
const W = getPublicKey(wSk);

let ctx: CharterContextValue | null = null;
function Probe() {
  ctx = useCharter();
  return null;
}

describe("approving this phone on an existing My Signet family", () => {
  let container: HTMLDivElement;
  let root: Root;

  beforeEach(() => {
    localStorage.clear();
    resetGuardianKeyCacheForTests();
    localStorage.setItem("charter.guardian.key.v1", nip19.nsecEncode(wSk));
    localStorage.setItem(
      STORAGE_KEY,
      JSON.stringify({
        children: [],
        requests: [],
        activity: [],
        signer: { connected: true, kind: "signet", autoSign: false, bunkerUri: "bunker://x?relay=wss://bunker.example" },
      }),
    );
    container = document.createElement("div");
    document.body.appendChild(container);
    act(() => {
      root = createRoot(container);
      root.render(
        <CharterProvider>
          <Probe />
        </CharterProvider>,
      );
    });
  });

  afterEach(() => {
    act(() => root.unmount());
    container.remove();
    localStorage.clear();
    resetGuardianKeyCacheForTests();
  });

  it("re-opens the bunker, gets R's approval of W, and keeps it with the household", async () => {
    expect(bunker.connects).toBe(0); // nothing dialled on load without an approval
    expect(ctx!.guardianKeys.state.root).toBeUndefined();
    await act(async () => {
      await ctx!.guardianKeys.approveInstall({ label: "Parent's phone" });
    });
    expect(bunker.connects).toBe(1);
    const rootRec = ctx!.guardianKeys.state.root!;
    expect(rootRec.pubkey).toBe(R);
    expect(rootRec.approval.tags).toContainEqual(["d", `kintrinsic/device/${W}`]);
    expect(ctx!.guardianKeys.rootPubkey()).toBe(R);
    expect(ctx!.guardianKeys.workingPubkey).toBe(W);
    expect(ctx!.state.signer).toMatchObject({ connected: true, kind: "signet" });
    const saved = JSON.parse(localStorage.getItem(STORAGE_KEY)!);
    expect(saved.guardianKeys.root.pubkey).toBe(R);
  });

  it("refuses to connect an identity other than the invited one, and approves nothing", async () => {
    const invited = "b2".repeat(32);
    await act(async () => {
      await expect(ctx!.pairSignet("bunker://y?relay=wss://bunker.example", { expectRoot: invited })).rejects.toThrow(
        /isn't the one the other parent invited/,
      );
    });
    expect(ctx!.guardianKeys.state.root).toBeUndefined();
    // The refused link is not kept.
    expect(ctx!.state.signer.bunkerUri).toBe("bunker://x?relay=wss://bunker.example");
  });
});

describe("approving this phone on a My Signet family with devices it has never heard from", () => {
  let container: HTMLDivElement;
  let root: Root;
  const machine = "c3".repeat(32);

  function mount(signerKind: "signet" | "local") {
    localStorage.setItem(
      STORAGE_KEY,
      JSON.stringify({
        children: [
          {
            id: "c1",
            name: "Sam",
            color: "#000",
            dependantPubkey: null,
            policies: [],
            devices: [{ id: "d1", label: "Sam's laptop", platform: "linux", pairing: "paired", devicePubkey: machine }],
          },
        ],
        requests: [],
        activity: [],
        signer:
          signerKind === "signet"
            ? { connected: true, kind: "signet", autoSign: false, bunkerUri: "bunker://x?relay=wss://bunker.example" }
            : { connected: true, kind: "local", autoSign: true },
      }),
    );
    container = document.createElement("div");
    document.body.appendChild(container);
    act(() => {
      root = createRoot(container);
      root.render(
        <CharterProvider>
          <Probe />
        </CharterProvider>,
      );
    });
  }

  beforeEach(() => {
    localStorage.clear();
    resetGuardianKeyCacheForTests();
    localStorage.setItem("charter.guardian.key.v1", nip19.nsecEncode(wSk));
  });

  afterEach(() => {
    act(() => root.unmount());
    container.remove();
    localStorage.clear();
    resetGuardianKeyCacheForTests();
  });

  it("a family already on My Signet: the silent device was offered by R, so approving passes", async () => {
    mount("signet");
    await act(async () => {
      await ctx!.guardianKeys.approveInstall({ label: "Parent's phone" });
    });
    expect(ctx!.guardianKeys.state.root?.pubkey).toBe(R);
    expect(ctx!.state.children[0].devices[0].pinnedKey).toBe(R);
  }, 15_000); // ~3 s alone; slower under a full parallel run

  it("a phone that signed with its own key: the silent device pins it, so connecting My Signet doesn't approve", async () => {
    mount("local");
    let result: Awaited<ReturnType<CharterContextValue["pairSignet"]>> | undefined;
    await act(async () => {
      result = await ctx!.pairSignet("bunker://x?relay=wss://bunker.example");
    });
    expect(result).toMatchObject({ approved: false });
    expect(result && !result.approved && result.error).toMatch(/already trust the key on this phone/);
    expect(ctx!.guardianKeys.state.root).toBeUndefined();
    expect(ctx!.state.children[0].devices[0].pinnedKey).toBe(W);
  });
});
