//! `charter-xclients` — "which process owns each window on this display?",
//! answered by the X **server** instead of by a property the ward can rewrite.
//!
//! The meter used to read `_NET_WM_PID` and `_NET_CLIENT_LIST` with `xprop`.
//! X grants every client on a display full property access to every other
//! client's windows, so `xprop -id <win> -remove _NET_WM_PID` made a metered
//! app unattributable — and under `TimeModel::Named` an unattributable window
//! charges nothing. Both properties are ward-writable; neither is identity.
//!
//! Identity here comes from two things the ward cannot forge:
//!
//! - **the pid** — XRes `QueryClientIds` with `LOCAL_CLIENT_PID`, which the
//!   server derives from the owning client's socket peer credentials. There is
//!   no property to delete and no value to set;
//! - **the window set** — `_NET_CLIENT_LIST` **plus** a walk of the real window
//!   tree. The property can now only ADD windows: deleting or emptying it hides
//!   nothing, because a window the ward can actually see is viewable in the
//!   tree and the walk finds it anyway.
//!
//! # Why a separate process
//!
//! The ward controls the X server on their own display — they can wedge it,
//! and on a hostile server a request can simply never be answered. charterd
//! runs this under `timeout 2`, exactly as it used to run `xprop`, so the worst
//! a wedged or hostile server can do is hang a short-lived child. A daemon task
//! blocked on an X reply would stall the tick loop for every child on the box.
//!
//! Runs unprivileged-shaped (no euid check — it reads, it never enacts) against
//! the `DISPLAY`/`XAUTHORITY` in its environment, and prints:
//!
//! ```text
//! v1
//! capped                     (only when a budget below truncated the answer)
//! dpms <on|standby|suspend|off|unknown>
//! focus <pid|->
//! active <pid|->
//! win <0xhexid> <pid|->
//! ```
//!
//! The `dpms` line is the display's POWER state, and it is here because it is
//! the only "is the ward actually at this machine?" signal on a Linux desktop
//! that the ward cannot set. logind's `IdleHint`/`LockedHint` — which the meter
//! used to trust — are writable by the session's own user with no polkit prompt
//! at all (`busctl --system call org.freedesktop.login1 \
//! /org/freedesktop/login1/session/self org.freedesktop.login1.Session \
//! SetIdleHint b true`), so a ward could loop that and never be charged a
//! second. The X server's DPMS power level is server state: setting it means
//! actually powering the monitor down, which is indistinguishable from not
//! using the machine because it IS not using the machine.
//!
//! Exit 0 with that on stdout, or non-zero with nothing on stdout. Anything the
//! server says is treated as data: no indexing, no `unwrap`, no panic path.

use std::collections::{BTreeMap, BTreeSet};
use std::error::Error;
use std::fmt::Write as _;

use x11rb::connection::{Connection, RequestConnection as _};
use x11rb::cookie::Cookie;
use x11rb::errors::ReplyError;
use x11rb::protocol::dpms::{self, ConnectionExt as _, InfoReply};
use x11rb::protocol::res::{self, ClientIdMask, ClientIdSpec, ConnectionExt as _};
use x11rb::protocol::xproto::{AtomEnum, ConnectionExt as _, InputFocus, MapState, WindowClass};
use x11rb::rust_connection::RustConnection;

type Window = u32;

/// A window narrower or shorter than this in either axis is not something a
/// ward is *using*: tray icons, the 1×1 helper windows toolkits create to own a
/// selection, IME candidate strips. Including them would attribute a browser's
/// own scaffolding as an open app, and — worse — would push the walk's window
/// count towards the cap on a busy desktop for no attribution gained.
const MIN_EDGE: u16 = 64;

/// Two levels below the root's own children. A reparenting window manager owns
/// the frame it wraps an app in, so the frame's pid is the WM's; the app's real
/// window is the frame's child, and one more level covers a WM that inserts a
/// decoration container between the two. Deeper than that is an app's own
/// internal widget windows, which add pids we already have.
const EXTRA_LEVELS: usize = 2;

/// Hard ceiling on windows REPORTED by the tree walk — windows that passed the
/// filter, so a ward cannot spend this budget on junk.
///
/// A ward can create windows faster than we can ask about them, and this probe
/// runs every 2 s: without a cap, "open ten thousand windows" makes the probe
/// time out, and a timed-out probe is a display charterd cannot read.
const MAX_WINDOWS: usize = 4096;

/// Ceiling on the attributes/geometry request PAIRS the walk issues. Separate
/// from [`MAX_WINDOWS`] because the filter itself costs a round trip: the cheap
/// windows a flood is made of are rejected, but they are not free to reject.
const MAX_PROBES: usize = 16384;

/// **Every cap in this file reports itself.** A silently truncated walk is a
/// fail-OPEN: create more root children than the walk will look at, delete
/// `_NET_CLIENT_LIST`, focus a junk window, and the snapshot comes back with no
/// app window and no `-` either — which charterd reads as "read fine, nothing
/// costing is open" and charges nothing. So any cap that bites sets this, the
/// run prints a bare `capped` line, and charterd treats the whole snapshot the
/// way it treats an unattributable window: fall back to the ward's process
/// table and withdraw any free-learning credit for that tick. A display we
/// could not fully read must never be cheaper than one we could.
#[derive(Default)]
struct Capped(bool);

fn main() {
    match run() {
        // Printed in one go, so a failure half way through leaves nothing
        // parseable on stdout for charterd to mistake for an answer.
        Ok(out) => print!("{out}"),
        Err(e) => {
            eprintln!("charter-xclients: {e}");
            std::process::exit(1);
        }
    }
}

fn run() -> Result<String, Box<dyn Error>> {
    let (conn, screen) = RustConnection::connect(None)?;
    let root = conn
        .setup()
        .roots
        .get(screen)
        .ok_or("the server named a screen it does not have")?
        .root;

    // First batch: everything that needs no earlier answer. `prefetch_…` starts
    // the QueryExtension round trip for XRes now rather than inside the first
    // `res::` call, so the atom interning and the focus/tree queries ride along
    // with it instead of waiting behind it.
    conn.prefetch_extension_information(res::X11_EXTENSION_NAME)?;
    conn.prefetch_extension_information(dpms::X11_EXTENSION_NAME)?;
    let active_atom = conn.intern_atom(true, b"_NET_ACTIVE_WINDOW")?;
    let list_atom = conn.intern_atom(true, b"_NET_CLIENT_LIST")?;
    let focus = conn.get_input_focus()?;
    let tree = conn.query_tree(root)?;

    // QueryClientIds is XRes 1.2. On an older server the request does not
    // exist, and there is no weaker answer worth returning — a pid read from a
    // property is exactly what this binary was written to stop trusting.
    let version = res::query_version(&conn, 1, 2)?.reply()?;
    if (version.server_major, version.server_minor) < (1, 2) {
        return Err(format!(
            "XRes {}.{} is too old — QueryClientIds needs 1.2",
            version.server_major, version.server_minor
        )
        .into());
    }

    // Issued here so it rides the same round trip as the replies read below.
    // Unlike XRes, a missing DPMS extension is NOT fatal: it costs the meter a
    // fact it would like, and the caller's mapping treats "unknown" exactly as
    // it treats "on" — the charging direction. Refusing to answer at all would
    // instead hand a ward with a DPMS-less server a display charterd cannot
    // read, which is strictly worse for everything else in this snapshot.
    let dpms = conn.dpms_info().ok();

    let active_atom = active_atom.reply()?.atom;
    let list_atom = list_atom.reply()?.atom;
    let focus_win = real_window(focus.reply()?.focus, root);
    let children = tree.reply()?.children;

    let mut capped = Capped::default();

    // Second batch: the two root properties, now that their atoms are known.
    // Read as `ANY` type rather than `WINDOW` — a ward who retypes the property
    // must not be able to make it unreadable, and this set only ever ADDS to
    // what the tree walk found.
    //
    // `_NET_ACTIVE_WINDOW` is one word by definition, so a longer value is a
    // malformed property rather than a cap biting, and is not reported as one.
    // A `_NET_CLIENT_LIST` longer than we asked for IS a cap biting: the same
    // hole as a truncated walk, reached through the property instead.
    let (active_prop, _) = window_prop(&conn, root, active_atom, 1)?;
    let (list_prop, list_truncated) = window_prop(&conn, root, list_atom, MAX_WINDOWS as u32)?;
    capped.0 |= list_truncated;

    let mut windows: BTreeSet<Window> = BTreeSet::new();
    windows.extend(focus_win);
    let active_win = active_prop
        .first()
        .copied()
        .and_then(|w| real_window(w, root));
    windows.extend(active_win);
    windows.extend(list_prop.iter().copied().filter(|w| *w != 0 && *w != root));
    walk(&conn, children, &mut windows, &mut capped)?;

    let pids = client_pids(&conn, &windows)?;
    let pid_of = |w: Option<Window>| w.and_then(|w| pids.get(&w).copied().flatten());

    let mut out = String::from("v1\n");
    if capped.0 {
        out.push_str("capped\n");
    }
    let _ = writeln!(out, "dpms {}", dpms_level(dpms));
    let _ = writeln!(out, "focus {}", field(pid_of(focus_win)));
    let _ = writeln!(out, "active {}", field(pid_of(active_win)));
    for (win, pid) in &pids {
        let _ = writeln!(out, "win 0x{win:x} {}", field(*pid));
    }
    Ok(out)
}

/// The monitor's power level, as `DPMSInfo` reports it.
///
/// `unknown` covers every way the question can go unanswered — no DPMS
/// extension on this server, a request that could not even be serialised, an X
/// error in the reply — and charterd maps it the same way it maps `on`: keep
/// charging. An idle classification must be POSITIVELY evidenced, exactly as
/// every other probe in this tree fails toward the costing direction.
///
/// `state` (DPMS ENABLED) is deliberately folded in ahead of `power_level`: a
/// server with DPMS disabled does no power management at all, so whatever
/// `power_level` it reports is stale bookkeeping rather than a monitor that is
/// off. `xset -dpms` therefore reads as `on` — the ward has turned off the only
/// thing that could ever stop their clock, which costs them time rather than
/// saving it.
fn dpms_level(info: Option<Cookie<'_, impl Connection, InfoReply>>) -> &'static str {
    let Some(reply) = info.and_then(|c| c.reply().ok()) else {
        return "unknown";
    };
    if !reply.state {
        return "on";
    }
    match u16::from(reply.power_level) {
        0 => "on",
        1 => "standby",
        2 => "suspend",
        3 => "off",
        _ => "unknown",
    }
}

/// `-` is "the server would not tell us", which charterd reads as evidence of a
/// window it cannot attribute — never as "no window".
fn field(pid: Option<u32>) -> String {
    pid.map_or_else(|| "-".to_string(), |p| p.to_string())
}

/// A focus/active answer that names an actual client window. `None` (0) and
/// `PointerRoot` (1) are focus states rather than windows, and the root belongs
/// to the server itself, so none of the three attributes to a ward's process.
fn real_window(win: Window, root: Window) -> Option<Window> {
    if win == u32::from(InputFocus::NONE)
        || win == u32::from(InputFocus::POINTER_ROOT)
        || win == root
    {
        return None;
    }
    Some(win)
}

/// One root property, read as a list of 32-bit window ids, plus whether the
/// server had MORE to give than we asked for (`bytes_after`) — the property's
/// own version of a cap biting. An absent property, a property of the wrong
/// format, or a `BadAtom` for a name the desktop never interned all mean the
/// same thing here: nothing to add, nothing truncated.
fn window_prop(
    conn: &RustConnection,
    root: Window,
    atom: u32,
    words: u32,
) -> Result<(Vec<Window>, bool), Box<dyn Error>> {
    if atom == 0 {
        return Ok((Vec::new(), false));
    }
    let reply = conn.get_property(false, root, atom, AtomEnum::ANY, 0, words)?;
    let Some(reply) = swallow_window_error(reply.reply())? else {
        return Ok((Vec::new(), false));
    };
    let truncated = reply.bytes_after > 0;
    let ids = reply.value32().map(Iterator::collect).unwrap_or_default();
    Ok((ids, truncated))
}

/// A window can be destroyed between the reply that named it and the request
/// that asks about it, and the server answers that with an X error about that
/// one window. That is a fact about the window, not about the display, so it
/// becomes `None` here. A connection-level failure IS a fact about the display
/// and is deliberately not swallowed: it must abort the run so charterd sees a
/// non-zero exit rather than a confidently empty answer.
fn swallow_window_error<R>(reply: Result<R, ReplyError>) -> Result<Option<R>, ReplyError> {
    match reply {
        Ok(r) => Ok(Some(r)),
        Err(ReplyError::X11Error(_)) => Ok(None),
        Err(e) => Err(e),
    }
}

/// Walk the real window tree, adding every window that looks like something on
/// screen. This is the half of the fix `_NET_CLIENT_LIST` cannot be trusted for:
/// the property is advisory and ward-writable, the tree is the server's own
/// state.
///
/// The two budgets are spent on different things ON PURPOSE. A flood of cheap
/// junk windows is rejected by the filter, so it spends `MAX_PROBES` (the cost
/// of asking) and not `MAX_WINDOWS` (the cost of reporting) — the ward cannot
/// push a real app window out of the answer by opening 4096 empty ones. When
/// either budget does bite, `capped` is set and charterd stops trusting the
/// snapshot's silence: this walk must never come back short and say nothing.
fn walk(
    conn: &RustConnection,
    children: Vec<Window>,
    out: &mut BTreeSet<Window>,
    capped: &mut Capped,
) -> Result<(), Box<dyn Error>> {
    let mut level = children;
    let mut probes = 0usize;
    let mut reported = 0usize;
    for depth in 0..=EXTRA_LEVELS {
        if level.is_empty() {
            break;
        }
        if level.len() > MAX_PROBES - probes {
            level.truncate(MAX_PROBES - probes);
            capped.0 = true;
        }
        probes += level.len();
        let mut kept = viewable(conn, &level)?;
        if kept.len() > MAX_WINDOWS - reported {
            kept.truncate(MAX_WINDOWS - reported);
            capped.0 = true;
        }
        reported += kept.len();
        out.extend(kept.iter().copied());
        if depth == EXTRA_LEVELS {
            break;
        }
        if reported >= MAX_WINDOWS || probes >= MAX_PROBES {
            // Stopping a level early: there are descendants — possibly the
            // real app windows, under frames we did look at — that nothing
            // in this run ever examined.
            capped.0 = true;
            break;
        }
        level = children_of(conn, &kept)?;
    }
    Ok(())
}

/// The windows in `candidates` that are actually on screen: mapped and
/// unobscured by an unmapped ancestor (`VIEWABLE`), drawable (`InputOutput` —
/// an `InputOnly` window has no pixels, it is an event catcher), and big enough
/// to be an app rather than a tray icon.
///
/// Both requests for every candidate are sent before any reply is read, so a
/// level of the tree costs one round trip instead of one per window.
fn viewable(conn: &RustConnection, candidates: &[Window]) -> Result<Vec<Window>, Box<dyn Error>> {
    let mut pending = Vec::with_capacity(candidates.len());
    for win in candidates {
        let attrs = conn.get_window_attributes(*win)?;
        let geom = conn.get_geometry(*win)?;
        pending.push((*win, attrs, geom));
    }
    let mut kept = Vec::new();
    for (win, attrs, geom) in pending {
        let (Some(attrs), Some(geom)) = (
            swallow_window_error(attrs.reply())?,
            swallow_window_error(geom.reply())?,
        ) else {
            continue;
        };
        if attrs.class == WindowClass::INPUT_OUTPUT
            && attrs.map_state == MapState::VIEWABLE
            && geom.width >= MIN_EDGE
            && geom.height >= MIN_EDGE
        {
            kept.push(win);
        }
    }
    Ok(kept)
}

/// Every direct child of each window, one round trip for the whole level.
fn children_of(conn: &RustConnection, parents: &[Window]) -> Result<Vec<Window>, Box<dyn Error>> {
    let mut pending = Vec::with_capacity(parents.len());
    for win in parents {
        pending.push(conn.query_tree(*win)?);
    }
    let mut out = Vec::new();
    for cookie in pending {
        if let Some(reply) = swallow_window_error(cookie.reply())? {
            out.extend(reply.children);
        }
    }
    Ok(out)
}

/// The owning client's pid for each window, from the server's own record of who
/// opened the connection that created the resource.
///
/// One request per window rather than one request naming every window: a single
/// spec list fails whole if ANY of its xids has gone stale, and one closing
/// window must not blank the attribution of every other window on the display.
fn client_pids(
    conn: &RustConnection,
    windows: &BTreeSet<Window>,
) -> Result<BTreeMap<Window, Option<u32>>, Box<dyn Error>> {
    let mut pending = Vec::with_capacity(windows.len());
    for win in windows {
        let spec = ClientIdSpec {
            client: *win,
            mask: ClientIdMask::LOCAL_CLIENT_PID,
        };
        pending.push((
            *win,
            conn.res_query_client_ids(std::slice::from_ref(&spec))?,
        ));
    }
    let mut out = BTreeMap::new();
    for (win, cookie) in pending {
        let pid = match swallow_window_error(cookie.reply())? {
            Some(reply) => local_pid(&reply),
            // The window went away mid-probe, or the server declined to
            // answer. Either way charterd is told `-`, not "no process".
            None => None,
        };
        out.insert(win, pid);
    }
    Ok(out)
}

/// Pull the pid out of a `QueryClientIds` reply.
///
/// A remote (TCP / X-forwarded) client has no local pid, and the server says so
/// by omitting the value or returning 0 rather than by erroring — so this is
/// `None` for a real window with a real owner we simply cannot name, which is
/// precisely the case charterd's unidentified-window fallback exists for.
fn local_pid(reply: &res::QueryClientIdsReply) -> Option<u32> {
    reply
        .ids
        .iter()
        .find(|id| u32::from(id.spec.mask) & u32::from(ClientIdMask::LOCAL_CLIENT_PID) != 0)
        .and_then(|id| id.value.first().copied())
        .filter(|pid| *pid != 0)
}
