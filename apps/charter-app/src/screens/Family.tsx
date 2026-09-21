import { useEffect, useState, type CSSProperties } from "react";
import QRCode from "qrcode";
import {
  Avatar,
  Banner,
  Button,
  Card,
  Pill,
  SectionLabel,
} from "../components/ui";
import { useCharter } from "../store/store";
import { goToWard } from "../domain/wardRoute";
import { updateAvailable } from "../store/updateCheck";
import {
  accountSummary,
  canOpenInstallWindow,
  installWindow,
  installWindowLabel,
  readWindows,
  writeWindow,
  INSTALL_WINDOW_MINUTES,
} from "./installWindow";
import type { Child, Device, SignerKind } from "../domain/types";
import { guardianPubkeyHex } from "../signer/guardianKey";
import { pairingSas } from "../domain/pairingSas";
import {
  guardianBunkerUri,
  guardianNpub,
  guardianPairingBunkerUri,
  guardianPairingQrContent,
} from "../signer/guardianPairing";
import { DEFAULT_RELAYS } from "../signer/config";
import { parseDevicePairingCode, parsePairInvite } from "../wire/deviceCode";
import { shortMachine } from "../store/unclaimedDevices";
import { QRScanner } from "../components/QRScanner";
import UnlockDevice from "./UnlockDevice";
import AppVersion from "../carrier/AppVersion";
import { isValidDevicePubkey } from "./offlineUnlock";
import { bestLiveness, livenessChip } from "../domain/liveness";

// Accent colors a parent can pick for a child's avatar/chips.
const CHILD_COLORS = ["#3B6FB2", "#2E7D5B", "#C4860F", "#7A4FB5", "#B23B30"];

/**
 * Family & setup screen — the Stage 0 headline is "set up a computer".
 *
 * Pairing direction matches the product model: the LAPTOP shows a code/QR and
 * Kintrinsic (this phone) SCANS it, then you name it and "Govern this device".
 * Crypto is mocked; the flow is real.
 *
 * - Manage children (add / rename / remove)
 * - Per child: list their devices (Connected / Disconnected + disconnect/remove)
 *   and a prominent "Set up a computer" scan-and-name flow
 * - A clearly-disabled "Add game logins" teaser (Stage 2, coming later)
 * - Turn on parent approval (the confirm step) for your own changes/approvals
 * - Gentle first-run onboarding when there are no children yet
 */
export default function Family() {
  const { state } = useCharter();
  const hasChildren = state.children.length > 0;

  return (
    <>
      {/* id + scroll-margin: Home's getting-started scrolls here (the sticky
          header is ~72px, so offset the anchor so the heading isn't hidden). */}
      <div id="setup-children" style={{ scrollMarginTop: 80 }}>
        <SectionLabel>{hasChildren ? "Children" : "Add your first child"}</SectionLabel>
        {/* Family-level, above the cards: an unheard-of phone is the family's to
            place, and one banner can't be mistaken for one ward's business. */}
        <UnclaimedPhones />
        <div className="stack">
          {state.children.map((child) => (
            <ChildCard key={child.id} child={child} />
          ))}
          <AddChild startOpen={!hasChildren} />
        </div>
      </div>

      <div id="setup-approval" style={{ scrollMarginTop: 80 }}>
        <SectionLabel>Parent approval</SectionLabel>
        <p className="card-sub" style={{ margin: "0 4px 10px" }}>
          Parent approval holds your key so limits and “more time” decisions are
          signed by you. To connect a child's computer or phone, follow{" "}
          <a href="#/setup" style={{ color: "var(--brand)", fontWeight: 600 }}>
            the setup guide
          </a>
          . Connecting Signet instead of the built-in key is optional.
        </p>
        <SignerCard />
      </div>
      {/* Foot of the page: which Kintrinsic this actually is, and a way out
          when it is behind. Renders nothing in an ordinary browser tab —
          there is no shell to be out of date. */}
      <AppVersion />
    </>
  );
}

// ---------------------------------------------------------------------------
// Per-child card
// ---------------------------------------------------------------------------

function ChildCard({ child }: { child: Child }) {
  const { editChild, removeChild } = useCharter();
  const [editing, setEditing] = useState(false);
  const [draftName, setDraftName] = useState(child.name);
  // The colour spot is how a ward is recognised at a glance on every screen —
  // Home cards, avatars, the family list — so it belongs wherever the name is
  // editable. Picking it only once, when the child was created, meant living
  // with it forever.
  const [draftColor, setDraftColor] = useState(child.color);
  const [confirmingRemove, setConfirmingRemove] = useState(false);

  const connectedCount = child.devices.filter((d) => d.pairing === "paired").length;
  // "Connected" now means reachable NOW, not merely set up — the reading a
  // guardian would take from the word anyway.
  const childChip = livenessChip(bestLiveness(child.devices, Date.now()), Date.now());

  function saveName() {
    const next = draftName.trim();
    if (next) editChild(child.id, next, draftColor);
    setEditing(false);
  }

  /** Re-open the editor from the ward's CURRENT values, so a cancelled edit
   *  never leaves a stale draft to be saved by accident next time. */
  function startEditing() {
    setDraftName(child.name);
    setDraftColor(child.color);
    setEditing(true);
  }

  return (
    <Card>
      <div className="row-between" style={{ alignItems: "flex-start" }}>
        <div style={{ display: "flex", gap: 14, alignItems: "center", minWidth: 0 }}>
          {/* The avatar previews the DRAFT while editing, so the choice is
              visible on the thing it actually changes rather than only after
              saving. */}
          <Avatar name={editing ? draftName || child.name : child.name} color={editing ? draftColor : child.color} />
          <div style={{ minWidth: 0 }}>
            {editing ? (
              <div className="stack" style={{ gap: 10 }}>
                <label className="field" style={{ margin: 0 }}>
                  <span className="field-label">Name</span>
                  <input
                    className="input"
                    autoFocus
                    value={draftName}
                    onChange={(e) => setDraftName(e.target.value)}
                    onKeyDown={(e) => {
                      if (e.key === "Enter") saveName();
                      if (e.key === "Escape") setEditing(false);
                    }}
                  />
                </label>
                <ColorChoice
                  value={draftColor}
                  onChange={setDraftColor}
                  groupName={`child-color-${child.id}`}
                />
              </div>
            ) : (
              <>
                {/* The name is the way into this ward's rules. It looked
                    tappable and wasn't, so the only route to a specific
                    child's limits was the tab strip on another screen. */}
                <button
                  type="button"
                  onClick={() => goToWard("limits", child.id)}
                  aria-label={`Open ${child.name}'s limits`}
                  style={{
                    display: "block",
                    width: "100%",
                    padding: 0,
                    margin: 0,
                    border: 0,
                    background: "none",
                    textAlign: "left",
                    cursor: "pointer",
                    color: "inherit",
                    font: "inherit",
                  }}
                >
                  <h2 className="card-title" style={{ margin: 0 }}>
                    {child.name} <span aria-hidden="true">›</span>
                  </h2>
                </button>
                {/* Only when there is nothing to count. With devices set up the
                    list right below names every one of them and says whether
                    it's reachable — "1 device connected" above it was the third
                    statement of the same fact on this one card. */}
                {connectedCount === 0 ? (
                  <p className="card-sub">{deviceSummary(connectedCount)}</p>
                ) : (
                  <button
                    type="button"
                    className="ward-heading-link"
                    style={{ marginLeft: 0, padding: "2px 0" }}
                    onClick={() => goToWard("activity", child.id)}
                  >
                    Their week <span aria-hidden="true">›</span>
                  </button>
                )}
              </>
            )}
          </div>
        </div>
        {!editing && <Pill tone={childChip.tone}>{childChip.text}</Pill>}
      </div>

      {editing ? (
        <div className="stack" style={{ marginTop: 14 }}>
          <Button block onClick={saveName}>
            Save
          </Button>
          <Button variant="secondary" block onClick={() => setEditing(false)}>
            Cancel
          </Button>
        </div>
      ) : (
        <>
          {/* Devices + setup */}
          <div style={{ marginTop: 16 }}>
            <Devices child={child} />
          </div>

          {/* Stage 2 teaser — clearly disabled, "coming later". */}
          <GameLoginsTeaser />

          {/* Manage */}
          <div className="row-between" style={{ marginTop: 16, gap: 8 }}>
            <Button variant="ghost" onClick={startEditing}>
              Rename
            </Button>
            {!confirmingRemove && (
              <Button variant="ghost" onClick={() => setConfirmingRemove(true)}>
                Remove
              </Button>
            )}
          </div>

          {confirmingRemove && (
            <div style={{ marginTop: 12 }}>
              <Banner tone="warn">
                Remove {child.name}? This releases every device of theirs from
                Kintrinsic — all limits lift — and clears their pending
                requests. A device that is offline is released when it next
                connects.
              </Banner>
              <div className="row-between" style={{ marginTop: 10, gap: 8 }}>
                <Button variant="secondary" block onClick={() => setConfirmingRemove(false)}>
                  Keep
                </Button>
                <Button variant="danger" block onClick={() => removeChild(child.id)}>
                  Remove {child.name}
                </Button>
              </div>
            </div>
          )}
        </>
      )}
    </Card>
  );
}

function deviceSummary(connected: number): string {
  if (connected === 0) return "No device set up yet";
  if (connected === 1) return "1 device connected";
  return `${connected} devices connected`;
}

function platformEmoji(platform: Device["platform"]): string {
  return platform === "android" ? "📱" : "💻";
}

/** "moments ago" / "3 min ago" / "2 h ago" / silence beyond a day. */
function lastSeenLabel(lastSeenAt: number | undefined, now: number): string | null {
  if (!lastSeenAt) return null;
  const secs = Math.max(0, Math.floor((now - lastSeenAt) / 1000));
  if (secs < 90) return "seen moments ago";
  if (secs < 3600) return `seen ${Math.floor(secs / 60)} min ago`;
  if (secs < 86400) return `seen ${Math.floor(secs / 3600)} h ago`;
  return `not seen for ${Math.floor(secs / 86400)} d`;
}

// ---------------------------------------------------------------------------
// Devices: list + "set up a computer" flow
// ---------------------------------------------------------------------------

function Devices({ child }: { child: Child }) {
  const [setupOpen, setSetupOpen] = useState(false);
  const hasDevices = child.devices.length > 0;

  return (
    <div>
      <div className="field-label" style={{ marginBottom: hasDevices ? 8 : 0 }}>
        Devices
      </div>

      {hasDevices && (
        <ul
          className="list"
          style={{ listStyle: "none", margin: "0 0 12px", padding: 0 }}
        >
          {child.devices.map((device) => (
            <DeviceRow key={device.id} child={child} device={device} />
          ))}
        </ul>
      )}


      {setupOpen ? (
        <SetupComputer child={child} onClose={() => setSetupOpen(false)} />
      ) : (
        <Button
          variant={hasDevices ? "secondary" : "primary"}
          block
          onClick={() => setSetupOpen(true)}
        >
          {hasDevices ? "Set up another device" : "Set up a device"}
        </Button>
      )}
    </div>
  );
}

/**
 * Phones that paired with this guardian but never made it into their records.
 *
 * Pairing can finish on the phone's side alone: it holds the guardian key and
 * heartbeats every 60s, while Kintrinsic has no device to seal clauses to — so
 * every rule silently goes nowhere (Mia, 2026-07-27). Nothing recovered
 * from it, yet the phone's own STATUS was arriving the whole time. This offers
 * that evidence back as one tap.
 *
 * Rendered ONCE for the family, not inside each child's card. An unclaimed
 * phone belongs to the family until someone says whose it is, so per-child
 * rendering repeated the same phone under every ward — and invited exactly the
 * mis-assignment this screen exists to prevent (decented saw Mia's Pixel
 * offered under Robin, whose own phone was already set up).
 *
 * The guardian still picks the ward rather than the app guessing: a ward is a
 * local label at this stage, so a heartbeat carries nothing identifying WHO it
 * belongs to. Guessing would repeat the original mistake — assuming a pairing
 * instead of confirming one.
 */
function UnclaimedPhones() {
  const { state, unclaimed, claimDevice } = useCharter();
  const [claimedTo, setClaimedTo] = useState<Record<string, string>>({});
  const children = state.children;

  if ((unclaimed.length === 0 && Object.keys(claimedTo).length === 0) || children.length === 0)
    return null;

  return (
    <div className="stack" style={{ marginBottom: 12 }}>
      {/* Claiming removes the machine from `unclaimed` in the SAME render, so
          the confirmation — and its "save their rules again" instruction, the
          actual recovery step — must live outside that list or it never
          displays at all (it didn't, until this). */}
      {Object.entries(claimedTo).map(([machine, name]) => (
        <Banner key={`claimed-${machine}`} tone="ok">
          <p className="card-sub" aria-live="polite" style={{ margin: 0 }}>
            Set up as {name}’s phone (<code>{shortMachine(machine)}</code>).
            {" "}{name}’s rules are being sent to it now — the activity feed
            confirms when they’ve gone.
          </p>
        </Banner>
      ))}
      {unclaimed.map((u) => (
        <Banner key={u.machine} tone="warn">
          <div>
            A phone you set up with a pairing code from this app is reaching out
            to you, but isn’t recorded here yet — so nothing you set can reach
            it.
            {u.appVersionName ? ` It says it’s running Kintrinsic ${u.appVersionName}.` : ""}
          </div>
          <div className="card-sub" style={{ marginTop: 6 }}>
            Its device code starts <code>{shortMachine(u.machine)}</code> — check
            that against the code on the phone’s Kintrinsic screen before you say
            whose it is.
          </div>
          <div style={{ marginTop: 8, display: "flex", flexWrap: "wrap", gap: 8 }}>
            {children.map((c) => (
              <Button
                key={c.id}
                variant="secondary"
                onClick={() => {
                  const d = claimDevice(c.id, u.machine, `${c.name}’s phone`);
                  if (d) setClaimedTo((m) => ({ ...m, [u.machine]: c.name }));
                }}
              >
                {children.length === 1
                  ? `This is ${c.name}’s phone`
                  : `It’s ${c.name}’s`}
              </Button>
            ))}
          </div>
        </Banner>
      ))}
    </div>
  );
}

function DeviceRow({ child, device }: { child: Child; device: Device }) {
  const { unpairDevice, removeDevice, reconnectDevice, deviceStatus, updateManifest, debManifest, sendCharterUpdate, openMaintenanceWindow } =
    useCharter();
  // When the install window this guardian opened runs out — an absolute expiry,
  // matching the clause, so the screen closes it on its own instead of latching
  // "opened" and disagreeing with a phone that has long since re-locked.
  // Read from storage, not component state: a reload during an open window would
  // otherwise forget it, taking "Close now" away for exactly as long as the
  // loosening lasts.
  const [installOpenUntil, setInstallOpenUntil] = useState<number | undefined>(
    () => readWindows(Math.floor(Date.now() / 1000))[child.id],
  );
  const [nowUnix, setNowUnix] = useState(() => Math.floor(Date.now() / 1000));
  const [unlockOpen, setUnlockOpen] = useState(false);
  const [confirmingDisconnect, setConfirmingDisconnect] = useState(false);
  const [updateSent, setUpdateSent] = useState(false);
  const paired = device.pairing === "paired";
  // Self-update (#44): offer only when the device REPORTS a version that is
  // strictly behind the site's published artifact. Phones only — the Linux
  // warden ships as a .deb, not this APK.
  const reportedVersion = device.devicePubkey
    ? deviceStatus[device.devicePubkey]?.appVersionCode
    : undefined;
  // What the device SAYS it is running, shown verbatim. Without this a
  // guardian tapping "Update Kintrinsic" had no way to see whether anything moved
  // — the button simply reappeared, which looks identical whether the update
  // failed, is still downloading, or never left.
  const reportedName = device.devicePubkey
    ? deviceStatus[device.devicePubkey]?.appVersionName
    : undefined;
  const canUpdate =
    paired && device.platform === "android" && updateAvailable(updateManifest, reportedVersion);
  // Linux is told, not offered: the laptop reports its version now, but Kintrinsic
  // can't remotely install a .deb yet, so pointing at the download page is the
  // honest affordance rather than a button that would do nothing.
  // A Kintrinsic update that keeps failing. Previously invisible: the phone
  // retried in silence while this card showed only an Update button that kept
  // reappearing, which looks identical to "downloading" (two days lost,
  // 2026-07-24..26).
  const health = device.devicePubkey
    ? deviceStatus[device.devicePubkey]?.updateHealth
    : undefined;
  const linuxBehind =
    paired &&
    device.platform === "linux" &&
    updateAvailable(debManifest as never, reportedVersion);
  const updateArrived = updateSent && !canUpdate;
  // Reconnect only ever reuses a REAL stored key (the old path re-"scanned" a
  // sentinel string and clobbered the key with a mock). Without one, the honest
  // route back is the full set-up flow.
  const canReconnect = !!device.devicePubkey && /^[0-9a-f]{64}$/.test(device.devicePubkey);
  // Offline unlock needs the device's real machine key to derive the code.
  // Available for any paired device that has one — the parent may need it the
  // moment the child is locked out, not only after some "locked" signal.
  const canUnlock = paired && isValidDevicePubkey(device.devicePubkey);
  const seen = lastSeenLabel(device.lastSeenAt, Date.now());
  // Kintrinsic's install lock is closed on every chartered phone, which is why a
  // ward could not update an app from their store at all. A guardian-opened,
  // signed, self-closing window is the stand-down — the same clause a stuck
  // phone is rescued with, offered plainly instead of only when something broke.
  const canInstallWindow = canOpenInstallWindow(device);
  // What the phone says came through the last window — the guardian's account
  // of their own loosening, not a feed of what the ward installs.
  const account = device.devicePubkey
    ? deviceStatus[device.devicePubkey]?.installWindow
    : undefined;
  // Boots the phone went through with no warden running (S1). Safe mode is the
  // case it exists for: nothing runs, so nothing is enforced and nothing is
  // reported, and the phone comes back looking exactly as it did. The device
  // cannot witness its own absence — it can only count the boots it missed.
  const gap = device.devicePubkey
    ? deviceStatus[device.devicePubkey]?.enforcementGap
    : undefined;
  const openWindow = installWindow(installOpenUntil, nowUnix);
  useEffect(() => {
    if (!installOpenUntil) return;
    // Re-read the CLOCK each tick rather than counting ticks: the OS freezes a
    // backgrounded PWA's timers, and a counter would come back minutes wrong
    // (the staleness trap that made rules look like they hadn't landed).
    const id = setInterval(() => setNowUnix(Math.floor(Date.now() / 1000)), 1000);
    return () => clearInterval(id);
  }, [installOpenUntil]);

  return (
    <li
      style={{
        display: "flex",
        flexWrap: "wrap",
        alignItems: "center",
        gap: 12,
        padding: "12px 14px",
        borderBottom: "1px solid var(--line)",
      }}
    >
      <span aria-hidden="true" style={{ fontSize: 22, lineHeight: 1 }}>
        {platformEmoji(device.platform)}
      </span>
      <div style={{ flex: "1 1 180px", minWidth: 0 }}>
        <div style={{ fontWeight: 600, overflow: "hidden", textOverflow: "ellipsis" }}>
          {device.label}
        </div>
        {/* Two separate facts, no longer conflated: whether Kintrinsic GOVERNS
            this device (a setup question) and whether it is reachable right
            now (an operational one). "Connected and managed" answered only the
            first while sounding like both. */}
        <div className="card-sub">
          {paired ? "Managed by Kintrinsic" : "Disconnected"}
          {paired ? ` · ${seen ?? "waiting to hear from it"}` : ""}
        </div>
        {reportedName && (
          <div className="card-sub">Kintrinsic {reportedName}</div>
        )}
        {canUpdate && updateSent && (
          <div className="card-sub">
            Update sent. The phone installs it on its next check-in — the
            version above changes when it lands.
          </div>
        )}
        {updateArrived && <div className="card-sub">Updated ✓</div>}
        {openWindow.open && (
          <div className="card-sub" style={{ color: "var(--ok, #1a6b45)" }}>
            Installs are open — {installWindowLabel(openWindow.secondsLeft)}.{" "}
            {child.name} can install and update apps from the store until then.
            The phone closes it on its own; “Close now” shuts it sooner, as soon
            as the phone next hears from you.
          </div>
        )}
        {account && (
          <div className="card-sub">
            {openWindow.open
              ? `So far this window: ${accountSummary(account)}`
              : `When installs were last open, ${new Date(
                  account.startedAt * 1000,
                ).toLocaleString()} — ${accountSummary(account)}`}
            {account.changes.length > 0 && (
              <ul style={{ margin: "4px 0 0", paddingLeft: 18 }}>
                {account.changes.map((c) => (
                  <li key={`${c.pkg}-${c.at}`}>
                    {c.label} — {c.kind === "installed" ? "new" : "updated"}
                  </li>
                ))}
              </ul>
            )}
          </div>
        )}
        {health && health.attempts > 2 && (
          <div className="card-sub" style={{ color: "var(--warn, #8a5a00)" }}>
            Update isn’t completing — {health.lastError ?? "the install keeps failing"} (
            {health.attempts} tries since {new Date(health.sinceUnix * 1000).toLocaleDateString()}).
            {openWindow.open
              ? " You can repair it by cable while installs are open."
              : " Allow installs to repair it by cable."}
          </div>
        )}
        {gap && (
          <div className="card-sub" style={{ color: "var(--warn, #8a5a00)" }}>
            {gap.unexplainedBoots === 1
              ? "This phone started up once with Kintrinsic not running"
              : `This phone started up ${gap.unexplainedBoots} times with Kintrinsic not running`}{" "}
            (noticed {new Date(gap.lastNoticedAt * 1000).toLocaleDateString()}). That
            happens after a repair or a restore — and it is also what safe mode
            looks like, where Android switches off apps like Kintrinsic. Kintrinsic
            can’t tell which from here, so it’s telling you rather than
            guessing. Worth asking {child.name} about.
          </div>
        )}
        {linuxBehind && debManifest && (
          <div className="card-sub">
            Update available ({debManifest.versionName}) — install it from{" "}
            <a href="https://charter.signet.you/download">the download page</a>.
          </div>
        )}
      </div>
      <div style={{ display: "flex", alignItems: "center", gap: 8, marginLeft: "auto", flexWrap: "wrap", justifyContent: "flex-end" }}>
      <Pill tone={paired ? "ok" : "neutral"}>{paired ? "On" : "Off"}</Pill>
      {paired ? (
        <div style={{ display: "flex", gap: 4 }}>
          {canUpdate && !updateSent && updateManifest && (
            <Button
              variant="ghost"
              aria-label={`Update Kintrinsic on ${device.label} to ${updateManifest.versionName}`}
              onClick={() => {
                void sendCharterUpdate(child.id).then((ok) => setUpdateSent(ok));
              }}
            >
              Update Kintrinsic
            </Button>
          )}
          {openWindow.open && (
            <Button
              variant="ghost"
              aria-label={`Close the install window on ${device.label} now`}
              onClick={() => {
                // Zero minutes = an expiry at issue: a newer clause the phone
                // reads as already shut. Nothing to undo if it fails, because
                // the window closes on its own regardless.
                void openMaintenanceWindow(child.id, 0).then((ok) => {
                  if (ok) {
                    writeWindow(child.id, null, Math.floor(Date.now() / 1000));
                    setInstallOpenUntil(undefined);
                  }
                });
              }}
            >
              Close now
            </Button>
          )}
          {canInstallWindow && !openWindow.open && (
            <Button
              variant="ghost"
              aria-label={`Allow installs on ${device.label} for ${INSTALL_WINDOW_MINUTES} minutes`}
              onClick={() => {
                void openMaintenanceWindow(child.id, INSTALL_WINDOW_MINUTES).then(
                  (ok) => {
                    // Only start the countdown if the clause was actually signed
                    // and sent — a cancelled signer must not leave this screen
                    // claiming a window the phone never received.
                    if (ok) {
                      const nowUnix = Math.floor(Date.now() / 1000);
                      const until = nowUnix + INSTALL_WINDOW_MINUTES * 60;
                      writeWindow(child.id, until, nowUnix);
                      setInstallOpenUntil(until);
                    }
                  },
                );
              }}
            >
              Allow installs
            </Button>
          )}
          {canUnlock && (
            <Button
              variant="ghost"
              aria-label={`Unlock ${device.label} with a code`}
              onClick={() => setUnlockOpen(true)}
            >
              Unlock
            </Button>
          )}
          <Button
            variant="ghost"
            aria-label={`Disconnect ${device.label}`}
            onClick={() => setConfirmingDisconnect(true)}
          >
            Disconnect
          </Button>
        </div>
      ) : (
        <div style={{ display: "flex", gap: 4 }}>
          {canReconnect && (
            <Button
              variant="ghost"
              aria-label={`Reconnect ${device.label}`}
              onClick={() => reconnectDevice(child.id, device.id)}
            >
              Reconnect
            </Button>
          )}
          <Button
            variant="ghost"
            aria-label={`Remove ${device.label}`}
            onClick={() => removeDevice(child.id, device.id)}
          >
            Remove
          </Button>
        </div>
      )}
      </div>

      {/* Disconnect publishes a signed RELEASE — the most destructive wire
          action there is — and with "Approve without the extra tap" on, no
          sign sheet stands in front of it. One mis-tap in this crowded row
          must not un-govern a device, so it gets the same two-step confirm
          as removing a child. */}
      {confirmingDisconnect && (
        <div style={{ marginTop: 12 }}>
          <Banner tone="warn">
            Disconnect {device.label}? Kintrinsic stops managing it — every
            limit lifts — and it would need pairing again to come back.
          </Banner>
          <div className="row-between" style={{ marginTop: 10, gap: 8 }}>
            <Button variant="secondary" block onClick={() => setConfirmingDisconnect(false)}>
              Keep
            </Button>
            <Button
              variant="danger"
              block
              onClick={() => {
                setConfirmingDisconnect(false);
                unpairDevice(child.id, device.id);
              }}
            >
              Disconnect {device.label}
            </Button>
          </div>
        </div>
      )}

      {unlockOpen && (
        <UnlockDevice
          child={child}
          device={device}
          onClose={() => setUnlockOpen(false)}
        />
      )}
    </li>
  );
}

/**
 * Set-up flow. The PHONE path is one scan: this screen shows a QR (the
 * guardian's pairing link + a one-time token); the parent scans it with the
 * CHILD phone's camera; the Kintrinsic app opens, pairs, and echoes the token on
 * its heartbeat — and the device appears here by itself, real pubkey bound,
 * ready to name. Typing the device code stays as the no-camera fallback, and
 * remains the primary path for computers.
 */
function SetupComputer({ child, onClose }: { child: Child; onClose: () => void }) {
  const {
    state,
    addDevice,
    confirmPairing,
    enableLocalSigner,
    phonePairing,
    beginPhonePairing,
    cancelPhonePairing,
    sendPairOffer,
  } = useCharter();
  const [step, setStep] = useState<"scan" | "name">("scan");
  const [kind, setKind] = useState<Device["platform"]>("linux");
  const [code, setCode] = useState("");
  const [codeError, setCodeError] = useState(false);
  const [label, setLabel] = useState(`${child.name}'s laptop`);
  const [manualEntry, setManualEntry] = useState(false);
  const [pairing, setPairing] = useState(false);
  const [pairError, setPairError] = useState(false);
  const [qrSrc, setQrSrc] = useState<string | null>(null);
  const [qrContent, setQrContent] = useState<string | null>(null);
  /** The six-digit code both screens show for this pairing link (S6). */
  const [pairSas, setPairSas] = useState<string | null>(null);
  const [cableMode, setCableMode] = useState(false);
  const [cableSupported, setCableSupported] = useState(false);
  // True while a cable provision is mid-flight — locks the kind toggle so a
  // stray click can't unmount SetupPhoneCable and orphan the provision (Fix 3).
  const [cableBusy, setCableBusy] = useState(false);

  const kindNoun = kind === "android" ? "phone" : "computer";
  const codeParses = parseDevicePairingCode(code) !== null;
  const haveLocalSigner = state.signer.kind === "local";
  // The QR path and the cable path are mutually exclusive: entering cable mode
  // makes phoneScanMode false, so the QR-mint effect's cleanup cancels the QR
  // token (Fix 1) — the single shared pairing token is never owned by both.
  const phoneScanMode = kind === "android" && !manualEntry && step === "scan" && !cableMode;

  // Checked lazily via a dynamic import, only once the parent has picked
  // "Phone" — nothing WebUSB-related loads until it's actually relevant.
  useEffect(() => {
    if (kind !== "android") return;
    let alive = true;
    import("../provision/webusb")
      .then(({ webUsbSupported }) => {
        if (alive) setCableSupported(webUsbSupported());
      })
      .catch(() => {
        // A failed chunk load (e.g. a stale service-worker chunk) must fail
        // safe — assume no cable support rather than leak an unhandled reject.
        if (alive) setCableSupported(false);
      });
    return () => {
      alive = false;
    };
  }, [kind]);

  // Mint the one-time token + QR when the phone-scan wait is on screen;
  // cancel the wait whenever it isn't (leaving the flow kills the token).
  useEffect(() => {
    if (!phoneScanMode || !haveLocalSigner) {
      cancelPhonePairing();
      setQrSrc(null);
      return;
    }
    const token = beginPhonePairing(child.id);
    const pk = guardianPubkeyHex();
    const content = guardianPairingQrContent(pk, DEFAULT_RELAYS, token);
    setQrContent(content);
    let alive = true;
    QRCode.toDataURL(content, { width: 480, margin: 1 }).then((url) => {
      if (alive) setQrSrc(url);
    });
    // The short code the ward's landing page shows for this exact link (S6).
    void pairingSas(pk, token).then((code) => {
      if (alive) setPairSas(code);
    });
    return () => {
      alive = false;
      setQrContent(null);
      setPairSas(null);
      cancelPhonePairing();
    };
    // cableMode is a dep so LEAVING cable mode re-mints the QR — otherwise the
    // displayed QR would encode the cancelled (dead) token and scanning hangs.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [phoneScanMode, haveLocalSigner, child.id, cableMode]);

  // The phone scanned our QR and announced itself: bind its real pubkey and
  // jump straight to naming it.
  useEffect(() => {
    if (phonePairing?.childId === child.id && phonePairing.foundMachine && step === "scan") {
      setCode(phonePairing.foundMachine);
      setCodeError(false);
      setStep("name");
    }
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [phonePairing?.foundMachine]);

  function chooseKind(next: Device["platform"]) {
    setKind(next);
    setManualEntry(false);
    if (next !== "android") setCableMode(false);
    // Only swap the default label if the parent hasn't customised it.
    const defaults = [`${child.name}'s laptop`, `${child.name}'s phone`];
    if (defaults.includes(label)) {
      setLabel(next === "android" ? `${child.name}'s phone` : `${child.name}'s laptop`);
    }
  }

  function next() {
    if (!codeParses) {
      setCodeError(true);
      return;
    }
    setCodeError(false);
    setStep("name");
  }

  async function govern() {
    const name = label.trim() || `${child.name}'s ${kindNoun}`;
    const device = addDevice(child.id, name, kind);
    const paired = confirmPairing(child.id, device, code.trim());
    if (!paired) {
      // Shouldn't happen (the code was pre-validated), but never pretend:
      // fall back to the scan step with the error showing.
      setCodeError(true);
      setStep("scan");
      return;
    }

    // Scan-to-pair: tell the computer who we are. This MUST happen here, on a
    // deliberate press, and not back in the scan callback — sending it there
    // raced the naming step, whose autoFocus raises the keyboard and buries
    // the approval sheet underneath it. A parent cannot approve what they
    // cannot see, and a dismissed approval throws SignerCancelled.
    const invite = parsePairInvite(code.trim());
    if (invite) {
      setPairing(true);
      try {
        await sendPairOffer(invite.machine, invite.token);
      } catch {
        // NEVER swallow this. Silently discarding a cancelled approval is how
        // the first attempt failed: nothing was sent, nothing was said, and
        // both ends sat waiting for each other indefinitely.
        setPairing(false);
        setPairError(true);
        return;
      }
      setPairing(false);
    }
    onClose();
  }

  return (
    <div className="stack">
      {step === "scan" ? (
        <>
          <div className="row-between" style={{ gap: 8 }}>
            <Button
              variant={kind === "linux" ? "primary" : "secondary"}
              block
              disabled={cableBusy}
              onClick={() => chooseKind("linux")}
            >
              💻 Computer
            </Button>
            <Button
              variant={kind === "android" ? "primary" : "secondary"}
              block
              disabled={cableBusy}
              onClick={() => chooseKind("android")}
            >
              📱 Phone
            </Button>
          </div>

          {kind === "android" && cableMode ? (
            !haveLocalSigner ? (
              // Mirrors the QR flow's gate (below): the cable path also
              // needs an active local guardian signer — `guardianPubkeyHex()`
              // must not lazily mint a key outside the backup path, and the
              // store's STATUS polling only runs once `signer.kind ===
              // "local"`. Without this, a provisioned phone would orphan.
              <>
                <Banner tone="info">
                  Pairing a phone needs your guardian key — turn on parent
                  approval first and this becomes a no-scanning cable setup.
                </Banner>
                <Button block onClick={() => enableLocalSigner()}>
                  Turn on parent approval
                </Button>
                <Button variant="ghost" block onClick={() => setCableMode(false)}>
                  Back
                </Button>
              </>
            ) : (
              <SetupPhoneCable
                child={child}
                cableSupported={cableSupported}
                onClose={onClose}
                onBack={() => setCableMode(false)}
                onBusyChange={setCableBusy}
              />
            )
          ) : (
            <>
              {phoneScanMode ? (
                !haveLocalSigner ? (
                  <>
                    <Banner tone="info">
                      Pairing a phone needs your guardian key — turn on parent
                      approval first and this becomes a one-scan setup.
                    </Banner>
                    <Button block onClick={() => enableLocalSigner()}>
                      Turn on parent approval
                    </Button>
                  </>
                ) : (
                  <>
                    <Banner tone="info">
                      <span>
                        Scan this with {child.name}'s <b>phone camera</b>. The
                        Kintrinsic app opens — tap <b>Pair with guardian</b> there,
                        and the phone will appear here by itself.
                      </span>
                    </Banner>

                    {qrSrc ? (
                      <img
                        src={qrSrc}
                        data-pairing-link={qrContent ?? undefined}
                        alt="Pairing code — scan with the child's phone camera"
                        style={{
                          width: "100%",
                          maxWidth: 240,
                          margin: "0 auto",
                          borderRadius: "var(--radius-card)",
                          background: "#fff",
                          padding: 8,
                        }}
                      />
                    ) : (
                      <Viewfinder />
                    )}

                    {/* The other half of the pairing check (S6). The phone's
                        landing page shows this same six digits, derived from
                        this guardian key and this one-time token. A ward sent
                        a link by a stranger sees a code that matches nothing
                        on any parent's screen — which is the whole defence,
                        and only works if the family actually compares them.
                        So the instruction is here, not just over there. */}
                    {pairSas && (
                      <div style={{ textAlign: "center" }}>
                        <div
                          style={{
                            fontSize: 28,
                            fontWeight: 700,
                            letterSpacing: "0.12em",
                            fontFamily: "ui-monospace, SFMono-Regular, Menlo, monospace",
                          }}
                        >
                          {pairSas}
                        </div>
                        <div className="card-sub">
                          {child.name}'s phone will show this code — check it
                          matches before they tap to confirm.
                        </div>
                      </div>
                    )}

                    <div
                      className="card-sub"
                      style={{ textAlign: "center" }}
                      aria-live="polite"
                    >
                      Waiting for the phone to scan…
                    </div>

                    <Button variant="ghost" block onClick={() => setManualEntry(true)}>
                      No camera? Type the code instead
                    </Button>
                    <Button variant="ghost" block onClick={onClose}>
                      Not now
                    </Button>
                  </>
                )
              ) : (
                <>
                  <Banner tone="info">
                    {kind === "android"
                      ? `On ${child.name}'s phone, open the Kintrinsic app — it shows a device code (eight groups of letters and numbers). Type or paste it below.`
                      : `On ${child.name}'s computer, open Kintrinsic — it shows a device code. Point this phone at it, or type the code below.`}
                  </Banner>

                  {kind === "linux" && (
                    <QRScanner
                      active={step === "scan"}
                      onScan={(data) => {
                        // The laptop's QR carries its device code, and — since
                        // scan-to-pair — a one-time token too. A stray QR
                        // (parse fails) returns false so the camera keeps
                        // scanning.
                        if (parseDevicePairingCode(data) === null) return false;
                        setCode(data);
                        setCodeError(false);
                        // With a token we can finish the pairing ourselves:
                        // the laptop cannot learn our key any other way, so
                        // without this the scan only ever taught US about IT.
                        // Fire-and-forget — the laptop confirms by appearing.
                        // The offer is NOT sent here — see govern(). Sending it
                        // from this callback raced the naming step's keyboard
                        // and buried the approval sheet beneath it.
                        setStep("name");
                        return true;
                      }}
                    />
                  )}

                  <label className="field" htmlFor="setup-code" style={{ margin: 0 }}>
                    <span className="field-label">
                      The code shown on the {kindNoun}
                    </span>
                    <input
                      id="setup-code"
                      className="input"
                      autoComplete="off"
                      spellCheck={false}
                      placeholder="e.g. a32c5831 10b72aa4 7fd30e6a …"
                      value={code}
                      onChange={(e) => {
                        setCode(e.target.value);
                        if (codeError) setCodeError(false);
                      }}
                      onKeyDown={(e) => {
                        if (e.key === "Enter" && code.trim()) next();
                      }}
                    />
                  </label>

                  {codeError && (
                    <Banner tone="warn">
                      That doesn't look like a device code — it's the eight groups of
                      letters and numbers shown on the {kindNoun}'s Kintrinsic screen.
                      Check for typos and try again.
                    </Banner>
                  )}

                  <Button block disabled={!code.trim()} onClick={next}>
                    I've entered it — next
                  </Button>
                  {kind === "android" && (
                    <Button variant="ghost" block onClick={() => setManualEntry(false)}>
                      Back to the QR code
                    </Button>
                  )}
                  <Button variant="ghost" block onClick={onClose}>
                    Not now
                  </Button>
                </>
              )}

              {kind === "android" && cableSupported && haveLocalSigner && (
                <Button variant="ghost" block onClick={() => setCableMode(true)}>
                  🔌 Set up with a cable (no scanning)
                </Button>
              )}
            </>
          )}
        </>
      ) : (
        <>
          <Banner tone="info">
            Last step — give this {kindNoun} a name you'll recognise, then hand
            it to {child.name} to manage.
          </Banner>

          <label className="field" htmlFor="setup-name" style={{ margin: 0 }}>
            <span className="field-label">Device name</span>
            {/* Deliberately NOT autoFocus: it raised the keyboard the moment
                this step appeared, which covered the approval sheet. The name
                is pre-filled and usually needs no edit at all. */}
            <input
              id="setup-name"
              className="input"
              value={label}
              onChange={(e) => setLabel(e.target.value)}
              onKeyDown={(e) => {
                if (e.key === "Enter" && label.trim()) void govern();
              }}
            />
          </label>

          {pairError && (
            <Banner tone="warn">
              That wasn't approved, so the computer was never told about this
              phone. Tap “Govern this device” again and approve when asked.
            </Banner>
          )}

          <Button
            block
            disabled={!label.trim() || pairing}
            onClick={() => {
              setPairError(false);
              void govern();
            }}
          >
            {pairing ? "Connecting…" : "Govern this device"}
          </Button>
          <Button variant="ghost" block disabled={pairing} onClick={() => setStep("scan")}>
            Back
          </Button>
        </>
      )}
    </div>
  );
}

// ---------------------------------------------------------------------------
// Set up a phone with a cable (no scanning) — WebUSB provisioning
// ---------------------------------------------------------------------------

type CablePhase = "intro" | "connecting" | "waiting" | "error";

/**
 * The no-camera alternative to the QR scan: drive the phone over a USB cable
 * end to end — connect via WebUSB, install Kintrinsic, make it device owner,
 * then fire the SAME `bunker://` pairing intent the QR carries (its own
 * one-time token). It reuses `beginPhonePairing` / `phonePairing` /
 * `confirmPairing` — the exact STATUS-echo match the QR flow uses — so the
 * phone appearing here works identically either way.
 */
function SetupPhoneCable({
  child,
  cableSupported,
  onClose,
  onBack,
  onBusyChange,
}: {
  child: Child;
  cableSupported: boolean;
  onClose: () => void;
  onBack: () => void;
  onBusyChange?: (busy: boolean) => void;
}) {
  const { phonePairing, beginPhonePairing, cancelPhonePairing, addDevice, confirmPairing } =
    useCharter();
  const [phase, setPhase] = useState<CablePhase>("intro");
  const [detail, setDetail] = useState("");
  const [error, setError] = useState<string | null>(null);
  const [showManual, setShowManual] = useState(false);

  // Cancel the outstanding pairing token on ANY unmount, regardless of phase
  // (Fix 2). This is the safety net: a cancelled token means a late connect()
  // success can't echo-match and bind the phone under the wrong flow. It's
  // idempotent, and on the success path the device is already bound before
  // unmount, so cancelling here is safe. Empty deps → runs once, on unmount.
  // eslint-disable-next-line react-hooks/exhaustive-deps
  useEffect(() => () => cancelPhonePairing(), []);

  // Lift the busy flag so the parent can lock the kind toggle while a provision
  // is mid-flight (Fix 3) — a stray toggle click must not unmount us mid-run.
  // The cleanup clears it on unmount (e.g. a deliberate Cancel during "waiting")
  // so the toggle re-enables once we leave; the batched false→true on a phase
  // change is harmless.
  useEffect(() => {
    onBusyChange?.(phase !== "intro" && phase !== "error");
    return () => onBusyChange?.(false);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [phase]);

  // The phone finished provisioning and echoed our one-time token on its
  // STATUS heartbeat: bind its real pubkey and finish, exactly like the QR
  // flow's match.
  useEffect(() => {
    if (phase !== "waiting") return;
    if (phonePairing?.childId !== child.id || !phonePairing.foundMachine) return;
    const device = addDevice(child.id, `${child.name}'s phone`, "android");
    const paired = confirmPairing(child.id, device, phonePairing.foundMachine);
    if (!paired) {
      setError("The phone paired, but its key couldn't be bound. Try again.");
      setPhase("error");
      return;
    }
    onClose();
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [phase, phonePairing?.foundMachine]);

  async function connect() {
    setError(null);
    setPhase("connecting");
    setDetail("Connecting to the phone…");
    let session: import("../provision/webusb").AdbSession | undefined;
    try {
      const { connectPhone } = await import("../provision/webusb");
      const { runProvision } = await import("../provision/provision");
      const { fetchReleaseApk } = await import("../provision/apk");

      session = await connectPhone();

      setDetail("Downloading the Kintrinsic app…");
      const { bytes } = await fetchReleaseApk();

      const token = beginPhonePairing(child.id);
      const bunkerUri = guardianPairingBunkerUri(guardianPubkeyHex(), DEFAULT_RELAYS, token);

      await runProvision(session, bytes, bunkerUri, (p) => setDetail(p.detail));

      await session.close();
      session = undefined;
      setPhase("waiting");
    } catch (e) {
      cancelPhonePairing();
      if (session) {
        try {
          await session.close();
        } catch {
          // best-effort — nothing more to do if the session is already gone
        }
      }
      setError(
        e instanceof Error ? e.message : "Something went wrong connecting to the phone.",
      );
      setPhase("error");
    }
  }

  if (phase === "waiting") {
    return (
      <div className="stack">
        <Banner tone="info">
          <span>
            Almost there — on the phone, tap <b>Pair with guardian</b> to
            finish. No scanning, no terminal — just that one tap.
          </span>
        </Banner>
        <div className="card-sub" style={{ textAlign: "center" }} aria-live="polite">
          Waiting for the phone to appear…
        </div>
        <Button variant="ghost" block onClick={onBack}>
          Cancel
        </Button>
      </div>
    );
  }

  if (phase === "error") {
    return (
      <div className="stack">
        <Banner tone="warn">{error}</Banner>
        <Button block onClick={connect}>
          Try again
        </Button>
        <Button variant="ghost" block onClick={() => setShowManual((v) => !v)}>
          {showManual ? "Hide manual instructions" : "Do it manually instead"}
        </Button>
        {showManual && (
          <p className="card-sub" style={{ margin: 0 }}>
            From a terminal with the phone connected over USB debugging, run{" "}
            <code style={{ fontSize: 12 }}>android/scripts/charter-provision.sh</code> in the
            Kintrinsic repo. It walks through the same steps as this screen.
          </p>
        )}
        <Button variant="ghost" block onClick={onBack}>
          Back
        </Button>
      </div>
    );
  }

  if (phase === "connecting") {
    return (
      <div className="stack">
        <Banner tone="info">Keep the phone plugged in and this tab open.</Banner>
        <div className="card-sub" style={{ textAlign: "center" }} aria-live="polite">
          {detail}
        </div>
      </div>
    );
  }

  // intro
  return (
    <div className="stack">
      <Banner tone="info">
        <span>
          Finish {child.name}'s phone through the first-run screens{" "}
          <b>without signing into any account</b>, then turn on{" "}
          <b>Developer options</b> and <b>USB debugging</b> (Settings → About
          phone → tap the build number 7 times, then Settings → System →
          Developer options). Plug it into this computer with a USB cable.
        </span>
      </Banner>

      {cableSupported ? (
        <Button block onClick={connect}>
          Connect the phone
        </Button>
      ) : (
        <p className="card-sub" style={{ textAlign: "center", opacity: 0.7 }}>
          Cable setup needs Chrome, Edge, or Brave on a computer.
        </p>
      )}

      <Button variant="ghost" block onClick={onBack}>
        Back to scanning
      </Button>
      <Button variant="ghost" block onClick={onClose}>
        Not now
      </Button>
    </div>
  );
}

/** A calm camera-viewfinder stand-in for the scan step (decorative only). */
function Viewfinder() {
  return (
    <div
      role="img"
      aria-label="Camera viewfinder — point at the code on the computer"
      style={{
        position: "relative",
        width: "100%",
        aspectRatio: "1 / 1",
        maxWidth: 220,
        margin: "0 auto",
        borderRadius: "var(--radius-card)",
        background:
          "repeating-linear-gradient(135deg, var(--surface-2) 0 12px, var(--surface) 12px 24px)",
        border: "1px solid var(--line)",
        boxShadow: "var(--shadow-1)",
        display: "grid",
        placeItems: "center",
      }}
    >
      {/* Corner brackets */}
      {(["tl", "tr", "bl", "br"] as const).map((corner) => (
        <span key={corner} aria-hidden="true" style={cornerStyle(corner)} />
      ))}
      <span aria-hidden="true" style={{ fontSize: 40, opacity: 0.5 }}>
        ⛶
      </span>
    </div>
  );
}

function cornerStyle(corner: "tl" | "tr" | "bl" | "br"): CSSProperties {
  const size = 26;
  const inset = 14;
  const thick = 3;
  const brand = "var(--brand)";
  const base: CSSProperties = { position: "absolute", width: size, height: size };
  const top = corner[0] === "t";
  const left = corner[1] === "l";
  return {
    ...base,
    [top ? "top" : "bottom"]: inset,
    [left ? "left" : "right"]: inset,
    [top ? "borderTop" : "borderBottom"]: `${thick}px solid ${brand}`,
    [left ? "borderLeft" : "borderRight"]: `${thick}px solid ${brand}`,
    borderTopLeftRadius: corner === "tl" ? 8 : 0,
    borderTopRightRadius: corner === "tr" ? 8 : 0,
    borderBottomLeftRadius: corner === "bl" ? 8 : 0,
    borderBottomRightRadius: corner === "br" ? 8 : 0,
  };
}

// ---------------------------------------------------------------------------
// Stage 2 teaser — game logins (coming later)
// ---------------------------------------------------------------------------

function GameLoginsTeaser() {
  return (
    <div
      style={{
        marginTop: 12,
        display: "flex",
        alignItems: "center",
        gap: 12,
        padding: "12px 14px",
        borderRadius: "var(--radius-control)",
        border: "1px dashed var(--line)",
        background: "var(--surface-2)",
        opacity: 0.8,
      }}
    >
      <span aria-hidden="true" style={{ fontSize: 22, lineHeight: 1 }}>
        🎮
      </span>
      <div style={{ flex: "1 1 180px", minWidth: 0 }}>
        <div style={{ fontWeight: 600 }}>Add game logins</div>
        <p className="card-sub" style={{ marginTop: 2 }}>
          Approve sign-ins to games and apps. Coming in a later update.
        </p>
      </div>
      <Pill tone="neutral">Soon</Pill>
    </div>
  );
}

// ---------------------------------------------------------------------------
// Add child
// ---------------------------------------------------------------------------

function AddChild({ startOpen }: { startOpen: boolean }) {
  const { addChild } = useCharter();
  const [open, setOpen] = useState(startOpen);
  const [name, setName] = useState("");
  const [color, setColor] = useState(CHILD_COLORS[0]);

  function submit(e: React.FormEvent) {
    e.preventDefault();
    if (!name.trim()) return;
    addChild(name.trim(), color);
    setName("");
    setColor(CHILD_COLORS[0]);
    setOpen(false);
  }

  if (!open) {
    return (
      <Button variant="secondary" block onClick={() => setOpen(true)}>
        Add a child
      </Button>
    );
  }

  return (
    <Card>
      <form onSubmit={submit}>
        <label className="field" htmlFor="new-child-name">
          <span className="field-label">Child's name</span>
          <input
            id="new-child-name"
            className="input"
            autoFocus
            placeholder="e.g. Sam"
            value={name}
            onChange={(e) => setName(e.target.value)}
          />
        </label>

        <ColorChoice value={color} onChange={setColor} groupName="new-child-color" />

        <div className="stack" style={{ marginTop: 4 }}>
          <Button type="submit" block disabled={!name.trim()}>
            Add child
          </Button>
          {!startOpen && (
            <Button type="button" variant="ghost" block onClick={() => setOpen(false)}>
              Cancel
            </Button>
          )}
        </div>
      </form>
    </Card>
  );
}

/** The ward's colour spot. Shared by "add a child" and "edit a child" so the
 *  two can't drift into offering different palettes for the same field.
 *  `groupName` keeps each instance's radios their own group — several cards
 *  can be on screen at once, and a shared name would make picking a colour for
 *  one ward silently deselect another's. */
function ColorChoice({
  value,
  onChange,
  groupName,
}: {
  value: string;
  onChange: (c: string) => void;
  groupName: string;
}) {
  return (
    <fieldset style={{ border: 0, margin: 0, padding: 0 }} className="field">
      <legend className="field-label" style={{ padding: 0 }}>
        Colour
      </legend>
      <div style={{ display: "flex", gap: 12, flexWrap: "wrap" }}>
        {CHILD_COLORS.map((c) => {
          const selected = c === value;
          return (
            <label
              key={c}
              className="color-option"
              aria-label={`Use this colour${selected ? " (selected)" : ""}`}
            >
              <input
                type="radio"
                name={groupName}
                className="color-input"
                value={c}
                checked={selected}
                onChange={() => onChange(c)}
                style={srOnlyStyle}
              />
              <span
                className="color-swatch"
                aria-hidden="true"
                style={{
                  display: "inline-block",
                  width: 32,
                  height: 32,
                  borderRadius: "50%",
                  background: c,
                  boxShadow: selected
                    ? "0 0 0 3px var(--surface), 0 0 0 5px var(--text)"
                    : "var(--shadow-1)",
                }}
              />
            </label>
          );
        })}
      </div>
    </fieldset>
  );
}

const srOnlyStyle: CSSProperties = {
  position: "absolute",
  width: 1,
  height: 1,
  padding: 0,
  margin: -1,
  overflow: "hidden",
  clip: "rect(0 0 0 0)",
  whiteSpace: "nowrap",
  border: 0,
};

// ---------------------------------------------------------------------------
// Signer
// ---------------------------------------------------------------------------

function SignerCard() {
  const { state, connectSigner, enableLocalSigner, pairSignet, disconnectSigner, setAutoSign } = useCharter();
  const signer = state.signer;
  const [busy, setBusy] = useState(false);
  const [pairing, setPairing] = useState(false);
  const [uri, setUri] = useState("");
  const [pairError, setPairError] = useState<string | null>(null);

  async function run(fn: () => Promise<void>) {
    setBusy(true);
    try {
      await fn();
    } finally {
      setBusy(false);
    }
  }

  async function copyText(text: string) {
    try {
      await navigator.clipboard.writeText(text);
    } catch {
      const ta = document.createElement("textarea");
      ta.value = text;
      ta.style.position = "fixed";
      ta.style.opacity = "0";
      document.body.appendChild(ta);
      ta.select();
      try {
        document.execCommand("copy");
      } catch {
        // clipboard unavailable — nothing more we can do
      }
      document.body.removeChild(ta);
    }
  }

  async function doPair() {
    setPairError(null);
    try {
      await pairSignet(uri.trim());
      setPairing(false);
      setUri("");
    } catch (e) {
      setPairError(e instanceof Error ? e.message : "Couldn't connect to Signet.");
    }
  }

  if (!signer.connected) {
    return (
      <Card>
        <h2 className="card-title">Turn on parent approval</h2>
        <p className="card-sub" style={{ marginBottom: 14 }}>
          Add a quick confirm step so only you can change limits or approve
          requests — a second lock on a parent's decisions. Choose the app
          you'll tap to confirm it's really you.
        </p>
        {pairing ? (
          <div className="stack">
            <p className="card-sub" style={{ margin: 0 }}>
              In Signet, copy your connection link and paste it here.
            </p>
            <input
              className="input"
              placeholder="bunker://…"
              value={uri}
              disabled={busy}
              onChange={(e) => setUri(e.target.value)}
              autoCapitalize="none"
              autoCorrect="off"
              spellCheck={false}
            />
            {pairError && (
              <p className="card-sub" style={{ margin: 0, color: "var(--danger, #B23B30)" }}>
                {pairError}
              </p>
            )}
            <Button block disabled={busy || !uri.trim().startsWith("bunker://")} onClick={() => run(doPair)}>
              Connect
            </Button>
            <Button
              variant="ghost"
              block
              disabled={busy}
              onClick={() => {
                setPairing(false);
                setPairError(null);
              }}
            >
              Cancel
            </Button>
          </div>
        ) : (
          <div className="stack">
            <Button block disabled={busy} onClick={() => run(enableLocalSigner)}>
              Set up on this phone
            </Button>
            <Button block disabled={busy} onClick={() => setPairing(true)}>
              Set up with Signet
            </Button>
            <Button
              variant="secondary"
              block
              disabled={busy}
              onClick={() => run(() => connectSigner("heartwood" as SignerKind))}
            >
              Set up with Heartwood
            </Button>
          </div>
        )}
        <p className="card-sub" style={{ marginTop: 12 }}>
          Simplest: “Set up on this phone” keeps the guardian key right here — no
          extra app. You can also set limits first; they’ll save now and apply
          once approval is on.
        </p>
      </Card>
    );
  }

  return (
    <Card>
      <div className="row-between">
        <h2 className="card-title" style={{ margin: 0 }}>
          Parent approval
        </h2>
        <Pill tone="ok">On</Pill>
      </div>
      {signer.kind === "local" && (() => {
        const pk = guardianPubkeyHex();
        const bunker = guardianBunkerUri(pk, DEFAULT_RELAYS);
        return (
          <div className="stack" style={{ marginTop: 12 }}>
            {/* This used to end "it links that laptop to this phone", which a
                second parent reasonably read as the way to add THEMSELVES —
                when the laptop being paired is a ward's device. Kintrinsic has one
                pinned guardian key by contract, so naming whose computer this
                is matters more than the mechanics. */}
            <p className="card-sub" style={{ margin: 0 }}>
              Pair a computer your child uses: on that computer open “Kintrinsic
              Setup”, choose “Pair a guardian”, and paste this. It puts that
              computer under the rules you set here.
            </p>
            <p className="card-sub" style={{ margin: 0 }}>
              This isn’t how another parent joins — Kintrinsic has one guardian key,
              held on this phone. Sharing the household today means sharing that
              key, through Back up key below.
            </p>
            <code style={{ wordBreak: "break-all", fontSize: 12, opacity: 0.85 }}>{bunker}</code>
            <Button block disabled={busy} onClick={() => run(() => copyText(bunker))}>
              Copy pairing link
            </Button>
            <p className="card-sub" style={{ margin: 0, opacity: 0.7 }}>
              Your guardian ID: {guardianNpub(pk).slice(0, 20)}…
            </p>
          </div>
        );
      })()}
      {signer.kind === "local" && <KeyBackupSection busy={busy} />}

      <p className="card-sub" style={{ marginTop: 4 }}>
        Limit changes and approvals are confirmed by you
        {signer.label ? ` with ${signer.label}` : ""}.
      </p>

      <div
        className="row-between"
        style={{ marginTop: 16, gap: 12, alignItems: "flex-start" }}
      >
        <div style={{ minWidth: 0 }}>
          <div style={{ fontWeight: 600 }}>Approve without the extra tap</div>
          <p className="card-sub" style={{ marginTop: 2 }}>
            When on, your decisions go through right away. This only applies to
            choices you make here — it never approves your child's requests for
            you.
          </p>
        </div>
        <Switch
          checked={signer.autoSign}
          disabled={busy}
          label="Approve without the extra tap"
          onChange={(on) => run(() => setAutoSign(on))}
        />
      </div>

      <p className="card-sub" style={{ marginTop: 4 }}>
        You can turn this off any time.
      </p>

      <div style={{ marginTop: 16 }}>
        <Button
          variant="ghost"
          disabled={busy}
          onClick={() => run(() => disconnectSigner())}
        >
          Turn off parent approval
        </Button>
      </div>
    </Card>
  );
}

// ---------------------------------------------------------------------------
// Guardian key backup / restore — the recovery story
// ---------------------------------------------------------------------------

/**
 * Back up (and restore) the guardian key. This is the ONE piece of recovery a
 * family can't do without: the guardian secret lives only in this browser, so
 * a lost phone would orphan every paired device. The backup is passphrase-
 * encrypted; restoring the same key on a new device revives every pairing with
 * no re-pairing (the devices pinned the guardian PUBKEY, which is unchanged).
 */
function KeyBackupSection({ busy }: { busy: boolean }) {
  const { restoreGuardianKey } = useCharter();
  const [mode, setMode] = useState<null | "backup" | "restore">(null);
  const [pass, setPass] = useState("");
  const [pass2, setPass2] = useState("");
  const [blob, setBlob] = useState("");
  const [restoreBlob, setRestoreBlob] = useState("");
  const [error, setError] = useState<string | null>(null);
  const [ok, setOk] = useState<string | null>(null);
  const [working, setWorking] = useState(false);

  function reset() {
    setMode(null);
    setPass("");
    setPass2("");
    setBlob("");
    setRestoreBlob("");
    setError(null);
    setOk(null);
  }

  async function makeBackup() {
    setError(null);
    if (pass.length < 8) {
      setError("Use a passphrase of at least 8 characters.");
      return;
    }
    if (pass !== pass2) {
      setError("The two passphrases don't match.");
      return;
    }
    setWorking(true);
    try {
      const { encryptGuardianBackup } = await import("../signer/keyBackup");
      const { loadOrCreateGuardianKey } = await import("../signer/guardianKey");
      const { exportPersistedState } = await import("../store/store");
      setBlob(await encryptGuardianBackup(loadOrCreateGuardianKey(), exportPersistedState(), pass));
    } catch (e) {
      setError(e instanceof Error ? e.message : "Couldn't make a backup.");
    } finally {
      setWorking(false);
    }
  }

  function downloadBackup() {
    const file = new Blob([blob], { type: "text/plain" });
    const url = URL.createObjectURL(file);
    const a = document.createElement("a");
    a.href = url;
    a.download = "charter-guardian-backup.txt";
    a.click();
    URL.revokeObjectURL(url);
  }

  async function doRestore() {
    setError(null);
    setWorking(true);
    try {
      const { decryptGuardianBackup } = await import("../signer/keyBackup");
      const backup = await decryptGuardianBackup(restoreBlob, pass);
      const pk = await restoreGuardianKey(backup.secret);
      if (backup.stateJson) {
        // The v2 blob carries the household — install it and boot from it.
        const { importPersistedState } = await import("../store/store");
        importPersistedState(backup.stateJson);
        setOk(`Restored guardian ${pk.slice(0, 8)}… and your family — reloading.`);
        setTimeout(() => window.location.reload(), 1500);
      } else {
        setOk(`Restored guardian ${pk.slice(0, 8)}…. Your paired devices are back.`);
      }
      setRestoreBlob("");
      setPass("");
    } catch (e) {
      setError(e instanceof Error ? e.message : "Couldn't restore that backup.");
    } finally {
      setWorking(false);
    }
  }

  if (mode === null) {
    return (
      <div className="stack" style={{ marginTop: 16 }}>
        <div style={{ fontWeight: 600 }}>Back up your guardian key</div>
        <p className="card-sub" style={{ marginTop: 2 }}>
          Your key and family setup live only on this device. One backup
          carries both — restore it on a new phone or in the Kintrinsic app and
          everything comes back: children, devices, rules, no re-pairing.
        </p>
        <div className="row-between" style={{ gap: 8 }}>
          <Button variant="secondary" block disabled={busy} onClick={() => setMode("backup")}>
            Back up key
          </Button>
          <Button variant="ghost" block disabled={busy} onClick={() => setMode("restore")}>
            Restore from backup
          </Button>
        </div>
      </div>
    );
  }

  if (mode === "backup") {
    return (
      <div className="stack" style={{ marginTop: 16 }}>
        <div style={{ fontWeight: 600 }}>Back up your guardian key</div>
        {!blob ? (
          <>
            <p className="card-sub" style={{ marginTop: 2 }}>
              Choose a passphrase. You'll need it to restore — we can't recover
              it for you, so keep it somewhere safe.
            </p>
            <input
              className="input"
              type="password"
              placeholder="Passphrase (8+ characters)"
              aria-label="Backup passphrase"
              value={pass}
              onChange={(e) => setPass(e.target.value)}
            />
            <input
              className="input"
              type="password"
              placeholder="Repeat passphrase"
              aria-label="Repeat backup passphrase"
              value={pass2}
              onChange={(e) => setPass2(e.target.value)}
            />
            {error && <Banner tone="warn">{error}</Banner>}
            <Button block disabled={working} onClick={makeBackup}>
              {working ? "Encrypting…" : "Create backup"}
            </Button>
            <Button variant="ghost" block onClick={reset}>
              Cancel
            </Button>
          </>
        ) : (
          <>
            <Banner tone="ok">
              Encrypted. Save this somewhere safe — a password manager, or the
              downloaded file. Anyone with it AND your passphrase can act as the
              guardian.
            </Banner>
            <code
              data-backup-blob
              style={{ wordBreak: "break-all", fontSize: 11, opacity: 0.85 }}
            >
              {blob}
            </code>
            <div className="row-between" style={{ gap: 8 }}>
              <Button
                variant="secondary"
                block
                onClick={() => navigator.clipboard?.writeText(blob)}
              >
                Copy
              </Button>
              <Button variant="secondary" block onClick={downloadBackup}>
                Download .txt
              </Button>
            </div>
            <Button variant="ghost" block onClick={reset}>
              Done
            </Button>
          </>
        )}
      </div>
    );
  }

  // restore
  return (
    <div className="stack" style={{ marginTop: 16 }}>
      <div style={{ fontWeight: 600 }}>Restore your guardian key</div>
      {ok ? (
        <>
          <Banner tone="ok">{ok}</Banner>
          <Button variant="ghost" block onClick={reset}>
            Done
          </Button>
        </>
      ) : (
        <>
          <p className="card-sub" style={{ marginTop: 2 }}>
            Paste your backup and its passphrase. This replaces the key on this
            phone with the backed-up one.
          </p>
          <textarea
            className="input"
            rows={3}
            placeholder="CHARTER-KEYBAK.…"
            aria-label="Backup blob"
            style={{ fontFamily: "monospace", fontSize: 11 }}
            value={restoreBlob}
            onChange={(e) => setRestoreBlob(e.target.value)}
          />
          <input
            className="input"
            type="password"
            placeholder="Passphrase"
            aria-label="Restore passphrase"
            value={pass}
            onChange={(e) => setPass(e.target.value)}
          />
          {error && <Banner tone="warn">{error}</Banner>}
          <Button
            block
            disabled={working || !restoreBlob.trim() || !pass}
            onClick={doRestore}
          >
            {working ? "Restoring…" : "Restore key"}
          </Button>
          <Button variant="ghost" block onClick={reset}>
            Cancel
          </Button>
        </>
      )}
    </div>
  );
}

// ---------------------------------------------------------------------------
// Switch (accessible toggle)
// ---------------------------------------------------------------------------

function Switch({
  checked,
  onChange,
  disabled,
  label,
}: {
  checked: boolean;
  onChange: (on: boolean) => void;
  disabled?: boolean;
  label: string;
}) {
  return (
    <button
      type="button"
      role="switch"
      aria-checked={checked}
      aria-label={label}
      disabled={disabled}
      onClick={() => onChange(!checked)}
      style={{
        flex: "0 0 auto",
        appearance: "none",
        border: 0,
        cursor: disabled ? "default" : "pointer",
        width: 52,
        height: 32,
        borderRadius: 999,
        padding: 3,
        background: checked ? "var(--ok)" : "var(--line)",
        transition: "background-color 0.15s ease",
        opacity: disabled ? 0.6 : 1,
        display: "inline-flex",
        alignItems: "center",
      }}
    >
      <span
        aria-hidden="true"
        style={{
          display: "block",
          width: 26,
          height: 26,
          borderRadius: "50%",
          background: "#fff",
          boxShadow: "var(--shadow-1)",
          transform: checked ? "translateX(20px)" : "translateX(0)",
          transition: "transform 0.15s ease",
        }}
      />
    </button>
  );
}
