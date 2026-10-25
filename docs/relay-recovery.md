# Recovering from a relay outage

Kintrinsic uses three public relays by default: `wss://relay.snort.social`,
`wss://nos.lol` and `wss://relay.primal.net`. Wardens publish and read across
the configured list. A failed endpoint does not prevent another endpoint
from carrying traffic; relay attempts run concurrently. Cached clauses keep
applying when every relay is unavailable.

`relay.trotters.cc` now requires an allowlisted identity. Existing family
pairings keep that configured relay and gain the three public defaults when
updated, without unpairing or clearing clauses. Other custom relays are
preserved. This migration runs once; later guardian choices are honoured.
Trotters is absent from the public defaults for new pairings and can be added
explicitly after the administrator has allowlisted the family's keys.

## Joining a relay allowlist

Each device has its own public key, separate from the guardian's key. On a
Linux computer, `cat /var/lib/charter/device.pub` prints it; it is also shown
under **About this device** in the updated console. On Android, long-press
the full device code under **About this device**. Those 64 hexadecimal
characters identify the same key as its `npub` encoding. The guardian's full
public key is in **Family → Parent approval → Copy pairing link**, immediately
after `bunker://`. Share public keys with the relay administrator, never a
secret key or backup.

Updated clients support NIP-42 authentication using these stable keys. The
relay must permit NIP-59 kind-1059 gift wraps from an authenticated session;
their outer signing keys are intentionally temporary. Installed Android
0.6.12 does not support this authentication yet, so allowlisting alone may
not recover it. The administrator may need a temporary exception allowing
gift wraps addressed to the family's keys without authentication, so a
guardian-approved update can arrive. Relay access does not give the relay
the guardian's authority to change a charter.

Changing the guardian app alone cannot change the relay list inside an older
ward installation. Recovery must use a route that installation already
supports; the new relay-code screen is available only after its update lands.

## Already stranded Android phones

Do not assume that an APK can be installed locally. A paired release build
reasserts its install restrictions independently of screen-time locks, and
also disables USB debugging. An emergency screen unlock does **not** open an
install window. The ordinary maintenance approval still needs delivery over
the phone's existing relays, so it cannot solve a sole-relay outage itself.

Check the installed version and its existing pairing screen first. Older
builds that expose **Pair with guardian** can accept a fresh pairing link
locally using the **same guardian key** and a new relay list. The core accepts
that without a relay round trip and keeps the existing subject and cached
clauses. Do not disconnect the device in the guardian app first: that sends
a RELEASE through the unavailable relay. This is version-dependent: newer
ward home screens hide the pairing controls once paired, so a scanned link
must not be assumed to provide a usable recovery screen on every version.

When neither a usable local pairing screen nor authorised debugging access
exists, an updated guardian app or APK file alone cannot recover that phone.
For Trotters' private-relay change, arrange the temporary compatibility
exception described above with its administrator before sending a signed
maintenance/update instruction. Establish the available route for the
affected version before attempting an update.

## Linux administrator recovery

Use the computer's administrator account to install the updated Linux
package, keeping its existing pairing data. This does not need approval
through the guardian relay. The existing `charter-recovery` tool also offers
an administrator-only pause if enforcement obstructs repair.

## Relay changes after upgrading

To change relays after that:

1. In the guardian app, open **Family → the device → Relays**.
2. Enter at least two `wss://` addresses, preferably from different operators,
   then choose **Create relay code**. A local guardian key works offline;
   an external signer needs its own connection.
3. Apply the signed code within ten minutes. On Android, open Kintrinsic →
   **About this device → Change relays**, paste the code and choose **Apply**.
   On Linux, sign into the administrator account, open Kintrinsic →
   **About this device**, paste the code and choose **Apply relay change**.
   The command-line equivalent is `sudo charter-pair --relay-code < relay-code.json`.
4. After the ward confirms the change, choose **The device applied this —
   save relays** in the guardian app, so both ends use the same endpoints.

The code is bound to the device and signed by its existing guardian. It
cannot change the guardian or charter. Expired, altered and previously
applied codes are rejected; generate a fresh one if needed.

Viewing keys and relays is available to the ward. Editing is authorised by
the existing guardian's signature, with an additional administrator/polkit
boundary on Linux. Unrestricted ward editing would let a ward point the
warden at unreachable relays and suppress new instructions and reports.
Keeping the last verified clauses enforced during an outage prevents relay
failure itself from lifting restrictions.

## Updating the installed family devices

The recovery release versions are ward **0.6.13 (44)**, guardian **0.1.17 (18)**
and Linux **0.7.10**. The Linux package includes console **0.5.1**.

1. Have the relay administrator allowlist the guardian and each device key.
   Installed Android 0.6.12 also needs the compatibility exception above;
   allowlisting does not add authentication support to that installed app.
2. Check that existing devices report to the guardian again before sending
   update instructions. Keep the same guardian key, device keys and app data.
3. Publish the reviewed, signed releases using `docs/releasing.md`. Android
   signing uses the existing release certificate and rotation lineage in CI.
   Compile-only checks and native libraries are not installable APK updates.
4. Update the guardian app, then send the ward update from the guardian app.
   The old ward's approved self-update route can lift its install restriction
   for the install; an emergency screen unlock does not do that.
5. Install the Linux package from the computer's administrator account.
6. Check About on every updated device: the public npub, Trotters plus the
   independent relays, and the guardian-authorised relay-change controls.
   Confirm a fresh device report before considering the migration complete.

Trotters remains a family-configured relay, alongside the public alternatives.
No step requires unpairing, removing the warden or resetting a device.
