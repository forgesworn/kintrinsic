// Has the parent saved a backup of their guardian key?
//
// The key lives only in this browser (see guardianKey.ts), so until a backup
// has been exported a lost phone means setting every device up again. This is
// the one small fact the "save a backup" reminder needs.
//
// It is kept on its OWN localStorage key, not inside the persisted household:
// the household is itself part of the backup blob, so a flag stored there would
// always be written *after* the copy that carries it. It records WHICH key was
// backed up (the public key), so replacing the guardian key with a different
// one correctly brings the reminder back.
//
// The "dismiss for this session" choice lives in sessionStorage: it forgets
// itself when the tab closes, so a parent who ignores the reminder is asked
// again next time rather than never.

const BACKED_UP_KEY = "charter.keyBackup.v1";
const DISMISSED_KEY = "charter.keyBackup.nudgeDismissed";

/** Fired on `window` whenever either fact changes, so open screens re-read. */
export const BACKUP_STATUS_EVENT = "charter:key-backup-status";

function read(store: () => Storage, key: string): string | null {
  try {
    return store().getItem(key);
  } catch {
    return null;
  }
}

function write(store: () => Storage, key: string, value: string): void {
  try {
    store().setItem(key, value);
  } catch {
    // Storage unavailable (private mode / quota): the reminder simply stays.
  }
}

function announce(): void {
  try {
    window.dispatchEvent(new Event(BACKUP_STATUS_EVENT));
  } catch {
    /* no window (tests, SSR) — nothing is listening */
  }
}

/** True once a backup of THIS guardian key has been exported. */
export function isKeyBackedUp(pubkeyHex: string): boolean {
  return read(() => localStorage, BACKED_UP_KEY) === pubkeyHex;
}

/** Record that the parent has exported (copied or downloaded) a backup. */
export function markKeyBackedUp(pubkeyHex: string): void {
  write(() => localStorage, BACKED_UP_KEY, pubkeyHex);
  announce();
}

export function isBackupNudgeDismissed(): boolean {
  return read(() => sessionStorage, DISMISSED_KEY) === "1";
}

/** Hide the reminder until the tab is closed. */
export function dismissBackupNudge(): void {
  write(() => sessionStorage, DISMISSED_KEY, "1");
  announce();
}
