// Small presentation helpers for the asks inbox — turning a raw bucketId
// into the words a guardian reads, joined against the saved policy so a
// group's LABEL (not its wire id) is what shows.

import type { AppBucketRule } from "./types";

/**
 * The group label a bucket-hit `time.extend` ask is about, joined against the
 * saved policy's counted groups. A group that no longer exists by the time
 * the ask is answered (renamed, merged, deleted since the device sent it)
 * still renders — by its raw id — rather than vanish the card or blank the
 * heading. Returns `undefined` only when the ask itself carries no bucketId
 * (not a bucket ask at all).
 */
export function bucketGroupLabel(
  bucketId: string | undefined,
  buckets: AppBucketRule[] | undefined,
): string | undefined {
  if (!bucketId) return undefined;
  return buckets?.find((b) => b.id === bucketId)?.label ?? bucketId;
}
