// The child's own day-boundary tz, resolved with ONE precedence everywhere a
// guardian-side signer needs "which day is it for them": schedule.tz, then
// budget.tz, then the buckets clause's own tz.
//
// C-1 (hardware round, 2026-08-03): every call site here used to stop at
// schedule/budget, which are both WHOLE-DEVICE clauses. Named times invites a
// ward with NEITHER — a counted/on-request group and nothing else — and that
// ward's buckets clause carries a REQUIRED tz of its own (`BucketsPolicy.tz`),
// never consulted. The result: `realSigner.ts`'s "this child's time zone is
// unknown" refusal fired on EVERY grant for the feature's own minimal
// configuration — a buckets-only ward could never be given more time, by ask
// or by gift. The refusal itself is correct and stays (a guardian phone's own
// tz is not a clause tz and can overshoot the device's end-of-day cap — see
// `wire/grant.ts`'s `pickExtendTz`/`endOfDayUnix` docs); it just has to have
// consulted all three sources before it gives up.
//
// Used for: a gift's expiry (`giveTime`), a stand-down's expiry (`standDown`),
// and an `app.open` "Rest of today" window (`Approvals.tsx`). A `time.extend`
// GRANT's tz is dimension-aware (the LOCKED axis's own tz first) and goes
// through `wire/grant.ts`'s `pickExtendTz` instead — see `decisionTiming.ts`.

import type { Policy } from "./types";

export function resolveChildTz(policy: Policy | undefined): string | undefined {
  return policy?.schedule?.tz ?? policy?.budget?.tz ?? policy?.buckets?.tz;
}
