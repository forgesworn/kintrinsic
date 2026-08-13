import { useCharter } from "../store/store";
import { signPrompt } from "../domain/signPrompt";
import { Button } from "./ui";

/**
 * SignSheet — the manual-approval confirm step.
 *
 * Shown only when auto-sign is off and the parent triggers a signed action
 * (a rule change or a request decision). It surfaces the "Approve in <signer>"
 * step the Signer contract requires. Copy stays jargon-free: the parent is
 * confirming *their own* decision, not approving something on the child's behalf.
 *
 * It also SAYS WHAT IS BEING SIGNED (S9, review 2026-08-07). The store has
 * always threaded a `detail` through — "Release device ab12cd34…", "Give 15
 * more minutes to Rook" — and this sheet used to drop it on the floor, so a
 * confirm dialog for the single most destructive wire action Kintrinsic has
 * (RELEASE: un-manage a device entirely) read exactly like a confirm dialog
 * for changing bedtime. "Nothing changes until you confirm" is only a
 * meaningful promise if the parent can see what the something is.
 */
export function SignSheet() {
  const { pendingSignature, resolveSignature } = useCharter();
  if (!pendingSignature) return null;

  // What the sheet says depends on WHERE the approval happens (an external
  // signer is somewhere to go, the local key is already here) and, for a
  // request decision, which way it went — a denial must never say "Approve".
  const prompt = signPrompt(
    pendingSignature.signerKind,
    pendingSignature.signerLabel,
    pendingSignature.action,
    pendingSignature.decision,
  );

  return (
    <div
      className="sheet-backdrop"
      role="dialog"
      aria-modal="true"
      aria-label="Confirm"
      onClick={() => resolveSignature(false)}
    >
      <div className="sheet" onClick={(e) => e.stopPropagation()}>
        <h2 className="card-title">{prompt.title}</h2>
        {pendingSignature.detail && (
          <p
            style={{
              margin: "8px 0 0",
              padding: "10px 12px",
              borderRadius: 8,
              background: "var(--surface-2, rgba(0,0,0,0.04))",
              fontWeight: 600,
              wordBreak: "break-word",
            }}
          >
            {pendingSignature.detail}
          </p>
        )}
        <p className="card-sub" style={{ margin: "12px 0 18px" }}>
          Nothing changes until you confirm.
        </p>
        <div className="stack">
          <Button block onClick={() => resolveSignature(true)}>
            {prompt.cta}
          </Button>
          <Button variant="secondary" block onClick={() => resolveSignature(false)}>
            Cancel
          </Button>
        </div>
      </div>
    </div>
  );
}
