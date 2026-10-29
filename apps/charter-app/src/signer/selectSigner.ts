import type { Policy, SignerState } from "../domain/types";
import type { ChildTarget } from "./realSigner";
import type { Signer } from "./Signer";
import { createMockSigner, type ConfirmGate } from "./mockSigner";
import { createSignetSigner } from "./signetSigner";
import { createLocalSigner } from "./localSigner";
import { createUnsetSigner } from "./unsetSigner";
import type { RealSignerDeps } from "./realSigner";
import type { LocalRouteHooks } from "./routePublish";

export interface SelectSignerDeps {
  resolveChild: (childId: string) => ChildTarget | undefined;
  /** The child's current policy list (app-scope aggregation — see RealSigner). */
  childPolicies: (childId: string) => Policy[];
  confirmGate: ConfirmGate;
  /** Home-network delivery bookkeeping, shared by both real signers. */
  localRoute?: LocalRouteHooks;
  /** Guardian roots and working keys, for the Signet signer (see SignetSignerConfig). */
  keys?: {
    knownRoot: () => string | undefined;
    workingKey: () => Uint8Array;
    pickOps: NonNullable<RealSignerDeps["pickOps"]>;
  };
  /** Fall back to the demo mock signer (development only). Defaults to `import.meta.env.DEV`. */
  allowMock?: boolean;
}

/**
 * Pick the concrete Signer for a persisted signer state:
 *   kind "local"  → the local-key self-signer (this phone holds the key)
 *   bunkerUri set  → the Signet NIP-46 bunker signer
 *   otherwise      → nothing set up: a disconnected signer that refuses to
 *                    sign (the demo mock only in development)
 * `kind` "local" is checked FIRST so it never carries a bunkerUri.
 */
export function selectSigner(signerState: SignerState, deps: SelectSignerDeps): Signer {
  if (signerState.kind === "local") {
    return createLocalSigner({
      resolveChild: deps.resolveChild,
      childPolicies: deps.childPolicies,
      confirmGate: deps.confirmGate,
      localRoute: deps.localRoute,
      // Start disconnected (mount effect reconnects), but carry autoSign forward.
      initial: { connected: false, kind: "local", autoSign: signerState.autoSign },
    });
  }
  if (signerState.bunkerUri) {
    return createSignetSigner({
      getBunkerUri: async () => signerState.bunkerUri!,
      resolveChild: deps.resolveChild,
      childPolicies: deps.childPolicies,
      localRoute: deps.localRoute,
      ...(deps.keys ?? {}),
    });
  }
  if (deps.allowMock ?? import.meta.env.DEV) {
    return createMockSigner({ initial: signerState, confirmGate: deps.confirmGate });
  }
  return createUnsetSigner(signerState);
}
