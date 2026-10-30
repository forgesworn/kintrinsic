//! Golden vectors for tagged-window schedule evaluation (§Times). The producer
//! is `scripts/gen-schedule-vectors.mjs`, which carries its own per-minute
//! reference evaluator; this side proves the Rust port (boundary search over
//! the same windows) agrees on every instant: which time is in force, where
//! its contiguous span ends, what a locked span waits for, what starts next,
//! and the ward's default ask type.

use charter_proto::GrantTimes;
use charter_schedule::{GrantSchedule, LockCause, TimedStatus};
use charter_spine::times::{default_ask_time, evaluate_times, next_window};
use serde_json::Value;

#[test]
fn tagged_window_vectors_agree_with_the_reference() {
    let file: Value = charter_testkit::golden::load_json("schedule/times_schedule_vectors.json");
    let vectors = file["vectors"].as_array().expect("vectors array");
    assert!(vectors.len() >= 20, "suspiciously few vectors");

    for v in vectors {
        let name = v["name"].as_str().unwrap_or("?");
        let sched: GrantSchedule = serde_json::from_value(v["schedule"].clone())
            .unwrap_or_else(|e| panic!("{name}: schedule: {e}"));
        // `null`, or a body that does not validate (v:2), is ABSENT.
        let times = if v["times"].is_null() {
            None
        } else {
            GrantTimes::from_value(&v["times"]).ok()
        };
        let now = v["now_unix"].as_i64().unwrap();
        let expect = &v["expect"];

        let got = evaluate_times(&sched, times.as_ref(), now);
        match expect["state"].as_str().unwrap() {
            "unbounded" => assert_eq!(got, TimedStatus::Unbounded, "{name}"),
            "open" => assert_eq!(
                got,
                TimedStatus::Open {
                    time: expect["time"].as_str().map(str::to_string),
                    span_end: expect["spanEnd"].as_i64().unwrap(),
                },
                "{name}"
            ),
            "locked" => {
                let (cause, secs) = match got {
                    TimedStatus::Locked {
                        cause,
                        seconds_to_open,
                    } => (cause, seconds_to_open),
                    other => panic!("{name}: expected locked, got {other:?}"),
                };
                let want_cause = match expect["cause"].as_str().unwrap() {
                    "paused" => LockCause::Paused,
                    "outsideWindows" => LockCause::OutsideWindows,
                    "unknownTime" => {
                        LockCause::UnknownTime(expect["unknownTime"].as_str().unwrap().to_string())
                    }
                    other => panic!("{name}: cause {other}"),
                };
                assert_eq!(cause, want_cause, "{name}: cause");
                assert_eq!(
                    secs,
                    expect["secondsToOpen"].as_u64(),
                    "{name}: secondsToOpen"
                );
            }
            other => panic!("{name}: state {other}"),
        }

        // The remaining helpers are only meaningful for a bounded schedule.
        let nw = next_window(&sched, times.as_ref(), now);
        let want = &v["nextWindow"];
        if want.is_null() {
            assert_eq!(nw, None, "{name}: nextWindow");
        } else {
            let nw = nw.unwrap_or_else(|| panic!("{name}: expected a nextWindow"));
            assert_eq!(nw.time.as_deref(), want["time"].as_str(), "{name}: nw.time");
            assert_eq!(
                nw.start,
                want["start"].as_i64().unwrap(),
                "{name}: nw.start"
            );
            assert_eq!(nw.start_hhmm, want["startHhmm"].as_str().unwrap(), "{name}");
            assert_eq!(
                nw.day_offset as u64,
                want["dayOffset"].as_u64().unwrap(),
                "{name}"
            );
        }
        assert_eq!(
            default_ask_time(&sched, times.as_ref(), now).as_deref(),
            v["defaultAsk"].as_str(),
            "{name}: defaultAsk"
        );
    }
}
