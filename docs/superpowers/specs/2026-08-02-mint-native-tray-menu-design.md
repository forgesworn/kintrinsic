# A tray menu Mint draws itself

**Date:** 2026-08-02
**Status:** approved, building

## The ask

Clicking the Charter tray icon opens a window. decented wants what the volume and
battery icons do: a panel that drops from the icon, in the desktop's own
styling, with an "Open Charter" entry at the foot that opens the full app.

## What we learned first

Cinnamon's volume and battery popups are **applets** — code running inside the
panel process, drawn with the desktop's own toolkit. A tray icon cannot draw
one. So the question was only ever *how close can we get*.

The obvious route was the StatusNotifierItem `ItemIsMenu` flag, which tells a
panel "show my menu instead of calling me". **It does not work on Mint.**
Verified by turning the flag on, driving a real primary click over D-Bus, and
watching the traffic to the tray: Cinnamon called `Activate` anyway. With the
flag on, our side refuses `Activate` — so left-click did nothing at all, which
is worse than today. `strings` on `xapp-sn-watcher` shows no `ItemIsMenu` at
any casing; the property is simply not read.

Two facts from the same probe shaped the design:

- `Activate` carries the **exact screen coordinates** of the click (sent 900,20;
  received 900,20 intact).
- Mint ships its own icon API, `XAppStatusIcon`, with `set_primary_menu()` —
  a real GTK menu on left-click. `libxapp1` is installed by default
  (3.2.2+zena), and Mint's own tray apps (`mintUpdate`, `mintreport-tray`) use
  it. All eight symbols we need are exported.

That last one is the route: a menu Mint itself draws and themes.

## Shape: one brain, two shells

`charter-tray` keeps everything that thinks — the tested view model, the
charterd client, the notifications — and keeps today's ksni shell for GNOME,
KDE and everything else. A new sibling renders the Mint version.

**`apps/charter-tray-xapp`** — a new crate at the repo root, *outside* the
`linux/` workspace. Not a workaround: `apps/charter-console` already lives there
for exactly this reason, because the headless CI gate host has no GTK
development libraries (`linux/ci/linux-ci.yml`: "The bare host lacks
WebKitGTK/GTK/libsoup"). Adding GTK to a workspace member would break
`clippy --all-features` and `build (real)` on that host.

`libxapp` is loaded at **runtime with dlopen**, not linked. The `.deb` therefore
gains no new package dependency and still installs on plain Debian/Ubuntu where
`libxapp1` may not exist. GTK3 itself is linked normally — already implied by
the existing `libwebkit2gtk-4.1-0` dependency.

### Choosing the shell

`charter-tray` reads `XDG_CURRENT_DESKTOP` at startup. Cinnamon (and Mint's
MATE/XFCE editions) hand off to the XApp shell by `exec` — process replacement,
so one process and one icon. Anything else runs today's path.

Deliberately **not** probed from the session bus. `org.x.StatusIconMonitor.*`
only appears once the panel claims it, which at login may be after we start —
that is precisely the race that made 0.6.0's tray invisible (see
`tray-dies-before-watcher`). The environment variable is set before autostart
runs and cannot race.

If `libxapp` won't load, or the icon can't be created, the XApp shell falls back
to today's ksni shell rather than showing nothing. Same principle as the 0.6.1
fix: a companion surface may lose its looks, never its presence.

### The menu

Left-click; right-click shows the same menu.

```
46m left today                    header, not clickable
▃▃▃▃▃▃▃▃▃▁▁▁▁▁▁▁                  bar: used vs the day
2h 35m until the day's hours end
─────────────────────────────
Play      ▃▃▃▃▁▁▁▁▁▁      25m     one row per named allowance
School    ▃▃▃▃▃▃▃▃▃▃   no cap
─────────────────────────────
Ask for 15 more minutes           greyed when asking isn't offered
Ask for 30 more minutes
Ask for 60 more minutes
─────────────────────────────
Open Charter…                     the full app
```

Every figure already arrives in `TimeLeftView` (`buckets`, `used_today_seconds`,
`budget_day_seconds`, `schedule_seconds`). **No wire change, no daemon change.**

### The menu is data, not drawing

The rows become a pure function in the tested `charter_tray` library:
`menu_rows(Option<&TimeLeftView>) -> Vec<MenuRow>`, where a row is a header with
an optional bar fraction, a plain line, a separator, an ask, or the open-app
entry. Both shells render the same list, so the ksni menu and the Mint menu
cannot drift apart, and the copy stays where its tests are — inside the CI gate.

## Decisions taken

- **"Open Charter…" opens the full app**, not `--popup`, as asked. On a ward's
  account the full console is largely guardian settings behind pkexec; what that
  actually looks like gets reported after the visual check, not guessed at here.
- **No drag slider on Mint.** A GTK menu cannot hold one reliably. Fixed
  15/30/60 entries replace it, as in the approved mockup.
- `charter-console --popup` stays in the binary — it is still the ward's own day
  view — but nothing in the tray invokes it any more.

## Testing

- `menu_rows` and the shell-choice predicate are pure and unit-tested in the CI
  gate, alongside the existing model tests.
- The GTK shell is thin. It is verified visually on this laptop, which is the
  same Mint 22.3 Cinnamon as the target, with a screenshot — before decented is
  asked to touch anything.
- The 0.6.1 empty-bus regression test still covers the ksni shell.

## Out of scope

A real Cinnamon applet (considered, rejected: Cinnamon-only and a second UI to
keep in step forever) and docking the existing window under the icon
(considered, rejected: still a window).
