# Guardian keys: hardware gates

Hardware tests for *Guardian roots and working keys (v1)* (`spec/contract.md`)
on branch `feat/guardian-keys-rebased`. Run them in order: each gate assumes the one
before it passed. The test PIN is **123456** throughout.

Record each step as pass or fail; for a failure, note what the screen showed
and save the ward log (Linux: `journalctl -u charterd --since "-15 min"`;
Android: `adb logcat -d | grep -i charter`).

## Devices (roles)

| Role | Device |
|---|---|
| Guardian phone A | the guardian test phone, with My Signet holding identity **A** (Heartwood-backed for gate 1) |
| Guardian phone B | a second phone with My Signet holding a different identity **B** |
| Android ward | a spare Android ward that may be factory-reset (Device Owner provisioning) |
| Linux ward | a Linux ward running `charterd`, with an administrator account |

## Gate 0: build, install, no regressions

The branch carries main's version numbers (ward 0.7.5 (54), Linux 0.8.3) and
takes no bump until release. The guardian app's versioning gate wants ward code
55 and Linux code 804 or above (`docs/guardian-keys-open-questions.md`, "The
versioning gate's thresholds before release"), and treats anything lower as a
ward without this section. So build the wards from a throwaway branch that
differs only in those numbers. Never merge it.

```
git switch -c test/guardian-keys-gates feat/guardian-keys-rebased
# android/app/build.gradle.kts: versionCode = 55, versionName = "0.7.6"
# linux/Cargo.toml:             version = "0.8.4"
(cd linux && cargo update --workspace)
git commit -am "test: gate build numbers for guardian keys"
```

Artefacts are built locally into `/var/tmp/kintrinsic-test/` (ward APK
0.7.6 (55), Linux `.deb` 0.8.4, `SHA256SUMS`):

```
source ~/Android/env.sh
cd android && CHARTER_ALPHA_DEBUG_SIGNING=1 ./scripts/build-apk.sh
#   the APK is android/app/build/outputs/apk/release/app-release.apk
cd ../linux && cargo run -q -p xtask -- deb
#   the package is linux/target/deb/*.deb
```

1. Guardian app test build, served to the phones over USB (a `localhost`
   origin is a secure context, so the camera and service worker work):

   ```
   cd apps/charter-app && npm ci && npm run build && npx vite preview --host 127.0.0.1 --port 4173
   adb -s <phone> reverse tcp:4173 tcp:4173
   ```

   Open `http://localhost:4173` on the phone. This is a separate origin from
   the production app, so it starts with an empty family.
2. Upgrade an already-paired Android ward in place. The local test APK is
   debug-signed, and fielded wards carry the release certificate with its
   rotation lineage, so it installs only on a fresh or reset ward (gates 1–3).
   The in-place upgrade needs the CI-signed build: push the throwaway branch,
   tag it (`ward-v0.7.6`, which must match its `versionName`), let
   `release-artifacts.yml` produce the draft release (it waits for the sysadmin
   on the `release` environment, and is never published), then send the draft's
   APK with `adb install -r`. A paired release build blocks local installs, so
   if `adb` is refused use a maintenance window from the guardian app first.
   Delete the tag and the draft after the gates, so they can't collide with the
   real release.
   **Expect:** the ward keeps its pairing and every existing rule is still
   enforced. About this device → Guardians lists one guardian. The production guardian
   app still sees the ward's STATUS and can still change a rule (the ward
   treats the single pinned guardian exactly as before).
3. Upgrade a paired Linux ward: `sudo apt install ./kintrinsic_0.8.4_*.deb`
   (the apt source the package installs offers nothing newer than this).
   **Expect:** as step 2. `charter status` shows the guardian list, and
   `/var/lib/charter/guardian-state.json` exists with mode 0600.

## Gate 1: a Heartwood-first family from scratch

1. Factory-reset the Android ward and provision the 0.7.6 test APK as Device
   Owner with `android/scripts/charter-provision.sh <apk>`. QR provisioning
   pins the published release APK by checksum, so it can't install a test
   build.
2. Guardian phone A, test build: Settings → Parent approval → **Other ways (for
   people who already use Signet or Heartwood)** → **Set up with Heartwood** →
   paste My Signet's connection link for identity A → **Connect**.
   **Expect:** My Signet asks once to sign a *kind 31120* event (this phone's
   approval). Approve it. **If My Signet refuses kind 31120**, stop and record
   it: that is a My Signet permission change, not a Kintrinsic bug.
3. Settings → add the child → on the child's settings page (⚙), Devices →
   **Set up a device** → **📱 Phone**. Scan the pairing code with the Android ward and check the
   six-digit code matches on both screens. Under a My Signet root the ward
   can't announce itself to phone A, so then tap **Type the device code** and
   type the code the ward shows (About this device), and name the device.
   **Expect:** the guardian app shows the ward. Ward About this device →
   Guardians lists one guardian (A's short npub) with one working key.
4. Settings → **Guardians** → **Set the guardian PIN**: **123456**, twice →
   **Set guardian PIN**.
5. Change a rule for the child (e.g. today's screen-time limit).
   **Expect:** the ward enforces it within a minute. Ward About this device
   shows A as the guardian who set it.
6. Linux ward (optional in this gate): on the computer, Kintrinsic →
   **Connect a phone** shows a code; on phone A, the child's settings page →
   **Set up another device** → **💻 Computer** → scan it.
   **Expect:** `charter status` shows root A (not the phone's working key)
   and one working key.
7. What main added, signed by phone A's working key (ward About this device
   shows A as the guardian who set each rule):
   - **Wi-Fi & Bluetooth:** the child's settings page → Rules → **Wi-Fi &
     Bluetooth** → Wi-Fi to saved networks only, Bluetooth to paired devices
     only → save. **Expect:** on the ward, joining a new network and pairing a
     new Bluetooth device are refused within a minute; **Any** lifts it again.
   - **Times:** the child's settings page → **Times** → add Learning time with
     a window covering now → save. **Expect:** the ward enforces Learning
     time's rules, and the child's **Now** card names Learning time within a
     minute. Remove the window: it returns to Free time.
   - **Battery codes:** the device's page → **Battery & responsiveness** →
     pick a preset → **Create signed battery code** (My Signet signs it: the
     code is signed by the root) → on the ward, within ten minutes, About this
     device → **Change relays / battery** → paste. **Expect:** the code is
     accepted and About shows the new battery line. A second paste of the same
     code is refused.
   - **Locked Settings row:** lock the ward (a schedule or a used-up limit),
     then use the Wi-Fi and Bluetooth buttons on the lock screen. **Expect:**
     the phone's own Settings pages open and obey the tiers above, and no
     third-party app can serve them. Accounts, app controls (force
     stop, disable, clear) and default apps are restricted, and lifted again on
     unlock. The default dialer still works past the lock, and a call from the
     lock screen connects, exactly as on ward 0.7.5.

## Gate 2: a second parent joins, then leaves

1. Phone A: Settings → **Guardians** → **Invite a parent** → **Scan their QR
   code** (or paste B's npub) → **Next** → **Send the invitation**.
   **Expect:** the **Invitation sent** screen shows *Your guardian code*: a
   QR and A's npub. (It is also at Settings → Guardians → **Show my guardian
   code**.) Keep it on screen. The ward does not show the waiting invitation
   to anyone who has not entered the guardian PIN (gate 9).
2. Phone B, test build (its own `adb reverse`): Settings → **Join a family** →
   **Connect My Signet** → paste identity B's connection link and B's npub
   (*The guardian ID you were invited as*, required) → **Connect**. My Signet
   signs once (this phone's kind-31120 approval). (Phone B can instead join
   with **Join with this phone only**, which needs no My Signet; gate 7 covers
   that route.)
   **Expect:** if B connects with an identity other than the npub given, the
   app refuses with a readable error, disconnects and signs nothing.
3. Phone B now shows **Scan the other parent's guardian code**. First the
   refusals: paste `hello` into *Or paste their guardian code* → **Use this
   code**. **Expect:** "That isn't a guardian code. It starts with npub1."
   Paste B's own npub → **Use this code**. **Expect:** "That's your own
   guardian code. …". Then tap **Scan the other parent's guardian code** and
   scan the QR on phone A (step 1).
   **Expect:** the screen moves on to **Now confirm in person**, showing
   "Family list from: npub1…" with A's short npub.
4. At the Android ward: About this device → Guardians → **Guardian presence**
   → **Show a code for this** on "Add … as a guardian" → Guardian PIN 123456 →
   **Show code** → a QR and short code appear with a 10-minute countdown.
   - A wrong PIN 5 times in a row, then a 6th try: **expect** a 1-minute wait
     that survives a reboot.
   - Rotate the screen: it returns to the list (known; mint again).
5. Phone B: **Confirm in person** (on the join screen, or Settings →
   Guardians) → scan the ward's QR.
   **Expect:** within a minute, ward About this device lists two guardians
   (A and B). Phone A's Guardians screen lists B as "Other parent".
   Phone B's Guardians screen lists the ward as "Device …" marked
   *unconfirmed*, and says it is waiting for A's family list (A's short
   npub) and sends it nothing meanwhile. Open Kintrinsic on phone A: within a
   minute phone B shows the child and its devices, the ward ready
   and any device B hasn't confirmed at marked *unconfirmed*. **Invite a
   parent** on phone B isn't held back by a device B hasn't confirmed at.
6. Linux ward: as administrator, Kintrinsic → **Connect a phone** → under
   *Confirm in person*, **Show the code to confirm** (asks for the
   administrator password) → phone B scans the code. In a terminal,
   `sudo charter-presence --list` lists what is pending.
   **Expect:** `charter status` lists roots A and B, and phone B no longer
   marks the computer *unconfirmed*.
7. Phone B changes a rule. **Expect:** the ward enforces it and shows B as
   the guardian who set it. Phone A sees the new rule within a
   minute, both in the ward's own report and in the family list, because the
   list carries the rules in force (gate 6 relies on this).
8. Objection: phone A → Guardians → **Remove the other parent (npub1…)** →
   **Start removing them**. Confirm it in person at the Android ward
   (presence code, PIN 123456, phone A → Guardians → **Confirm in person**).
   **Expect:** STATUS shows a pending removal with "takes effect" 72 hours
   out. Leave phone B's Kintrinsic open and online and do not touch it: its
   app objects on its own, with no **Object** button to tap (gate 6 checks
   the wording). **Expect:** within 5 minutes the pending removal
   disappears on the ward; both guardians remain.
9. Phone B: Guardians → **Leave the family**, then **Leave the family**
   again in the warning.
   **Expect:** B disappears from the ward at once. Phone B shows "This phone
   no longer has access to …". The rule B set in step 7 **stays enforced**
   until phone A changes that rule.
10. Phone A: Guardians. **Expect:** **Leave the family** is disabled with
   "You're the only guardian on … Unpair it instead."

## Gate 3: a lost phone

1. Give identity A a second install: a second browser on phone A (or phone
   B), test build → Settings → **Join a family** → **Connect My Signet** →
   identity A's connection link and A's own npub → **Connect**. On **Scan
   the other parent's guardian code**, tap **This is a second phone for my
   own identity** (no scan: the same identity trusts itself; scanning A's
   own code is refused as "your own guardian code"). Nothing is pending for
   a second install of the same identity, so it connects one device by its
   device code: Settings → add the child → on the child's settings page,
   **Set up a device** → **📱 Phone** → **Type the device code** → the ward's
   code → name it.
   **Expect:** ward About this device shows root A with two working keys.
   With phone A's Kintrinsic open, the second install then shows
   the whole family, with one child (not two).
2. From the second install: Guardians → **Lost a phone?** → **Withdraw** next
   to the first install's phone → **Withdraw that phone for good**.
   **Expect:** within a minute the ward lists one working key under A. The
   first install shows "This phone no longer has access to …". A rule change
   sent from the first install is refused by the ward (it never applies).
   Rules it set earlier stay enforced until the second install changes them.
3. Restart the ward. **Expect:** the withdrawal holds (the key never comes
   back).

## Gate 4: the guardian PIN reaches a phone that was asleep

**Devices:** guardian phone A (any identity), Android ward freshly provisioned with no PIN set (gate 1 step 1).

1. Ward: switch to airplane mode, or power it off.
2. Wait **at least 15 minutes** (longer than the old 10-minute limit).
3. Phone A: Settings → **Guardians** → **Set the guardian PIN**: **123456**, twice → **Set guardian PIN**.
   **Expect:** "Setting the guardian PIN on <ward name>…", and it stays that way.
4. Ward: turn airplane mode off, or power on. Wait up to 5 minutes. Do not touch phone A.
   **Expect:** phone A stops saying "Setting the guardian PIN…" and shows the PIN as set.
5. Ward: About this device → Guardians → **Guardian presence** → **Show a code for this** → **123456**.
   **Expect:** the screen asks for the PIN (it does not say that none is
   set); after 123456, a QR and short code appear.
6. Check that nothing else changed: the ward's relay list and rules are as before step 3.

**Fail if:** the PIN is refused as too old, phone A keeps saying "sending" after the ward has been online 10 minutes, or the ward's relays change.

## Gate 5: a phone-held identity moves to My Signet, wards already paired

**Devices:** phone A with **no** My Signet (a fresh test-build origin; the phone's own key is the identity), Android ward, Linux ward, phone B with My Signet holding a spare identity **A2**. For the Heartwood variant, A2 is on a Heartwood.

1. Phone A: pair the Android ward and the Linux ward the ordinary way. Set the guardian PIN **123456**. Set one rule (a screen-time limit).
2. Phone A: Settings → **Guardians** → **Move my identity to My Signet**. Paste A2's connection link.
   **Expect:** a message that this phone will use a separate day-to-day key; then My Signet (or the Heartwood) is asked to sign. With a Heartwood, count the button presses and note them.
3. Phone A: **Confirm in person** at the Android ward (presence code, PIN **123456**) and at the Linux ward (administrator password).
   **Expect:** each ward switches as you confirm. `charter status` on Linux, and About this device on Android, list A2 as the guardian. The rule from step 1 is still enforced throughout. **No device is paired again.**
4. Phone A, after both wards have switched: **Delete the old key** → confirm.
   **Expect:** the app still works; a new rule from phone A reaches both wards.
5. Settings → Key backup: make a new backup. **Expect:** the app tells you older backups no longer bring your family back.

**Variant:** do step 3 for the Android ward only and wait out a week (or expire the move). **Expect:** the app shows "Finish your identity move" and refuses invite, remove and leave until it is finished.

## Gate 6: removal with automatic objection

**Devices:** phones A and B (the end state of gate 2 step 7: both guardians on the Android ward), Android ward. Keep phone B's Kintrinsic open and online.

1. Phone A: Guardians → **Remove the other parent (npub1…)** → **Start removing them**. Confirm in person at the Android ward.
   **Expect:** "Removal started" on A; the 72 hours run.
2. Phone B: do **not** touch anything. Wait up to 5 minutes.
   **Expect:** phone B shows "<A> asked to remove you from <child>'s devices. We've objected on your behalf, so you stay a guardian." and a **Leave the family instead** button.
3. Ward: after the next check-in, About this device → Guardians lists A and B. In the guardian-only presence list, the removal is gone.
4. Phone A: **Remove the other parent** again. **Expect:** it is dropped again (an objection blocks re-issue for 7 days), or A is told so.
5. Phone B: close the app for the whole of a fresh removal, then reopen it within 72 hours. **Expect:** the objection goes out on reopening and the removal is dropped.
6. Phone B: **Leave the family instead** → **Leave the family** again. **Expect:** B disappears from the ward at once.

**Limit:** letting a removal run its full 72 hours against a parent with no working app takes three days; record it as not run.

## Gate 7: a second parent joins with only a phone

**Devices:** phone A (My Signet, identity A), phone B with **no** My Signet (a fresh test-build origin), Android ward, Linux ward.

1. Phone B: Settings → **Join a family** → **Join with this phone only**.
   **Expect:** a guardian ID shown as a QR and as text.
2. Phone A: Guardians → **Invite a parent** → **Scan their QR code** → name them → **Send the invitation**.
3. Phone B: scan A's guardian code. **Expect:** "Now confirm in person".
4. Phone B: confirm in person at the Android ward (PIN **123456**) and at the Linux ward (administrator password).
   **Expect:** both wards list roots A and B, with B having its own day-to-day key.
5. Phone B changes a rule. **Expect:** the ward enforces it and names B as the setter.
6. Phone B: the join note says to back up the guardian identity. Settings → Key backup → make a backup.
7. Phone B: **Lost a phone?** **Expect:** it says the key is kept on this phone and to restore a backup if the phone is lost.

## Gate 8: a phone clock 10 minutes ahead

**Devices:** guardian phone B and guardian phone A (both guardians on the Android ward), Android ward on correct time.

1. Phone B: switch off automatic date and time; set the clock **10 minutes ahead**.
2. Phone B: change a rule (today's screen-time limit).
   **Expect:** the ward enforces it within a minute. It is not refused.
3. Phone B: within a few minutes, the child's page shows the clock notice ("…'s clock is behind this phone's; rules still apply, in the order they arrive.") or the notice of the same meaning for this device. The notice stays for about 3 days.
4. Phone A (correct clock): change the **same** kind of rule afterwards.
   **Expect:** the ward enforces A's change. B's future date does not hold it back.
5. Phone B: turn automatic time back on; change the rule again.
   **Expect:** it is enforced.

**Fail if** any change in steps 2 to 5 is refused, or A's change in step 4 is ignored.

## Gate 9: pending changes are hidden from the child but listed to the confirmer

**Devices:** phones A and B, Android ward, Linux ward.

1. Phone A: Invite phone B's identity (gate 2 step 1). **Do not confirm** anything.
2. Android ward: About this device → Guardians. **Expect:** the guardians are listed with short IDs; no invitation, no "change waiting" line.
3. Linux ward, as an ordinary user: `charter status`, the tray menu, and `cat /run/charter/guardians.json`. **Expect:** no pending entry in any.
4. Android ward: Guardians → **Guardian presence** → enter **123456**. **Expect:** the invitation is listed, as "Add … as a guardian".
5. Linux ward: Kintrinsic → **Connect a phone** → **Show the code to confirm** (administrator password), and `sudo charter-presence --list`. **Expect:** the invitation is listed.
6. Repeat 1 to 5 with a pending removal (gate 6 step 1, before phone B's app reacts: keep B offline).

## Gate 10: restore from a backup with a My Signet identity

**Devices:** phone A with My Signet identity A (the state after gate 1), a second phone or browser profile, Android ward.

1. Phone A: Settings → Key backup. **Expect:** it says the identity is in My Signet, so the backup carries the family only. Make a backup with a passphrase and save it.
2. Second phone, fresh test-build origin: Settings → Key backup → **Restore from backup**, using the passphrase.
   **Expect:** the child and devices return. A notice says your family was restored and that this phone has a new day-to-day key for My Signet to approve.
3. **Approve this phone.** My Signet signs once.
4. **Expect:** the notice now says your devices still trust the lost phone's key, with **Withdraw "…"**. Tap it and confirm.
   **Expect:** within a minute the ward lists one day-to-day key under A, and phone A shows "This phone no longer has access to …".
5. Second phone: change a rule. **Expect:** the ward enforces it. The ward was never touched.
6. Restart the ward. **Expect:** the withdrawal holds.

**Variant:** repeat with a phone-held identity. **Expect:** the restore brings back the identity and family, makes a new day-to-day key, and the backup contains no PIN.

## Gate 11: the Android ward shows nothing before the PIN

**Devices:** phone A, phone B, Android ward with the guardian PIN already set (**123456**, gate 1 step 4). The ward APK and its native library must be built together from the same branch.

1. Phone A: Invite phone B's identity (gate 2 step 1). **Do not confirm** anything.
2. Android ward: About this device. **Expect:** no pending line.
3. Android ward: About this device → Guardians → **Guardian presence**.
   **Expect:** only the PIN field. No invitation, no names, no list.
4. Enter **123456**.
   **Expect:** the list shows "Add … (npub1…) as a guardian", who asked, and when it expires.
5. Tap **Show a code for this**.
   **Expect:** a QR code and the typed form appear. Phone B scans it (gate 2 step 5): the co-guardian is confirmed.
6. Press Home, then reopen the screen.
   **Expect:** it asks for the PIN again.
7. Backoff: enter a wrong PIN five times, then **123456**.
   **Expect:** "Too many wrong PINs" with a countdown, and no list. After the countdown, **123456** shows the list again.

**Fail if** any name, invitation or count appears before the PIN is accepted.

## Gate 12: a working key cannot change the relay list

**Devices:** phone A (My Signet identity A, so its daily signing is by a working key), Android ward.

**Needs:** a relay code for a new relay list that is signed by a working key, not by the identity. The Android build report lists the guardian app sending its working key to the carrier as not yet done; if the app cannot make such a code, record this gate as not run and say why.

1. Make the relay code for a new relay list, signed by a working key.
2. Android ward: About this device → **Change relays / battery** → paste the code.
   **Expect:** the error "Only a parent's own identity can change this device's relays." The ward's relay list is unchanged.
3. Repeat with a relay code signed by the identity (My Signet signs it).
   **Expect:** it is accepted and About this device shows the new relays.

## Gate 13: a ward that cannot read its guardian state (Android)

**Devices:** a spare Android ward that can be sacrificed, paired as in gate 1. Do not use the family's real ward.

1. Corrupt both of the ward's guardian state files (`guardian-state.json` and its `.2` fallback, in the ward app's private storage). If the debug-signed test APK does not allow `adb shell run-as`, record this gate as not run.
2. Restart the ward app, then lock the ward.
   **Expect:** the lock screen's first line, and About this device, say "Rules can't be changed on this phone. Ask a parent." The lock still holds: the ward stays locked and the existing rules still apply.
3. Restore the files, or unpair and pair again, and confirm the notice goes away.

## Gate 14: the Linux console confirms a change in person

**Devices:** Linux ward with the Kintrinsic console and a polkit prompt, an administrator account, phone B (invited by phone A, gate 9 step 1, nothing confirmed). Test PIN is not used here; the administrator password is.

1. Console, as administrator: **Connect** → **Confirm a change in person**. Enter the administrator password.
   **Expect:** the list appears with the pending invitation. (The password may be asked a second time at step 2; that is expected unless polkit remembers the first.)
2. Pick the invitation, enter the password.
   **Expect:** a QR and a short code appear. Phone B scans it and the co-guardian is confirmed.
3. Open the console as the child's (ward) account, not an administrator.
   **Expect:** the card is not shown.
4. Enter a wrong password or cancel the prompt.
   **Expect:** a readable refusal; note whether the toast stays long enough to read (it lasts about 2.6 seconds).

## Gate 15: the console shows who set each rule

**Devices:** Linux ward with two guardians (the end state of gate 2 step 6), phones A and B.

1. Phone A sets one rule and phone B sets a different one, both for this Linux ward.
2. Console → **Limits**.
   **Expect:** a "Who set these rules" section. Each rule names its setter: the label if there is one, otherwise the signing key's short npub beside its root's short npub.
3. Compare with `charter status`.
   **Expect:** the same setters; with no label, the command line shows the root alone.

## Gate 16: the Linux ward cannot read its guardian state

**Devices:** a Linux ward that can be sacrificed, with the console, the tray and the Mint tray.

1. Copy `/var/lib/charter/guardian-state.json` and `guardian-state.json.2` somewhere safe. As root, corrupt both. Restart `charterd`.
2. **Expect:** `/run/charter/guardians.json` has `"unavailable":true`. Every console page shows the banner "Rules can't be changed on this computer. Ask a parent." The tray menu and the Mint tray menu show the same notice, and the lock screen's first line says it. The lock still holds.
3. Put the copies back and restart `charterd`.
   **Expect:** the notice disappears everywhere within a minute.

## Known limits for these gates

- The 72-hour removal can only be tested as far as the pending state and the
  objection; letting one run its course takes three days.
- A parent who joins starts from the family's rules, not a blank slate. A
  rule that can't be shown as a setting appears as "Set from <name>" with a
  **Replace** button (`docs/guardian-keys-open-questions.md`, "Rules a joining
  parent doesn't know").
- A second install of the same identity needs one device code before the
  family reaches it ("A second install of the same identity").
