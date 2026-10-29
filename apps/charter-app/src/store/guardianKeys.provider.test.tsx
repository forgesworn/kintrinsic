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
});
