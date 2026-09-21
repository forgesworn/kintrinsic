// Which of a child's devices must be sent a RELEASE before the child's record
// is deleted. Removing the child drops the only copy of each `devicePubkey`,
// and with it any way to ever reach that device again — so every device that
// still has one is released first, whatever its pairing label says (a pubkey
// on a "pending" row is still a ward that may be enforcing a charter).
export function devicesToRelease(
  child: { devices: { id: string; devicePubkey?: string | null }[] } | undefined,
): string[] {
  return (child?.devices ?? [])
    .filter((d) => typeof d.devicePubkey === "string" && /^[0-9a-f]{64}$/.test(d.devicePubkey))
    .map((d) => d.id);
}
