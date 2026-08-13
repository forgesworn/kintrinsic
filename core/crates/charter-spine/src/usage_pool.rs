//! Load the freshest broker-verified USAGE_SYNC view for a subject into the
//! enforcer's [`ConsolidatedUsage`] input. Shared by both wardens: the broker
//! stores what `verify_usage_sync` accepted (guardian-pinned, ts-monotonic);
//! this only re-shapes it. Any parse/decode oddity yields `None` — the pool
//! degrades to local-only enforcement, never fails open wider than that.

use charter_proto::{UsageSyncPayload, USAGE_SYNC_STORE_KEY};
use charter_schedule::{ConsolidatedUsage, MinuteSet};
use charter_sys::persistence::ChildClauseStore;
use charter_sys::SystemLayer;

/// The stored consolidated view for `subject_hex`, if any.
pub fn load_consolidated<S: SystemLayer>(sys: &S, subject_hex: &str) -> Option<ConsolidatedUsage> {
    load_consolidated_from(sys.child_clauses(), subject_hex)
}

/// [`load_consolidated`] over a bare store — for wardens (Android) that hold
/// the `ChildClauseStore` directly rather than behind a `SystemLayer`.
pub fn load_consolidated_from(
    store: &impl ChildClauseStore,
    subject_hex: &str,
) -> Option<ConsolidatedUsage> {
    let json = store
        .get_child_clause(subject_hex, USAGE_SYNC_STORE_KEY)
        .ok()
        .flatten()?;
    let payload = UsageSyncPayload::from_json(&json).ok()?;
    Some(ConsolidatedUsage {
        day_key: payload.day_key,
        spent_elsewhere_today_secs: payload.spent_elsewhere_today_secs,
        week_key: payload.week_key,
        spent_elsewhere_week_secs: payload.spent_elsewhere_week_secs,
        // The broker verified the bitmap decodes before storing; decode again
        // defensively — a corrupt store entry degrades to the scalar path.
        elsewhere_minutes_today: payload
            .elsewhere_minutes_today
            .as_deref()
            .and_then(MinuteSet::from_b64url),
    })
}

#[cfg(all(test, feature = "mock"))]
mod tests {
    use super::*;
    use charter_sys::MockSystem;

    const SUBJECT: &str = "cdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcd";

    fn stored(json: &str) -> Option<ConsolidatedUsage> {
        let sys = MockSystem::new(1_000);
        sys.child_clauses()
            .put_child_clause(SUBJECT, USAGE_SYNC_STORE_KEY, 500, json)
            .unwrap();
        load_consolidated(&sys, SUBJECT)
    }

    #[test]
    fn loads_scalars_and_bitmap() {
        let mut bm = MinuteSet::default();
        bm.set(700);
        bm.set(701);
        let json = format!(
            "{{\"v\":1,\"subject\":\"{SUBJECT}\",\"ts\":500,\"dayKey\":\"2026-06-29\",\
             \"spentElsewhereTodaySecs\":1800,\"elsewhereMinutesToday\":\"{}\"}}",
            bm.to_b64url()
        );
        let c = stored(&json).expect("loads");
        assert_eq!(c.day_key, "2026-06-29");
        assert_eq!(c.spent_elsewhere_today_secs, 1800);
        assert_eq!(c.elsewhere_minutes_today.unwrap().count(), 2);
    }

    #[test]
    fn absent_or_corrupt_is_none() {
        let sys = MockSystem::new(1_000);
        assert!(load_consolidated(&sys, SUBJECT).is_none());
        assert!(stored("not json").is_none());
    }
}
