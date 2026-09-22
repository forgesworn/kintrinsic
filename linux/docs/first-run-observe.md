# Kintrinsic for Linux — safe first run (observe → escalate)

The warden has never run live on real hardware, and it is *designed* to be hard
to escape. So the first run on your daily driver is **observe mode**: it logs
what it *would* do and touches nothing. You then escalate one rung at a time.

## Before you start — the escape hatches (know these cold)
- You keep `sudo`. `charter-setup`'s brick-guard refuses to manage the only
  admin — so set up the **child's** account, and you stay admin.
- **`sudo systemctl stop charterd`** always recovers the machine: the unit's
  `ExecStopPost` thaws every frozen slice and re-enables VT switching. Run it
  from **your own** admin session.
- **VT switching is off while the lock is up — there is no `Ctrl+Alt+F3`
  escape to a text console during a lock.** If you ever need out of a locked
  screen instead of your own session: the lock's own **Log out** button ends
  the session and drops you at the login screen, where you sign in to your
  admin account and open **Kintrinsic → Recovery → Pause** (or run the
  command above once you're there). Pressing **Ctrl+Alt+Shift+Q** at the lock
  shows this exact route on screen — **but only while no phone guardian is
  paired**; once one is paired, that chord opens the offline unlock-code
  entry instead (you won't have paired one yet in this walk-through).
- **Timeshift** snapshot (below) is a full rollback if anything feels off.

## 0. Snapshot (2 min)
Open **Timeshift** → **Create**. Wait for it to finish. This is your undo.

## 1. Build the package (on your dev box)
From `linux/`:
```bash
cargo run -p xtask -- deb
```
This emits a `.deb` under the workspace target dir (`kintrinsic_*.deb`). Copy
it to the laptop if you built elsewhere.

## 2. Install
```bash
sudo apt install ./kintrinsic_*.deb    # or: sudo dpkg -i kintrinsic_*.deb
```
Installing only registers + starts the daemon. No lockdown is applied yet.

## 3. Set up the CHILD account in observe mode (no noexec)
Replace `bob` with your child's account name:
```bash
sudo CHARTER_ENFORCE=observe CHARTER_SKIP_NOEXEC=1 charter-setup bob
```
- `CHARTER_ENFORCE=observe` → the daemon logs decisions, applies nothing.
- `CHARTER_SKIP_NOEXEC=1` → leaves `/tmp` + `/dev/shm` untouched (the one
  system-wide change is skipped for the first run).
- This moves `bob` out of the admin groups and seeds default limits
  (wake 07:00, bedtime 20:00, 120 min/day, in the timezone `charter-setup`
  resolves from the host — it refuses to guess UTC). **You** are untouched.

(This is the same script the **Kintrinsic** app's **Set up Kintrinsic**
button runs, headless — you can do the rest of this walk-through from the
GUI once you trust the observe-mode behaviour below.)

## 4. Watch it think (zero risk)
```bash
journalctl -u charterd -f
```
You'll see `charterd: enforce mode = Observe` and, when `bob` is logged in and
past a limit, lines like:
```
observe: WOULD enforce uid 1001 (bob) — freeze + lock: Time's up for today [source=device-only]
```
Log in as `bob`, set the bedtime earlier in the **Kintrinsic** app's **"The
rules"** tab (or wait past a limit), and confirm the *decision* is correct —
**nothing actually locks**.

## 5. Escalate one rung at a time
Edit `/etc/charter/charterd.env`, change the `CHARTER_ENFORCE=` line, then
`sudo systemctl restart charterd` after each change:

1. **`freeze-only`** — apps in the child session freeze past a limit, but no lock
   screen. Verify `bob`'s apps pause and that `sudo systemctl stop charterd`
   thaws them.
2. **`enforce`** (or delete the line — enforce is the default) — full: freeze +
   the fullscreen lock + VT lock + web filter. Verify the lock draws on
   **bob's** session, and that **Kintrinsic → Recovery** (your admin
   password) and `sudo systemctl stop charterd` both release it.

## If anything misbehaves
- `sudo systemctl stop charterd` (thaws everything), or
- **Kintrinsic → Recovery** from the menu (pause / turn off / thaw), or
- Restore the Timeshift snapshot.

Once `enforce` behaves, the box is doing the real thing — and you never gambled
the daily driver to get there.
