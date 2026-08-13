# Charter for Linux — install, setup & use (Linux Mint)

A parent's runbook for putting Charter on a child's Linux Mint machine: install
it, lock the account down, set screen-time limits (on the box **or** from your
phone), and use it day to day. Targets **Linux Mint 22.x (Cinnamon / X11)**.

> **Before you start — honest status.** This is the **first real walk-through**.
> The code is built and tested, but it has not yet been proven on a real Mint
> box end to end. Do this on a **spare machine or a VM**, with a **separate
> admin account for you** (so a mis-set limit can't lock you out while we shake
> out the rough edges). Steps that are still rougher than they should be for a
> non-technical parent are flagged **⚠ rough edge**.

---

## Two ways to use Charter

- **A · Device-only (no phone).** You set the limits right on the machine in a
  little app. Simplest; needs nothing else. **Start here.**
- **B · Remote (from your phone).** You set the limits in **MyCharter** on your
  phone and they're delivered to the device, signed by your key in **Signet**.
  More moving parts — do **A** first, then add **B**.

Sections 1–4 are common + Path A. Section 5 onward is Path B.

---

## 1. What you'll need

- A Mint 22.x machine for the child, with a **normal (non-admin) account**, e.g.
  `kid`.
- A **separate admin account for you** — Charter takes the child out of the
  admin groups, so don't manage the box from their account.
- The Charter package `charter_<version>_amd64.deb`. Download it on the child's
  machine — get the current `.deb` from the download page
  (<https://kintrinsic.app/download.html>); it downloads from Blossom
  (content-addressed), so the file is named by its hash.
  Or build it from this repo instead, if you have the Rust toolchain:
  ```sh
  cargo run -p xtask -- deb     # lands in linux/target/deb/
  ```
- *(Path B only)* **Signet** set up on your phone (your signing key) and access
  to **MyCharter** at `https://charter.mysignet.app`.

## 2. Install the package

Double-click the `.deb` (it opens in the GDebi installer) and click **Install**,
or from a terminal:

```sh
sudo apt install ./charter_<version>_amd64.deb     # pulls deps (systemd, dbus)
```

This installs the daemon (`charterd`), the child's CLI (`charter`), the lock
screen (`charter-lock`), the setup helper (`charter-setup`), the **"Charter
Setup"** and **"Charter Screen Time"** menu apps, and the enforcement plumbing
(systemd / D-Bus / polkit / fapolicyd).

## 3. Lock the child's account down

Open your applications menu, search **"Charter Setup"**, and launch it. Enter
your admin password when asked, then **pick the child's account** and confirm.

*(Same thing from a terminal: `sudo charter-setup kid`.)*

It's safe to re-run. It takes the child out of `sudo`/`adm`/`lpadmin`, puts them
in the `charter-managed` group, writes `/etc/charter/charterd.env` (so the lock
screen can reach their session), and starts the `charterd` service.

> ⚠ **rough edge:** if the child doesn't log in on display `:0`, edit
> `CHARTER_DISPLAY` in `/etc/charter/charterd.env` and
> `sudo systemctl restart charterd`.

---

## 4. Path A — set limits on the box (no phone)

Open **"Charter Screen Time"** from the menu, enter your admin password, and set:

- **Allowed from / until** — the hours the machine may be used (e.g. `07:00`–
  `20:00`). Outside them, it locks.
- **Daily minutes** — the on-screen time cap per day.
- **Weekend** (optional) — a separate Sat/Sun window.

**Save.** Within a few seconds `charterd` is enforcing it. That's the whole of
Path A — you're done. (Under the hood this writes
`/etc/charter/limits.d/<child>.json`, which only an admin can edit.)

**More than one child?** Give each child their own login and run **Charter
Setup** + **Charter Screen Time** for each. Whoever is logged in is the one whose
time is counted, and each child is judged on their own limits.

---

## 5. Path B — manage it from your phone (remote)

This adds remote control on top of Path A. The shape:

```
  Your phone                          The child's machine
  ──────────                          ───────────────────
  Signet  ── holds your key
  MyCharter ── you set limits  ──(signed, over a relay)──▶  charterd enforces
```

You pair the two **once**: the device learns to trust your key, and your phone
learns the device's address.

### 5a. Get your guardian link from Signet
In **Signet** on your phone, create/choose your guardian identity and copy its
**connection link** — a `bunker://…` string (it contains your public key + a
relay). You'll use this in two places below.

### 5b. Tell the device to trust you
On the child's machine, open **"Charter — Pair Guardian"** from the menu (enter
your admin password). Paste **your guardian link** from 5a, then paste **your
child's ID** from MyCharter (their Signet "dependant ID"). It pins your key and
links the child, so your phone-set limits will take effect.

> Use the **graphical** "Charter — Pair Guardian" for this — it's the path that
> both pins your key **and** links the child's ID (both are required, or your
> phone-set limits won't apply). The `charter pair` terminal command only
> validates/queries pairing; it does **not** pin the guardian, so don't rely on
> it to set up pairing.

### 5c. Get this device's pairing code
On the child's machine, open **"Charter — This Device's Code"** from the menu. It
shows a **QR + a 64-character code** — you'll scan/type it into MyCharter next.
*(No password needed; it's public.)*

### 5d. In MyCharter (your phone)
Open **`https://charter.mysignet.app`** and:

1. **Turn on parent approval → "Set up with Signet"**, and **paste the same
   `bunker://…` link** from 5a. This connects MyCharter to your key in Signet.
2. **Add the child** (just a name).
3. **Set up their computer** → when it asks for the device's code, **scan the QR
   or type the code** from 5c.

### 5e. Set limits remotely
Open the child on the **Limits** screen, set their allowed hours / daily cap, and
**Save**. MyCharter builds the rule, asks Signet to **sign** it with your key,
and sends it to the device, which verifies your signature and starts enforcing —
no phone-side "trust me" button, every change is signed by you.

> ⚠ **rough edge:** today you confirm each change in Signet (every limit edit
> prompts a signature). You can turn on "approve without the extra tap" in
> MyCharter to stop the per-edit prompt.

---

## 6. Using it day to day

**You (parent):**
- Change limits any time — on the box (Charter Screen Time) or your phone
  (MyCharter). **The phone takes charge:** once you've set a child's limits from
  MyCharter, those are authoritative for that child; the on-box limits are the
  fallback for a child you haven't set from the phone (or while it's offline).
- *(Path B)* When the child asks to run/install something or asks for more time,
  you get a request to **approve or deny** in MyCharter.

**The child:** on their machine —
```sh
charter time-left        # how much time is left today (works with or without a phone)
charter ask-for-more 15  # ask you for 15 more minutes when a limit is hit
charter run ~/Game.AppImage   # ask you to allow an app
charter status           # check whether your parent has answered a request
```
`ask-for-more` and `run` need the phone guardian (Path B) — until you've paired,
they reply that Charter isn't connected to a phone yet. `time-left` works either
way.

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

**The parent's account is never governed** — only the child's. So if a limit ever
locks *the child's* screen when it shouldn't, log in as **your admin account**
(switch users at the greeter) and open **"Charter — Recovery"** from the menu:

- **Pause** — unfreeze the child now, keep your settings (resume any time).
- **Resume** — start enforcing again.
- **Turn Charter off** — stop enforcing entirely (re-enable from Charter Setup).

If you can't reach the desktop at all, recovery is also one command from your
admin account (or another TTY, `Ctrl+Alt+F3`):

```sh
sudo systemctl stop charterd.service   # stops + thaws everything immediately
```

Full undo / uninstall:

```sh
sudo systemctl disable --now charterd.service   # stop enforcing
sudo gpasswd -a kid sudo                         # give the account admin back
sudo apt remove charter                          # remove Charter entirely
```

---

## What's proven vs what you're testing

- **Built + tested (logic):** the whole enforce loop (allowed-hours, daily cap,
  multi-child, freeze + lock + web filter) and the remote signing pipeline are
  unit-tested and build clean.
- **What this walk-through is checking:** that it all behaves on a *real* Mint
  box — the lock actually holding, the install + wizards being clear, and the
  remote pairing being followable. Note where you get stuck; the **⚠ rough
  edge** flags are the spots we already expect to be rougher than normie-grade.
- **Needs the guardian app to exist:** Path B assumes **Signet** can sign and
  **MyCharter** is reachable. If Signet's Charter support isn't live yet, Path A
  (device-only) is fully self-contained and the right thing to walk first.
