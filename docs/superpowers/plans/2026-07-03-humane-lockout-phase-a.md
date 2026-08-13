# Humane Lockout — Phase A Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Turn the child lock from a passive input-swallowing wall into a humane, interactive lockout — a clear "you're out of time" panel with **Log out / Shut down / Suspend** buttons, preceded by on-screen **warnings** — so a locked child is never trapped and never yanked without notice, and a shared machine is freed by logging out.

**Architecture:** `charter-lock` (a root-run, fullscreen override-redirect X11 window) gains a pure, unit-tested UI model (button layout + click hit-testing → `Action`) and executes the chosen action as root (`loginctl terminate-user` / `systemctl poweroff|suspend`). `charterd` passes the locked child's uid to the lock and delivers the enforcer's already-computed 10-/1-minute warnings to the child's session via `notify-send`. The freeze topology stays whole-user-slice unless the hardware gate shows the lock can't draw over it, in which case one function switches to the app-slice.

**Tech Stack:** Rust, `x11rb` (pure-Rust X11), systemd/`loginctl`, `notify-send` (libnotify), Cinnamon/Mint (X11).

## Global Constraints

- Spec: `docs/superpowers/specs/2026-07-03-humane-lockout-design.md` (decisions settled 2026-07-03).
- **Invariants:** never trapped (Log out / Shut down / Suspend always available); never a surprise (warnings precede lockout); per-child/per-session (a sibling with time is never blocked); the guardian's recovery rails are untouched.
- **Lock actions (approved):** Phase A = **Log out + Shut down + Suspend**. "Ask for more time" is Phase B — not in this plan.
- **Warning cadence (approved):** notify at **10 min** and **1 min**, plus a **"locking now"** notice as the lock draws. (An animated 30-second countdown overlay is a noted enhancement, not required for Phase A.)
- **Display stack:** X11 / Cinnamon only. No Wayland.
- **Power actions:** Shut down **and** Suspend.
- The lock runs **as root** (charterd spawns it with no privilege drop) — so it can `loginctl`/`systemctl` directly.
- No new crates/dependencies (x11rb + std::process only).
- Four CI gates from `linux/`: `cargo fmt --all --check`, `cargo clippy --all-targets --all-features -- -D warnings`, `cargo test --workspace`, `cargo build --workspace --no-default-features --features real`.
- Build the installer with `cargo run -p xtask -- deb` (from `linux/`).

## File Structure

- `linux/crates/charter-lock/src/lock_ui.rs` — **new.** Pure UI model: `Action`, `Button`, `Rect`, `button_layout`, `hit_test`, `action_label`. No X11, no I/O — fully unit-tested.
- `linux/crates/charter-lock/src/main.rs` — **modify.** Draw the buttons, receive `ButtonPress`, map clicks → `Action`, execute the action. Read `CHARTER_LOCK_UID`.
- `linux/crates/charterd/src/runtime.rs` — **modify.** (a) pass `CHARTER_LOCK_UID` to the lock; (b) deliver `Warn(Ten)`/`Warn(One)` effects to the child via `notify-send`; add a pure `notify_argv` + `user_for_uid` helper with tests.
- `linux/crates/charterd/src/enforcer_runtime.rs` — **modify only if the hardware gate requires it** (Task 6 contingency): switch `managed_freeze_target` to the app-slice.

---

### Task 1: Lock UI model — buttons + hit-testing (pure)

**Files:**
- Create: `linux/crates/charter-lock/src/lock_ui.rs`
- Modify: `linux/crates/charter-lock/src/main.rs` (add `mod lock_ui;` + re-exports)

**Interfaces:**
- Produces: `enum Action { Logout, Shutdown, Suspend }`; `struct Rect { x: i16, y: i16, width: u16, height: u16 }`; `struct Button { rect: Rect, label: &'static str, action: Action }`; `fn button_layout(screen_w: u16, screen_h: u16) -> Vec<Button>`; `fn hit_test(buttons: &[Button], x: i16, y: i16) -> Option<Action>`.

- [ ] **Step 1: Write the failing test**

Create `linux/crates/charter-lock/src/lock_ui.rs` with only the tests first will not compile; instead write the module with the test module and stubbed items so the RED is a failing assertion. Put this at the bottom of the new file:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn layout_places_three_buttons_in_a_centered_row_on_screen() {
        let b = button_layout(1920, 1080);
        assert_eq!(b.len(), 3);
        assert_eq!(
            b.iter().map(|x| x.action).collect::<Vec<_>>(),
            vec![Action::Logout, Action::Shutdown, Action::Suspend]
        );
        // All buttons sit fully on screen and below the vertical middle.
        for btn in &b {
            assert!(btn.rect.x >= 0);
            assert!((btn.rect.x as i32 + btn.rect.width as i32) <= 1920);
            assert!(btn.rect.y as i32 > 540);
        }
        // The row is centered: left margin == right margin (±1px).
        let first = &b[0];
        let last = &b[2];
        let left = first.rect.x as i32;
        let right = 1920 - (last.rect.x as i32 + last.rect.width as i32);
        assert!((left - right).abs() <= 1, "row not centered: {left} vs {right}");
    }

    #[test]
    fn hit_test_maps_a_click_inside_a_button_to_its_action() {
        let b = button_layout(1920, 1080);
        let mid = |r: &Rect| (r.x + (r.width / 2) as i16, r.y + (r.height / 2) as i16);
        let (lx, ly) = mid(&b[0].rect);
        assert_eq!(hit_test(&b, lx, ly), Some(Action::Logout));
        let (sx, sy) = mid(&b[2].rect);
        assert_eq!(hit_test(&b, sx, sy), Some(Action::Suspend));
        // A click in dead space hits nothing.
        assert_eq!(hit_test(&b, 5, 5), None);
    }
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cd linux && cargo test -p charter-lock lock_ui`
Expected: FAIL — `button_layout`/`hit_test`/`Action` not found (compile error).

- [ ] **Step 3: Write minimal implementation**

Put this ABOVE the test module in `lock_ui.rs`:

```rust
//! Pure UI model for the humane lock: where the action buttons sit and which
//! action a click maps to. No X11, no I/O — so the layout + hit-testing is
//! fully unit-tested; `main.rs` only draws these rects and execs the result.

/// A sanctioned escape the locked child may always take.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    Logout,
    Shutdown,
    Suspend,
}

/// A screen rectangle (X11 coordinates: origin top-left, pixels).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Rect {
    pub x: i16,
    pub y: i16,
    pub width: u16,
    pub height: u16,
}

impl Rect {
    fn contains(&self, x: i16, y: i16) -> bool {
        x >= self.x
            && y >= self.y
            && (x as i32) < self.x as i32 + self.width as i32
            && (y as i32) < self.y as i32 + self.height as i32
    }
}

/// A drawn button: its rect, its label, and the action it triggers.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Button {
    pub rect: Rect,
    pub label: &'static str,
    pub action: Action,
}

const BTN_W: u16 = 300;
const BTN_H: u16 = 72;
const BTN_GAP: u16 = 40;

/// The Phase-A buttons, in order, as a horizontally-centered row sitting below
/// the message (which `main.rs` draws around screen-middle).
pub fn button_layout(screen_w: u16, screen_h: u16) -> Vec<Button> {
    let specs = [
        ("Log out", Action::Logout),
        ("Shut down", Action::Shutdown),
        ("Suspend", Action::Suspend),
    ];
    let n = specs.len() as u16;
    let total_w = n * BTN_W + (n - 1) * BTN_GAP;
    let start_x = (((screen_w as i32) - (total_w as i32)) / 2).max(0) as i16;
    let y = ((screen_h as i32) / 2 + 100).min(i16::MAX as i32) as i16;
    specs
        .iter()
        .enumerate()
        .map(|(i, (label, action))| {
            let x = start_x + (i as i16) * (BTN_W + BTN_GAP) as i16;
            Button {
                rect: Rect { x, y, width: BTN_W, height: BTN_H },
                label,
                action: *action,
            }
        })
        .collect()
}

/// The action for a click at `(x, y)`, or `None` if it missed every button.
pub fn hit_test(buttons: &[Button], x: i16, y: i16) -> Option<Action> {
    buttons.iter().find(|b| b.rect.contains(x, y)).map(|b| b.action)
}
```

Then in `main.rs`, add near the top (after the doc comment / `use std::error::Error;`):

```rust
mod lock_ui;
use lock_ui::{button_layout, hit_test, Action, Button};
```

- [ ] **Step 4: Run test to verify it passes**

Run: `cd linux && cargo test -p charter-lock lock_ui`
Expected: PASS (2 tests). (`Button`/`hit_test`/`button_layout` may warn as unused until Task 2 — that's fine; `cargo test` does not `-D warnings`.)

- [ ] **Step 5: Commit**

```bash
git add linux/crates/charter-lock/src/lock_ui.rs linux/crates/charter-lock/src/main.rs
git commit -m "feat(charter-lock): pure UI model — action buttons + click hit-testing"
```

---

### Task 2: Draw the buttons + execute the clicked action (charter-lock)

**Files:**
- Modify: `linux/crates/charter-lock/src/main.rs`

**Interfaces:**
- Consumes: `button_layout`, `hit_test`, `Action`, `Button` (Task 1).
- Produces: `fn perform(action: Action, uid: Option<u32>)` (execs the sanctioned command); `fn managed_uid_from_env() -> Option<u32>` (reads `CHARTER_LOCK_UID`).

- [ ] **Step 1: Write the failing test**

Add to `main.rs`'s `#[cfg(test)] mod tests`:

```rust
    #[test]
    fn action_argv_is_the_expected_sanctioned_command() {
        assert_eq!(
            action_argv(Action::Logout, Some(1002)),
            Some(vec!["loginctl".to_string(), "terminate-user".to_string(), "1002".to_string()])
        );
        assert_eq!(
            action_argv(Action::Shutdown, Some(1002)),
            Some(vec!["systemctl".to_string(), "poweroff".to_string()])
        );
        assert_eq!(
            action_argv(Action::Suspend, None),
            Some(vec!["systemctl".to_string(), "suspend".to_string()])
        );
        // Logout needs a uid; without one it is not offered.
        assert_eq!(action_argv(Action::Logout, None), None);
    }
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cd linux && cargo test -p charter-lock action_argv`
Expected: FAIL — `action_argv` not found.

- [ ] **Step 3: Write minimal implementation**

Add to `main.rs` (above the test module). This factors the command choice into a pure, testable `action_argv`; `perform` is the thin exec wrapper:

```rust
use std::process::Command;

/// The sanctioned command for an action. `Logout` needs the managed uid; if it
/// is unknown the button is not offered (returns `None`), never a wrong target.
fn action_argv(action: Action, uid: Option<u32>) -> Option<Vec<String>> {
    match action {
        Action::Logout => uid.map(|u| {
            vec!["loginctl".to_string(), "terminate-user".to_string(), u.to_string()]
        }),
        Action::Shutdown => Some(vec!["systemctl".to_string(), "poweroff".to_string()]),
        Action::Suspend => Some(vec!["systemctl".to_string(), "suspend".to_string()]),
    }
}

/// Read the managed child's uid the daemon passes in (for the Log out target).
fn managed_uid_from_env() -> Option<u32> {
    std::env::var("CHARTER_LOCK_UID").ok().and_then(|s| s.trim().parse().ok())
}

/// Execute the sanctioned action as root. Best-effort: a failed spawn is logged,
/// never a panic (the lock must stay up).
fn perform(action: Action, uid: Option<u32>) {
    if let Some(argv) = action_argv(action, uid) {
        if let Err(e) = Command::new(&argv[0]).args(&argv[1..]).status() {
            eprintln!("charter-lock: action {argv:?} failed: {e}");
        }
    }
}
```

- [ ] **Step 4: Run test to verify it passes**

Run: `cd linux && cargo test -p charter-lock`
Expected: PASS (all charter-lock tests).

- [ ] **Step 5: Draw buttons + handle clicks in the X11 loop**

In `run_lock`, make three changes.

(a) Extend the window's event mask and the pointer grab to receive button presses. Change the `event_mask(...)` in `CreateWindowAux::new()`:

```rust
            .event_mask(EventMask::EXPOSURE | EventMask::KEY_PRESS | EventMask::BUTTON_PRESS),
```

and change the `grab_pointer` `event_mask` argument from `EventMask::NO_EVENT` to:

```rust
        EventMask::BUTTON_PRESS,
```

(b) Compute the layout once and draw the buttons inside `draw`. Add a `buttons: &[Button]` parameter to `draw` and, after drawing the two text lines, draw each button (a filled box + centered label). Add, before the `conn.flush()?` at the end of `draw`:

```rust
    for b in buttons {
        conn.poly_fill_rectangle(
            win,
            detail_gc,
            &[x11rb::protocol::xproto::Rectangle {
                x: b.rect.x,
                y: b.rect.y,
                width: b.rect.width,
                height: b.rect.height,
            }],
        )?;
        // Label in the box, drawn with the background color so it reads on the
        // light button (image_text8 paints fg on bg; swap by using title_gc's
        // fill — here we simply center the label with the detail font).
        let label = b.label.as_bytes();
        let label_w = (label.len() as u16).saturating_mul(DETAIL_ADVANCE);
        let lx = b.rect.x + centered_x(b.rect.width, label_w);
        let ly = b.rect.y + (b.rect.height as i16) / 2 + 5;
        conn.image_text8(win, title_gc, lx, ly, label)?;
    }
```

(c) In `run_lock`, compute the buttons and pass them to `draw`, and handle `ButtonPress` in the loop:

```rust
    let buttons = button_layout(w, h);
    let uid = managed_uid_from_env();
    draw(&conn, win, title_gc, detail_gc, w, h, text, &buttons)?;
    loop {
        match conn.wait_for_event()? {
            Event::Expose(_) => draw(&conn, win, title_gc, detail_gc, w, h, text, &buttons)?,
            Event::ButtonPress(ev) => {
                if let Some(action) = hit_test(&buttons, ev.event_x, ev.event_y) {
                    perform(action, uid);
                }
            }
            _ => {}
        }
    }
```

Update the two existing `draw(...)` call sites and the `draw` signature to include `buttons: &[Button]`.

- [ ] **Step 6: Verify it builds + tests pass**

Run: `cd linux && cargo build -p charter-lock && cargo test -p charter-lock`
Expected: builds clean; all tests pass. (Visual correctness is verified on hardware in Task 5.)

- [ ] **Step 7: Commit**

```bash
git add linux/crates/charter-lock/src/main.rs
git commit -m "feat(charter-lock): interactive lock — Log out / Shut down / Suspend buttons"
```

---

### Task 3: Pass the locked child's uid to the lock (charterd)

**Files:**
- Modify: `linux/crates/charterd/src/runtime.rs` (the lock-launch block, ~line 406-415)

**Interfaces:**
- Consumes: `d.uid` (the `ChildDecision` uid, already in scope in `apply_child_decisions`).
- Produces: the `CHARTER_LOCK_UID` env the lock reads in Task 2.

- [ ] **Step 1: Add the env var to the lock command**

In `apply_child_decisions`, in the block that builds `cmd = Command::new(lock_bin)`, add the uid env alongside the existing `.env(...)` calls:

```rust
                cmd.env("DISPLAY", display)
                    .env("CHARTER_LOCK_UID", d.uid.to_string())
                    .env("CHARTER_LOCK_TITLE", title)
                    .env("CHARTER_LOCK_DETAIL", detail);
```

- [ ] **Step 2: Verify it builds**

Run: `cd linux && cargo build -p charterd`
Expected: builds clean.

- [ ] **Step 3: Commit**

```bash
git add linux/crates/charterd/src/runtime.rs
git commit -m "feat(charterd): pass the locked child's uid to the lock (Log out target)"
```

---

### Task 4: Deliver the 10-/1-minute warnings to the child (charterd)

**Files:**
- Modify: `linux/crates/charterd/src/runtime.rs`

**Interfaces:**
- Consumes: `EnforcerEffect::Warn(WarnLevel)` (from `charter_schedule`; emitted by the enforcer each tick), the `passwd` string, `d.uid`, and the active session display.
- Produces: `fn user_for_uid(passwd: &str, uid: u32) -> Option<String>`; `fn notify_argv(user: &str, uid: u32, display: &str, summary: &str, body: &str) -> Vec<String>`; delivery wired into `apply_child_decisions`.

- [ ] **Step 1: Write the failing test**

Add to `runtime.rs`'s `#[cfg(test)] mod tests` (create the module if absent — mirror the crate's test style):

```rust
    #[test]
    fn user_for_uid_reverses_the_passwd_lookup() {
        let passwd = "root:x:0:0::/root:/bin/bash\nchild:x:1002:1002::/home/child:/bin/bash\n";
        assert_eq!(super::user_for_uid(passwd, 1002).as_deref(), Some("child"));
        assert_eq!(super::user_for_uid(passwd, 9999), None);
    }

    #[test]
    fn notify_argv_runs_notify_send_as_the_child_in_their_session() {
        let argv = super::notify_argv("child", 1002, ":0", "Charter", "10 minutes left");
        // Runs as the child, with their session display + bus, critical urgency.
        assert_eq!(argv[0], "runuser");
        assert!(argv.contains(&"child".to_string()));
        assert!(argv.iter().any(|a| a == "DISPLAY=:0"));
        assert!(argv.iter().any(|a| a == "DBUS_SESSION_BUS_ADDRESS=unix:path=/run/user/1002/bus"));
        assert!(argv.iter().any(|a| a == "notify-send"));
        assert!(argv.contains(&"10 minutes left".to_string()));
    }
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cd linux && cargo test -p charterd user_for_uid`
Expected: FAIL — `user_for_uid` / `notify_argv` not found.

- [ ] **Step 3: Write minimal implementation**

Add to `runtime.rs` (near the other passwd helpers like `home_for_uid`):

```rust
/// The username for a uid, from `/etc/passwd` contents (reverse of
/// `uid_for_user`). Used to run `notify-send` as the child in their own session.
fn user_for_uid(passwd: &str, uid: u32) -> Option<String> {
    for line in passwd.lines() {
        let mut f = line.split(':');
        let name = f.next()?;
        let _pw = f.next();
        let u: u32 = f.next()?.parse().ok()?;
        if u == uid {
            return Some(name.to_string());
        }
    }
    None
}

/// Argv to pop a desktop notification in the CHILD's session (as the child, so
/// their session bus accepts it). charterd is root, so `runuser` drops to the
/// child and `env` supplies their display + session bus. `notify-send` ships
/// with libnotify (present on Cinnamon).
fn notify_argv(user: &str, uid: u32, display: &str, summary: &str, body: &str) -> Vec<String> {
    vec![
        "runuser".to_string(),
        "-u".to_string(),
        user.to_string(),
        "--".to_string(),
        "env".to_string(),
        format!("DISPLAY={display}"),
        format!("DBUS_SESSION_BUS_ADDRESS=unix:path=/run/user/{uid}/bus"),
        "notify-send".to_string(),
        "-u".to_string(),
        "critical".to_string(),
        "-a".to_string(),
        "Charter".to_string(),
        summary.to_string(),
        body.to_string(),
    ]
}

/// Fire-and-forget a warning notification to the child (best-effort; a missing
/// notify-send or session is never fatal to enforcement).
fn notify_child(passwd: &str, uid: u32, display: &str, summary: &str, body: &str) {
    if let Some(user) = user_for_uid(passwd, uid) {
        let argv = notify_argv(&user, uid, display, summary, body);
        let _ = Command::new(&argv[0]).args(&argv[1..]).spawn();
    }
}
```

- [ ] **Step 4: Wire delivery into the tick**

In `apply_child_decisions`, the `Warn` effects currently fall through the `match eff` (in the reconcile change they are not handled). Add warning delivery in the per-child loop. Resolve the child's display once (reuse `active_session_x`'s result already computed for the lock, or resolve per child). Add, inside the `for d in decisions` loop, after the freeze reconcile block:

```rust
        for eff in &d.effects {
            if let EnforcerEffect::Warn(level) = eff {
                let (display, _) = active_session_x()
                    .unwrap_or_else(|| (std::env::var("CHARTER_DISPLAY").unwrap_or_else(|_| ":0".into()), None));
                let body = match level {
                    charter_schedule::WarnLevel::Ten => "10 minutes left",
                    charter_schedule::WarnLevel::One => "1 minute left — save your game",
                };
                notify_child(passwd, d.uid, &display, "Charter — time's almost up", body);
            }
        }
```

Ensure `WarnLevel` is imported (extend the `charter_schedule::{...}` use if needed) and that `apply_child_decisions` receives `passwd` (it already takes `passwd: &str`).

- [ ] **Step 5: Run tests + build**

Run: `cd linux && cargo test -p charterd && cargo build -p charterd`
Expected: the two new tests pass; builds clean.

- [ ] **Step 6: Commit**

```bash
git add linux/crates/charterd/src/runtime.rs
git commit -m "feat(charterd): deliver 10-/1-minute warnings to the child via notify-send"
```

---

### Task 5: Gates + full build (fmt/clippy/test/real-build + .deb)

**Files:** none (verification).

- [ ] **Step 1: Format**

Run: `cd linux && cargo fmt --all && cargo fmt --all --check`
Expected: clean.

- [ ] **Step 2: Clippy (all features, all targets, deny warnings)**

Run: `cd linux && cargo clippy --all-targets --all-features -- -D warnings`
Expected: no warnings. (Note: `charter-lock`'s X11 code is always compiled; fix any lint the new draw/exec code raises.)

- [ ] **Step 3: Workspace tests + real build**

Run: `cd linux && cargo test --workspace && cargo build --workspace --no-default-features --features real`
Expected: all tests pass; real build succeeds.

- [ ] **Step 4: Build the installer**

Run: `cd linux && cargo run -p xtask -- deb`
Expected: prints the path to `target/deb/charter_0.1.0_amd64.deb`.

- [ ] **Step 5: Commit any fmt-only changes**

```bash
git add -A && git commit -m "chore(linux): fmt after humane-lockout Phase A" || echo "nothing to commit"
```

---

### Task 6: Hardware gate (decented) — and the freeze-topology contingency

**This is the on-metal verification, run by decented with turnkey steps from the controller. It also decides the one contingency edit.**

**Setup:** install the fresh `.deb`, then escalate to full enforce mode:
```
sudo dpkg -i .../charter_0.1.0_amd64.deb && sudo sed -i 's/^CHARTER_ENFORCE=.*/CHARTER_ENFORCE=enforce/' /etc/charter/charterd.env && sudo systemctl restart charterd
```
Recovery is unchanged: `sudo systemctl stop charterd` thaws + unlocks.

**Gates to confirm (controller reads logs / drives; decented is the eyes + hands):**
1. **Lock draws over the frozen session.** Log in as a past-limit `child`; the lock panel appears with the message **and the three buttons**. *If instead the screen is dead (no panel) — the whole-user-slice freeze has frozen this box's Xorg.* → apply the **contingency** below, rebuild, retest.
2. **Buttons work.** Clicking **Log out** returns to the greeter (session ends); **Shut down** powers off; **Suspend** suspends. Input to the child's apps stays blocked.
3. **Warnings show.** With a limit ~10 min away, the child sees "10 minutes left", then "1 minute left" notifications before the lock.
4. **Shared machine.** While `child` is locked, a second child/sibling **with time** logs in and uses the machine normally; the locked child's **Log out** freed the seat.

**Contingency edit (only if gate 1 shows a dead screen):** switch the freeze target from the whole user slice to the child's **app slice** so Xorg/compositor survive. In `linux/crates/charterd/src/enforcer_runtime.rs`:

```rust
pub fn managed_freeze_target(uid: u32) -> String {
    // App slice only: pauses the child's apps while leaving the session's
    // display server alive so the (root) lock can draw over it.
    format!("user.slice/user-{uid}.slice/user@{uid}.service/app.slice")
}
```

Update the `is_valid_freeze_target` guard (`charter-schedule/src/enforcer.rs`) if its `ends_with(".slice")` / prefix checks reject the deeper path, and its unit tests. Then re-run Steps in Task 5 + re-gate. (Kept as a contingency because the current code comment asserts the whole-slice freeze is intentionally lock-compatible; hardware decides.)

---

## Self-Review

**Spec coverage:**
- Interactive lock, never trapped (Log out/Shut down/Suspend) → Tasks 1–3 + gate 2. ✓
- Warnings, no surprise → Task 4 + gate 3. ✓
- Shared machine / per-session, log-out-to-free → gate 4 (per-session enforcement already exists; Log out is the new lever). ✓
- Freeze topology (apps vs whole session) → Task 6 contingency (hardware-decided, as the spec says "verified on the freeze rung"). ✓
- Recovery rails untouched → no changes to stop/recovery paths. ✓
- Approved decisions (10/1 warnings, no grace, Log out+Shut down+Suspend, X11-only, Shut down+Suspend) → Tasks 1/2/4 use exactly these. ✓
- "Ask for more time" (Phase B) → explicitly out of scope. ✓

**Placeholder scan:** every code step has complete code; every run step has an exact command + expected result. The one contingent edit (Task 6) is gated on a named, observable hardware condition, not a TBD.

**Type consistency:** `Action`/`Button`/`Rect`/`button_layout`/`hit_test` defined in Task 1 and consumed unchanged in Task 2; `action_argv`/`managed_uid_from_env`/`perform` consistent within Task 2; `CHARTER_LOCK_UID` written in Task 3, read in Task 2; `user_for_uid`/`notify_argv`/`notify_child` consistent within Task 4; `WarnLevel::{Ten,One}` matches `charter_schedule`.

**Scope:** one subsystem (the on-device lockout UX); Phase B (request-time) is a separate future plan.
