// The feel of the pull-to-refresh gesture, kept as pure numbers so it can be
// tuned and tested without a touchscreen.

/** How far the finger travel is allowed to move the content, in px. */
export const PULL_MAX = 96;
/** Pull past this and releasing refreshes. */
export const PULL_THRESHOLD = 64;
/** Rubber-band factor: the content follows the finger at half speed, so the
 *  gesture feels resisted rather than loose. */
export const PULL_DAMPING = 0.5;

/** Finger travel → content offset. */
export function pullOffset(dy: number): number {
  if (dy <= 0) return 0;
  return Math.min(PULL_MAX, dy * PULL_DAMPING);
}

/** Whether releasing at this offset triggers a refresh. */
export function willRefresh(offset: number): boolean {
  return offset >= PULL_THRESHOLD;
}
