// One relay round-trip fetching the latest verified release announcement for
// every channel. This is the D2 replacement for the origin JSON feeds — the
// callers keep the origin fetch as a fallback until D3 retires it.
//
// Fail-quiet like updateCheck: any relay trouble resolves to nulls within the
// deadline; offline must stay calm.

import { SimplePool } from "nostr-tools";
import { latestRelease, type ReleaseManifest } from "./releaseEvent";
import {
  RELEASE_CHANNELS,
  RELEASE_PUBKEY_HEX,
  RELEASE_RELAYS,
  SOFTWARE_RELEASE_KIND,
} from "./releaseTrust";

/** Injectable relay query (tests feed events without a network). */
export type ReleaseQuery = (relays: string[], filter: object) => Promise<unknown[]>;

/** How long a relay round may take before we shrug and fall back. */
export const RELEASE_QUERY_DEADLINE_MS = 8_000;

async function poolQuery(relays: string[], filter: object): Promise<unknown[]> {
  const pool = new SimplePool();
  try {
    return await pool.querySync(relays, filter as never, {
      maxWait: RELEASE_QUERY_DEADLINE_MS,
    });
  } finally {
    pool.close(relays);
  }
}

export interface ReleaseManifests {
  /** charter-apk — the ward enforcer APK. */
  ward: ReleaseManifest | null;
  /** mycharter-apk — the guardian carrier APK. */
  carrier: ReleaseManifest | null;
  /** charter-deb — the Linux warden. */
  deb: ReleaseManifest | null;
}

/**
 * Query the release relays once and split per channel. Only events signed by
 * the pinned release key survive; the query's author filter is a bandwidth
 * hint, never the trust boundary.
 */
export async function fetchAllReleaseManifests(
  q: ReleaseQuery = poolQuery,
  pinnedPk?: string,
): Promise<ReleaseManifests> {
  const none: ReleaseManifests = { ward: null, carrier: null, deb: null };
  let events: unknown[];
  try {
    const deadline = new Promise<unknown[]>((resolve) =>
      setTimeout(() => resolve([]), RELEASE_QUERY_DEADLINE_MS + 1_000),
    );
    events = await Promise.race([
      q(RELEASE_RELAYS, {
        kinds: [SOFTWARE_RELEASE_KIND],
        authors: [pinnedPk ?? RELEASE_PUBKEY_HEX],
        "#d": [...RELEASE_CHANNELS],
      }),
      deadline,
    ]);
  } catch {
    return none;
  }
  return {
    ward: latestRelease(events, "charter-apk", pinnedPk),
    carrier: latestRelease(events, "mycharter-apk", pinnedPk),
    deb: latestRelease(events, "charter-deb", pinnedPk),
  };
}
