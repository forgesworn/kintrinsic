import { useEffect, useState } from "react";
import { Avatar, Banner, Button, Card, Pill, SectionLabel } from "../components/ui";
import { liveness, livenessChip } from "../domain/liveness";
import { useLocalRoute } from "../carrier/useLocalRoute";
import { appLockNote, deviceHealthNotes } from "../domain/deviceHealth";
import { deviceProblemLine } from "./deviceProblem";
import type { Child, Device } from "../domain/types";
import { wardPath, wardSettingsPath } from "../domain/wardRoute";
import { useCharter } from "../store/store";
import type { SendResult } from "../store/clauseDelivery";
import { linuxUpdateOffer, updateAvailable } from "../store/updateCheck";
import {
  accountSummary,
  canOpenInstallWindow,
  installWindow,
  installWindowLabel,
  INSTALL_WINDOW_MINUTES,
  readWindows,
  writeWindow,
} from "./installWindow";
import { isValidDevicePubkey } from "./offlineUnlock";
import Home from "./Home";
import UnlockDevice from "./UnlockDevice";
import DeviceRelays from "./DeviceRelays";
import DeviceDebugging from "./DeviceDebugging";
import SetupComputer from "./SetupComputer";
import { ColorChoice } from "./ColorChoice";
import "./child-admin.css";

// The child page's administration parts: the devices list, the device page,
// the set-up wizard as a page, the controls that cover every one of the
// child's devices, and managing the child (see docs/guardian-ux-design.md,
// "Child page"). The household parts (parent approval, guardians, adding a
// child) are on Settings.

const WARN = { color: "var(--warn, #8a5a00)" } as const;

function go(path: string): void {
  window.location.hash = `/${path}`;
}

/** Go to a page in place of the current one, so Back doesn't return to a page
 *  that no longer makes sense (a device just disconnected or removed, a
 *  finished set-up). Only the fallback: the app shell passes `onDone`, which
 *  steps back instead, so the child's page isn't left in history twice. */
function goReplacing(path: string): void {
  window.location.replace(`#/${path}`);
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
// Devices: the list on the child page
// ---------------------------------------------------------------------------

/** The devices section: one row per device, then "Set up another device". */
export function ChildDevices({ child }: { child: Child }) {
  const { sessions } = useLocalRoute();
  const { deviceStatus } = useCharter();
  const now = Date.now();
  const hasDevices = child.devices.length > 0;

  return (
    <div>
      {hasDevices && (
        <div className="list admin-list" style={{ marginBottom: 12 }}>
          {child.devices.map((d) => {
            const status = d.devicePubkey ? deviceStatus[d.devicePubkey] : undefined;
            const chip = livenessChip(
              liveness({ paired: d.pairing === "paired", lastSeenAt: d.lastSeenAt }, now),
              now,
              d.pairing === "paired" && !!d.devicePubkey && sessions.has(d.devicePubkey),
            );
            const problem = deviceProblemLine(d, status, now);
            return (
              <button
                key={d.id}
                type="button"
                className="row"
                aria-label={`${d.label}, ${chip.text}${problem ? `. ${problem}` : ""}`}
                onClick={() => go(`${wardPath(child.id)}/device/${encodeURIComponent(d.id)}`)}
              >
                <span aria-hidden="true" style={{ fontSize: 20 }}>
                  {platformEmoji(d.platform)}
                </span>
                <span className="row-main">
                  <span className="row-title">{d.label}</span>
                  {problem && (
                    <span className="row-sub admin-warn" style={{ display: "block" }}>
                      {problem}
                    </span>
                  )}
                </span>
                <Pill tone={chip.tone}>{chip.text}</Pill>
                <span className="row-chevron" aria-hidden="true">
                  ›
                </span>
              </button>
            );
          })}
        </div>
      )}
      <Button
        variant={hasDevices ? "secondary" : "primary"}
        block
        className="lever-btn"
        onClick={() => go(`${wardPath(child.id)}/add-device`)}
      >
        {hasDevices ? "Set up another device" : "Set up a device"}
      </Button>
    </div>
  );
}

// ---------------------------------------------------------------------------
// Child-wide device settings
// ---------------------------------------------------------------------------

/**
 * Allow installs and Update Kintrinsic. They sit among the devices, but each
 * clause goes to every one of the child's phones, so they are labelled as
 * covering all of them and kept off any one device's page.
 */
export function ChildDeviceSettings({ child }: { child: Child }) {
  const { deviceStatus, updateManifest, sendCharterUpdate, openMaintenanceWindow } = useCharter();
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
  // `true` once the update reached the phone's way; a "queued" object when it
  // is only held for the home network — which must not read as sent.
  const [updateSent, setUpdateSent] = useState<SendResult>(false);
  // The install window the carrier is holding for the home network: signed,
  // but the phone has not got it, so no "Installs are open" countdown.
  const [windowHeld, setWindowHeld] = useState<string | null>(null);

  const openWindow = installWindow(installOpenUntil, nowUnix);
  useEffect(() => {
    if (!installOpenUntil) return;
    // Re-read the CLOCK each tick rather than counting ticks: the OS freezes a
    // backgrounded PWA's timers, and a counter would come back minutes wrong
    // (the staleness trap that made rules look like they hadn't landed).
    const id = setInterval(() => setNowUnix(Math.floor(Date.now() / 1000)), 1000);
    return () => clearInterval(id);
  }, [installOpenUntil]);

  // Kintrinsic's install lock is closed on every chartered phone, which is why a
  // ward could not update an app from their store at all. A guardian-opened,
  // signed, self-closing window is the stand-down — the same clause a stuck
  // phone is rescued with, offered plainly instead of only when something broke.
  const phones = child.devices.filter(canOpenInstallWindow);
  // Self-update (#44): offer only when a phone REPORTS a version that is
  // strictly behind the site's published artifact. Phones only — the Linux
  // warden ships as a .deb, not this APK.
  const behind = phones.filter((d) =>
    updateAvailable(updateManifest, d.devicePubkey ? deviceStatus[d.devicePubkey]?.appVersionCode : undefined),
  );
  const canUpdate = behind.length > 0;
  const updateArrived = updateSent && !canUpdate;
  // What a phone says came through the last window — the guardian's account
  // of their own loosening, not a feed of what the ward installs.
  const accounts = phones.flatMap((d) => {
    const account = d.devicePubkey ? deviceStatus[d.devicePubkey]?.installWindow : undefined;
    return account ? [{ device: d, account }] : [];
  });

  if (phones.length === 0 && !canUpdate) return null;

  return (
    <section id="all-devices" className="scroll-anchor child-device-settings" aria-label={`All of ${child.name}'s devices`}>
      <SectionLabel>All of {child.name}’s devices</SectionLabel>
      <Card>
        <p className="card-sub" style={{ marginTop: 0 }}>
          These apply to every one of {child.name}’s devices, not to one of them.
        </p>

        {phones.length > 0 && (
          <div className="stack" style={{ gap: 8 }}>
            <div style={{ fontWeight: 600 }}>Installs</div>
            {windowHeld && !openWindow.open && (
              <div className="card-sub" style={WARN}>
                {windowHeld}
              </div>
            )}
            {openWindow.open && (
              <div className="card-sub" style={{ color: "var(--ok, #1a6b45)" }}>
                Installs are open — {installWindowLabel(openWindow.secondsLeft)}.{" "}
                {child.name} can install and update apps from the store until then.
                The phone closes it on its own; “Close now” shuts it sooner, as soon
                as the phone next hears from you.
              </div>
            )}
            {!openWindow.open && !windowHeld && (
              <div className="card-sub">
                Opens installs for {INSTALL_WINDOW_MINUTES} minutes, then they close again on their own.
              </div>
            )}
            {accounts.map(({ device, account }) => (
              <div key={device.id} className="card-sub">
                {accounts.length > 1 ? `${device.label}: ` : ""}
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
            ))}
            {openWindow.open ? (
              <Button
                variant="secondary"
                block
                aria-label={`Close the install window on all of ${child.name}'s devices now`}
                onClick={() => {
                  // Zero minutes = an expiry at issue: a newer clause the phone
                  // reads as already shut. Nothing to undo if it fails, because
                  // the window closes on its own regardless.
                  void openMaintenanceWindow(child.id, 0).then((sent) => {
                    if (sent) {
                      writeWindow(child.id, null, Math.floor(Date.now() / 1000));
                      setInstallOpenUntil(undefined);
                      // A close the carrier is holding: the phone stays open
                      // until it gets it, or until the window lapses.
                      setWindowHeld(
                        typeof sent === "object"
                          ? `Not closed on the phone yet. ${sent.message} It closes on its own when the window runs out.`
                          : null,
                      );
                    }
                  });
                }}
              >
                Close now
              </Button>
            ) : (
              <Button
                variant="secondary"
                block
                aria-label={`Allow installs on all of ${child.name}'s devices for ${INSTALL_WINDOW_MINUTES} minutes`}
                onClick={() => {
                  void openMaintenanceWindow(child.id, INSTALL_WINDOW_MINUTES).then((sent) => {
                    // Only start the countdown if the clause was actually signed
                    // and sent — a cancelled signer must not leave this screen
                    // claiming a window the phone never received, and nor must
                    // a window the carrier is only holding for the home network.
                    if (sent === true) {
                      const now = Math.floor(Date.now() / 1000);
                      const until = now + INSTALL_WINDOW_MINUTES * 60;
                      writeWindow(child.id, until, now);
                      setInstallOpenUntil(until);
                      setNowUnix(now);
                      setWindowHeld(null);
                    } else if (sent) {
                      setWindowHeld(
                        `Installs aren’t open yet. ${sent.message} The window still closes ${INSTALL_WINDOW_MINUTES} minutes after you allowed it.`,
                      );
                    }
                  });
                }}
              >
                Allow installs
              </Button>
            )}
          </div>
        )}

        {(canUpdate || updateArrived) && (
          <div className="stack" style={{ gap: 8, marginTop: phones.length > 0 ? 16 : 0 }}>
            <div style={{ fontWeight: 600 }}>Kintrinsic on the phones</div>
            {canUpdate && updateSent === true && (
              <div className="card-sub">
                Update sent. Each phone installs it on its next check-in — the
                version on its device page changes when it lands.
              </div>
            )}
            {canUpdate && typeof updateSent === "object" && (
              <div className="card-sub" style={WARN}>
                Update not sent yet. {updateSent.message}
              </div>
            )}
            {updateArrived && <div className="card-sub">Updated ✓</div>}
            {canUpdate && !updateSent && updateManifest && (
              <>
                <div className="card-sub">
                  {behind.length === 1 ? behind[0].label : `${behind.length} phones`} can update to{" "}
                  {updateManifest.versionName}.
                </div>
                <Button
                  variant="secondary"
                  block
                  aria-label={`Update Kintrinsic on all of ${child.name}'s phones to ${updateManifest.versionName}`}
                  onClick={() => {
                    void sendCharterUpdate(child.id).then((sent) => setUpdateSent(sent));
                  }}
                >
                  Update Kintrinsic
                </Button>
              </>
            )}
          </div>
        )}
      </Card>
    </section>
  );
}

// ---------------------------------------------------------------------------
// Manage the child
// ---------------------------------------------------------------------------

/** Rename / colour and Remove child. */
export function ChildManage({ child }: { child: Child }) {
  const { editChild, removeChild } = useCharter();
  const [editing, setEditing] = useState(false);
  const [draftName, setDraftName] = useState(child.name);
  // The colour spot is how a ward is recognised at a glance on every screen —
  // Home cards, avatars, the settings list — so it belongs wherever the name is
  // editable. Picking it only once, when the child was created, meant living
  // with it forever.
  const [draftColor, setDraftColor] = useState(child.color);
  const [confirmingRemove, setConfirmingRemove] = useState(false);

  function save() {
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
    <div className="child-manage">
      <SectionLabel>Manage</SectionLabel>
      <Card>
        {editing ? (
          <div className="stack" style={{ gap: 12 }}>
            {/* The avatar previews the DRAFT while editing, so the choice is
                visible on the thing it actually changes rather than only after
                saving. */}
            <Avatar name={draftName || child.name} color={draftColor} />
            <label className="field" style={{ margin: 0 }}>
              <span className="field-label">Name</span>
              <input
                className="input"
                autoFocus
                value={draftName}
                onChange={(e) => setDraftName(e.target.value)}
                onKeyDown={(e) => {
                  if (e.key === "Enter") save();
                  if (e.key === "Escape") setEditing(false);
                }}
              />
            </label>
            <ColorChoice value={draftColor} onChange={setDraftColor} groupName={`child-color-${child.id}`} />
            <Button block onClick={save}>
              Save
            </Button>
            <Button variant="secondary" block onClick={() => setEditing(false)}>
              Cancel
            </Button>
          </div>
        ) : (
          <div className="stack" style={{ gap: 8 }}>
            <Button variant="secondary" block onClick={startEditing}>
              Rename or change colour
            </Button>
            {!confirmingRemove && (
              <Button variant="secondary" block onClick={() => setConfirmingRemove(true)}>
                Remove {child.name}
              </Button>
            )}
          </div>
        )}

        {!editing && confirmingRemove && (
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
              <Button
                variant="danger"
                block
                onClick={() => {
                  removeChild(child.id);
                  go("home");
                }}
              >
                Remove {child.name}
              </Button>
            </div>
          </div>
        )}
      </Card>
    </div>
  );
}

// ---------------------------------------------------------------------------
// The device page
// ---------------------------------------------------------------------------

/**
 * One device: its status and every note about it, then what can be done with
 * it. The Home page of a device, reached from the child page's devices list.
 */
export function DevicePage({
  childId,
  deviceId,
  onDone,
}: {
  childId: string;
  deviceId: string;
  /** Leave the page once the device is disconnected or removed. */
  onDone?: () => void;
}) {
  const { state } = useCharter();
  const child = state.children.find((c) => c.id === childId);
  const device = child?.devices.find((d) => d.id === deviceId);
  // A device that was removed (or a child who was) leaves nothing to show.
  if (!child || !device) return <Home />;
  return <DeviceDetail child={child} device={device} onDone={onDone ?? (() => goReplacing(wardSettingsPath(child.id)))} />;
}

function DeviceDetail({ child, device, onDone }: { child: Child; device: Device; onDone: () => void }) {
  const { unpairDevice, removeDevice, reconnectDevice, deviceStatus, updateManifest, debManifest, sendLinuxUpdate } =
    useCharter();
  const [unlockOpen, setUnlockOpen] = useState(false);
  const [confirmingDisconnect, setConfirmingDisconnect] = useState(false);
  // `true` once the update reached the computer; a "queued" object when it is
  // only held for the home network — which must not read as sent.
  const [updateSent, setUpdateSent] = useState<SendResult>(false);
  const { sessions: homeSessions } = useLocalRoute();

  const paired = device.pairing === "paired";
  const status = device.devicePubkey ? deviceStatus[device.devicePubkey] : undefined;
  const nowMs = Date.now();
  // What the device SAYS it is running, shown verbatim. Without this a
  // guardian had no way to see whether an update moved — the button simply
  // reappeared, which looks identical whether the update failed, is still
  // downloading, or never left.
  const reportedName = status?.appVersionName;
  const reportedVersion = status?.appVersionCode;
  const phoneBehind =
    paired && device.platform === "android" && updateAvailable(updateManifest, reportedVersion);
  // A Kintrinsic update that keeps failing. Previously invisible: the phone
  // retried in silence while the card showed only an Update button that kept
  // reappearing, which looks identical to "downloading" (two days lost,
  // 2026-07-24..26).
  const health = status?.updateHealth;
  // Linux: from charterd 0.7.12 the laptop installs a release the guardian's
  // `update` clause and the release key's announcement both name, so a behind
  // laptop gets "Install update". An older charterd never reads the clause, so
  // it is told, not offered: the download page is the honest affordance there.
  const linuxOffer = linuxUpdateOffer(device, deviceStatus, debManifest);
  // The laptop has caught up with the update that was sent.
  const updateArrived = updateSent === true && !phoneBehind && !linuxOffer;
  // Reconnect only ever reuses a REAL stored key (the old path re-"scanned" a
  // sentinel string and clobbered the key with a mock). Without one, the honest
  // route back is the full set-up flow.
  const canReconnect = !!device.devicePubkey && /^[0-9a-f]{64}$/.test(device.devicePubkey);
  // Offline unlock needs the device's real machine key to derive the code.
  // Available for any paired device that has one — the parent may need it the
  // moment the child is locked out, not only after some "locked" signal.
  const canUnlock = paired && isValidDevicePubkey(device.devicePubkey);
  const seen = lastSeenLabel(device.lastSeenAt, nowMs);
  const onHomeNetwork = paired && !!device.devicePubkey && homeSessions.has(device.devicePubkey);
  // Boots the phone went through with no warden running (S1). Safe mode is the
  // case it exists for: nothing runs, so nothing is enforced and nothing is
  // reported, and the phone comes back looking exactly as it did. The device
  // cannot witness its own absence — it can only count the boots it missed.
  const gap = status?.enforcementGap;
  // Paused-by-admin, an enforcement gap (Linux's seconds-shaped account,
  // distinct from `gap` above which is Android's boot-count one), relay
  // trouble, a broken transport, and a ward clock stepping backwards. Each note
  // carries the age of the reading it's drawn from, and `stale` says when that
  // reading itself is too old to trust as "right now".
  const { notes: healthNotes, stale: healthStale } = deviceHealthNotes(status, nowMs);
  // Whether fapolicyd is enforcing the app-lock rules on this LINUX ward.
  // Absent (Android, or an older Linux ward) shows nothing at all.
  const appLock = appLockNote(status?.appLockArmed);
  const installsOpen = installWindow(
    readWindows(Math.floor(nowMs / 1000))[child.id],
    Math.floor(nowMs / 1000),
  ).open;

  return (
    <div className="stack device-page">
      {/* Status first, then every conditional note; the actions follow. */}
      <Card>
        <div className="row-between" style={{ gap: 12, alignItems: "flex-start" }}>
          <div style={{ display: "flex", gap: 12, minWidth: 0 }}>
            <span aria-hidden="true" style={{ fontSize: 24, lineHeight: 1 }}>
              {platformEmoji(device.platform)}
            </span>
            <div style={{ minWidth: 0 }}>
              <h2 className="card-title" style={{ margin: 0 }}>
                {device.label}
              </h2>
              {/* Two separate facts, not conflated: whether Kintrinsic GOVERNS
                  this device (a setup question) and whether it is reachable
                  right now (an operational one). */}
              <div className="card-sub">
                {paired ? "Managed by Kintrinsic" : "Disconnected"}
                {paired ? ` · ${onHomeNetwork ? "On home network" : (seen ?? "waiting to hear from it")}` : ""}
              </div>
            </div>
          </div>
          <Pill tone={paired ? "ok" : "neutral"}>{paired ? "On" : "Off"}</Pill>
        </div>

        <div className="stack" style={{ gap: 6, marginTop: 10 }}>
          {reportedName && <div className="card-sub">Kintrinsic {reportedName}</div>}
          {appLock && (
            <div className="card-sub" style={appLock.tone === "warn" ? WARN : undefined}>
              {appLock.text}
              {appLock.hint && (
                <>
                  {" — "}
                  {appLock.hint
                    .split(/(`[^`]+`)/)
                    .map((part, i) =>
                      part.startsWith("`") ? <code key={i}>{part.slice(1, -1)}</code> : part,
                    )}
                </>
              )}
            </div>
          )}
          {phoneBehind && updateManifest && (
            <div className="card-sub">
              Update available ({updateManifest.versionName}). Update every phone at once from{" "}
              <a href={`#/${wardSettingsPath(child.id)}`}>{child.name}’s settings</a>, under “All of {child.name}’s devices”.
            </div>
          )}
          {health && health.attempts > 2 && (
            <div className="card-sub" style={WARN}>
              Update isn’t completing — {health.lastError ?? "the install keeps failing"} (
              {health.attempts} tries since {new Date(health.sinceUnix * 1000).toLocaleDateString()}).
              {device.platform === "linux"
                ? " It tries again every six hours; you can also install it by hand from the download page."
                : installsOpen
                  ? " You can repair it by cable while installs are open."
                  : ` Allow installs from ${child.name}’s page to repair it by cable.`}
            </div>
          )}
          {gap && (
            <div className="card-sub" style={WARN}>
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
          {linuxOffer?.kind === "download" && (
            <div className="card-sub">
              Update available ({linuxOffer.deb.versionName}) — install it from{" "}
              <a href="https://kintrinsic.app/download.html">the download page</a>.
            </div>
          )}
          {linuxOffer?.kind === "install" && !updateSent && (
            <div className="card-sub">Update available ({linuxOffer.deb.versionName}).</div>
          )}
          {linuxOffer?.kind === "install" && updateSent === true && (
            <div className="card-sub">
              Update sent. The computer installs it when it next hears from you —
              the version above changes when it lands.
            </div>
          )}
          {linuxOffer?.kind === "install" && typeof updateSent === "object" && (
            <div className="card-sub" style={WARN}>
              Update not sent yet. {updateSent.message}
            </div>
          )}
          {updateArrived && <div className="card-sub">Updated ✓</div>}
          {healthNotes.length > 0 && healthStale && (
            <div className="card-sub" style={WARN}>
              This device hasn't reported in a while — the notes below may be out of date.
            </div>
          )}
          {healthNotes.map((n) => (
            <div key={n.key} className="card-sub" style={WARN}>
              {n.text}
            </div>
          ))}
        </div>
      </Card>

      {/* The actions, in the order a guardian is most likely to need them. */}
      {canUnlock && (
        <Card className="device-section">
          <h3 className="card-title" style={{ margin: 0 }}>
            Unlock code
          </h3>
          <p className="card-sub">
            For when {device.label} is locked and can’t get online. It shows a short code on its
            lock screen; type it here and read the unlock code back to it.
          </p>
          <Button
            block
            aria-label={`Unlock ${device.label} with a code`}
            onClick={() => setUnlockOpen(true)}
          >
            Unlock with a code
          </Button>
        </Card>
      )}

      {paired && linuxOffer?.kind === "install" && !updateSent && (
        <Card className="device-section">
          <h3 className="card-title" style={{ margin: 0 }}>
            Update
          </h3>
          <Button
            variant="secondary"
            block
            aria-label={`Install Kintrinsic ${linuxOffer.deb.versionName} on ${device.label}`}
            onClick={() => {
              void sendLinuxUpdate(child.id, device.id).then((sent) => setUpdateSent(sent));
            }}
          >
            Install update
          </Button>
        </Card>
      )}

      {paired && (
        <div className="card device-section">
          <DeviceDebugging child={child} device={device} />
        </div>
      )}
      {paired && (
        <div className="card device-section">
          <DeviceRelays child={child} device={device} />
        </div>
      )}

      {/* Disconnect publishes a signed RELEASE — the most destructive wire
          action there is — and with "Approve without the extra tap" on, no
          sign sheet stands in front of it. One mis-tap must not un-govern a
          device, so it gets the same two-step confirm as removing a child. */}
      <Card className="device-section">
        {paired ? (
          <>
            {!confirmingDisconnect && (
              <Button
                variant="secondary"
                block
                aria-label={`Disconnect ${device.label}`}
                onClick={() => setConfirmingDisconnect(true)}
              >
                Disconnect
              </Button>
            )}
            {confirmingDisconnect && (
              <>
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
                      onDone();
                    }}
                  >
                    Disconnect {device.label}
                  </Button>
                </div>
              </>
            )}
          </>
        ) : (
          <div className="stack" style={{ gap: 8 }}>
            {canReconnect && (
              <Button
                block
                aria-label={`Reconnect ${device.label}`}
                onClick={() => reconnectDevice(child.id, device.id)}
              >
                Reconnect
              </Button>
            )}
            <Button
              variant="danger"
              block
              aria-label={`Remove ${device.label}`}
              onClick={() => {
                removeDevice(child.id, device.id);
                onDone();
              }}
            >
              Remove
            </Button>
          </div>
        )}
      </Card>

      {unlockOpen && <UnlockDevice child={child} device={device} onClose={() => setUnlockOpen(false)} />}
    </div>
  );
}

// ---------------------------------------------------------------------------
// Set up a device
// ---------------------------------------------------------------------------

/**
 * The set-up wizard as a page for this child: computer code, phone QR, the
 * factory-reset QR, the cable, and the Signet-root variants. Finishing, or
 * "Not now", goes back to the child's devices.
 */
export function AddDevicePage({ childId, onDone }: { childId: string; onDone?: () => void }) {
  const { state } = useCharter();
  const child = state.children.find((c) => c.id === childId);
  if (!child) return <Home />;
  return (
    <SetupComputer
      child={child}
      onClose={() => {
        (onDone ?? (() => goReplacing(wardSettingsPath(child.id))))();
        // The route change scrolls to the top; settle on the devices after it (they
        // are on the child's settings page).
        requestAnimationFrame(() =>
          requestAnimationFrame(() =>
            document.getElementById("devices")?.scrollIntoView?.({ block: "start" }),
          ),
        );
      }}
    />
  );
}
