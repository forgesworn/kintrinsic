// The Charter lock model. The ONLY thing that can dismiss the lock is a daemon
// `LockStateChanged{locked:false}` signal — no web action can unlock it. (The
// real un-dismissability is enforced by the daemon's cgroup freeze + VT-disable
// + input grab; the web layer must never *believe* it can unlock.)

export class LockState {
  private locked = false;
  private reason: string | undefined;

  isLocked(): boolean {
    return this.locked;
  }

  currentReason(): string | undefined {
    return this.reason;
  }

  // A web action (button, key, close) attempting to dismiss the lock. ALWAYS a
  // no-op — returns false and never changes the locked state.
  requestDismissFromWeb(): boolean {
    return false;
  }

  // The ONLY mutator: the daemon's authoritative LockStateChanged signal.
  onLockStateChanged(locked: boolean, reason?: string): void {
    this.locked = locked;
    this.reason = locked ? reason : undefined;
  }
}
