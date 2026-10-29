# Guardian keys: hardware gates

Hardware tests for *Guardian roots and working keys (v1)* (`spec/contract.md`)
on branch `feat/guardian-keys`. Run them in order: each gate assumes the one
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

Artefacts are built locally into `/var/tmp/kintrinsic-test/` (ward APK
0.6.18 (49), Linux `.deb` 0.7.13, `SHA256SUMS`).

1. Guardian app test build, served to the phones over USB (a `localhost`
   origin is a secure context, so the camera and service worker work):

   ```
   cd apps/charter-app && npm run build && npx vite preview --host 127.0.0.1 --port 4173
   adb -s <phone> reverse tcp:4173 tcp:4173
   ```

   Open `http://localhost:4173` on the phone. This is a separate origin from
   the production app, so it starts with an empty family.
2. Upgrade an already-paired Android ward in place. The local test APK is
   debug-signed, and fielded wards now carry the release certificate, so it
   installs only on a fresh or reset ward (gates 1–3). The in-place upgrade
   needs the CI-signed build: tag the branch head (`ward-v0.6.18`), let
   `release-artifacts.yml` produce the draft release (never published), then
   `adb install -r <that APK>`.
   **Expect:** the ward keeps its pairing and every existing rule is still
   enforced. About this device → Guardians lists one guardian. The production guardian
   app still sees the ward's STATUS and can still change a rule (the ward
   treats the single pinned guardian exactly as before).
3. Upgrade a paired Linux ward: `sudo apt install ./kintrinsic_0.7.13_*.deb`.
   **Expect:** as step 2. `charter status` shows the guardian list, and
   `/var/lib/charter/guardian-state.json` exists with mode 0600.

## Gate 1: a Heartwood-first family from scratch

1. Factory-reset the Android ward and provision the 0.6.18 test APK as Device
   Owner (QR provisioning as usual).
2. Guardian phone A, test build: Family → Parent approval → **Other ways (for
   people who already use Signet or Heartwood)** → **Set up with Heartwood** →
   paste My Signet's connection link for identity A → **Connect**.
   **Expect:** My Signet asks once to sign a *kind 31120* event (this phone's
   approval). Approve it. **If My Signet refuses kind 31120**, stop and record
   it: that is a My Signet permission change, not a Kintrinsic bug.
3. Family → add the child → on the child's card, **Set up a device** →
   **📱 Phone**. Scan the pairing code with the Android ward and check the
   six-digit code matches on both screens. Under a My Signet root the ward
   can't announce itself to phone A, so then tap **Type the device code** and
   type the code the ward shows (About this device), and name the device.
   **Expect:** the guardian app shows the ward. Ward About this device →
   Guardians lists one guardian (A's short npub) with one working key.
4. Family → **Guardians** → **Set the guardian PIN**: **123456**, twice →
   **Set guardian PIN**.
5. Change a rule for the child (e.g. today's screen-time limit).
   **Expect:** the ward enforces it within a minute. Ward About this device
   shows A as the guardian who set it.
6. Linux ward (optional in this gate): on the computer, Kintrinsic →
   **Connect a phone** shows a code; on phone A, the child's card → **Set up
   another device** → **💻 Computer** → scan it.
   **Expect:** `charter status` shows root A (not the phone's working key)
   and one working key.

## Gate 2: a second parent joins, then leaves

1. Phone A: Family → **Guardians** → **Invite a parent** → **Scan their QR
   code** (or paste B's npub) → **Next** → **Send the invitation**.
   **Expect:** the ward's About this device → Guardians says "1 change is
   waiting for a guardian to confirm it at this device."
2. Phone B, test build (its own `adb reverse`): Family → **Join a family** →
   **Connect My Signet** → paste identity B's connection link and B's npub
   (*The guardian ID you were invited as*, required) → **Connect**. My Signet
   signs once (this phone's kind-31120 approval).
   **Expect:** if B connects with an identity other than the npub given, the
   app refuses with a readable error, disconnects and signs nothing.
3. At the Android ward: About this device → Guardians → **Guardian presence**
   → **Show a code for this** on "Add … as a guardian" → Guardian PIN 123456 →
   **Show code** → a QR and short code appear with a 10-minute countdown.
   - A wrong PIN 5 times in a row, then a 6th try: **expect** a 1-minute wait
     that survives a reboot.
   - Rotate the screen: it returns to the list (known; mint again).
4. Phone B: **Confirm in person** (on the join screen, or Family →
   Guardians) → scan the ward's QR.
   **Expect:** within a minute, ward About this device lists two guardians
   (A and B). Phone A's Guardians screen lists B as "Other parent".
   Phone B's Guardians screen says it has confirmed at a device and waits for
   the family. Open Kintrinsic on phone A: within a minute phone B's Family
   shows the child and its devices, the ward ready and any device B hasn't
   confirmed at marked *unconfirmed*.
5. Linux ward: as administrator, Kintrinsic → **Connect a phone** → under
   *Confirm in person*, **Show the code to confirm** (asks for the
   administrator password) → phone B scans the code. In a terminal,
   `sudo charter-presence --list` lists what is pending.
   **Expect:** `charter status` lists roots A and B, and phone B no longer
   marks the computer *unconfirmed*.
6. Phone B changes a rule. **Expect:** the ward enforces it and shows B as
   the guardian who set it. Phone A sees what the ward now enforces in the
   ward's own report within a minute (rosters carry names and devices, not
   rules).
7. Objection: phone A → Guardians → **Remove the other parent (npub1…)** →
   **Start removing them**. Confirm it in person at the Android ward
   (presence code, PIN 123456, phone A → Guardians → **Confirm in person**).
   **Expect:** STATUS shows a pending removal with "takes effect" 72 hours
   out. Phone B's Family shows "The other parent has asked to remove you as
   a guardian", and its Guardians screen an **Object** button. Tap it (My
   Signet signs and seals it). **Expect:** the pending removal disappears on
   the ward; both guardians remain.
8. Phone B: Guardians → **Leave the family**, then **Leave the family**
   again in the warning.
   **Expect:** B disappears from the ward at once. Phone B shows "This phone
   no longer has access to …". The rule B set in step 6 **stays enforced**
   until phone A changes that rule.
9. Phone A: Guardians. **Expect:** **Leave the family** is disabled with
   "You're the only guardian on … Unpair it instead."

## Gate 3: a lost phone

1. Give identity A a second install: a second browser on phone A (or phone
   B), test build → Family → **Join a family** → **Connect My Signet** →
   identity A's connection link and A's own npub → **Connect**. Nothing is
   pending for a second install of the same identity, so it connects one
   device by its device code: Family → add the child → **Set up a device** →
   **📱 Phone** → **Type the device code** → the ward's code → name it.
   **Expect:** ward About this device shows root A with two working keys.
   With phone A's Kintrinsic open, the second install's Family then shows
   the whole family, with one child (not two).
2. From the second install: Guardians → **Lost a phone?** → **Withdraw** next
   to the first install's phone → **Withdraw that phone for good**.
   **Expect:** within a minute the ward lists one working key under A. The
   first install shows "This phone no longer has access to …". A rule change
   sent from the first install is refused by the ward (it never applies).
   Rules it set earlier stay enforced until the second install changes them.
3. Restart the ward. **Expect:** the withdrawal holds (the key never comes
   back).

## Known limits for these gates

- A family whose root is the phone's own key (no My Signet) can't move to
  My Signet without re-pairing. That decision is for a design session
  (`docs/guardian-keys-open-questions.md`, "Moving a phone-held root").
- The 72-hour removal can only be tested as far as the pending state and the
  objection; letting one run its course takes three days.
- A parent who joins starts with no rules of their own for the child: a rule
  they set replaces only that kind of rule
  (`docs/guardian-keys-open-questions.md`, "Rules a joining parent doesn't
  know").
- A second install of the same identity needs one device code before the
  family reaches it ("A second install of the same identity").
