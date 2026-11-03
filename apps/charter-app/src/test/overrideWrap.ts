// A gift-wrapped break-glass audit exactly as the ward builds it (`nip59::wrap`):
// an unsigned kind 31000 rumor inside a machine-signed seal inside a throwaway
// wrap addressed to the guardian. Shared by the wire and provider tests.
import { finalizeEvent, generateSecretKey, getPublicKey, nip44, type NostrEvent } from "nostr-tools";

export interface WrapOpts {
  guardianPk: string;
  /** The machine's key: it signs the seal (and is the rumor author unless `rumorPk` says otherwise). */
  machineSk: Uint8Array;
  kind?: number;
  tags?: string[][];
  /** The rumor's and seal's time; the wrap's too unless `wrapAt` says otherwise. */
  createdAt?: number;
  wrapAt?: number;
  /** Seal signer, when it should not be the machine. */
  sealSk?: Uint8Array;
  /** Rumor author, when it should not be the machine. */
  rumorPk?: string;
}

/** The tags the ward emits for an emergency unlock, plus any extras (e.g. `at`). */
export const overrideTags = (extra: string[][] = []): string[][] => [
  ["t", "charter-device"],
  ["outcome", "override"],
  ["op", "unlock.breakglass"],
  ["scope", "full"],
  ["durationSecs", "300"],
  ...extra,
];

export function overrideWrap(o: WrapOpts): NostrEvent {
  const createdAt = o.createdAt ?? Math.floor(Date.now() / 1000);
  const sealSk = o.sealSk ?? o.machineSk;
  const rumor = {
    kind: o.kind ?? 31000,
    pubkey: o.rumorPk ?? getPublicKey(o.machineSk),
    created_at: createdAt,
    tags: o.tags ?? overrideTags(),
    content: "",
  };
  const seal = finalizeEvent(
    {
      kind: 13,
      created_at: createdAt,
      tags: [],
      content: nip44.encrypt(JSON.stringify(rumor), nip44.getConversationKey(sealSk, o.guardianPk)),
    },
    sealSk,
  );
  const eph = generateSecretKey();
  return finalizeEvent(
    {
      kind: 1059,
      created_at: o.wrapAt ?? createdAt,
      tags: [["p", o.guardianPk]],
      content: nip44.encrypt(JSON.stringify(seal), nip44.getConversationKey(eph, o.guardianPk)),
    },
    eph,
  );
}
