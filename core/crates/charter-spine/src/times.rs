//! Times (§Times) as the wardens call them: the `Option<&GrantTimes>` face of
//! the schedule evaluation, plus the pure composition, in one place.
//!
//! The evaluation core lives in `charter_schedule::times` (which takes a
//! `known` predicate and so needs no `charter-proto`); the composition lives in
//! `charter_proto::times` (the only crate that sees `GrantApps` and
//! `GrantContent` together). This module joins them so a warden imports one
//! path.
//!
//! # Fail direction
//!
//! `times: None` means "no usable `times` clause" — never defined, unparseable,
//! or `v` too high (see [`GrantTimes::parse_or_absent`]). Every tagged window
//! then LOCKS and untagged windows are unaffected. A named window is a
//! restriction relative to Free time; opening it as Free would grant exactly
//! what the guardian meant to withhold.

use charter_schedule::{GrantSchedule, NextWindow, TimedStatus};

pub use charter_proto::{
    compose_apps, compose_content, GrantTimes, NamedTime, TimePosture, FREE_TIME_ID,
};
pub use charter_schedule::LockCause;

fn known_in(times: Option<&GrantTimes>) -> impl Fn(&str) -> bool + '_ {
    move |id| times.is_some_and(|t| t.get(id).is_some())
}

/// Which time is the device in at `now_unix`? See `charter_schedule::times`.
///
/// Every [`TimedStatus::Locked`] is `lockReason: 'schedule'` on the wire;
/// [`LockCause::UnknownTime`] carries the tag so the ward can be told "Nothing
/// is set up for this time yet".
pub fn evaluate_times(
    sched: &GrantSchedule,
    times: Option<&GrantTimes>,
    now_unix: i64,
) -> TimedStatus {
    charter_schedule::evaluate_times(sched, &known_in(times), now_unix)
}

/// The next runnable span to start today or tomorrow, for "Locked until
/// Learning time at 09:00". `None` ⇒ "Locked today".
pub fn next_window(
    sched: &GrantSchedule,
    times: Option<&GrantTimes>,
    now_unix: i64,
) -> Option<NextWindow> {
    charter_schedule::next_window(sched, &known_in(times), now_unix)
}

/// The ward's default "ask for more" type when NOT inside a window: the time
/// whose window most recently ended today, else the next one up today, else
/// Free (`None`). The in-window half of the rule ("that window's time") is the
/// caller's, from [`evaluate_times`]; see the schedule crate's doc.
pub fn default_ask_time(
    sched: &GrantSchedule,
    times: Option<&GrantTimes>,
    now_unix: i64,
) -> Option<String> {
    charter_schedule::default_ask_time(sched, &known_in(times), now_unix)
}
