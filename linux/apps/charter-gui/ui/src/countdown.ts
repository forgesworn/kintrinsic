// An interpolating countdown. It ticks locally for a smooth display but HARD
// re-syncs on every `TimeLeftChanged` signal — the daemon is the authority, so
// local drift never accumulates. `-1` means unlimited.

export class Countdown {
  private base = 0;
  private anchorMs = 0;
  private unlimited = false;

  // Re-sync from an authoritative TimeLeft snapshot.
  sync(effectiveSeconds: number, nowMs: number): void {
    this.unlimited = effectiveSeconds < 0;
    this.base = Math.max(0, effectiveSeconds);
    this.anchorMs = nowMs;
  }

  // Remaining seconds interpolated to `nowMs` (-1 = unlimited).
  remaining(nowMs: number): number {
    if (this.unlimited) return -1;
    const elapsed = Math.floor((nowMs - this.anchorMs) / 1000);
    return Math.max(0, this.base - elapsed);
  }
}
