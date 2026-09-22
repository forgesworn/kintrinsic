# Kintrinsic for Linux — install, setup & use (Linux Mint)

A parent's runbook for putting Kintrinsic on a child's Linux Mint machine:
install it, lock the account down, set screen-time limits (on the box **or**
from your phone), and use it day to day. Targets **Linux Mint 22.x (Cinnamon /
X11)**.

> **Before you start — honest status.** This is the **first real walk-through**.
> The code is built and tested, but it has not yet been proven on a real Mint
> box end to end. Do this on a **spare machine or a VM**, with a **separate
> admin account for you** (so a mis-set limit can't lock you out while we shake
> out the rough edges). Steps that are still rougher than they should be for a
> non-technical parent are flagged **⚠ rough edge**.

---

## One app, two ways to set limits

Everything — setup, the rules, connecting a phone, recovery — lives in one
app: **Kintrinsic**.

- **On the box (no phone).** Set the limits right in the Kintrinsic app.
  Simplest; needs nothing else. **Start here.**
- **From your phone.** Connect a phone from the app's **Connect a phone**
  tab, then set limits in **Kintrinsic** on your phone (delivered to the
  device, signed by your key). More moving parts — do the on-box path first,
  then connect a phone.

---

## 1. What you'll need

- A Mint 22.x machine for the child, with a **normal (non-admin) account**,
  e.g. `kid`.
- A **separate admin account for you** — Kintrinsic takes the child out of
  the admin groups, so don't manage the box from their account.
- The Kintrinsic package `kintrinsic_<version>_amd64.deb`. Download it on the
  child's machine — get the current `.deb` from the download page
  (<https://kintrinsic.app/download.html>); it downloads from Blossom
  (content-addressed), so the file is named by its hash.
  Or build it from this repo instead, if you have the Rust toolchain:
  ```sh
  cargo run -p xtask -- deb     # lands in linux/target/deb/
  ```
- *(Optional)* Kintrinsic on your phone, if you want to connect it — see
  §5 below.

## 2. Install the package

Double-click the `.deb` (it opens in the GDebi installer) and click **Install**,
or from a terminal:

```sh
sudo apt install ./kintrinsic_<version>_amd64.deb     # pulls deps (systemd, dbus)
```

This installs the daemon (`charterd`), the child's CLI (`charter`), the lock
screen (`charter-lock`), the setup helper (`charter-setup`), the single
**"Kintrinsic"** menu app, and the enforcement plumbing (systemd / D-Bus /
polkit / fapolicyd).

## 3. Set up Kintrinsic

Open your applications menu, launch **"Kintrinsic"**. On the **Home** tab,
choose your child's account and press **Set up Kintrinsic**. It asks for your
admin password (this is what actually does the work — `charter-setup` behind
a `pkexec` prompt).

*(Same thing from a terminal: `sudo charter-setup kid`.)*

It's safe to re-run. It takes the child out of `sudo`/`wheel`/`admin`/`adm`/
`lpadmin` (and warns if they're still in another root-equivalent group like
`docker`), puts them in the `charter-managed` group, writes
`/etc/charter/charterd.env` (so the lock screen can reach their session), and
starts the `charterd` service.

> ⚠ **rough edge:** the group change only takes effect on the child's
> **next login**. If they're already logged in, Kintrinsic Setup offers to
> end their session right there so the lockdown applies immediately —
> otherwise log them out and back in yourself.

## 4. Set the rules (on the box, no phone)

On the **"The rules"** tab, enter your admin password and set:

- **Allowed hours** — the hours the machine may be used (e.g. `07:00`–
  `20:00`). Outside them, it locks.
- **Time each day** — the on-screen time cap per day.

**Save.** Within a few seconds `charterd` is enforcing it. That's the whole
of the on-box path — you're done. (Under the hood this writes
`/etc/charter/limits.d/<child>.json`, which only an admin can edit.) You can
also turn on **Khan Academy is time-free** here, so learning time there
doesn't use up the daily cap.

**More than one child?** Give each child their own login and run **Set up
Kintrinsic** for each. Whoever is logged in is the one whose time is counted,
and each child is judged on their own limits.

---

## 5. Connect a phone (optional, remote control)

This adds remote control on top of the on-box limits. On the **"Connect a
phone"** tab (in the Kintrinsic app), press **Show the pairing code** — this
asks for your admin password, since only a parent may invite a guardian —
then scan the QR (or type the code) with **Kintrinsic on your phone**
(**Add your child → Set up a computer**). The page flips to "Connected" on
its own once the phone finishes.

From then on you can set limits, see time used, and approve "more time" from
Kintrinsic on your phone, from anywhere. The phone takes charge: once you've
set a child's limits from the phone, those are authoritative for that child;
the on-box limits are the fallback while it's offline.

> The `charter pair` terminal command does **not** pair a guardian — pairing
> needs an admin password, which only the Kintrinsic app can collect.
> Running it just tells you to use the app.

---

## 6. Using it day to day

**You (parent):**
- Change limits any time — on the box (**The rules**) or your phone
  (Kintrinsic).
- *(Once a phone is connected)* When the child asks to run/install something
  or asks for more time, you get a request to **approve or deny** on your
  phone.

**The child:** on their machine —
```sh
charter time-left        # how much time is left today (works with or without a phone)
charter ask-for-more 15  # ask you for 15 more minutes when a limit is hit
charter run ~/Game.AppImage   # ask you to allow an app
charter status            # check whether your parent has answered a request
```
`ask-for-more` and `run` need a connected phone — until you've connected one,
they reply that Kintrinsic isn't connected to a phone yet. `time-left` works
either way.

**When time's up:** their apps freeze and a full-screen message explains why
("Time's up for today" / "Outside allowed hours"). It clears itself when the
next allowed window opens — you don't have to do anything.

## 7. Check the lockdown worked

As the child (`su - kid`), confirm they **can't** escalate:

```sh
sudo -v               # should be refused (not in sudo)
flatpak install ...   # should be refused (only your approval can install)
```

## 8. If you get stuck — Recovery

**The parent's account is never governed** — only the child's. The lock
screen holds the keyboard and pointer and **VT switching is turned off while
it's up — there is no `Ctrl+Alt+F3` escape to a text console during a lock.**
These are the routes that actually work:

- **Log out → your admin session → Recovery.** The lock's own **Log out**
  button ends the child's session (it unfreezes it first) and drops you at
  the login screen. Sign in to **your admin account**, open **Kintrinsic**
  from the menu, go to the **Recovery** tab, and choose:
  - **Pause** — unfreeze the child now, keep your settings (resume any time).
  - **Resume** — start enforcing again.
  - **Turn off** — stop enforcing entirely (re-enable from **Set up
    Kintrinsic**).

  A locked child screen shows this exact route on request: press
  **Ctrl+Alt+Shift+Q** at the lock for a notice with these steps spelled
  out — **but only while no phone guardian is paired.** Once a phone is
  paired, that same chord does something different (below): it opens the
  offline unlock-code entry instead of this notice.

- **The offline unlock code (once a phone is paired).** Press
  **Ctrl+Alt+Shift+Q** at the lock screen — the same chord as above, now
  opening the code entry instead of the recovery notice because a guardian
  is connected. It shows a short code and the
  prompt "In Kintrinsic, open 'Unlock a device', enter this code, then type
  the 8-digit code it shows here." Open Kintrinsic on your phone, do that,
  and type the 8 digits it gives you — a correct code pauses enforcement
  immediately, no admin login needed. A handful of wrong tries locks entry
  out for a while (it backs off further each time), so it can't be guessed.

- **Shut down.** The lock's **Shut down** button powers the machine off. This
  does **not** unlock anything by itself — `charterd` self-heals and resumes
  enforcing on the next boot — but it's there if you need to step away.

If you're already at your own admin desktop (not locked out), you can also
just run:

```sh
sudo systemctl stop charterd.service   # stops + thaws everything immediately
```

Full undo / uninstall:

**Before you run `charter-setup`, note which admin groups the child's
account was already in** (`id kid`) — setup strips it from all of them, and
undoing that means putting each one back, not just `sudo`.

```sh
sudo systemctl disable --now charterd.service   # stop enforcing
# Restore every group charter-setup strips (only add back the ones the
# account actually had before setup — see the note above):
sudo gpasswd -a kid sudo
sudo gpasswd -a kid adm
sudo gpasswd -a kid lpadmin
sudo gpasswd -a kid wheel     # Fedora/RHEL-style admin group, if present
sudo gpasswd -a kid admin     # legacy Ubuntu admin group, if present
sudo gpasswd -a kid docker    # only if it was in docker before setup
sudo gpasswd -a kid lxd       # only if it was in lxd before setup
sudo gpasswd -a kid disk      # only if it was in disk before setup
sudo gpasswd -a kid shadow    # only if it was in shadow before setup
sudo gpasswd -a kid libvirt   # only if it was in libvirt before setup
sudo gpasswd -a kid kvm       # only if it was in kvm before setup
sudo apt remove kintrinsic                       # remove Kintrinsic entirely
```

---

## What's proven vs what you're testing

- **Built + tested (logic):** the whole enforce loop (allowed-hours, daily cap,
  multi-child, freeze + lock + web filter) and the remote signing pipeline are
  unit-tested and build clean.
- **What this walk-through is checking:** that it all behaves on a *real* Mint
  box — the lock actually holding, install + setup being clear, and phone
  pairing being followable. Note where you get stuck; the **⚠ rough edge**
  flags are the spots we already expect to be rougher than normie-grade.
- **Connecting a phone is optional.** The on-box path (§4) is fully
  self-contained; connect a phone (§5) only once that's working and you want
  remote control.
