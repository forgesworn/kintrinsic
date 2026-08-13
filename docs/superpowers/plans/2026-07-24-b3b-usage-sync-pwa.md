# B3b — MyCharter: usage aggregation, USAGE_SYNC publish, union weekly picture

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** MyCharter ingests each device's `activeMinutesToday` from STATUS, keeps per-minute history, publishes each device a guardian-signed USAGE_SYNC (31115) carrying the union-of-elsewhere, and renders the weekly picture from union totals with the "both at once" overlap visible.

**Architecture:** PWA-only (`apps/charter-app`). A TS `MinuteSet` mirrors the Rust codec byte-for-byte (frozen vectors pin both). `UsageHistory` grows a parallel per-day bitmap map. The publisher rides the existing 30s STATUS poll tick, change-gated per device. Wire uses the existing `giftWrapSignedInner` guardian envelope. Local-signer path only (same guard as the STATUS intake, store.tsx:937).

**Tech Stack:** TypeScript, vitest, nostr-tools; vectors from `core/crates/charter-testkit/vectors/usage_sync/`.

## Global Constraints
- Bitmap identical to Rust: 180 bytes LSB-first, base64url no-pad, 240 chars, strict decode.
- `UsageSyncPayload` camelCase, `v:1`, per contract §USAGE_SYNC (incl. `elsewhereMinutesToday`).
- `spentElsewhereTodaySecs` for device D = Σ over other devices of that child, same dayKey; `elsewhereMinutesToday` = union of the OTHER devices' bitmaps. Receiver excluded — no double-count.
- `ts` = wall seconds at publish; receivers enforce strictly-monotonic per subject (never send two in the same second for a subject: gate ≥1s).
- Chart: a day's total = |union across devices|·60 when every device with usage that day has a bitmap; else scalar sum (honest fallback). Overlap = Σ per-device minutes − union (shown as "both at once").
- Publish only when child has ≥2 devices with real (hex) devicePubkeys; throttle: only when a device's outbound view changed AND ≥60s since that device's last send.
- Gates: `npx tsc --noEmit`, `npx vitest run`, `npm run build` in apps/charter-app.

### Task 1: TS MinuteSet + vectors
Create `src/insights/minuteSet.ts`: `emptyMinutes(): Uint8Array(180)`, `setMinute(bits, m)`, `minuteCount(bits): number`, `unionMinutes(a,b): Uint8Array`, `encodeMinutes(bits): string`, `decodeMinutes(s): Uint8Array|null` (strict 240/alphabet), `isEmptyMinutes(bits)`. Test `src/insights/minuteSetVectors.test.ts` loads `../../core/crates/charter-testkit/vectors/usage_sync/minute_set_vectors.json` (readFileSync pattern of tetheringClauseVectors.test.ts): encode(set(minutes)) === pinned b64url; decode roundtrip; unions match unionCount (incl. aB64url/bB64url pins); every invalid → null. Commit `feat(app): TS MinuteSet — byte-identical to Rust (frozen vectors)`.

### Task 2: STATUS parse + history bitmaps
`src/wire/status.ts`: `DeviceStatus.activeMinutesToday?: string`; in `parseStatus`, accept only strings that `decodeMinutes` validates (else leave undefined — display never trusts junk). `src/insights/usageHistory.ts`: `UsageHistory` gains `minutesByMachine?: Record<machine, Record<dayKey, string>>`; `recordUsage(h, machine, dayKey, usedSecs, minutesB64?)` merges bitmaps by UNION (devices are authoritative but polls may interleave days), same same-ref no-op discipline; prune covers it. `weeklyView` computes per-day: if every device with secs>0 that day has a bitmap → `totalSecs = |union|*60`, `overlapSecs = (Σ per-device |bits|)*60 − totalSecs`; else scalar sum, overlap 0. `DayUsage.overlapSecs: number`. store.tsx `ingestDeviceStatus` passes `status.activeMinutesToday`. Extend `usageHistory.test.ts`: union day total counts simultaneous once; overlap computed; mixed (one device no bitmap) falls back to scalar sum; bitmap merge is union across polls. Commit.

### Task 3: wire 31115 + publisher
`src/wire/giftwrap.ts`: `export const CHARTER_DEVICE_USAGE_SYNC = 31115;` + `giftWrapUsageSync(payload: UsageSyncPayload, devicePubkey, guardian, createdAt)` via `giftWrapSignedInner`. `src/wire/usageSync.ts`: `interface UsageSyncPayload` (camelCase, contract fields) + `buildUsageSyncForDevice(child devices' day data, receiverMachine, subject, ts, dayKey): UsageSyncPayload` (Σ others' secs, union others' bitmaps). Vector test `src/wire/usageSyncVectors.test.ts` against `usage_sync_vectors.json`: for each `vectors[i].payload`, building from decomposed inputs OR at minimum: TS type serializes deep-equal (assert a constructed payload object equals pinned JSON); invalid entries rejected by a `parseUsageSync` guard if added — keep to producer assertions: construct each pinned payload via builder inputs embedded in the test. `contractConstants.test.ts`: assert 31115. `realSigner.ts`: `async publishUsageSyncs(childId, payloads: Map<devicePubkey, UsageSyncPayload>)` — same connect + fan-out shape as signClause but one distinct payload per device. store.tsx: after the poll tick ingests statuses, for each child with ≥2 real devices build per-device payloads from usageHistory (today, local dayKey per device's last STATUS dayKey — use the CHILD's devices' latest known dayKey; when they disagree (tz), skip the mismatching device this round), change-gate (JSON identity vs `lastUsageSyncSent` ref map) + 60s per-device floor, then `signer.publishUsageSyncs`. Local-signer only. Commit.

### Task 4: union chart + gates + docs
`WeeklyPicture.tsx`: day bar = union total; when `overlapSecs > 0` show a thin darker cap segment at the top of the stacked bar (legend: "both at once") — device stacking now draws each device's EXCLUSIVE minutes (device secs scaled so the stack sums to the union total: scale factor union/Σ) — keep it simple: stack devices proportionally to their journal minutes scaled to union, overlap cap drawn last. Reflective line unchanged. Playwright smoke with injected bitmap history (two devices, overlapping minutes) → screenshot shows the overlap cap + union total. Full gates. Update memo (B3b landed) + contract coordination note (aggregator implemented, PWA). Commit + push.

## Self-review
Types named once: `MinuteSet` funcs, `UsageSyncPayload`, `buildUsageSyncForDevice`, `publishUsageSyncs`, `DayUsage.overlapSecs`, `UsageHistory.minutesByMachine`. Fallbacks: no-bitmap devices → scalar; tz-mismatched dayKey → skip device that round; malformed bitmap → undefined at parse boundary.
