import type { SignerKind, SignerState } from "../domain/types";
import type { Signer } from "./Signer";

const NOT_SET = "Turn on parent approval first.";

/**
 * The signer before parent approval is set up, in a real build: it reports
 * itself disconnected and refuses every signature. (The demo mock signer,
 * which pretends to sign, is for development only; see `selectSigner`.)
 * Every caller already checks `status().connected` before signing; this is
 * the backstop so nothing can ever claim a change was signed when it wasn't.
 */
export function createUnsetSigner(initial: SignerState): Signer {
  let state: SignerState = { connected: false, kind: "none", autoSign: initial.autoSign };
  const refuse = async (): Promise<never> => {
    throw new Error(NOT_SET);
  };
  return {
    status: () => state,
    connect: async (kind: SignerKind) => {
      void kind;
      throw new Error(NOT_SET);
    },
    disconnect: async () => {
      state = { connected: false, kind: "none", autoSign: state.autoSign };
    },
    setAutoSign: async (on: boolean) => {
      state = { ...state, autoSign: on };
    },
    signClause: refuse,
    signUpdateClause: refuse,
    signDebuggingClause: refuse,
    releaseDevice: refuse,
    sendPairOffer: refuse,
    signDecision: refuse,
  };
}
