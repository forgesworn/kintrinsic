//! `charter-lock` — the on-display undismissable lock.
//!
//! A fullscreen, override-redirect X11 window that grabs the keyboard + pointer
//! so the managed child cannot interact with anything behind it, showing a
//! "time's up / access resumes …" message. Spawned by `charterd`'s
//! `SessionControl::show_lock` when the schedule/budget enforcer locks; the
//! daemon terminates the process to unlock (dropping the X connection releases
//! the grab automatically). It composes with the cgroup freeze (apps stopped)
//! and the VT lock (no `Ctrl+Alt+Fn` escape).
//!
//! x11rb speaks the X11 protocol in pure Rust (no libxcb/Xlib C deps), so this
//! builds on the headless gate; the live grab/draw runs on the desktop (VM).
//! The message resolution + layout maths are pure and unit-tested.

use std::error::Error;
use std::time::{Duration, Instant};

mod lock_ui;
mod render;
use lock_ui::{button_layout, hit_test, Action, Button};

/// Vendored faces (Bitstream Vera license, embedding permitted — see
/// assets/DEJAVU-LICENSE). Compiled in so the panel renders identically on any
/// host; `CHARTER_LOCK_FONT`/`CHARTER_LOCK_FONT_BOLD` override at runtime.
pub const TITLE_TTF: &[u8] = include_bytes!("../assets/DejaVuSans-Bold.ttf");
pub const BODY_TTF: &[u8] = include_bytes!("../assets/DejaVuSans.ttf");

/// What the lock says: a bold title, a detail line, and optional extra lines
/// (the child's schedule + usage — "when can I go on next?").
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LockText {
    pub title: String,
    pub detail: String,
    pub lines: Vec<String>,
}

impl LockText {
    /// Resolve from the environment (set by the daemon) then argv, falling back
    /// to safe defaults — the lock always has something to say.
    pub fn resolve(
        env_title: Option<String>,
        env_detail: Option<String>,
        env_lines: Option<String>,
        args: &[String],
    ) -> Self {
        let title = env_title
            .or_else(|| args.first().cloned())
            .filter(|s| !s.trim().is_empty())
            .unwrap_or_else(|| "Time's up for now".to_string());
        let detail = env_detail
            .or_else(|| args.get(1).cloned())
            .filter(|s| !s.trim().is_empty())
            .unwrap_or_else(|| "Ask your guardian — access resumes later.".to_string());
        let lines = env_lines
            .unwrap_or_default()
            .lines()
            .map(str::trim)
            .filter(|l| !l.is_empty())
            .take(7) // the card grows per line; keep it a dialog, not a page
            .map(String::from)
            .collect();
        LockText {
            title,
            detail,
            lines,
        }
    }
}

use std::process::Command;

/// Minutes a panel "Ask for more time" requests. The guardian chooses what to
/// actually grant in Kintrinsic — this is the opening ask, not the outcome.
const ASK_MINUTES: u32 = 30;

/// The sanctioned command for an action. `Logout` needs the managed uid; if it
/// is unknown the button is not offered (returns `None`), never a wrong target.
///
/// Logout THAWS the child's slice before terminating it: `terminate-user`
/// sends SIGTERM, and a frozen task can never run its handler — the session
/// hangs half-dead ("closing" forever) and its abandoned inhibitor FIFOs can
/// spin logind's event loop into a busy-loop that wedges D-Bus for the whole
/// box (observed live on Mint 22). The daemon's reconcile no longer re-freezes
/// once the session is gone.
fn action_argv(action: Action, uid: Option<u32>) -> Option<Vec<String>> {
    match action {
        Action::Logout => uid.map(|u| {
            vec![
                "/bin/sh".to_string(),
                "-c".to_string(),
                format!(
                    "echo 0 > /sys/fs/cgroup/user.slice/user-{u}.slice/cgroup.freeze 2>/dev/null; exec loginctl terminate-user {u}"
                ),
            ]
        }),
        Action::Shutdown => Some(vec!["systemctl".to_string(), "poweroff".to_string()]),
        Action::Suspend => Some(vec!["systemctl".to_string(), "suspend".to_string()]),
        // As the CHILD, not root: `charter ask-for-more` routes the request to
        // the locked dimension via the child's own D-Bus time-left readout
        // (keyed by caller uid). runuser from here escapes the frozen slice
        // (the spawn inherits the lock's cgroup, so it actually runs).
        Action::AskMoreTime => std::env::var("CHARTER_LOCK_USER").ok().map(|user| {
            vec![
                "runuser".to_string(),
                "-u".to_string(),
                user,
                "--".to_string(),
                "charter".to_string(),
                "ask-for-more".to_string(),
                ASK_MINUTES.to_string(),
            ]
        }),
    }
}

/// Read the managed child's uid the daemon passes in (for the Log out target).
fn managed_uid_from_env() -> Option<u32> {
    std::env::var("CHARTER_LOCK_UID")
        .ok()
        .and_then(|s| s.trim().parse().ok())
}

/// The guardian/admin escape chord `Ctrl+Alt+Shift+Q`. The lock holds the
/// keyboard grab and VT switching is disabled, so a wrongly-stuck lock would
/// otherwise strand even the guardian (their only out: the power button). What
/// the chord *does* now depends on whether the device is PAIRED (see
/// `run_lock`):
///
/// - PAIRED (an offline-unlock challenge/expected pair is in the environment):
///   the chord NO LONGER pauses directly — it TOGGLES the unlock-entry panel,
///   and enforcement only pauses once the guardian's 8-digit code (answered
///   from Kintrinsic for the shown challenge) is entered and matches. A watching
///   child can't turn a static chord into a bypass.
/// - UNPAIRED (device-only): there is no guardian to issue a code, so the chord
///   opens a short NOTICE naming the admin route out (Log out → sign in as the
///   administrator → Kintrinsic Recovery). It does NOT pause.
///
///   It used to pause directly, and that was a hole: the chord is a static,
///   publicly-documented key combination, so a ward who read the source or
///   watched a parent use it once could switch enforcement off with three
///   modifiers and a Q — no password, no rate limit, on the standalone
///   no-phone setup that is a fully supported way to run Kintrinsic. The paired
///   path has always been gated by an 8-digit code; this closes the gap.
///
///   Closing it strands nobody, which is why it is safe to do: the panel's own
///   **Log out** button ends the child's session (thawing it first), and from
///   the greeter the administrator signs into their own account where
///   `charter-recovery` — pkexec'd, admin password, and root-euid-authoritative
///   — offers the very same pause. The chord was a duplicate of that route with
///   the credential removed, not the only way home.
fn is_escape_chord(state: u16, keycode: u8, q_keycodes: &[u8]) -> bool {
    const SHIFT: u16 = 1; // ModMask::SHIFT
    const CONTROL: u16 = 4; // ModMask::CONTROL
    const ALT: u16 = 8; // ModMask::M1
    let mods = SHIFT | CONTROL | ALT;
    (state & mods) == mods && q_keycodes.contains(&keycode)
}

/// Write the recovery pause flag (matches `charterd`'s `pause_flag_path` and
/// `charter-recovery`): the daemon's next (~2s) tick thaws every child, kills
/// this lock, and re-enables VT switching. Reached ONLY by the guardian's
/// verified offline-unlock code (paired devices). Device-only machines pause
/// through `charter-recovery` in an admin session instead — the chord there
/// explains that route rather than taking it (see `is_escape_chord`).
/// Best-effort: failures are logged, the lock stays up.
fn pause_enforcement() {
    let flag = std::env::var("CHARTER_PAUSE_FLAG").unwrap_or_else(|_| "/run/charter/paused".into());
    if let Some(dir) = std::path::Path::new(&flag).parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    match std::fs::write(&flag, b"paused\n") {
        Ok(()) => eprintln!("charter-lock: enforcement paused ({flag})"),
        Err(e) => eprintln!("charter-lock: failed to write pause flag {flag}: {e}"),
    }
}

// ---------------------------------------------------------------------------
// Offline guardian unlock (paired devices).
//
// charterd hands the lock a fresh per-lock CHALLENGE and the EXPECTED 8-digit
// code (derived from the guardian↔machine NIP-44 conversation key). The parent
// reads the challenge into Kintrinsic, which returns the code; the parent types
// it here. On a constant-time match we pause enforcement — no network, no
// clock, no memorized secret. A per-attempt rate limit makes guessing the 10^8
// code space hopeless. Everything below is pure and unit-tested; the X11 loop
// only feeds it keystrokes and paints what it returns.
// ---------------------------------------------------------------------------

/// Digits in an unlock code.
const UNLOCK_DIGITS: usize = 8;
/// Wrong attempts before entry is locked out.
const FAIL_THRESHOLD: u32 = 5;
/// First lockout length; each subsequent lockout doubles it.
const BASE_COOLDOWN_SECS: u64 = 30;
/// Cap on the doubling cooldown (~5 minutes).
const MAX_COOLDOWN_SECS: u64 = 300;

/// Whether an env-supplied expected code is a well-formed unlock target: exactly
/// 8 ASCII digits (mirrors `charter_crypto::unlock::UNLOCK_CODE_DIGITS`, kept
/// inline so the lock pulls in no crypto crate).
fn valid_expected(expected: &str) -> bool {
    expected.len() == UNLOCK_DIGITS && expected.bytes().all(|b| b.is_ascii_digit())
}

/// Constant-time byte equality: a same-length check then an XOR-accumulate over
/// every byte with NO early return, so a wrong unlock code can't be refined by
/// timing. Inlined rather than adding a crypto dependency to the lock.
fn ct_eq(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    let mut diff = 0u8;
    for (x, y) in a.iter().zip(b.iter()) {
        diff |= x ^ y;
    }
    diff == 0
}

/// The notice the device-only chord raises: the admin's real route out, in the
/// order they have to do it. Deliberately says nothing a ward could use — the
/// steps all end at a password they do not have — and stays honest that the
/// lock is not being lifted here. Pure, so it is unit-tested.
fn recovery_notice_text() -> LockText {
    LockText {
        title: "For the administrator".to_string(),
        detail: "Kintrinsic is still enforcing limits. To pause it, sign in as the administrator."
            .to_string(),
        lines: vec![
            "1. Press Log out below to end this session.".to_string(),
            "2. Sign in to the administrator account.".to_string(),
            "3. Open Menu → Kintrinsic → Recovery, then choose Pause.".to_string(),
            "Press Esc to go back.".to_string(),
        ],
    }
}

/// The X keysym → ASCII-digit-byte table for '0'..='9' (keysyms
/// `0x0030..=0x0039`). Pure; the per-keysym keycode resolution that turns this
/// into a keycode→digit map is the only X-dependent step.
fn digit_keysym_table() -> [(u32, u8); 10] {
    let mut t = [(0u32, 0u8); 10];
    for (i, slot) in t.iter_mut().enumerate() {
        *slot = (0x0030 + i as u32, b'0' + i as u8);
    }
    t
}

/// The partial-entry readout: the digits typed so far, then '_' for each of the
/// eight slots not yet filled (e.g. "1 2 3 _ _ _ _ _").
fn entry_display(typed: &str) -> String {
    (0..UNLOCK_DIGITS)
        .map(|i| typed.as_bytes().get(i).map(|&b| b as char).unwrap_or('_'))
        .fold(String::new(), |mut s, c| {
            if !s.is_empty() {
                s.push(' ');
            }
            s.push(c);
            s
        })
}

/// The per-attempt rate limiter. After [`FAIL_THRESHOLD`] wrong codes, entry is
/// locked out for a cooldown that starts at [`BASE_COOLDOWN_SECS`] and doubles
/// per lockout (capped at [`MAX_COOLDOWN_SECS`]). Pure over an injected `now`
/// (`Instant`) — it holds no clock of its own — so it is fully unit-tested.
struct RateLimit {
    fails: u32,
    lockouts: u32,
    cooldown_until: Option<Instant>,
}

impl RateLimit {
    fn new() -> Self {
        Self {
            fails: 0,
            lockouts: 0,
            cooldown_until: None,
        }
    }

    /// Whether entry is accepted right now (not inside a cooldown window).
    fn accepting(&self, now: Instant) -> bool {
        match self.cooldown_until {
            Some(until) => now >= until,
            None => true,
        }
    }

    /// Whole seconds left in the current cooldown (0 if none / already elapsed),
    /// rounded UP so the countdown never displays a premature 0.
    fn cooldown_remaining(&self, now: Instant) -> u64 {
        match self.cooldown_until {
            Some(until) if until > now => {
                let d = until - now;
                d.as_secs() + u64::from(d.subsec_nanos() > 0)
            }
            _ => 0,
        }
    }

    /// Record a wrong code. When the fail count crosses [`FAIL_THRESHOLD`], arm
    /// (and return) the next doubling cooldown. A cooldown that has already
    /// elapsed resets the burst first, so the threshold is per-burst.
    fn on_fail(&mut self, now: Instant) -> Option<Instant> {
        if self.cooldown_until.is_some_and(|u| now >= u) {
            self.fails = 0;
            self.cooldown_until = None;
        }
        self.fails += 1;
        if self.fails >= FAIL_THRESHOLD {
            let shift = self.lockouts.min(4); // 30,60,120,240, then capped
            let secs = (BASE_COOLDOWN_SECS << shift).min(MAX_COOLDOWN_SECS);
            let until = now + Duration::from_secs(secs);
            self.cooldown_until = Some(until);
            self.lockouts += 1;
            self.fails = 0;
            return Some(until);
        }
        None
    }

    /// A correct code clears all rate-limit state.
    fn reset(&mut self) {
        self.fails = 0;
        self.lockouts = 0;
        self.cooldown_until = None;
    }
}

/// What the unlock panel is currently saying under the entry row.
#[derive(Clone, Copy)]
enum UnlockStatus {
    /// Awaiting entry (no message).
    Prompt,
    /// The last submitted code did not match.
    Wrong,
    /// A correct code was entered — pausing enforcement.
    Unlocking,
}

/// The offline-unlock session for one lock: the challenge shown, the expected
/// code, whether the entry panel is up, the digits typed so far, the rate
/// limiter, and the last outcome. Built from the environment charterd sets.
struct UnlockState {
    challenge: String,
    expected: String,
    active: bool,
    typed: String,
    limiter: RateLimit,
    status: UnlockStatus,
}

impl UnlockState {
    /// Build from `CHARTER_LOCK_CHALLENGE` + `CHARTER_LOCK_UNLOCK_EXPECT`.
    /// `None` (device-only, or a malformed expected code) means "not
    /// unlock-capable" — the caller keeps the direct escape chord.
    fn from_env() -> Option<Self> {
        let challenge = std::env::var("CHARTER_LOCK_CHALLENGE").ok()?;
        let expected = std::env::var("CHARTER_LOCK_UNLOCK_EXPECT").ok()?;
        if challenge.trim().is_empty() || !valid_expected(&expected) {
            return None;
        }
        Some(Self {
            challenge,
            expected,
            active: false,
            typed: String::new(),
            limiter: RateLimit::new(),
            status: UnlockStatus::Prompt,
        })
    }
}

/// Compose the unlock-entry panel: the challenge shown prominently as the
/// title, a short instruction, the partial entry, and a status/cooldown line.
fn unlock_panel_text(u: &UnlockState, now: Instant) -> LockText {
    let mut lines = vec![
        "In Kintrinsic, open ‘Unlock a device’, enter this code,".to_string(),
        "then type the 8-digit code it shows here:".to_string(),
        entry_display(&u.typed),
    ];
    let wait = u.limiter.cooldown_remaining(now);
    if wait > 0 {
        lines.push(format!("Too many tries — wait {wait}s"));
    } else {
        match u.status {
            UnlockStatus::Wrong => {
                lines.push("That code didn't match — check Kintrinsic.".to_string())
            }
            UnlockStatus::Unlocking => lines.push("Unlocking…".to_string()),
            UnlockStatus::Prompt => {}
        }
    }
    LockText {
        title: format!("Unlock code: {}", u.challenge),
        detail: "Show this code to your guardian.".to_string(),
        lines,
    }
}

/// Execute the sanctioned action as root. Best-effort: a failed spawn is logged,
/// never a panic (the lock must stay up). Every click is logged (the daemon's
/// journal is the only forensic trail for "why did the box shut down").
/// `CHARTER_LOCK_DRYRUN=1` logs the action without executing it, so the panel
/// can be exercised on a live desktop session.
fn perform(action: Action, uid: Option<u32>, dry_run: bool) {
    if let Some(argv) = action_argv(action, uid) {
        eprintln!(
            "charter-lock: click -> {action:?} ({argv:?}){}",
            if dry_run { " [dry-run]" } else { "" }
        );
        if dry_run {
            return;
        }
        // Fire-and-forget: never block the lock's event loop on the action
        // (terminate-user against a frozen session can wait out logind's stop
        // timeout). Reap in a detached thread so nothing is left un-waited.
        // A CLEAN environment, always. This process holds the offline-unlock
        // code (`CHARTER_LOCK_UNLOCK_EXPECT`) and the root X cookie's path in
        // its own, and "Ask for more time" runs `charter` AS THE WARD via
        // `runuser`, which keeps whatever it is handed: the code would sit in
        // a ward-uid process's `/proc/<pid>/environ`, readable by any ward
        // process outside the frozen slice — which could then simply type it
        // in. None of the sanctioned actions need anything but a PATH.
        match Command::new(&argv[0])
            .args(&argv[1..])
            .env_clear()
            .env(
                "PATH",
                "/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin",
            )
            .spawn()
        {
            Ok(mut child) => {
                std::thread::spawn(move || {
                    let _ = child.wait();
                });
            }
            Err(e) => eprintln!("charter-lock: action {argv:?} failed to spawn: {e}"),
        }
    }
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let text = LockText::resolve(
        std::env::var("CHARTER_LOCK_TITLE").ok(),
        std::env::var("CHARTER_LOCK_DETAIL").ok(),
        std::env::var("CHARTER_LOCK_LINES").ok(),
        &args,
    );
    // Offline preview: compose the panel and dump the raw BGRX canvas to a
    // file — no X server needed. For UI iteration and CI snapshots.
    if let Ok(path) = std::env::var("CHARTER_LOCK_RENDER_TO") {
        if let Err(e) = render_preview(&text, &path) {
            eprintln!("charter-lock: preview: {e}");
            std::process::exit(1);
        }
        return;
    }
    if let Err(e) = run_lock(&text) {
        eprintln!("charter-lock: {e}");
        std::process::exit(1);
    }
}

/// Render the panel at a fixed preview size and write the raw BGRX buffer.
fn render_preview(text: &LockText, path: &str) -> Result<(), Box<dyn Error>> {
    let (w, h) = (1920u16, 1200u16);
    let title_font = load_font("CHARTER_LOCK_FONT_BOLD", TITLE_TTF)?;
    let body_font = load_font("CHARTER_LOCK_FONT", BODY_TTF)?;
    let canvas = render::render_panel(
        w,
        h,
        text,
        &button_layout(
            w,
            h,
            text.lines.len() as u16,
            std::env::var("CHARTER_LOCK_CAN_ASK").is_ok_and(|v| v == "1"),
        ),
        &title_font,
        &body_font,
        None,
    );
    std::fs::write(path, &canvas.buf)?;
    eprintln!("charter-lock: preview {w}x{h} BGRX -> {path}");
    Ok(())
}

// ---------------------------------------------------------------------------
// X11 lock. Always compiled (pure-Rust x11rb), run on a real display.
// ---------------------------------------------------------------------------

use x11rb::connection::{Connection, RequestConnection as _};
use x11rb::protocol::composite::{self, ConnectionExt as _};
use x11rb::protocol::xproto::{
    ConnectionExt as _, CreateGCAux, CreateWindowAux, Drawable, EventMask, Gcontext, GrabMode,
    ImageFormat, Screen, StackMode, SubwindowMode, Window, WindowClass,
};
use x11rb::protocol::Event;
use x11rb::rust_connection::RustConnection;
use x11rb::{COPY_DEPTH_FROM_PARENT, CURRENT_TIME, NONE};

use ab_glyph::FontRef;

/// A face for the panel: the env-named TTF if provided and readable, else the
/// vendored DejaVu bytes.
fn load_font(env: &str, vendored: &'static [u8]) -> Result<FontRef<'static>, Box<dyn Error>> {
    if let Ok(path) = std::env::var(env) {
        if let Ok(bytes) = std::fs::read(&path) {
            // Leak: one font per process lifetime, loaded once at startup.
            if let Ok(f) = FontRef::try_from_slice(Box::leak(bytes.into_boxed_slice())) {
                return Ok(f);
            }
            eprintln!("charter-lock: {env}={path} unusable — using the vendored face");
        }
    }
    Ok(FontRef::try_from_slice(vendored)?)
}

/// One plain GC for `PutImage`. IncludeInferiors: Mutter-family compositors
/// (Cinnamon's Muffin) reparent their GL stage window INSIDE the composite
/// overlay, so plain drawing on the overlay is clipped to "underneath" the
/// stage and never shows. With IncludeInferiors the upload ignores
/// child-window clipping and lands over the stage's pixels (harmless for the
/// lock's own childless window).
fn gc(conn: &RustConnection, win: Window) -> Result<Gcontext, Box<dyn Error>> {
    let gc = conn.generate_id()?;
    conn.create_gc(
        gc,
        win,
        &CreateGCAux::new().subwindow_mode(SubwindowMode::INCLUDE_INFERIORS),
    )?;
    Ok(gc)
}

/// Upload the composed panel to `target` in row bands (each request stays well
/// under the X maximum-request size).
fn draw(
    conn: &RustConnection,
    target: Drawable,
    gc: Gcontext,
    depth: u8,
    canvas: &render::Canvas,
) -> Result<(), Box<dyn Error>> {
    const BAND_ROWS: usize = 64;
    let row_bytes = canvas.w as usize * 4;
    for (i, band) in canvas.buf.chunks(row_bytes * BAND_ROWS).enumerate() {
        let y = (i * BAND_ROWS) as i16;
        let rows = (band.len() / row_bytes) as u16;
        conn.put_image(
            ImageFormat::Z_PIXMAP,
            target,
            gc,
            canvas.w,
            rows,
            0,
            y,
            0,
            depth,
            band,
        )?;
    }
    conn.flush()?;
    Ok(())
}

/// Capture the current screen contents (the child's frozen desktop) as a BGRX
/// buffer — the dimmed backdrop behind the dialog card. Read from the
/// composite overlay when available (under a compositor that is the real
/// glass; the root window only holds the wallpaper), else the root. Must run
/// BEFORE the first paint, or we capture our own panel. Best-effort: `None`
/// falls back to a solid backdrop.
fn capture_screen(conn: &RustConnection, target: Drawable, w: u16, h: u16) -> Option<Vec<u8>> {
    let reply = conn
        .get_image(ImageFormat::Z_PIXMAP, target, 0, 0, w, h, !0)
        .ok()?
        .reply()
        .ok()?;
    (reply.data.len() == w as usize * h as usize * 4).then_some(reply.data)
}

/// Every keycode whose mapping contains one of `keysyms` (for the escape
/// chord: 'q'/'Q'). Resolved once at startup from the server's keyboard map.
fn keycodes_for(conn: &RustConnection, keysyms: &[u32]) -> Vec<u8> {
    let setup = conn.setup();
    let (min, max) = (setup.min_keycode, setup.max_keycode);
    let mut out = Vec::new();
    if let Ok(cookie) = conn.get_keyboard_mapping(min, max - min + 1) {
        if let Ok(map) = cookie.reply() {
            let per = map.keysyms_per_keycode as usize;
            for (i, chunk) in map.keysyms.chunks(per.max(1)).enumerate() {
                if chunk.iter().any(|s| keysyms.contains(s)) {
                    out.push(min + i as u8);
                }
            }
        }
    }
    out
}

/// The X Composite overlay window, if the extension is available.
///
/// Under a compositing WM (Cinnamon/Muffin) every window's pixels reach the
/// screen only when the compositor repaints — and the compositor lives in the
/// child's user slice, which charterd has just FROZEN. A frozen compositor
/// leaves the last composited frame on screen forever, so the lock's own
/// (redirected) window is mapped and grabbing input but never visible: the
/// child sees a frozen desktop and clicks invisible buttons. Drawing directly
/// on the overlay window bypasses redirection — the pixels land on the real
/// screen, and the frozen compositor cannot paint over them.
/// Read back one pixel from `target` and log it — post-paint forensics for the
/// journal: BG/BTN hex values prove the panel's pixels actually landed where
/// we drew them; anything else means something is still in the way.
fn probe_pixel(conn: &RustConnection, target: Drawable, x: i16, y: i16, what: &str) {
    let px = conn
        .get_image(ImageFormat::Z_PIXMAP, target, x, y, 1, 1, !0)
        .ok()
        .and_then(|c| c.reply().ok())
        .map(|r| {
            r.data
                .iter()
                .take(4)
                .rev()
                .fold(0u32, |acc, b| (acc << 8) | *b as u32)
        });
    match px {
        Some(v) => eprintln!("charter-lock: probe {what} ({x},{y}) = {v:#010x}"),
        None => eprintln!("charter-lock: probe {what} ({x},{y}) failed"),
    }
}

fn overlay_window(conn: &RustConnection, root: Window) -> Option<Window> {
    conn.extension_information(composite::X11_EXTENSION_NAME)
        .ok()
        .flatten()?;
    conn.composite_query_version(0, 3).ok()?.reply().ok()?;
    Some(
        conn.composite_get_overlay_window(root)
            .ok()?
            .reply()
            .ok()?
            .overlay_win,
    )
}

/// Grab the keyboard AND pointer for the lock window, verifying each reply's
/// `GrabStatus` (not just that the request was sent). A grab can transiently
/// fail — another client momentarily holds one, or the window isn't yet viewable
/// during a compositor/WM restart — so retry briefly. A partial grab (one of the
/// two) is released before the next attempt so we never spin holding half the
/// input. Returns an error if input can't be fully seized after the retries; the
/// caller exits non-zero and charterd re-spawns the lock next tick, rather than
/// leaving a decorative panel that doesn't capture input.
fn seize_input(conn: &RustConnection, win: Window) -> Result<(), Box<dyn Error>> {
    use x11rb::protocol::xproto::GrabStatus;
    const ATTEMPTS: u32 = 10;
    for attempt in 0..ATTEMPTS {
        let kb = conn
            .grab_keyboard(true, win, CURRENT_TIME, GrabMode::ASYNC, GrabMode::ASYNC)?
            .reply()?;
        let ptr = conn
            .grab_pointer(
                true,
                win,
                EventMask::BUTTON_PRESS,
                GrabMode::ASYNC,
                GrabMode::ASYNC,
                win,
                NONE,
                CURRENT_TIME,
            )?
            .reply()?;
        if kb.status == GrabStatus::SUCCESS && ptr.status == GrabStatus::SUCCESS {
            return Ok(());
        }
        // Release whichever half succeeded before retrying.
        if kb.status == GrabStatus::SUCCESS {
            conn.ungrab_keyboard(CURRENT_TIME)?;
        }
        if ptr.status == GrabStatus::SUCCESS {
            conn.ungrab_pointer(CURRENT_TIME)?;
        }
        conn.flush()?;
        eprintln!(
            "charter-lock: input grab attempt {}/{ATTEMPTS} failed (kb={:?}, ptr={:?}); retrying",
            attempt + 1,
            kb.status,
            ptr.status,
        );
        std::thread::sleep(std::time::Duration::from_millis(100));
    }
    Err("could not seize keyboard/pointer input after retries (another grab held?)".into())
}

fn run_lock(text: &LockText) -> Result<(), Box<dyn Error>> {
    let (conn, screen_num) = RustConnection::connect(None)?;
    let screen: &Screen = &conn.setup().roots[screen_num];
    let (root, w, h) = (screen.root, screen.width_in_pixels, screen.height_in_pixels);

    let win = conn.generate_id()?;
    conn.create_window(
        COPY_DEPTH_FROM_PARENT,
        win,
        root,
        0,
        0,
        w,
        h,
        0,
        WindowClass::INPUT_OUTPUT,
        screen.root_visual,
        &CreateWindowAux::new()
            .background_pixel(render::BG)
            .override_redirect(1) // bypass the WM — cannot be moved/closed
            .event_mask(EventMask::EXPOSURE | EventMask::KEY_PRESS | EventMask::BUTTON_PRESS),
    )?;
    // The overlay is the frozen-compositor path (see overlay_window); the lock's
    // own window covers the live- and no-compositor cases. Both show identical
    // pixels, so whichever path reaches the screen, the panel looks the same.
    let overlay = overlay_window(&conn, root);
    eprintln!(
        "charter-lock: up on {} ({w}x{h}), overlay={}",
        std::env::var("DISPLAY").unwrap_or_else(|_| "?".into()),
        if overlay.is_some() { "yes" } else { "no" },
    );

    // Capture the desktop BEFORE mapping our window — on a live compositor the
    // mapped window is composited within a frame and we would capture our own
    // panel instead of the desktop it should dim.
    let capture = capture_screen(&conn, overlay.unwrap_or(root), w, h);

    conn.map_window(win)?;
    conn.configure_window(
        win,
        &x11rb::protocol::xproto::ConfigureWindowAux::new().stack_mode(StackMode::ABOVE),
    )?;

    // Seize all input so nothing behind the lock can be reached. The grab reply
    // STATUS must be checked — `?` alone only catches request I/O, not an
    // AlreadyGrabbed/NotViewable/Frozen grab, and a lock that paints without
    // actually holding the grab lets keystrokes reach the window behind it (a
    // direct escape for a motivated ward). Fails hard if input can't be seized.
    seize_input(&conn, win)?;

    let title_font = load_font("CHARTER_LOCK_FONT_BOLD", TITLE_TTF)?;
    let body_font = load_font("CHARTER_LOCK_FONT", BODY_TTF)?;
    let gc = gc(&conn, win)?;
    let depth = screen.root_depth;
    conn.flush()?;

    // Compose the panel (rebuilt only when the ask feedback lands), upload on
    // expose and on a ~1s ticker (the ticker repaints the overlay after the
    // compositor's final frame lands, and self-heals if anything scribbles
    // over it). A click on a sanctioned-escape button is mapped to its action
    // and executed; all other input is swallowed by the grab. The daemon kills
    // this process to unlock (the dropped connection releases the grab,
    // destroys the window, and releases the overlay).
    let mut text = text.clone();
    // Offline guardian unlock (paired only): charterd passes a fresh challenge +
    // the expected code. When present, the escape chord opens an entry panel
    // instead of pausing directly, and a subtle always-visible hint tells a
    // parent the escape exists.
    let mut unlock = UnlockState::from_env();
    if unlock.is_some() {
        text.lines
            .push("Parent: press Ctrl+Alt+Shift+Q to unlock with Kintrinsic.".to_string());
    } else {
        // Device-only: the same chord, but it explains the admin route rather
        // than lifting anything (see `is_escape_chord`). Still advertised, so a
        // parent standing at a locked machine is never hunting for the way out.
        text.lines
            .push("Parent: press Ctrl+Alt+Shift+Q for administrator options.".to_string());
    }
    // Is the device-only admin-recovery notice currently on screen?
    let mut recovery_notice = false;
    // The humane ask is offered only when a guardian is paired to receive it
    // (the daemon sets both) — and only once per lock.
    let mut can_ask = std::env::var("CHARTER_LOCK_CAN_ASK").is_ok_and(|v| v == "1")
        && std::env::var("CHARTER_LOCK_USER").is_ok();
    // One place composes the whole panel — the normal card or the unlock-entry
    // card — so both paths render identically.
    let compose = |text: &LockText, buttons: &[Button]| -> render::Canvas {
        render::render_panel(
            w,
            h,
            text,
            buttons,
            &title_font,
            &body_font,
            capture.clone(),
        )
    };
    let mut buttons = button_layout(w, h, text.lines.len() as u16, can_ask);
    let mut canvas = compose(&text, &buttons);
    let uid = managed_uid_from_env();
    let dry_run = std::env::var("CHARTER_LOCK_DRYRUN").is_ok_and(|v| v == "1");
    // 'q'/'Q' keycodes for the guardian/admin escape chord (see is_escape_chord).
    let q_keycodes = keycodes_for(&conn, &[0x0071, 0x0051]);
    // Unlock-entry keys, resolved once from the server map: digit keycode →
    // ASCII digit (main row + numeric keypad), plus Return/BackSpace/Escape.
    let mut digit_map: std::collections::HashMap<u8, u8> = std::collections::HashMap::new();
    for (keysym, digit) in digit_keysym_table() {
        for kc in keycodes_for(&conn, &[keysym]) {
            digit_map.insert(kc, digit);
        }
    }
    for d in 0u8..10 {
        // KP_0..KP_9 = 0xffb0..0xffb9 (numeric keypad digits).
        for kc in keycodes_for(&conn, &[0xffb0 + d as u32]) {
            digit_map.insert(kc, b'0' + d);
        }
    }
    let return_keys = keycodes_for(&conn, &[0xff0d, 0xff8d]); // Return + KP_Enter
    let backspace_keys = keycodes_for(&conn, &[0xff08]);
    let escape_keys = keycodes_for(&conn, &[0xff1b]);
    let paint = |canvas: &render::Canvas, targets_overlay: bool| -> Result<(), Box<dyn Error>> {
        draw(&conn, win, gc, depth, canvas)?;
        if targets_overlay {
            if let Some(ov) = overlay {
                draw(&conn, ov, gc, depth, canvas)?;
            }
        }
        Ok(())
    };
    paint(&canvas, true)?;
    // One-shot forensics: prove the panel's pixels landed on the overlay (the
    // journal is all we get to see from a frozen child session).
    if let Some(ov) = overlay {
        probe_pixel(&conn, ov, 8, 8, "overlay/bg");
        if let Some(b) = buttons.last() {
            probe_pixel(
                &conn,
                ov,
                b.rect.x + (b.rect.width / 2) as i16,
                b.rect.y + (b.rect.height / 2) as i16,
                "overlay/button",
            );
        }
    }
    let mut last_tick = std::time::Instant::now();
    loop {
        while let Some(event) = conn.poll_for_event()? {
            match event {
                Event::Expose(_) => paint(&canvas, false)?,
                Event::ButtonPress(ev) => {
                    if let Some(action) = hit_test(&buttons, ev.event_x, ev.event_y) {
                        if action == Action::AskMoreTime && !can_ask {
                            continue; // already asked this lock
                        }
                        perform(action, uid, dry_run);
                        if action == Action::AskMoreTime {
                            // One-shot: swap the primary for a confirmation
                            // line so the child knows the ask is on its way.
                            can_ask = false;
                            text.lines
                                .push("Asked! Your guardian will see it shortly.".into());
                            buttons = button_layout(w, h, text.lines.len() as u16, false);
                            canvas = compose(&text, &buttons);
                            paint(&canvas, true)?;
                        }
                    }
                }
                Event::KeyPress(ev) => {
                    let chord = is_escape_chord(ev.state.into(), ev.detail, &q_keycodes);
                    match unlock.as_mut() {
                        // PAIRED (unlock-capable): the chord TOGGLES the entry
                        // panel; digits/BackSpace/Return/Escape drive it. The
                        // chord no longer pauses directly — only a matching code
                        // does. A watching child can't turn it into a bypass.
                        Some(u) => {
                            let now = Instant::now();
                            let mut dirty = false;
                            let mut do_pause = false;
                            if chord {
                                u.active = !u.active;
                                u.typed.clear();
                                u.status = UnlockStatus::Prompt;
                                dirty = true;
                            } else if u.active {
                                if escape_keys.contains(&ev.detail) {
                                    // Cancel back to the normal panel.
                                    u.active = false;
                                    u.typed.clear();
                                    u.status = UnlockStatus::Prompt;
                                    dirty = true;
                                } else if u.limiter.accepting(now) {
                                    if backspace_keys.contains(&ev.detail) {
                                        u.typed.pop();
                                        dirty = true;
                                    } else if return_keys.contains(&ev.detail) {
                                        if u.typed.len() == UNLOCK_DIGITS {
                                            if ct_eq(u.expected.as_bytes(), u.typed.as_bytes()) {
                                                u.status = UnlockStatus::Unlocking;
                                                u.limiter.reset();
                                                u.typed.clear();
                                                do_pause = true;
                                            } else {
                                                u.limiter.on_fail(now);
                                                u.status = UnlockStatus::Wrong;
                                                u.typed.clear();
                                            }
                                            dirty = true;
                                        }
                                    } else if let Some(&d) = digit_map.get(&ev.detail) {
                                        if u.typed.len() < UNLOCK_DIGITS {
                                            u.typed.push(d as char);
                                            u.status = UnlockStatus::Prompt;
                                            dirty = true;
                                        }
                                    }
                                } else {
                                    // Cooling down: ignore digit input, but
                                    // refresh so the countdown is current.
                                    dirty = true;
                                }
                            }
                            if dirty {
                                if u.active {
                                    let utext = unlock_panel_text(u, now);
                                    canvas = compose(&utext, &[]);
                                    buttons = Vec::new();
                                } else {
                                    buttons = button_layout(w, h, text.lines.len() as u16, can_ask);
                                    canvas = compose(&text, &buttons);
                                }
                                // Show "Unlocking…" BEFORE we pause, so the
                                // parent sees the outcome even as the daemon
                                // tears the lock down on its next tick.
                                paint(&canvas, true)?;
                            }
                            if do_pause {
                                if dry_run {
                                    eprintln!("charter-lock: offline unlock accepted [dry-run]");
                                } else {
                                    pause_enforcement();
                                }
                            }
                        }
                        // DEVICE-ONLY (unpaired): no guardian to issue a code, so
                        // the chord raises the admin-recovery NOTICE instead of
                        // pausing. Pausing on a bare, publicly-known chord was a
                        // credential-free bypass a ward could repeat at will; the
                        // gated route (Log out → admin session → Recovery) is
                        // spelled out on the notice. See `is_escape_chord`.
                        None => {
                            let toggle =
                                chord || (recovery_notice && escape_keys.contains(&ev.detail));
                            if toggle {
                                recovery_notice = !recovery_notice;
                                if recovery_notice {
                                    // No buttons on the notice would strand the
                                    // admin one step from Log out — the very
                                    // button step 1 tells them to press — so the
                                    // normal escapes stay on screen.
                                    let ntext = recovery_notice_text();
                                    buttons = button_layout(w, h, ntext.lines.len() as u16, false);
                                    canvas = compose(&ntext, &buttons);
                                } else {
                                    buttons = button_layout(w, h, text.lines.len() as u16, can_ask);
                                    canvas = compose(&text, &buttons);
                                }
                                paint(&canvas, true)?;
                                if dry_run {
                                    eprintln!(
                                        "charter-lock: device-only recovery notice {} [dry-run]",
                                        if recovery_notice {
                                            "shown"
                                        } else {
                                            "dismissed"
                                        }
                                    );
                                }
                            }
                        }
                    }
                }
                Event::Error(e) => eprintln!(
                    "charter-lock: X error {:?} (opcode {}.{})",
                    e.error_kind, e.major_opcode, e.minor_opcode
                ),
                _ => {}
            }
        }
        if last_tick.elapsed() >= Duration::from_secs(1) {
            // While the unlock panel is up, recompose it each tick so the
            // rate-limit countdown ("wait Ns") ticks down live.
            if let Some(u) = unlock.as_ref() {
                if u.active {
                    let utext = unlock_panel_text(u, Instant::now());
                    canvas = compose(&utext, &[]);
                    buttons = Vec::new();
                }
            }
            paint(&canvas, true)?;
            last_tick = Instant::now();
        }
        std::thread::sleep(Duration::from_millis(150));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolve_prefers_env_then_args_then_default() {
        // env wins
        let t = LockText::resolve(Some("Bedtime".into()), None, None, &["IGN".into()]);
        assert_eq!(t.title, "Bedtime");
        assert!(t.detail.contains("resumes")); // default detail
                                               // args fill in when env is absent
        let t = LockText::resolve(
            None,
            None,
            Some("  line one \n\n line two ".into()),
            &["Daily limit reached".into(), "Back at 7am".into()],
        );
        assert_eq!(t.title, "Daily limit reached");
        assert_eq!(t.detail, "Back at 7am");
        // extra lines are trimmed and blank ones dropped
        assert_eq!(t.lines, vec!["line one".to_string(), "line two".into()]);
        // blank env is ignored, falls through to default
        let t = LockText::resolve(Some("   ".into()), None, None, &[]);
        assert_eq!(t.title, "Time's up for now");
    }

    #[test]
    fn escape_chord_needs_all_three_modifiers_and_a_q() {
        let q = [24u8, 54u8];
        let all = 1 | 4 | 8; // Shift+Ctrl+Alt
        assert!(is_escape_chord(all, 24, &q));
        assert!(is_escape_chord(all | 16, 54, &q)); // extra mods (NumLock) fine
        assert!(!is_escape_chord(1 | 4, 24, &q)); // missing Alt
        assert!(!is_escape_chord(all, 25, &q)); // not a Q
        assert!(!is_escape_chord(0, 24, &q)); // bare key
    }

    // The device-only chord must never read as "enforcement is off now". It
    // names the admin route (which ends at a password the ward hasn't got) and
    // says plainly that limits are still being enforced.
    #[test]
    fn device_only_recovery_notice_points_at_the_gated_route_and_lifts_nothing() {
        let t = recovery_notice_text();
        let body = format!("{} {} {}", t.title, t.detail, t.lines.join(" "));
        assert!(body.contains("administrator"));
        assert!(body.contains("Log out"));
        assert!(body.contains("Recovery"));
        // Still enforcing — the notice explains, it does not unlock.
        assert!(body.contains("still enforcing"));
        // And it must not promise an unlock the chord no longer performs.
        assert!(!body.to_lowercase().contains("unlocked"));
        // Esc gets the ward's own panel back.
        assert!(body.contains("Esc"));
    }

    #[test]
    fn action_argv_is_the_expected_sanctioned_command() {
        // Logout: thaw the child's slice FIRST (SIGTERM never lands on frozen
        // tasks — the half-dead session can wedge logind), then terminate.
        let logout = action_argv(Action::Logout, Some(1002)).expect("offered with a uid");
        assert_eq!(logout[..2], ["/bin/sh".to_string(), "-c".to_string()]);
        assert!(logout[2].contains("user-1002.slice/cgroup.freeze"));
        assert!(logout[2].contains("loginctl terminate-user 1002"));
        assert!(logout[2].find("cgroup.freeze") < logout[2].find("terminate-user"));
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

    #[test]
    fn valid_expected_requires_exactly_eight_ascii_digits() {
        assert!(valid_expected("00012345"));
        assert!(!valid_expected("0001234")); // 7 digits
        assert!(!valid_expected("000123456")); // 9 digits
        assert!(!valid_expected("0001234a")); // non-digit
        assert!(!valid_expected(""));
    }

    #[test]
    fn ct_eq_matches_only_equal_byte_strings() {
        assert!(ct_eq(b"12345678", b"12345678"));
        assert!(!ct_eq(b"12345678", b"12345679"));
        assert!(!ct_eq(b"1234567", b"12345678")); // length mismatch is a non-match
        assert!(ct_eq(b"", b""));
        assert!(!ct_eq(b"", b"0"));
    }

    #[test]
    fn digit_keysym_table_covers_zero_through_nine() {
        let t = digit_keysym_table();
        assert_eq!(t.len(), 10);
        assert_eq!(t[0], (0x0030, b'0'));
        assert_eq!(t[9], (0x0039, b'9'));
        // Contiguous keysyms map to contiguous ASCII digits.
        for (i, (keysym, digit)) in t.iter().enumerate() {
            assert_eq!(*keysym, 0x0030 + i as u32);
            assert_eq!(*digit, b'0' + i as u8);
        }
    }

    #[test]
    fn entry_display_fills_typed_then_blanks() {
        assert_eq!(entry_display(""), "_ _ _ _ _ _ _ _");
        assert_eq!(entry_display("123"), "1 2 3 _ _ _ _ _");
        assert_eq!(entry_display("12345678"), "1 2 3 4 5 6 7 8");
    }

    #[test]
    fn rate_limit_thresholds_then_backs_off_with_doubling() {
        let mut r = RateLimit::new();
        let t0 = Instant::now();
        // The first FAIL_THRESHOLD-1 wrong codes still accept input.
        for _ in 0..(FAIL_THRESHOLD - 1) {
            assert!(r.on_fail(t0).is_none());
            assert!(r.accepting(t0));
        }
        // The threshold-crossing fail arms the base (30s) cooldown.
        let until = r.on_fail(t0).expect("cooldown armed at the threshold");
        assert!(!r.accepting(t0));
        assert_eq!((until - t0).as_secs(), BASE_COOLDOWN_SECS);
        assert!(!r.accepting(t0 + Duration::from_secs(BASE_COOLDOWN_SECS - 1)));
        assert!(r.accepting(t0 + Duration::from_secs(BASE_COOLDOWN_SECS)));
        // A second burst after reopening doubles the cooldown to 60s.
        let t1 = t0 + Duration::from_secs(BASE_COOLDOWN_SECS);
        for _ in 0..(FAIL_THRESHOLD - 1) {
            r.on_fail(t1);
        }
        let until2 = r.on_fail(t1).expect("second cooldown");
        assert_eq!((until2 - t1).as_secs(), BASE_COOLDOWN_SECS * 2);
    }

    #[test]
    fn rate_limit_caps_at_five_minutes_and_reset_clears_it() {
        let mut r = RateLimit::new();
        let mut t = Instant::now();
        let mut last = 0u64;
        // Drive successive lockouts: 30, 60, 120, 240, then capped at 300.
        for _ in 0..8 {
            for _ in 0..FAIL_THRESHOLD {
                if let Some(u) = r.on_fail(t) {
                    last = (u - t).as_secs();
                }
            }
            t += Duration::from_secs(last + 1); // step past the cooldown
        }
        assert_eq!(last, MAX_COOLDOWN_SECS); // doubling is capped
                                             // A correct code wipes the state: accepting again, burst counter reset.
        r.reset();
        assert!(r.accepting(t));
        assert!(r.on_fail(t).is_none());
    }

    #[test]
    fn cooldown_remaining_counts_down_whole_seconds() {
        let mut r = RateLimit::new();
        let t0 = Instant::now();
        for _ in 0..FAIL_THRESHOLD {
            r.on_fail(t0);
        }
        assert_eq!(r.cooldown_remaining(t0), BASE_COOLDOWN_SECS);
        // Rounds up: a fraction of a second left still reads as 1.
        assert_eq!(
            r.cooldown_remaining(t0 + Duration::from_millis(BASE_COOLDOWN_SECS * 1000 - 100)),
            1
        );
        assert_eq!(
            r.cooldown_remaining(t0 + Duration::from_secs(BASE_COOLDOWN_SECS)),
            0
        );
    }

    #[test]
    fn unlock_state_from_env_validates_and_hides_when_malformed() {
        // A serial guard so the env mutation below can't race another test.
        use std::sync::Mutex;
        static ENV_LOCK: Mutex<()> = Mutex::new(());
        let _g = ENV_LOCK.lock().unwrap();
        std::env::set_var("CHARTER_LOCK_CHALLENGE", "K7QN");
        std::env::set_var("CHARTER_LOCK_UNLOCK_EXPECT", "48437822");
        let u = UnlockState::from_env().expect("well-formed pair -> unlock-capable");
        assert!(!u.active);
        assert_eq!(u.challenge, "K7QN");
        assert_eq!(u.expected, "48437822");
        // A malformed expected code -> not unlock-capable (keep the direct chord).
        std::env::set_var("CHARTER_LOCK_UNLOCK_EXPECT", "48A37822");
        assert!(UnlockState::from_env().is_none());
        std::env::remove_var("CHARTER_LOCK_CHALLENGE");
        std::env::remove_var("CHARTER_LOCK_UNLOCK_EXPECT");
        assert!(UnlockState::from_env().is_none());
    }
}
