import type { Policy, SignerKind, SignerState } from "../domain/types";
import type { UpdateManifest } from "../wire/types";
import type { Signer } from "./Signer";

const DELAY_MS = 450;

const wait = (ms: number) => new Promise<void>((r) => setTimeout(r, ms));

function labelFor(kind: SignerKind): string | undefined {
  switch (kind) {
    case "signet":
      return "Signet";
    case "heartwood":
      return "Heartwood";
    default:
      return undefined;
  }
}

/**
 * confirmGate — how the UI surfaces a manual approval when autoSign is off.
 * Return true to proceed with signing, false to cancel. Defaults to a synchronous
 * `true` (auto-confirm) so non-UI callers/tests don't hang; the app passes a real
 * gate that shows an "Approve in Signet" sheet.
 *
 * `decision` is present iff `action === "decision"` — it's the same
 * "approved" | "denied" the caller passed to `signDecision`, threaded through
 * so the sheet can render a CTA that never says "Approve" for a no (see
 * `domain/signPrompt.ts`). `signerKind` rides alongside `signerLabel` so the
 * gate/prompt can tell "is this the local key" from the signer's actual
 * identity rather than pattern-matching its display label.
 *
 * NOTE: for a `"decision"` action on the LOCAL signer (`signerKind ===
 * "local"`), this gate is never invoked at all — see the skip in each
 * signer's `authorize()`. The Approve/Not now press on the list already IS
 * the confirmation.
 */
export type ConfirmGate = (context: {
  action: "clause" | "decision";
  decision?: "approved" | "denied";
  signerLabel: string;
  signerKind: SignerKind;
  detail: string;
}) => boolean | Promise<boolean>;

export class SignerCancelled extends Error {
  constructor() {
    super("Signing was cancelled.");
    this.name = "SignerCancelled";
  }
}

export interface MockSignerOptions {
  initial?: Partial<SignerState>;
  confirmGate?: ConfirmGate;
  delayMs?: number;
}

/**
 * mockSigner — a believable local stand-in for a real remote signer.
 * Tracks connected/kind/autoSign and resolves after a short delay. When
 * autoSign is false it routes through the confirm gate first.
 */
export class MockSigner implements Signer {
  private state: SignerState;
  private confirmGate: ConfirmGate;
  private delayMs: number;

  constructor(opts: MockSignerOptions = {}) {
    this.state = {
      connected: false,
      kind: "none",
      autoSign: false,
      label: undefined,
      ...opts.initial,
    };
    this.confirmGate = opts.confirmGate ?? (() => true);
    this.delayMs = opts.delayMs ?? DELAY_MS;
  }

  status(): SignerState {
    return { ...this.state };
  }

  async connect(kind: SignerKind): Promise<SignerState> {
    await wait(this.delayMs);
    this.state = {
      ...this.state,
      connected: kind !== "none",
      kind,
      label: labelFor(kind),
    };
    return this.status();
  }

  async disconnect(): Promise<void> {
    await wait(this.delayMs);
    this.state = {
      connected: false,
      kind: "none",
      autoSign: false,
      label: undefined,
    };
  }

  async setAutoSign(on: boolean): Promise<void> {
    this.state = { ...this.state, autoSign: on };
  }

  async signClause(childId: string, policy: Policy): Promise<{ ok: true }> {
    await this.authorize("clause", `Rule change for ${childId} (${policy.id})`);
    return { ok: true };
  }

  // `url` is omitted deliberately: the mock signs nothing, so it has no use
  // for the download location. TypeScript allows an implementation to take
  // fewer parameters than its interface declares.
  async signUpdateClause(childId: string, manifest: UpdateManifest): Promise<{ ok: true }> {
    await this.authorize("clause", `Update Kintrinsic to ${manifest.versionName} for ${childId}`);
    return { ok: true };
  }

  async sendPairOffer(): Promise<{ ok: true }> {
    return { ok: true };
  }

  async releaseDevice(): Promise<{ ok: true }> {
    // No wire in the mock — the store still updates its device list.
    return { ok: true };
  }

  async signDecision(
    requestId: string,
    decision: "approved" | "denied",
  ): Promise<{ ok: true }> {
    await this.authorize("decision", `${decision} request ${requestId}`, decision);
    return { ok: true };
  }

  /**
   * Shared signing path: confirm gate (when manual), then a short delay.
   *
   * A `"decision"` on the LOCAL signer skips the gate entirely — the
   * Approve/Not now press on the request list already showed the child, the
   * request and the amount under a button that said exactly what it would
   * do, so a second "are you sure" sheet adds nothing. `"clause"` (a rule
   * edit) is untouched: a bulk change genuinely benefits from "nothing
   * changes until you confirm", on every signer including the local one.
   */
  private async authorize(
    action: "clause" | "decision",
    detail: string,
    decision?: "approved" | "denied",
  ): Promise<void> {
    const skipGate = action === "decision" && this.state.kind === "local";
    if (!this.state.autoSign && !skipGate) {
      const ok = await this.confirmGate({
        action,
        decision,
        signerLabel: this.state.label ?? "your approval app",
        signerKind: this.state.kind,
        detail,
      });
      if (!ok) throw new SignerCancelled();
    }
    await wait(this.delayMs);
  }
}

/** Convenience factory mirroring how the store constructs its signer. */
export function createMockSigner(opts: MockSignerOptions = {}): Signer {
  return new MockSigner(opts);
}
