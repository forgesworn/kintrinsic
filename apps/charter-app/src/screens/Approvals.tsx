import { useMemo, useState } from "react";
import type {
  Child,
  ChildRequest,
  Device,
  RequestKind,
} from "../domain/types";
import {
  Banner,
  Button,
  Card,
  EmptyState,
  Pill,
} from "../components/ui";
import { WardHeading } from "../components/wards";
import { goToWard } from "../domain/wardRoute";
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
import { unrecognisedGuardTz, unrecognisedLine, unrecognisedRows, UNRECOGNISED_EXPLAINER } from "../domain/unrecognisedTime";
import { identityDisplayLabel } from "../domain/launchSignatures";
import { resolveChildTz } from "../domain/childTz";
import { appOpenWindowUnix, startOfDayUnix, type AppOpenWindow } from "../wire/grant";

// =============================================================================
// Approvals — the requests queue. The highest-frequency parent action.
//
// Everything funnels through ONE shape: the device said no, the child asked,
// and now it's the parent's call. Requests always come FROM a device (never a
// child account), so every card names the device out loud.
//
// time.extend  -> a grant stepper, so a parent can give less than asked.
// install.app  -> per-app, SINGLE-USE. "Approve VLC once" — never a standing
// run.program     "can install/run anything". The card says this in plain words.
//
// Pending requests are grouped by child, newest first. Big Approve / Not now.
// =============================================================================

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

/**
 * The card's heading. A bucket-hit time.extend or an app.open ask reads
 * better with the SPECIFIC thing named ("More Play time?", "Open Minecraft?")
 * than the generic device-phrased `req.title` `ingestDeviceRequest` stores —
 * the group label is only knowable here, joined against the saved policy.
 */
function displayTitle(req: ChildRequest, bucketLabel: string | undefined): string {
  // Belt and braces (F1 review): `identityDisplayLabel` resolves a raw
  // `cmdline:` string (which should never itself be device-reported, but
  // could ride back on an already-saved policy) to its signature's label
  // rather than showing the needle on the card's heading.
  if (req.kind === "app.open") return `Open ${identityDisplayLabel(req.appLabel, req.appId) ?? "this app"}?`;
  if (bucketLabel) return `More ${bucketLabel} time?`;
  return req.title;
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
  id: string;
  action: "approve" | "deny";
  /** Which one-tap window is in flight, for an app.open approve — so only
   *  THAT button reads "Sending…" while its siblings stay tappable-looking
   *  (they're still disabled via `isBusy`, just not lying about which one
   *  was pressed). */
  window?: AppOpenWindow;
}

export default function Approvals() {
  const {
    state,
    approveRequest,
    denyRequest,
    approveAppOpen,
    dismissRequest,
    setRequestChosenMinutes,
    deviceStatus,
  } = useCharter();
  const now = Date.now();

  const [busy, setBusy] = useState<BusyState | null>(null);
  // A decision that couldn't be delivered (e.g. a device ask with no signer
  // connected) — the request stays pending and the parent sees why.
  const [decideError, setDecideError] = useState<string | null>(null);

  // Pending requests grouped by child, each group newest-first, and the
  // groups themselves ordered by their most recent request.
  const groups = useMemo(() => {
    const pending = state.requests
      .filter((r) => r.status === "pending")
      .sort((a, b) => b.createdAt - a.createdAt);

    const byChild = new Map<string, ChildRequest[]>();
    for (const r of pending) {
      const list = byChild.get(r.childId) ?? [];
      list.push(r);
      byChild.set(r.childId, list);
    }

    const childById = new Map(state.children.map((c) => [c.id, c]));
    return Array.from(byChild.entries())
      .map(([childId, requests]) => ({
        child: childById.get(childId),
        requests,
      }))
      .filter(
        (g): g is { child: Child; requests: ChildRequest[] } =>
          g.child !== undefined,
      );
  }, [state.requests, state.children]);

  if (groups.length === 0) {
    return (
      <EmptyState emoji="🎉" title="All caught up">
        No requests right now. We'll let you know the moment someone asks.
      </EmptyState>
    );
  }

  const signerConnected = state.signer.connected;

  // Persisted on the REQUEST record (M-1, hardware round 2026-08-03), not
  // screen-local state — a `useState` here reset to the FULL requested
  // amount on remount, which is exactly what happens on a failed send
  // (C-1's error path: fail → go fix the tz → come back → approve). A
  // guardian who'd dialled 30 down to 5 got a retry that silently granted 30.
  const grantedFor = (req: ChildRequest): number =>
    req.chosenMinutes ?? req.minutesRequested ?? 0;

  const stepGrant = (req: ChildRequest, delta: number) => {
    const max = req.minutesRequested ?? 0;
    const next = Math.max(5, Math.min(max, grantedFor(req) + delta));
    setRequestChosenMinutes(req.id, next);
  };

  const onApprove = async (req: ChildRequest) => {
    if (busy) return;
    setBusy({ id: req.id, action: "approve" });
    setDecideError(null);
    try {
      const minutes =
        req.kind === "time.extend" ? grantedFor(req) : undefined;
      await approveRequest(req.id, minutes);
    } catch (e) {
      setDecideError(e instanceof Error ? e.message : "Couldn't send your decision.");
    } finally {
      setBusy(null);
    }
  };

  const onApproveAppOpen = async (req: ChildRequest, untilUnix: number, window: AppOpenWindow) => {
    if (busy) return;
    setBusy({ id: req.id, action: "approve", window });
    setDecideError(null);
    try {
      await approveAppOpen(req.id, untilUnix, window);
    } catch (e) {
      setDecideError(e instanceof Error ? e.message : "Couldn't send your decision.");
    } finally {
      setBusy(null);
    }
  };

  const onDeny = async (req: ChildRequest) => {
    if (busy) return;
    setBusy({ id: req.id, action: "deny" });
    setDecideError(null);
    try {
      await denyRequest(req.id);
    } catch (e) {
      setDecideError(e instanceof Error ? e.message : "Couldn't send your decision.");
    } finally {
      setBusy(null);
    }
  };

  // Local-only: signs nothing, tells the ward nothing (see store.tsx's
  // dismissRequest doc). No confirmation — it's reversible in effect (she can
  // ask again) and costs nothing to undo by simply asking once more.
  const onDismiss = (req: ChildRequest) => {
    dismissRequest(req.id);
  };

  const goToFamily = () => {
    window.location.hash = "/family";
  };

  return (
    <div className="stack">
      {decideError && <Banner tone="warn">{decideError}</Banner>}
      {!signerConnected && (
        <Banner tone="info">
          <span>
            You can decide now. Want approvals locked to just you? Turn on parent
            approval in Family.{" "}
            <button
              type="button"
              className="btn btn-ghost"
              style={{ minHeight: "auto", padding: "0 2px" }}
              onClick={goToFamily}
            >
              Go to Family
            </button>
          </span>
        </Banner>
      )}

      {groups.map(({ child, requests }) => {
        const deviceById = new Map<string, Device>(
          child.devices.map((d) => [d.id, d]),
        );
        // The device-scope policy — the source of a bucket's LABEL (a raw
        // wire id means nothing to a guardian) and, for app.open's "Rest of
        // today", the child's OWN tz (never the guardian phone's — the same
        // trap `endOfDayUnix` exists to avoid). Schedule, then budget, then
        // (a named-times-only ward's only tz) buckets — see C-1.
        const devicePolicy = child.policies.find((p) => p.scope.kind === "device");
        const childTz = resolveChildTz(devicePolicy);
        // The DEVICE's own day-boundary tz (schedule, then budget, then UTC —
        // never buckets.tz, unlike `childTz` above) for the unrecognised-time
        // day guard — deliberately a DIFFERENT resolution than `childTz`'s own
        // correct-for-its-purpose fallback to buckets.tz (review, New-1; see
        // `domain/unrecognisedTime.ts`'s `unrecognisedGuardTz` doc for why).
        const unrecognisedTz = unrecognisedGuardTz(devicePolicy);

        return (
          <section key={child.id} aria-label={`Requests from ${child.name}`}>
            {/* The same heading as every other screen uses for a ward — and it
                links onward, because "should I give ten minutes?" is often
                really "what does their day look like?". */}
            <WardHeading
              child={child}
              onOpen={() => goToWard("limits", child.id)}
              openLabel="Their limits"
            />
            {/* The honest position, at the moment it changes the decision.
                Giving ten minutes to a ward already an hour over, or with
                nothing left in the week, is a different call — and the
                guardian shouldn't have to go and look it up. */}
            {(() => {
              const note = standingNote(standingFor(child, deviceStatus));
              return note ? (
                <p
                  className="card-sub"
                  style={{ margin: "-4px 2px 10px 40px", color: "var(--warn, #9a6516)" }}
                >
                  {child.name} is {note}.
                </p>
              ) : null;
            })()}

            {/* Honest attribution's unrecognised-time line — see Home.tsx's
                identical block and `domain/unrecognisedTime.ts`'s file doc.
                A FLOOR, never a total; renders only when > 0. */}
            {(() => {
              const rows = unrecognisedRows(child, deviceStatus, Math.floor(now / 1000), unrecognisedTz);
              if (rows.length === 0) return null;
              const multiDevice = child.devices.filter((d) => d.pairing === "paired").length > 1;
              return (
                <div style={{ margin: "-4px 2px 10px 40px" }}>
                  {rows.map((row) => (
                    <p key={row.deviceId} className="card-sub" style={{ margin: 0 }}>
                      {unrecognisedLine(row, multiDevice)}
                    </p>
                  ))}
                  <p className="card-sub" style={{ margin: 0, opacity: 0.8 }}>
                    {UNRECOGNISED_EXPLAINER}
                  </p>
                </div>
              );
            })()}

            <div className="stack">
              {requests.map((req) => {
                const isBusy = busy?.id === req.id;
                const approving = isBusy && busy?.action === "approve";
                const denying = isBusy && busy?.action === "deny";
                const chip = limitChip(req);
                const isExtend = req.kind === "time.extend";
                const isArtifact =
                  req.kind === "install.app" || req.kind === "run.program";
                const isAppOpen = req.kind === "app.open";
                const granted = grantedFor(req);
                const askedFor = req.minutesRequested ?? 0;

                // install.app is verifiable only against the CURATED catalog —
                // that's the trusted source of the signing-cert the phone pins.
                // An uncurated app can't be safely approved (no digest to pin).
                const isInstall = req.kind === "install.app";
                const curated = isInstall && req.appId ? catalogLookup(req.appId) : undefined;
                const blockedUnverified = isInstall && !curated;

                // Requests always come from a device. Name it so it's never
                // mistaken for a remote/account ask.
                const device = req.deviceId
                  ? deviceById.get(req.deviceId)
                  : undefined;
                const deviceLabel = device?.label ?? "their device";

                // Bucket ask: join the raw wire id to the saved group's
                // label — a group renamed/removed since the device asked
                // still renders, by its raw id (never a blank heading).
                const bucketLabel =
                  isExtend && req.limitHit === "bucket"
                    ? bucketGroupLabel(req.bucketId, devicePolicy?.buckets?.buckets)
                    : undefined;
                // "45m of 1h today · 2h of 5h this week" — only when THIS
                // device's live STATUS actually reports the group. Caps drop
                // out (raw meters instead) while Counted times is paused —
                // same guard as `Home.tsx`/`GiveTime`.
                const statusGroup =
                  bucketLabel && device?.devicePubkey
                    ? deviceStatus[device.devicePubkey]?.groups?.find((g) => g.id === req.bucketId)
                    : undefined;
                // M-2: fold in today's already-granted extras too, so the
                // card a guardian reads before approving THIS ask is exactly
                // as honest as the Today card — the same join, same reason.
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
                    )[0]
                  : undefined;

                return (
                  <Card key={req.id} style={{ position: "relative" }}>
                    {/* The lightest of the three actions — ignore this ask
                        and stop it cluttering the list. Deliberately quiet
                        (faint, no background, no border) so it never
                        competes with Approve / Not now: those are real
                        answers the ward is told about, this is not. No
                        confirmation — nothing is signed, and she can just
                        ask again. */}
                    <button
                      type="button"
                      aria-label="Dismiss"
                      title="Ignore this ask — they won't be told"
                      disabled={isBusy}
                      onClick={() => onDismiss(req)}
                      className="muted"
                      style={{
                        position: "absolute",
                        top: 8,
                        right: 8,
                        width: 26,
                        height: 26,
                        background: "transparent",
                        border: "none",
                        borderRadius: "50%",
                        padding: 0,
                        lineHeight: 1,
                        fontSize: 13,
                        cursor: isBusy ? "default" : "pointer",
                        opacity: isBusy ? 0.3 : 0.5,
                      }}
                    >
                      ✕
                    </button>
                    <div
                      className="row-between"
                      style={{ alignItems: "flex-start", paddingRight: 28 }}
                    >
                      <h2 className="card-title">{displayTitle(req, bucketLabel)}</h2>
                      <span
                        style={{ fontSize: 22, lineHeight: 1 }}
                        aria-hidden="true"
                      >
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

                    {req.reason && (
                      <p style={{ margin: "10px 0 0" }}>
                        <span className="muted">They said: </span>
                        “{req.reason}”
                      </p>
                    )}

                    {/* "45m of 1h today · 2h of 5h this week" — the remaining
                        context that makes "more Play time?" an informed call,
                        when this device's live STATUS actually has it. */}
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
                        <span
                          aria-hidden="true"
                          style={{ fontSize: 18, lineHeight: 1.3 }}
                        >
                          🔒
                        </span>
                        <span style={{ fontSize: "var(--fs-small)" }}>
                          {singleUseLine(req)}
                        </span>
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
                        <strong>Not in your trusted apps.</strong> This app isn't on
                        your verified list, so Kintrinsic can't check it's the real one.
                        You can still say “Not now”.
                      </div>
                    )}

                    {isExtend && askedFor > 0 && (
                      <div style={{ marginTop: 16 }}>
                        <div
                          className="field-label"
                          id={`grant-label-${req.id}`}
                        >
                          Give them
                        </div>
                        <div
                          className="row-between"
                          role="group"
                          aria-labelledby={`grant-label-${req.id}`}
                          style={{ justifyContent: "flex-start", gap: 14 }}
                        >
                          <Button
                            variant="secondary"
                            aria-label="5 minutes less"
                            disabled={isBusy || granted <= 5}
                            onClick={() => stepGrant(req, -5)}
                            style={{ minWidth: 52, padding: 0 }}
                          >
                            −
                          </Button>
                          <span
                            aria-live="polite"
                            style={{
                              minWidth: 92,
                              textAlign: "center",
                              fontWeight: 700,
                              fontSize: 18,
                            }}
                          >
                            {granted} min
                          </span>
                          <Button
                            variant="secondary"
                            aria-label="5 minutes more"
                            disabled={isBusy || granted >= askedFor}
                            onClick={() => stepGrant(req, 5)}
                            style={{ minWidth: 52, padding: 0 }}
                          >
                            +
                          </Button>
                        </div>
                        {granted < askedFor && (
                          <p className="card-sub" style={{ marginTop: 8 }}>
                            They asked for {askedFor} min.
                          </p>
                        )}
                      </div>
                    )}

                    {/* app.open: one-tap windows stand in for the usual
                        Approve button — granting IS picking how long, there
                        is no separate "approve, then decide" step. */}
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
                            // "Rest of today" needs the child's OWN tz — never
                            // the guardian phone's, which can overshoot the
                            // device's own day roll. Hidden rather than
                            // quietly meaning midnight when it's unknown.
                            if (window === "restOfDay" && !childTz) return null;
                            // Only the TAPPED window reads "Sending…" — the
                            // other two stay labelled (still disabled via
                            // `isBusy`, but not claiming to be the one sent).
                            const sendingThis = approving && busy?.window === window;
                            return (
                              <Button
                                key={window}
                                variant="secondary"
                                disabled={isBusy}
                                onClick={() => {
                                  const untilUnix = appOpenWindowUnix(
                                    window,
                                    Math.floor(Date.now() / 1000),
                                    childTz,
                                  );
                                  if (untilUnix != null) onApproveAppOpen(req, untilUnix, window);
                                }}
                                style={{ flex: "1 1 28%", minWidth: 96 }}
                              >
                                {sendingThis ? "Sending…" : label}
                              </Button>
                            );
                          })}
                        </div>
                      </div>
                    )}

                    <div
                      style={{
                        display: "flex",
                        gap: 12,
                        marginTop: 18,
                      }}
                    >
                      {!isAppOpen && (
                        <Button
                          variant="primary"
                          block
                          disabled={isBusy || blockedUnverified}
                          onClick={() => onApprove(req)}
                        >
                          {approving
                            ? "Approving…"
                            : blockedUnverified
                              ? "Can't verify this app"
                              : isExtend && askedFor > 0
                                ? `Approve ${granted} min`
                                : isArtifact
                                  ? `Approve ${artifactVerb(req.kind)} once`
                                  : "Approve"}
                        </Button>
                      )}
                      <Button
                        variant="secondary"
                        block
                        disabled={isBusy}
                        onClick={() => onDeny(req)}
                      >
                        {denying ? "Saving…" : "Not now"}
                      </Button>
                    </div>
                  </Card>
                );
              })}
            </div>
          </section>
        );
      })}
    </div>
  );
}
