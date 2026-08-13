// The short pairing code both screens show (S6, review 2026-08-07).
//
// THE PROBLEM. `/pair` copies whatever `bunker://…` sits in the URL fragment
// into its "Open the Kintrinsic app" button. Anyone can send a ward a link on the
// legitimate domain —
//
//   https://charter.mysignet.app/pair#bunker://<attacker-pubkey>?relay=wss://…&token=…
//
// — carrying a stranger's guardian identity into the app's pairing intent. The
// domain is real, the page is real, the token is self-minted and proves
// nothing. If the ward taps confirm, the attacker becomes that device's
// guardian. Nothing on the page ever showed WHOSE identity was about to be
// pinned.
//
// WHY A SHORT CODE AND NOT JUST THE npub. The page shows both, but an npub is
// sixty-odd characters of base32 that no child on a phone is going to compare
// against a parent's screen, and a check nobody performs is not a control. Six
// digits, in two groups, is a check a family will actually do out loud — the
// same trick every secure-messaging app uses for exactly this reason.
//
// WHAT IT PROVES. Only that the two screens are talking about the same
// (guardian, token) pair. An attacker's link produces a perfectly consistent
// code of its own; the code is worthless unless it is COMPARED. That is why
// the copy on both sides asks for the comparison rather than presenting the
// digits as a seal of approval.
//
// The `/pair` page is plain static HTML with no bundler, so it carries its own
// copy of this derivation. `pairingSas.test.ts` extracts that copy and runs it
// against the same vectors as this one — a one-sided edit fails the build
// rather than silently showing two different codes.

/** Domain separator — a hash of these inputs must mean this and nothing else. */
const SAS_DOMAIN = "charter-pair-sas:v1";

/**
 * The six-digit pairing code for a (guardian pubkey, one-time token) pair,
 * formatted as `"123 456"`.
 *
 * Deliberately derived from BOTH: the pubkey alone would repeat across every
 * pairing this guardian ever does, so a code seen once (over a shoulder, in a
 * photo of the QR) would be replayable forever.
 */
export async function pairingSas(guardianPubkeyHex: string, token: string): Promise<string> {
  const input = `${SAS_DOMAIN}:${guardianPubkeyHex.toLowerCase()}:${token}`;
  const digest = new Uint8Array(
    await crypto.subtle.digest("SHA-256", new TextEncoder().encode(input)),
  );
  // 24 bits → 0..999999. The modulo bias is ~6% on the largest residues and
  // does not matter: this is a comparison code between two screens, not a
  // secret anyone has to guess.
  const n = ((digest[0] << 16) | (digest[1] << 8) | digest[2]) % 1_000_000;
  const s = String(n).padStart(6, "0");
  return `${s.slice(0, 3)} ${s.slice(3)}`;
}
