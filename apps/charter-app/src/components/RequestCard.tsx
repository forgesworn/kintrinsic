import { useState } from "react";
import type { Child, ChildRequest, Device, RequestKind } from "../domain/types";
import { Avatar, Banner, Button, Card, Pill } from "./ui";
import { LeverSheet } from "./LeverSheet";
import { useCharter } from "../store/store";
import { standingFor, standingNote } from "../domain/standing";
import { catalogLookup } from "../data/appCatalog";
import { bucketGroupLabel } from "../domain/askLabels";
import {
  groupExtrasToday,
  groupProgressBuckets,
  groupProgressLine,
  groupProgressRows,
} from "../domain/groupProgress";
import { identityDisplayLabel } from "../domain/launchSignatures";
import { resolveChildTz } from "../domain/childTz";
import {
  appOpenWindowUnix,
  startOfDayUnix,
  startOfWeekUnix,
  type AppOpenWindow,
} from "../wire/grant";
import { isAskExpired } from "../domain/askExpiry";
import { endAskDecision, tryBeginAskDecision, useAskDecisionInFlight } from "./askDecisionLock";
import { canUseTimes } from "../domain/times";

// One ask from a ward's device, as a card.
//
// Everything funnels through ONE shape: the device said no, the child asked,
// and now it's the guardian's call. Requests always come FROM a device (never
// a child account), so the full card names the device out loud.
//
// time.extend  -> "Approve N min" (N = what they asked, or what you chose);
//                 "Change amount" opens the same sheet as Give time.
// install.app  -> per-app, SINGLE-USE. "Approve VLC once", never a standing
// run.program     "can install/run anything". The card says this in plain words.
// app.open     -> one-tap windows: "30 min / 1 hour / Rest of today".
//
// `compact` is the Home / child-page list form: one line saying what is asked,
// and the answers. "Details" expands it to the full card.

/** Small helper: a warm, glanceable "how long ago" label. */
function timeAgo(ts: number, now: number): string {
  const mins = Math.round((now - ts) / 60000);
  if (mins < 1) return "Just now";
  if (mins < 60) return `${mins} min ago`;
  const hrs = Math.round(mins / 60);
  if (hrs < 24) return `${hrs} hr ago`;
  const days = Math.round(hrs / 24);
  return days === 1 ? "Yesterday" : `${days} days ago`;
}

/** A friendly emoji per kind of ask. */
const KIND_EMOJI: Record<RequestKind, string> = {
  "time.extend": "⏳",
  "install.app": "📦",
  "run.program": "🎮",
  "app.open": "🔓",
};

/** The one-tap windows an app.open allow offers, in the order they're shown. */
const APP_OPEN_WINDOWS: { window: AppOpenWindow; label: string }[] = [
  { window: "30m", label: "30 min" },
  { window: "1h", label: "1 hour" },
  { window: "restOfDay", label: "Rest of today" },
];

/** The window the compact card offers as its single answer. */
const COMPACT_WINDOW: AppOpenWindow = "1h";

/**
 * The card's heading. A bucket-hit time.extend or an app.open ask reads
 * better with the SPECIFIC thing named ("More Play time?", "Open Minecraft?")
 * than the generic device-phrased `req.title` `ingestDeviceRequest` stores —
 * the group label is only knowable here, joined against the saved policy.
 */
function displayTitle(
  req: ChildRequest,
  bucketLabel: string | undefined,
  typeLabel?: string,
): string {
  // Belt and braces (F1 review): `identityDisplayLabel` resolves a raw
  // `cmdline:` string (which should never itself be device-reported, but
  // could ride back on an already-saved policy) to its signature's label
  // rather than showing the needle on the card's heading.
  if (req.kind === "app.open") return `Open ${identityDisplayLabel(req.appLabel, req.appId) ?? "this app"}?`;
  if (bucketLabel) return `More ${bucketLabel} time?`;
  // Times: the kind of time asked for ("More Learning time?"), named the way a
  // bucket ask is. Free time is the plain ask and keeps its plain title.
  if (typeLabel) return `More ${typeLabel}?`;
  return req.title;
}

/** The compact card's one line: who asks for what, in plain words. */
function compactTitle(
  req: ChildRequest,
  childName: string,
  bucketLabel: string | undefined,
  typeLabel?: string,
): string {
  const what = identityDisplayLabel(req.appLabel, req.appId);
  switch (req.kind) {
    case "time.extend":
      if (bucketLabel) return `${childName} wants more ${bucketLabel} time`;
      if (typeLabel) {
        return req.minutesRequested
          ? `${childName} wants ${req.minutesRequested} more minutes of ${typeLabel}`
          : `${childName} wants more ${typeLabel}`;
      }
      return req.minutesRequested
        ? `${childName} wants ${req.minutesRequested} more minutes`
        : `${childName} wants more time`;
    case "app.open":
      return `${childName}: open ${what ?? "an app"}?`;
    case "install.app":
      return `${childName} wants to install ${what ?? "an app"}`;
    default:
      return `${childName} wants to run ${what ?? "a program"}`;
  }
}

/** What the limit-hit chip says, in plain words. */
function limitChip(req: ChildRequest): string | null {
  if (req.kind !== "time.extend") return null;
  if (req.limitHit === "schedule") return "Outside their hours";
  if (req.limitHit === "budget") return "Out of time today";
  return "More time";
}

/** The verb a per-artifact request is asking permission for. */
function artifactVerb(kind: RequestKind): "install" | "open" {
  return kind === "install.app" ? "install" : "open";
}

/**
 * The load-bearing Stage 1 promise, in plain words: approving here is for
 * THIS app, THIS once — never a blanket "can install/run anything".
 */
function singleUseLine(req: ChildRequest): string {
  const what = identityDisplayLabel(req.appLabel, req.appId) ?? "this app";
  return req.kind === "install.app"
    ? `You're approving ${what} this once — not letting them install anything else.`
    : `You're allowing ${what} this once — not letting them open anything else.`;
}

interface BusyState {
  action: "approve" | "deny";
  /** Which one-tap window is in flight, for an app.open approve — so only
   *  THAT button reads "Sending…" while its siblings stay tappable-looking
   *  (they're still disabled via `isBusy`, just not lying about which one
   *  was pressed). */
  window?: AppOpenWindow;
}

export function RequestCard({
  request: req,
  child,
  compact = false,
}: {
  request: ChildRequest;
  child: Child;
  compact?: boolean;
}) {
  const {
    state,
    approveRequest,
    denyRequest,
    approveAppOpen,
    dismissRequest,
    deviceStatus,
  } = useCharter();
  const now = Date.now();

  const [busy, setBusy] = useState<BusyState | null>(null);
  // A decision that couldn't be delivered (e.g. a device ask with no signer
  // connected) — the request stays pending and the guardian sees why.
  const [decideError, setDecideError] = useState<string | null>(null);
  const [expanded, setExpanded] = useState(false);
  const [sheet, setSheet] = useState<"approve" | "give" | "retype" | null>(null);
  const [note, setNote] = useState<string | null>(null);

  // Busy while THIS card is sending or while any other ask is: only one
  // decision is in flight at a time across the whole screen.
  const decisionInFlight = useAskDecisionInFlight();
  const isBusy = busy !== null || decisionInFlight;
  const approving = busy?.action === "approve";
  const denying = busy?.action === "deny";
  const chip = limitChip(req);
  const isExtend = req.kind === "time.extend";
  const isArtifact = req.kind === "install.app" || req.kind === "run.program";
  const isAppOpen = req.kind === "app.open";
  // Persisted on the REQUEST record (M-1, hardware round 2026-08-03), not
  // component state — a `useState` here reset to the FULL requested amount on
  // remount, which is exactly what happens on a failed send (C-1's error path:
  // fail → go fix the tz → come back → approve). A guardian who'd dialled 30
  // down to 5 got a retry that silently granted 30.
  const granted = req.chosenMinutes ?? req.minutesRequested ?? 0;
  const askedFor = req.minutesRequested ?? 0;
  // The ward's own broker ages a Pending ask out to Expired (terminal) after
  // 24h and refuses to enact a GRANT for it (7ba043b, 002c211) — approving or
  // denying here would sign and send a decision the device has already stopped
  // listening for. Dismiss (the quiet ✕) still works: it signs nothing and
  // only tidies this list.
  const expired = isAskExpired(req.createdAt, now);

  // install.app is verifiable only against the CURATED catalog — that's the
  // trusted source of the signing-cert the phone pins. An uncurated app can't
  // be safely approved (no digest to pin).
  const isInstall = req.kind === "install.app";
  const curated = isInstall && req.appId ? catalogLookup(req.appId) : undefined;
  const blockedUnverified = isInstall && !curated;

  // The device-scope policy — the source of a bucket's LABEL (a raw wire id
  // means nothing to a guardian) and, for app.open's "Rest of today", the
  // child's OWN tz (never the guardian phone's — the same trap `endOfDayUnix`
  // exists to avoid). Schedule, then budget, then (a named-times-only ward's
  // only tz) buckets — see C-1.
  const devicePolicy = child.policies.find((p) => p.scope.kind === "device");
  const childTz = resolveChildTz(devicePolicy);

  // Requests always come from a device. Name it so it's never mistaken for a
  // remote/account ask.
  const deviceById = new Map<string, Device>(child.devices.map((d) => [d.id, d]));
  const device = req.deviceId ? deviceById.get(req.deviceId) : undefined;
  const deviceLabel = device?.label ?? "their device";

  // Bucket ask: join the raw wire id to the saved group's label — a group
  // renamed/removed since the device asked still renders, by its raw id
  // (never a blank heading).
  const bucketLabel =
    isExtend && req.limitHit === "bucket"
      ? bucketGroupLabel(req.bucketId, devicePolicy?.buckets?.buckets)
      : undefined;
  // Times: the kind of time the ward asked for more of. A time deleted since
  // the ask still has an id on it but nothing to name it, and approving "as
  // asked" would send a grant for a time the device no longer has (it enacts
  // nothing), so that case offers only a different kind.
  const typeLabel =
    isExtend && req.time ? devicePolicy?.times?.find((t) => t.id === req.time)?.label : undefined;
  const unknownType = isExtend && !!req.time && typeLabel === undefined;
  // Re-typing sends a gift of another kind to the asking device, which an older
  // (or never-reported) ward would land as Free time whatever was chosen.
  const askerCanType = !!device && canUseTimes(device, deviceStatus);
  const hasOtherKinds = ((devicePolicy?.times?.length ?? 0) > 0 || !!req.time) && askerCanType;
  // "45m of 1h today · 2h of 5h this week" — only when THIS device's live
  // STATUS actually reports the group. Caps drop out (raw meters instead)
  // while Counted times is paused.
  const statusGroup =
    bucketLabel && device?.devicePubkey
      ? deviceStatus[device.devicePubkey]?.groups?.find((g) => g.id === req.bucketId)
      : undefined;
  // M-2: fold in today's already-granted extras too, so the card a guardian
  // reads before approving THIS ask is exactly as honest as the status line —
  // the same join, same reason.
  const groupRow = statusGroup
    ? groupProgressRows(
        [statusGroup],
        groupProgressBuckets(devicePolicy?.buckets),
        devicePolicy?.buckets
          ? groupExtrasToday(
              state.activity,
              child.id,
              startOfDayUnix(Math.floor(now / 1000), devicePolicy.buckets.tz) * 1000,
            )
          : undefined,
        devicePolicy?.buckets
          ? groupExtrasToday(
              state.activity,
              child.id,
              startOfWeekUnix(
                Math.floor(now / 1000),
                devicePolicy.buckets.tz,
                devicePolicy.buckets.weekStart ?? "mon",
              ) * 1000,
            )
          : undefined,
      )[0]
    : undefined;

  // The honest position, at the moment it changes the decision. Giving ten
  // minutes to a ward already an hour over, or with nothing left in the week,
  // is a different call — and the guardian shouldn't have to go and look it up.
  const standing = isExtend ? standingNote(standingFor(child, deviceStatus)) : null;

  const run = async (next: BusyState, fn: () => Promise<void>) => {
    const token = tryBeginAskDecision();
    if (!token) return;
    setBusy(next);
    setDecideError(null);
    try {
      await fn();
    } catch (e) {
      setDecideError(e instanceof Error ? e.message : "Couldn't send your decision.");
    } finally {
      endAskDecision(token);
      setBusy(null);
    }
  };

  const onApprove = () =>
    run({ action: "approve" }, () => approveRequest(req.id, isExtend ? granted : undefined));

  const onApproveAppOpen = (window: AppOpenWindow) => {
    // "Rest of today" needs the child's OWN tz — never the guardian phone's,
    // which can overshoot the device's own day roll.
    const untilUnix = appOpenWindowUnix(window, Math.floor(Date.now() / 1000), childTz);
    if (untilUnix == null) return;
    void run({ action: "approve", window }, () => approveAppOpen(req.id, untilUnix, window));
  };

  const onDeny = () => run({ action: "deny" }, () => denyRequest(req.id));

  // Local-only: signs nothing, tells the ward nothing (see store.tsx's
  // dismissRequest doc). No confirmation — it's reversible in effect (she can
  // ask again) and costs nothing to undo by simply asking once more.
  const onDismiss = () => dismissRequest(req.id);

  /** The approve button's words, shared by both forms. */
  const approveLabel = approving
    ? "Approving…"
    : expired
      ? "Expired"
      : blockedUnverified
        ? "Can't verify this app"
        : isExtend && askedFor > 0
          ? `Approve ${granted} min${typeLabel ? ` of ${typeLabel}` : ""}`
          : isArtifact
            ? `Approve ${artifactVerb(req.kind)} once`
            : "Approve";

  const sheets = (
    <>
      {sheet === "approve" && (
        <LeverSheet
          mode="approve"
          child={child}
          request={req}
          onClose={(n) => {
            setSheet(null);
            if (n) setNote(n);
          }}
        />
      )}
      {sheet === "retype" && (
        <LeverSheet
          mode="retype"
          child={child}
          request={req}
          onClose={(n) => {
            setSheet(null);
            if (n) setNote(n);
          }}
        />
      )}
      {sheet === "give" && (
        <LeverSheet
          mode="give"
          child={child}
          onClose={(n) => {
            setSheet(null);
            if (n) setNote(n);
          }}
        />
      )}
    </>
  );

  // -------------------------------------------------------------------------
  // Compact: one line, and the answers.
  // -------------------------------------------------------------------------
  if (compact && !expanded) {
    return (
      <Card className="request-compact">
        {/* The line itself opens the full card: a separate "Details" row under
            the answers cost every ask a third of its height on Home. */}
        <button
          type="button"
          className="request-compact-head"
          aria-expanded={false}
          onClick={() => setExpanded(true)}
        >
          <Avatar name={child.name} color={child.color} size={32} />
          <span className="request-compact-title">
            {compactTitle(req, child.name, bucketLabel, typeLabel)}
          </span>
          <span className="request-compact-more">Details ›</span>
        </button>
        {expired && (
          <p className="card-sub" style={{ color: "var(--warn, #8a5a00)", margin: "8px 0 0" }}>
            This ask expired on the device.
          </p>
        )}
        {decideError && (
          <p className="card-sub" style={{ color: "var(--warn, #b45309)", margin: "8px 0 0" }} role="alert">
            {decideError}
          </p>
        )}
        <div className="request-actions">
          {expired ? (
            <>
              {isExtend && (
                <Button className="lever-primary" onClick={() => setSheet("give")}>
                  Give time
                </Button>
              )}
              <Button variant="secondary" className="lever-primary" onClick={onDismiss}>
                Dismiss
              </Button>
            </>
          ) : (
            <>
              {isAppOpen ? (
                <Button
                  className="lever-primary"
                  disabled={isBusy}
                  onClick={() => onApproveAppOpen(COMPACT_WINDOW)}
                >
                  {approving ? "Sending…" : "1 hour"}
                </Button>
              ) : (
                <Button
                  className="lever-primary"
                  disabled={isBusy || blockedUnverified || unknownType}
                  onClick={() => void onApprove()}
                >
                  {approveLabel}
                </Button>
              )}
              <Button
                variant="secondary"
                className="lever-primary"
                disabled={isBusy}
                onClick={() => void onDeny()}
              >
                {denying ? "Saving…" : "Not now"}
              </Button>
            </>
          )}
        </div>
        {note && (
          <p className="card-sub" style={{ marginTop: 6 }} aria-live="polite">
            {note}
          </p>
        )}
        {sheets}
      </Card>
    );
  }

  // -------------------------------------------------------------------------
  // The full card.
  // -------------------------------------------------------------------------
  return (
    <Card style={{ position: "relative" }}>
      {/* The lightest of the three actions — ignore this ask and stop it
          cluttering the list. Deliberately quiet (faint, no background, no
          border) so it never competes with Approve / Not now: those are real
          answers the ward is told about, this is not. No confirmation —
          nothing is signed, and she can just ask again. The glyph is small;
          the tap target is not. */}
      <button
        type="button"
        aria-label="Dismiss"
        title="Ignore this ask — they won't be told"
        disabled={isBusy}
        onClick={onDismiss}
        className="muted request-dismiss"
        style={{ opacity: isBusy ? 0.3 : 0.5, cursor: isBusy ? "default" : "pointer" }}
      >
        ✕
      </button>
      <div className="row-between" style={{ alignItems: "flex-start", paddingRight: 40 }}>
        <h2 className="card-title">{displayTitle(req, bucketLabel, typeLabel)}</h2>
        <span style={{ fontSize: 22, lineHeight: 1 }} aria-hidden="true">
          {KIND_EMOJI[req.kind]}
        </span>
      </div>

      {/* Always make the source clear: this came from a device. */}
      <p
        className="card-sub"
        style={{
          marginBottom: 4,
          display: "flex",
          alignItems: "center",
          gap: 6,
          flexWrap: "wrap",
        }}
      >
        <span aria-hidden="true">💻</span>
        <span>From {deviceLabel}</span>
        <span aria-hidden="true">·</span>
        <span>{timeAgo(req.createdAt, now)}</span>
      </p>

      {isExtend && req.time && (
        <p className="card-sub" style={{ marginBottom: 4 }}>
          {typeLabel
            ? `${child.name} asks for ${askedFor > 0 ? `${askedFor} min of ` : ""}${typeLabel} on ${deviceLabel}.`
            : `${child.name} asked for a kind of time that isn’t set up any more.`}
        </p>
      )}

      {standing && (
        <p className="card-sub" style={{ color: "var(--warn, #9a6516)" }}>
          {child.name} is {standing}.
        </p>
      )}

      {expired && (
        <p className="card-sub" style={{ color: "var(--warn, #8a5a00)" }}>
          This ask expired on the device — it's more than a day old and the device won't act on
          it now.
          {isExtend && (
            <>
              {" "}
              <button
                type="button"
                className="btn btn-ghost"
                style={{ minHeight: 48, padding: "0 4px" }}
                onClick={() => setSheet("give")}
              >
                Give {child.name} time instead
              </button>
            </>
          )}
        </p>
      )}

      {req.reason && (
        <p style={{ margin: "10px 0 0" }}>
          <span className="muted">They said: </span>“{req.reason}”
        </p>
      )}

      {/* "45m of 1h today · 2h of 5h this week" — the remaining context that
          makes "more Play time?" an informed call, when this device's live
          STATUS actually has it. */}
      {groupRow && (
        <p className="card-sub" style={{ margin: "6px 0 0" }}>
          {groupProgressLine(groupRow)}
        </p>
      )}

      {chip && (
        <div style={{ marginTop: 12 }}>
          <Pill tone="warn">{chip}</Pill>
        </div>
      )}

      {/* Stage 1: per-app, single-use framing — explicit, calm. */}
      {isArtifact && (
        <div
          style={{
            marginTop: 14,
            padding: "12px 14px",
            background: "var(--brand-bg)",
            borderRadius: "var(--radius-control)",
            display: "flex",
            gap: 10,
            alignItems: "flex-start",
          }}
        >
          <span aria-hidden="true" style={{ fontSize: 18, lineHeight: 1.3 }}>
            🔒
          </span>
          <span style={{ fontSize: "var(--fs-small)" }}>{singleUseLine(req)}</span>
        </div>
      )}

      {/* install.app trust: verified publisher, or a clear block. */}
      {isInstall && curated && (
        <div style={{ marginTop: 12 }}>
          <Pill tone="ok">✓ Verified · {curated.publisher}</Pill>
        </div>
      )}
      {blockedUnverified && (
        <div
          style={{
            marginTop: 14,
            padding: "12px 14px",
            background: "var(--warn-bg, #3a2a10)",
            borderRadius: "var(--radius-control)",
            fontSize: "var(--fs-small)",
          }}
        >
          <strong>Not in your trusted apps.</strong> This app isn't on your verified list, so
          Kintrinsic can't check it's the real one. You can still say “Not now”.
        </div>
      )}

      {isExtend && askedFor > 0 && granted < askedFor && (
        <p className="card-sub" style={{ marginTop: 10 }}>
          They asked for {askedFor} min.
        </p>
      )}

      {/* app.open: one-tap windows stand in for the usual Approve button —
          granting IS picking how long, there is no separate "approve, then
          decide" step. */}
      {isAppOpen && (
        <div style={{ marginTop: 16 }}>
          <div className="field-label" id={`open-label-${req.id}`}>
            Open for…
          </div>
          <div
            role="group"
            aria-labelledby={`open-label-${req.id}`}
            style={{ display: "flex", flexWrap: "wrap", gap: 8 }}
          >
            {APP_OPEN_WINDOWS.map(({ window, label }) => {
              // Hidden rather than quietly meaning midnight when the child's
              // tz is unknown.
              if (window === "restOfDay" && !childTz) return null;
              // Only the TAPPED window reads "Sending…" — the other two stay
              // labelled (still disabled via `isBusy`, but not claiming to be
              // the one sent).
              const sendingThis = approving && busy?.window === window;
              return (
                <Button
                  key={window}
                  variant="secondary"
                  className="lever-primary"
                  disabled={isBusy || expired}
                  onClick={() => onApproveAppOpen(window)}
                  style={{ flex: "1 1 28%", minWidth: 96 }}
                >
                  {sendingThis ? "Sending…" : label}
                </Button>
              );
            })}
          </div>
        </div>
      )}

      {decideError && (
        <p className="card-sub" style={{ color: "var(--warn, #b45309)", marginTop: 12 }} role="alert">
          {decideError}
        </p>
      )}

      <div style={{ display: "flex", gap: 12, marginTop: 18 }}>
        {!isAppOpen && (
          <Button
            variant="primary"
            block
            className="lever-primary"
            disabled={isBusy || blockedUnverified || expired || unknownType}
            onClick={() => void onApprove()}
          >
            {approveLabel}
          </Button>
        )}
        <Button
          variant="secondary"
          block
          className="lever-primary"
          disabled={isBusy || expired}
          onClick={() => void onDeny()}
        >
          {denying ? "Saving…" : "Not now"}
        </Button>
      </div>

      {isExtend && askedFor > 0 && !expired && (
        <div style={{ marginTop: 6, textAlign: "center" }}>
          <button
            type="button"
            className="request-details"
            disabled={isBusy}
            onClick={() => setSheet("approve")}
          >
            Change amount
          </button>
        </div>
      )}
      {/* Only a real ask (it has something on the wire to answer) and only when
          there is another kind to give: Free time instead of Learning time, or
          the other way round. */}
      {isExtend && !!req.reqId && askedFor > 0 && !expired && hasOtherKinds && (
        <div style={{ marginTop: 6, textAlign: "center" }}>
          <button
            type="button"
            className="request-details"
            disabled={isBusy}
            onClick={() => setSheet("retype")}
          >
            Give a different kind of time
          </button>
        </div>
      )}
      {compact && (
        <div style={{ marginTop: 6, textAlign: "center" }}>
          <button type="button" className="request-details" onClick={() => setExpanded(false)}>
            Less
          </button>
        </div>
      )}
      {note && (
        <p className="card-sub" style={{ marginTop: 6 }} aria-live="polite">
          {note}
        </p>
      )}
      {sheets}
    </Card>
  );
}

/** Shown above the asks while no parent approval is set up: deciding works, but
 *  anyone holding the phone can decide. */
export function ParentApprovalNote() {
  const { state } = useCharter();
  if (state.signer.connected) return null;
  return (
    <Banner tone="info">
      <span>
        You can decide now. Want approvals locked to just you? Turn on parent approval in
        Settings.{" "}
        <button
          type="button"
          className="btn btn-ghost"
          style={{ minHeight: 48, padding: "0 2px" }}
          onClick={() => {
            window.location.hash = "/settings";
          }}
        >
          Go to Settings
        </button>
      </span>
    </Banner>
  );
}
