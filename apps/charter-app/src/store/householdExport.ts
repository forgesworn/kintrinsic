// What the household backup carries of the persisted state. The state is the
// app's own record and stays whole on this phone; the copy that leaves it in
// a backup drops what must never travel:
// - the guardian PIN record (salt and scrypt hash) kept while a PIN is still
//   being sent to a device: a six-digit PIN's hash is cheap to search
//   offline, and the PIN is the only lock on presence at an Android ward.
//   Once every device reports the PIN in `pinSetAt` (or a newer one) the
//   record is gone anyway.

/** The persisted state JSON as a backup should carry it; unusable JSON is passed through unchanged. */
export function householdForBackup(stateJson: string | null): string | null {
  if (!stateJson) return stateJson;
  let parsed: unknown;
  try {
    parsed = JSON.parse(stateJson);
  } catch {
    return stateJson;
  }
  if (typeof parsed !== "object" || parsed === null || Array.isArray(parsed)) return stateJson;
  const state = parsed as { guardianKeys?: { pins?: Record<string, Record<string, unknown>> } };
  const pins = state.guardianKeys?.pins;
  if (!pins || typeof pins !== "object") return stateJson;
  const stripped: Record<string, Record<string, unknown>> = {};
  for (const [machine, p] of Object.entries(pins)) {
    if (typeof p !== "object" || p === null) continue;
    const { record: _record, ...rest } = p;
    void _record;
    stripped[machine] = rest;
  }
  return JSON.stringify({ ...state, guardianKeys: { ...state.guardianKeys, pins: stripped } });
}

/**
 * This identity's working keys as a backup lists them (public keys and
 * labels, never secrets): this install's own W, when it isn't the identity
 * itself, and every other key the devices list under the identity.
 */
export function backupWorkingKeys(opts: {
  root: string;
  workingPubkey: string | null;
  installLabel?: string;
  reported: readonly { root: string; keys: readonly string[] }[];
}): { pubkey: string; label?: string }[] {
  const out = new Map<string, { pubkey: string; label?: string }>();
  if (opts.workingPubkey && opts.workingPubkey !== opts.root) {
    out.set(opts.workingPubkey, { pubkey: opts.workingPubkey, ...(opts.installLabel ? { label: opts.installLabel } : {}) });
  }
  for (const r of opts.reported) {
    if (r.root !== opts.root) continue;
    for (const k of r.keys) if (!out.has(k)) out.set(k, { pubkey: k });
  }
  return [...out.values()];
}

/**
 * A backup's household with the restore recorded in it (`guardianKeys.
 * restoredFrom`), so the phone can offer to withdraw the lost phone's key once
 * it boots from it. Unusable JSON is passed through unchanged.
 */
export function withRestoreNote(stateJson: string, workingKeys: readonly { pubkey: string; label?: string }[], at: number): string {
  let parsed: unknown;
  try {
    parsed = JSON.parse(stateJson);
  } catch {
    return stateJson;
  }
  if (typeof parsed !== "object" || parsed === null || Array.isArray(parsed)) return stateJson;
  const state = parsed as { guardianKeys?: Record<string, unknown> };
  const gk = typeof state.guardianKeys === "object" && state.guardianKeys !== null ? state.guardianKeys : { v: 1 };
  return JSON.stringify({ ...state, guardianKeys: { ...gk, restoredFrom: { at, workingKeys: [...workingKeys] } } });
}
