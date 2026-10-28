// The ward app's release feed, read fresh at the moment a setup QR is shown
// so the code always names the current release (docs/qr-setup.md). Kept apart
// from `apk.ts` (the cable path's APK download) on purpose.

import { MANIFEST_PATH } from "../store/updateCheck";

/** Fetch `/charter-apk.json` from this app's own origin, bypassing caches.
 *  Throws on a network failure or a non-2xx answer; the shape is checked by
 *  `buildQrPayload`, which refuses a feed with no certificate. */
export async function fetchApkFeed(fetchFn: typeof fetch = fetch): Promise<unknown> {
  const resp = await fetchFn(MANIFEST_PATH, { cache: "no-store" });
  if (!resp.ok) throw new Error(`The release feed could not be read (${resp.status}).`);
  return (await resp.json()) as unknown;
}
