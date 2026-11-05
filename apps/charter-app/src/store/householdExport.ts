// What the household backup carries of the persisted state. The state is the
// app's own record and stays whole on this phone; the copy that leaves it in
// a backup drops what must never travel:
// - the guardian PIN record (salt and scrypt hash) kept while a PIN is still
//   being sent to a device: a six-digit PIN's hash is cheap to search
//   offline, and the PIN is the only lock on presence at an Android ward.
//   Once every device reports `pinSet` the record is gone anyway.

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
