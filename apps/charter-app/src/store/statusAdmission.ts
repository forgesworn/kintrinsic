// Who gets to write into this guardian's storage by heartbeating at it.
//
// Sealing a STATUS to the guardian needs only the guardian's PUBLIC key (see
// ./unclaimedDevices for the long form), so an authentic wrap proves nothing
// about who sent it. `unclaimedDevices` already applies that to ADOPTION; this
// applies it to INGESTION, which used to store every parseable status keyed by
// its self-declared `machine` — an unbounded, persisted map anyone could grow
// by spraying fresh keypairs, until localStorage's quota took the family's
// own state writes down with it.
import type { Child } from "../domain/types";

export type StatusAdmission =
  /** A device some child claims — ingest fully. */
  | "claimed"
  /** Not claimed, but echoing a pairing token THIS guardian minted: the one
   *  proof a stranger cannot forge. Kept as a live status only (so the pairing
   *  flow and the unclaimed-device recovery can see it) — no usage history. */
  | "pairing"
  /** Anyone else. Dropped before anything is recorded. */
  | "stranger";

export function admitStatus(
  status: { machine: string; pairToken?: string | null },
  children: Pick<Child, "devices">[],
  liveTokens: ReadonlySet<string>,
): StatusAdmission {
  for (const c of children) {
    for (const d of c.devices) {
      if (d.devicePubkey && d.devicePubkey === status.machine) return "claimed";
    }
  }
  if (status.pairToken && liveTokens.has(status.pairToken)) return "pairing";
  return "stranger";
}
