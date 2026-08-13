// What the confirm sheet says, which depends on WHERE the approval happens
// AND, for a request decision, WHICH way it went.
//
// "Approve in Signet" is an instruction — leave this app, approve in that one.
// Applied to the local key it became "Approve in This phone": it told a parent
// to go somewhere they were already standing, with a capital T mid-sentence
// (decented, 2026-07-28: "it does not feel right"). When the key is here, the
// sheet just asks.
//
// A second problem (2026-08-04): the sheet's call to action was hard-coded to
// "Approve" no matter which button on the request list raised it — so denying
// a child's request required pressing a button labelled "Approve". A local
// decision now skips this sheet entirely (see mockSigner.ts / realSigner.ts's
// `authorize`); when an EXTERNAL signer still raises it, the CTA must say
// "Deny", never "Approve", for a denial.

import type { SignerKind } from "./types";

export interface SignPrompt {
  title: string;
  cta: string;
}

/**
 * Is this the signer that signs on THIS device? Prefer the signer's `kind`
 * (`"local"`, set by `selectSigner.ts` / `realSigner.ts`'s `labelFor`) over
 * its display `label` — a label is user/product-facing text, not an identity
 * signal, and comparing against it broke once already (see the header above).
 */
function isLocal(signerKind: SignerKind): boolean {
  return signerKind === "local";
}

export function signPrompt(
  signerKind: SignerKind,
  signerLabel: string,
  action: "clause" | "decision",
  decision?: "approved" | "denied",
): SignPrompt {
  const what = action === "decision" ? "your decision" : "this change";
  const verb = action === "decision" && decision === "denied" ? "Deny" : "Approve";
  if (isLocal(signerKind)) {
    return { title: `Confirm ${what}`, cta: verb };
  }
  return { title: `Confirm in ${signerLabel}`, cta: `${verb} in ${signerLabel}` };
}
