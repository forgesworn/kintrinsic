//! JNI surface for the Charter Android warden (port-spec §2).
//!
//! The Rust core owns decide/verify/schedule/crypto, single-sourced with the
//! Linux warden. Kotlin owns DevicePolicyManager enforcement and calls in here.
//! All payloads are UTF-8 JSON; hex is strict lowercase. Never call these from
//! the JVM main thread — every call takes the global warden lock.

mod breakglass;
mod dto;
pub mod install;
pub mod outbox;
pub mod relay;
pub mod system;
mod warden;

use jni::objects::{JClass, JObject, JString};
use jni::sys::{jint, jlong, jstring};
use jni::JNIEnv;

use warden::{Warden, WARDEN};

/// Bump on any breaking change to this surface; Kotlin refuses a mismatch.
pub const ABI_VERSION: jint = 1;

// ---- helpers --------------------------------------------------------------

fn jstr(env: &mut JNIEnv, s: JString) -> Result<String, String> {
    env.get_string(&s)
        .map(|s| s.into())
        .map_err(|e| format!("bad jstring: {e}"))
}

fn opt_jstr(env: &mut JNIEnv, s: JString) -> Option<String> {
    if s.is_null() {
        return None;
    }
    env.get_string(&s).ok().map(|s| s.into())
}

fn out(env: &JNIEnv, s: &str) -> jstring {
    env.new_string(s)
        .map(|s| s.into_raw())
        .unwrap_or(std::ptr::null_mut())
}

fn err_json(msg: &str) -> String {
    format!("{{\"error\":{}}}", serde_json::Value::String(msg.into()))
}

// ---- entry points ---------------------------------------------------------

#[no_mangle]
pub extern "system" fn Java_org_forgesworn_charter_native_CharterNative_charterAbiVersion(
    _env: JNIEnv,
    _class: JClass,
) -> jint {
    ABI_VERSION
}

#[no_mangle]
pub extern "system" fn Java_org_forgesworn_charter_native_CharterNative_charterInit(
    mut env: JNIEnv,
    _class: JClass,
    base_dir: JString,
    enforce_mode: JString,
    app_version_code: jlong,
) -> jstring {
    let base = match jstr(&mut env, base_dir) {
        Ok(s) => s,
        Err(e) => return out(&env, &err_json(&e)),
    };
    let mode = jstr(&mut env, enforce_mode).unwrap_or_else(|_| "enforce".into());
    let mut guard = WARDEN.lock().unwrap_or_else(|e| e.into_inner());
    // Idempotent: never clobber a live warden (e.g. MainActivity relaunching
    // while the service holds the locked enforcer state) — return the existing
    // one. A second init only updates the enforce mode.
    if let Some(w) = guard.as_mut() {
        w.set_enforce_mode(&mode);
        return out(
            &env,
            &serde_json::to_string(&w.init_result()).unwrap_or_default(),
        );
    }
    match Warden::init(&base, &mode, app_version_code.max(0) as u64) {
        Ok(w) => {
            let res = serde_json::to_string(&w.init_result()).unwrap_or_default();
            *guard = Some(w);
            out(&env, &res)
        }
        Err(e) => out(&env, &err_json(&e)),
    }
}

#[no_mangle]
pub extern "system" fn Java_org_forgesworn_charter_native_CharterNative_charterDeviceCode(
    env: JNIEnv,
    _class: JClass,
) -> jstring {
    with_warden(&env, |w| w.device_code())
}

#[no_mangle]
pub extern "system" fn Java_org_forgesworn_charter_native_CharterNative_charterPairingState(
    env: JNIEnv,
    _class: JClass,
) -> jstring {
    with_warden(&env, |w| {
        serde_json::to_string(&w.pairing_state()).unwrap_or_default()
    })
}

#[no_mangle]
pub extern "system" fn Java_org_forgesworn_charter_native_CharterNative_charterSetPairing(
    mut env: JNIEnv,
    _class: JClass,
    guardian_hex: JString,
    subject_hex: JString,
) -> jstring {
    let g = match jstr(&mut env, guardian_hex) {
        Ok(s) => s,
        Err(e) => return out(&env, &err_json(&e)),
    };
    let s = match jstr(&mut env, subject_hex) {
        Ok(s) => s,
        Err(e) => return out(&env, &err_json(&e)),
    };
    with_warden_mut(&env, |w| match w.set_pairing(&g, &s) {
        Ok(()) => serde_json::to_string(&w.pairing_state()).unwrap_or_default(),
        Err(e) => err_json(&e),
    })
}

/// The REAL pairing: paste the guardian's `bunker://` link from MyCharter.
/// Returns the new `PairingState` JSON, or `{"error": …}` with normie text.
#[no_mangle]
pub extern "system" fn Java_org_forgesworn_charter_native_CharterNative_charterPair(
    mut env: JNIEnv,
    _class: JClass,
    bunker_uri: JString,
    now_unix: jlong,
) -> jstring {
    let uri = match jstr(&mut env, bunker_uri) {
        Ok(s) => s,
        Err(e) => return out(&env, &err_json(&e)),
    };
    with_warden_mut(&env, |w| match w.pair(&uri, now_unix as u64) {
        Ok(state) => serde_json::to_string(&state).unwrap_or_default(),
        Err(e) => err_json(&e),
    })
}

/// One slow-tick relay round: pull + authenticate + store CLAUSE wraps, then
/// emit the STATUS heartbeat. Blocking network IO — worker thread only.
#[no_mangle]
pub extern "system" fn Java_org_forgesworn_charter_native_CharterNative_charterPollOnce(
    env: JNIEnv,
    _class: JClass,
    now_unix: jlong,
) -> jstring {
    // Orchestrates its own locking: the relay network phases run with the
    // global warden lock RELEASED, so enforcement ticks are never stalled.
    // catch_unwind here (not with_warden — that would hold the lock): a panic
    // must never unwind across the FFI boundary.
    let res = std::panic::catch_unwind(|| {
        serde_json::to_string(&warden::poll_once_locked(&WARDEN, now_unix as u64))
            .unwrap_or_default()
    })
    .unwrap_or_else(|_| err_json("internal error (panic recovered)"));
    out(&env, &res)
}

/// Submit a brokered ask (op = "time.extend", params JSON). Persists Pending
/// before publishing (M16); returns {"reqId": …} or {"error": …}. Blocking
/// network IO — worker thread only.
#[no_mangle]
pub extern "system" fn Java_org_forgesworn_charter_native_CharterNative_charterSubmitRequest(
    mut env: JNIEnv,
    _class: JClass,
    op: JString,
    params_json: JString,
) -> jstring {
    let op = match jstr(&mut env, op) {
        Ok(s) => s,
        Err(e) => return out(&env, &err_json(&e)),
    };
    let params = match jstr(&mut env, params_json) {
        Ok(s) => s,
        Err(e) => return out(&env, &err_json(&e)),
    };
    with_warden(&env, |w| match w.submit_request(&op, &params) {
        Ok(req_id) => format!("{{\"reqId\":\"{req_id}\"}}"),
        Err(e) => err_json(&e),
    })
}

/// Newest-first request records (`[RequestRecord]` JSON).
#[no_mangle]
pub extern "system" fn Java_org_forgesworn_charter_native_CharterNative_charterListRequests(
    env: JNIEnv,
    _class: JClass,
    limit: jint,
) -> jstring {
    with_warden(&env, |w| {
        serde_json::to_string(&w.list_requests(limit.max(0) as usize)).unwrap_or_default()
    })
}

/// Records for one reqId ("" = all).
#[no_mangle]
pub extern "system" fn Java_org_forgesworn_charter_native_CharterNative_charterQueryStatus(
    mut env: JNIEnv,
    _class: JClass,
    req_id: JString,
) -> jstring {
    let id = jstr(&mut env, req_id).unwrap_or_default();
    with_warden(&env, |w| {
        serde_json::to_string(&w.query_status(&id)).unwrap_or_default()
    })
}

/// Cancel a Pending ask; returns "true" iff it was Pending.
#[no_mangle]
pub extern "system" fn Java_org_forgesworn_charter_native_CharterNative_charterCancelRequest(
    mut env: JNIEnv,
    _class: JClass,
    req_id: JString,
) -> jstring {
    let id = jstr(&mut env, req_id).unwrap_or_default();
    with_warden(&env, |w| w.cancel_request(&id).to_string())
}

/// Every guardian-approved install currently parked (`[PendingInstall]` JSON),
/// oldest-first. Kotlin performs each install (verifying signing continuity)
/// then reports the outcome via `charterInstallResult`.
#[no_mangle]
pub extern "system" fn Java_org_forgesworn_charter_native_CharterNative_charterDrainInstalls(
    env: JNIEnv,
    _class: JClass,
    now_unix: jlong,
) -> jstring {
    with_warden(&env, |w| {
        serde_json::to_string(&w.drain_installs(now_unix as u64)).unwrap_or_default()
    })
}

/// Report an install outcome: `status` ∈ {"ok","terminal","transient"}. `ok`
/// and `terminal` clear the directive; `transient` leaves it for a retry.
#[no_mangle]
pub extern "system" fn Java_org_forgesworn_charter_native_CharterNative_charterInstallResult(
    mut env: JNIEnv,
    _class: JClass,
    req_id: JString,
    status: JString,
    reason: JString,
) -> jstring {
    let id = jstr(&mut env, req_id).unwrap_or_default();
    let status = jstr(&mut env, status).unwrap_or_default();
    let reason = jstr(&mut env, reason).unwrap_or_default();
    with_warden(&env, |w| {
        w.install_result(&id, &status, &reason);
        "{\"ok\":true}".to_string()
    })
}

/// Is a guardian's maintenance window open? Kotlin stands the install lock
/// down while it is, so a cabled phone can be repaired.
#[no_mangle]
pub extern "system" fn Java_org_forgesworn_charter_native_CharterNative_charterMaintenanceOpen(
    env: JNIEnv,
    _class: JClass,
    now: jlong,
) -> jstring {
    with_warden(&env, |w| {
        serde_json::json!({ "open": w.maintenance_open(now.max(0) as u64) }).to_string()
    })
}

/// A stuck update, as JSON, or "" when nothing is failing. Stamped onto STATUS
/// so the guardian sees "couldn't download it, 47 tries" instead of an Update
/// button that silently reappears.
#[no_mangle]
pub extern "system" fn Java_org_forgesworn_charter_native_CharterNative_charterInstallHealth(
    env: JNIEnv,
    _class: JClass,
    now: jlong,
) -> jstring {
    with_warden(&env, |w| w.install_health(now.max(0) as u64))
}

/// The sole ward's standing per-app policy AS ENFORCED AT `now` (`GrantApps`
/// JSON), or "" when none applies. Any live time-boxed hold is already folded
/// into the lists, so Kotlin suspends the resulting package set exactly as
/// before, level-triggered. Takes the clock because a hold ends at an absolute
/// instant and must be re-resolved each tick.
#[no_mangle]
pub extern "system" fn Java_org_forgesworn_charter_native_CharterNative_charterAppPolicy(
    env: JNIEnv,
    _class: JClass,
    now: jlong,
) -> jstring {
    with_warden(&env, |w| w.app_policy(now))
}

/// The sole ward's LIVE app holds at `now` — a JSON array of
/// `{pkg, state, untilUnix}`, or "" when none. Kotlin diffs it against what it
/// last saw so the ward is told when an app opens for a while, and told again
/// when it goes back to its usual rule.
#[no_mangle]
pub extern "system" fn Java_org_forgesworn_charter_native_CharterNative_charterAppHolds(
    env: JNIEnv,
    _class: JClass,
    now: jlong,
) -> jstring {
    with_warden(&env, |w| w.app_holds(now))
}

/// The sole ward's per-app RULE suspensions for `now_unix` — the packages whose
/// `appRules` access is `Blocked` right now (blocked outright, or outside their
/// allowed hours) — as a JSON array of package ids, or `[]` when none. Schedule-
/// dependent, so it is recomputed each call. Kotlin unions this with the standing
/// per-app policy and suspends the union, level-triggered.
#[no_mangle]
pub extern "system" fn Java_org_forgesworn_charter_native_CharterNative_charterAppRuleSuspensions(
    env: JNIEnv,
    _class: JClass,
    now_unix: jlong,
) -> jstring {
    with_warden(&env, |w| {
        serde_json::to_string(&w.app_rule_suspensions(now_unix)).unwrap_or_default()
    })
}

/// The sole ward's per-bucket ("named time") suspensions at `now_unix` — the
/// packages whose bucket has spent either axis (day or week) right now — as a
/// JSON array of package ids, or `[]` when none. Extra-adjusted (a granted
/// `time.extend`/gift to a bucket lifts both walls today) and recomputed each
/// call, like [`charterAppRuleSuspensions`]. Kotlin unions this into the same
/// suspend set: spending a bucket suspends only ITS apps, never the device.
#[no_mangle]
pub extern "system" fn Java_org_forgesworn_charter_native_CharterNative_charterBucketSuspensions(
    env: JNIEnv,
    _class: JClass,
    now_unix: jlong,
) -> jstring {
    with_warden(&env, |w| {
        serde_json::to_string(&w.bucket_suspensions(now_unix)).unwrap_or_default()
    })
}

/// Every named bucket's day/week picture plus the labelled `askFirst` list —
/// `{"buckets":[BucketView...],"askFirst":[{"pkg","label"}]}` — the ward's own
/// mirror surface (spec D-Bus parity). Built for Task 9's Kotlin UI; not yet
/// consumed by any enforcement path.
#[no_mangle]
pub extern "system" fn Java_org_forgesworn_charter_native_CharterNative_charterBucketViews(
    env: JNIEnv,
    _class: JClass,
    now_unix: jlong,
) -> jstring {
    with_warden(&env, |w| w.bucket_views_json(now_unix))
}

/// The sole ward's tethering posture at `now_unix`: `"blocked"`, `"raw"`, or
/// `"filtered"` (the `tethering` clause). Recomputed per tick — time-boxed
/// grants end AT their `until`. Kotlin applies it level-triggered: blocked ⇒
/// DISALLOW_CONFIG_TETHERING; raw ⇒ tethering config free for the window;
/// filtered ⇒ system Wi-Fi hotspot stays locked while the Charter Hotspot runs.
#[no_mangle]
pub extern "system" fn Java_org_forgesworn_charter_native_CharterNative_charterTetheringMode(
    env: JNIEnv,
    _class: JClass,
    now_unix: jlong,
) -> jstring {
    with_warden(&env, |w| w.tethering_mode(now_unix).to_string())
}

/// The ward-facing "your charter" mirror (spec D8): `{"locked","minutesLeft",
/// "detail","lines"}` JSON, or "" when no charter is configured yet.
#[no_mangle]
pub extern "system" fn Java_org_forgesworn_charter_native_CharterNative_charterScheduleView(
    env: JNIEnv,
    _class: JClass,
    now_unix: jlong,
) -> jstring {
    with_warden(&env, |w| w.schedule_view(now_unix))
}

/// The ward's lifeline numbers (`lifeline` clause) as `[{"label","number"}]`
/// JSON, or "" when none apply. The lock screen renders one call button per
/// entry (spec D9 — a locked phone is still a phone).
#[no_mangle]
pub extern "system" fn Java_org_forgesworn_charter_native_CharterNative_charterLifeline(
    env: JNIEnv,
    _class: JClass,
) -> jstring {
    with_warden(&env, |w| w.lifeline())
}

/// The ward breaks the glass: an immediate emergency unlock, loudly
/// journaled for the guardian. Returns `{"allowed",...}` JSON. Refused only
/// when the guardian hasn't enabled it — never for lack of network.
#[no_mangle]
pub extern "system" fn Java_org_forgesworn_charter_native_CharterNative_charterBreakGlass(
    env: JNIEnv,
    _class: JClass,
    now_unix: jlong,
) -> jstring {
    with_warden_mut(&env, |w| w.break_glass(now_unix as u64))
}

/// The ward's effective web-content policy as a `DnsFilterPlan` wrapper
/// (`{"revision","plan"}`), or "" when not paired. Kotlin applies it in a
/// DO-pinned VpnService, level-triggered on the revision.
#[no_mangle]
pub extern "system" fn Java_org_forgesworn_charter_native_CharterNative_charterDnsPlan(
    env: JNIEnv,
    _class: JClass,
) -> jstring {
    with_warden(&env, |w| w.web_dns_plan())
}

/// The install window as THIS DEVICE saw it:
/// `{"startedAt":N,"endedAt":N|null,"open":bool}`, or `{}` when none has ever
/// opened here. Mutating — it records the open/close edges as it observes them,
/// which is what survives a guardian closing the window early.
#[no_mangle]
pub extern "system" fn Java_org_forgesworn_charter_native_CharterNative_charterMaintenanceSpan(
    env: JNIEnv,
    _class: JClass,
    now: jlong,
) -> jstring {
    with_warden_mut(&env, |w| w.maintenance_span(now.max(0) as u64))
}

/// Kotlin reports what changed during that span (only the platform knows
/// package install times); it rides the next STATUS as the guardian's account.
#[no_mangle]
pub extern "system" fn Java_org_forgesworn_charter_native_CharterNative_charterSetInstallWindow(
    mut env: JNIEnv,
    _class: JClass,
    report_json: JString,
) -> jstring {
    let json = jstr(&mut env, report_json).unwrap_or_default();
    with_warden_mut(&env, |w| {
        w.set_install_window(&json);
        "{\"ok\":true}".to_string()
    })
}

/// Kotlin reports the platform boot counter it is running under
/// (`Settings.Global.BOOT_COUNT`); the warden counts the boots it MISSED and
/// rides the tally on STATUS, which is the only trace a safe-mode holiday
/// leaves behind (S1).
#[no_mangle]
pub extern "system" fn Java_org_forgesworn_charter_native_CharterNative_charterNoteBoot(
    env: JNIEnv,
    _class: JClass,
    boot_count: jlong,
    now_unix: jlong,
) -> jstring {
    with_warden_mut(&env, |w| {
        w.note_boot(boot_count.max(0) as u64, now_unix.max(0) as u64);
        "{\"ok\":true}".to_string()
    })
}

/// Kotlin reports the device's installed launchable apps (JSON `[{pkg,label}]`);
/// they ride the next STATUS so the guardian can pick apps by name.
#[no_mangle]
pub extern "system" fn Java_org_forgesworn_charter_native_CharterNative_charterSetInstalledApps(
    mut env: JNIEnv,
    _class: JClass,
    apps_json: JString,
) -> jstring {
    let json = jstr(&mut env, apps_json).unwrap_or_default();
    with_warden_mut(&env, |w| {
        w.set_installed_apps(&json);
        "{\"ok\":true}".to_string()
    })
}

#[no_mangle]
pub extern "system" fn Java_org_forgesworn_charter_native_CharterNative_charterIngestClause(
    mut env: JNIEnv,
    _class: JClass,
    event_json: JString,
    now_unix: jlong,
) -> jstring {
    let ev = match jstr(&mut env, event_json) {
        Ok(s) => s,
        Err(e) => return out(&env, &err_json(&e)),
    };
    with_warden_mut(&env, |w| {
        serde_json::to_string(&w.ingest_clause(&ev, now_unix as u64)).unwrap_or_default()
    })
}

/// `foreground_pkg` is the SAME single foreground-package probe Kotlin already
/// makes for the screen credit (`WardenController.tickAndApply`) — never a
/// second probe. It attributes this tick's elapsed interval to a named-times
/// bucket ("Play") alongside the ordinary screen credit, when the package
/// belongs to one.
#[no_mangle]
pub extern "system" fn Java_org_forgesworn_charter_native_CharterNative_charterTick(
    mut env: JNIEnv,
    _class: JClass,
    active_subject_hex: JString,
    screen_interactive: jni::sys::jboolean,
    now_unix: jlong,
    foreground_pkg: JString,
) -> jstring {
    let active = opt_jstr(&mut env, active_subject_hex);
    let fg = opt_jstr(&mut env, foreground_pkg);
    let screen = screen_interactive != 0;
    with_warden_mut(&env, |w| {
        let decisions = w.tick(active.as_deref(), screen, now_unix, fg.as_deref());
        serde_json::to_string(&decisions).unwrap_or_default()
    })
}

#[no_mangle]
pub extern "system" fn Java_org_forgesworn_charter_native_CharterNative_charterTimeLeft(
    env: JNIEnv,
    _class: JClass,
    _subject_hex: JString,
    now_unix: jlong,
) -> jstring {
    with_warden(&env, |w| {
        serde_json::to_string(&w.time_left(now_unix)).unwrap_or_default()
    })
}

#[no_mangle]
pub extern "system" fn Java_org_forgesworn_charter_native_CharterNative_charterLockInfo(
    env: JNIEnv,
    _class: JClass,
    _subject_hex: JString,
    now_unix: jlong,
) -> jstring {
    with_warden(&env, |w| {
        serde_json::to_string(&w.lock_info(now_unix)).unwrap_or_default()
    })
}

/// Packages that may keep playing through the lock + the grace remaining.
/// `audio_playing` comes from the platform (`AudioManager.isMusicActive`) —
/// the exemption is about audio actually sounding, not about an app being named.
#[no_mangle]
pub extern "system" fn Java_org_forgesworn_charter_native_CharterNative_charterListeningView(
    env: JNIEnv,
    _class: JClass,
    now_unix: jlong,
    locked: jni::sys::jboolean,
    audio_playing: jni::sys::jboolean,
) -> jstring {
    with_warden_mut(&env, |w| {
        w.listening_view(now_unix as u64, locked != 0, audio_playing != 0)
    })
}

/// Packages that may be OPENED though the device is locked. Takes the lock
/// REASON because only "schedule" and "budget" exempt anything — a
/// stand-down or a malformed charter never does.
#[no_mangle]
pub extern "system" fn Java_org_forgesworn_charter_native_CharterNative_charterAlwaysAvailable(
    mut env: JNIEnv,
    _class: JClass,
    now_unix: jlong,
    locked: jni::sys::jboolean,
    lock_reason: JString,
) -> jstring {
    let reason = match jstr(&mut env, lock_reason) {
        Ok(s) => s,
        Err(e) => return out(&env, &err_json(&e)),
    };
    with_warden(&env, |w| {
        w.always_available_view(now_unix as u64, locked != 0, &reason)
    })
}

#[no_mangle]
pub extern "system" fn Java_org_forgesworn_charter_native_CharterNative_charterSetEnforceMode(
    mut env: JNIEnv,
    _class: JClass,
    mode: JString,
) {
    if let Ok(m) = jstr(&mut env, mode) {
        if let Some(w) = WARDEN.lock().unwrap_or_else(|e| e.into_inner()).as_mut() {
            w.set_enforce_mode(&m);
        }
    }
}

#[no_mangle]
pub extern "system" fn Java_org_forgesworn_charter_native_CharterNative_charterShutdown(
    _env: JNIEnv,
    _class: JClass,
    _obj: JObject,
) {
    *WARDEN.lock().unwrap_or_else(|e| e.into_inner()) = None;
}

// ---- lock plumbing --------------------------------------------------------

fn with_warden(env: &JNIEnv, f: impl FnOnce(&Warden) -> String) -> jstring {
    // Recover from a poisoned lock instead of bricking every later call, and
    // never let a panic unwind across the FFI boundary (UB) — surface an error.
    let guard = WARDEN.lock().unwrap_or_else(|e| e.into_inner());
    let res = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| match guard.as_ref() {
        Some(w) => f(w),
        None => err_json("not initialized"),
    }))
    .unwrap_or_else(|_| err_json("internal error (panic recovered)"));
    out(env, &res)
}

fn with_warden_mut(env: &JNIEnv, f: impl FnOnce(&mut Warden) -> String) -> jstring {
    let mut guard = WARDEN.lock().unwrap_or_else(|e| e.into_inner());
    let res = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| match guard.as_mut() {
        Some(w) => f(w),
        None => err_json("not initialized"),
    }))
    .unwrap_or_else(|_| err_json("internal error (panic recovered)"));
    out(env, &res)
}
