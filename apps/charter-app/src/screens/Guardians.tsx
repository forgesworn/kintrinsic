import { useEffect, useState, type ReactNode } from "react";
import { Banner, Button, Card, Pill, SectionLabel } from "../components/ui";
import { useCharter } from "../store/store";
import GuardianPin from "./GuardianPin";
import GuardianInvite from "./GuardianInvite";
import GuardianJoin from "./GuardianJoin";
import GuardianMove from "./GuardianMove";
import GuardianLostPhone from "./GuardianLostPhone";
import PresenceScanner from "./PresenceScanner";
import { SendSummary, type ReportLike } from "./GuardianSendSummary";
import {
  androidWardsWithoutPin,
  departedWards,
  durationWords,
  familyWards,
  myRootOf,
  goGuardians,
  leaveGate,
  moveGate,
  pendingFor,
  pendingTitle,
  readinessLine,
  removalsAgainstMe,
  removeGate,
  rootsView,
  shortNpub,
  type Gate,
  type PendingView,
  type WardRef,
} from "./guardiansView";

function useNowUnix(): number {
  const [now, setNow] = useState(() => Math.floor(Date.now() / 1000));
  useEffect(() => {
    const t = setInterval(() => setNow(Math.floor(Date.now() / 1000)), 30_000);
    return () => clearInterval(t);
  }, []);
  return now;
}

function ErrorLine({ children }: { children: ReactNode }) {
  return (
    <p role="alert" className="card-sub" style={{ margin: 0, color: "var(--danger, #B23B30)" }}>
      {children}
    </p>
  );
}

/** A disabled action's one-line reason. */
function Reason({ gate }: { gate: Gate }) {
  return gate.ok ? null : (
    <p className="card-sub" style={{ margin: 0 }}>
      {gate.reason}
    </p>
  );
}

/**
 * Before anything that needs a guardian in person: a family with an Android
 * ward this phone hasn't sent a guardian PIN to sets one first, since a ward
 * phone with no PIN can't show a presence code.
 */
function PinGate({ wards, children, multiRoot }: { wards: WardRef[]; children: ReactNode; multiRoot: boolean }) {
  const { guardianKeys } = useCharter();
  const [passed, setPassed] = useState(false);
  const missing = androidWardsWithoutPin(wards, guardianKeys.state);
  if (passed || missing.length === 0) return <>{children}</>;
  return (
    <div className="stack">
      <GuardianPin wards={missing} onDone={() => setPassed(true)} onCancel={() => goGuardians()} />
      {multiRoot && (
        <Button variant="ghost" block onClick={() => setPassed(true)}>
          The other parent already set a PIN
        </Button>
      )}
    </div>
  );
}

/** Guardians: who looks after the family's devices, and the changes waiting at each. */
export default function Guardians({ sub }: { sub: string }) {
  const { state, guardianKeys } = useCharter();
  const wards = familyWards(state.children);
  const me = myRootOf(guardianKeys);
  const multiRoot = rootsView(wards, me).length > 1;
  const back = () => goGuardians();
  const [page, arg] = sub.split("/");

  switch (page) {
    case "invite":
      return (
        <PinGate wards={wards} multiRoot={multiRoot}>
          <GuardianInvite onBack={back} />
        </PinGate>
      );
    case "join":
      return <GuardianJoin onBack={back} onConfirm={() => goGuardians("presence")} />;
    case "presence":
      return (
        <PinGate wards={wards} multiRoot={multiRoot}>
          <PresenceScanner onBack={back} />
        </PinGate>
      );
    case "pin": {
      const android = wards.filter((w) => w.device.platform === "android");
      return android.length === 0 ? (
        <Card>
          <p className="card-sub">There's no ward phone in the family, so no guardian PIN is needed.</p>
          <Button variant="ghost" block onClick={back}>
            Back
          </Button>
        </Card>
      ) : (
        <GuardianPin wards={android} onDone={back} onCancel={back} />
      );
    }
    case "move":
      return (
        <PinGate wards={wards} multiRoot={multiRoot}>
          <GuardianMove onBack={back} onConfirm={() => goGuardians("presence")} />
        </PinGate>
      );
    case "lost":
      return <GuardianLostPhone onBack={back} />;
    case "remove":
      return (
        <PinGate wards={wards} multiRoot={multiRoot}>
          <RemoveParent target={arg ?? ""} />
        </PinGate>
      );
    default:
      return <Overview />;
  }
}

function Overview() {
  const { state, guardianKeys } = useCharter();
  const now = useNowUnix();
  const wards = familyWards(state.children);
  const names = Object.fromEntries(wards.map((w) => [w.device.devicePubkey, w.device.label]));
  const me = myRootOf(guardianKeys);
  const roots = rootsView(wards, me);
  const keys = guardianKeys.state;
  const readiness = guardianKeys.familyReadiness();
  const againstMe = removalsAgainstMe(wards, keys, me);
  const roster = guardianKeys.unconfirmedRosterDevices();
  const departed = departedWards(state.children);
  const [busy, setBusy] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [report, setReport] = useState<{ what: string; report: ReportLike } | null>(null);
  const [confirmLeave, setConfirmLeave] = useState(false);
  const [rosterNote, setRosterNote] = useState<string | null>(null);

  async function act(what: string, fn: () => Promise<ReportLike>) {
    setBusy(what);
    setError(null);
    setReport(null);
    try {
      setReport({ what, report: await fn() });
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e));
    } finally {
      setBusy(null);
    }
  }

  const notReady = readiness.children.flatMap((c) => c.readiness.wards.filter((w) => !w.readiness.ready));
  const inviteGate: Gate = readiness.ready
    ? { ok: true }
    : readiness.children.length === 0
      ? { ok: false, reason: "Pair a device first, then invite another parent." }
      : {
          ok: false,
          reason: `Update Kintrinsic on ${[...new Set(notReady.map((w) => names[w.machine] ?? "a device"))].join(", ")} first, then invite another parent.`,
        };
  const leave = leaveGate(wards, me);
  const remove = removeGate(keys);
  const move = moveGate(keys, readiness.ready);
  const otherRoots = roots.filter((r) => !r.you);
  const rosterGate: Gate = keys.root
    ? { ok: true }
    : { ok: false, reason: "Only a My Signet or Heartwood identity shares the family with another guardian." };

  return (
    <>
      <div style={{ margin: "0 4px 12px" }}>
        <Button variant="ghost" onClick={() => (window.location.hash = "/family")}>
          ‹ Family
        </Button>
      </div>

      {againstMe.map((r) => (
        <div key={r.d} style={{ marginBottom: 12 }}>
          <Banner tone="warn">
            <strong>The other parent has asked to remove you</strong> from {r.devices.join(", ")}.
            {r.takesEffectAt !== undefined &&
              (r.takesEffectAt > now
                ? ` It can take effect in ${durationWords(r.takesEffectAt - now)}, once they confirm in person.`
                : " It takes effect as soon as they confirm in person.")}{" "}
            If you object, it's dropped and you both stay. Your{" "}
            {keys.root?.via === "heartwood" ? "Heartwood" : "My Signet"} signs the objection and may ask more than
            once.
            <div style={{ marginTop: 8 }}>
              <Button
                variant="danger"
                disabled={busy !== null}
                onClick={() => void act("Objection", () => guardianKeys.objectToRemoval(r.d))}
              >
                {busy === "Objection" ? "Objecting…" : "Object"}
              </Button>
            </div>
          </Banner>
        </div>
      ))}

      {report && (
        <Card style={{ marginBottom: 12 }}>
          <h2 className="card-title">{report.what} sent</h2>
          <SendSummary report={report.report} names={names} />
        </Card>
      )}
      {error && (
        <div style={{ marginBottom: 12 }}>
          <ErrorLine>{error}</ErrorLine>
        </div>
      )}

      <SectionLabel>Guardians</SectionLabel>
      <Card>
        {roots.length === 0 ? (
          <p className="card-sub" style={{ margin: 0 }}>
            {wards.length === 0
              ? "No device is connected to this phone yet."
              : "None of your devices has reported its guardians yet. They do once they have the latest Kintrinsic."}
          </p>
        ) : (
          <div className="stack">
            {roots.map((r) => (
              <div key={r.root}>
                <div className="row-between">
                  <span style={{ fontWeight: 600 }}>{r.you ? "You" : "Other parent"}</span>
                  {r.you ? <Pill tone="ok">you</Pill> : <Pill>guardian</Pill>}
                </div>
                <p className="card-sub" style={{ margin: "2px 0" }}>
                  {shortNpub(r.root)} · on {r.devices.join(", ")}
                </p>
                {r.keys.length === 0 ? (
                  <p className="card-sub" style={{ margin: 0 }}>
                    {r.root === guardianKeys.workingPubkey ? "Kept on this phone." : "Signs for itself."}
                  </p>
                ) : (
                  r.keys.map((k) => (
                    <p key={k} className="card-sub" style={{ margin: 0 }}>
                      Phone {k.slice(0, 8)}
                      {k === guardianKeys.workingPubkey ? " (this phone)" : ""}
                    </p>
                  ))
                )}
              </div>
            ))}
          </div>
        )}
      </Card>

      {(wards.length > 0 || departed.length > 0 || roster.length > 0) && <SectionLabel>Devices</SectionLabel>}
      {wards.length > 0 && (
        <div className="stack">
          {wards.map((w) => (
            <DeviceCard key={w.device.id} ward={w} now={now} busy={busy} act={act} />
          ))}
        </div>
      )}
      {departed.map(({ device }) => (
        <Card key={device.id} style={{ marginTop: 12 }}>
          <p className="card-sub" style={{ margin: 0 }}>
            This phone no longer has access to {device.label}.
          </p>
        </Card>
      ))}
      {roster.length > 0 && (
        <Card style={{ marginTop: 12 }}>
          <div className="stack" style={{ gap: 6 }}>
            {roster.map((d) => (
              <div key={d.machine} className="row-between">
                <span>{d.name}</span>
                <Pill tone="warn">unconfirmed</Pill>
              </div>
            ))}
            <p className="card-sub" style={{ margin: 0 }}>
              The other guardian shared these. They show as unconfirmed until they report to this phone.
            </p>
          </div>
        </Card>
      )}

      <SectionLabel>Changes</SectionLabel>
      <Card>
        <div className="stack">
          <Button block disabled={!inviteGate.ok} onClick={() => goGuardians("invite")}>
            Invite a parent
          </Button>
          <Reason gate={inviteGate} />

          <Button variant="secondary" block onClick={() => goGuardians("presence")}>
            Confirm in person
          </Button>

          {wards.length === 0 && (
            <Button variant="secondary" block onClick={() => goGuardians("join")}>
              Join a family
            </Button>
          )}

          {wards.some((w) => w.device.platform === "android") && (
            <Button variant="secondary" block onClick={() => goGuardians("pin")}>
              {wards.some((w) => keys.pinSentAt[w.device.devicePubkey] !== undefined)
                ? "Change the guardian PIN"
                : "Set the guardian PIN"}
            </Button>
          )}

          <Button
            variant="secondary"
            block
            disabled={!rosterGate.ok || busy !== null}
            onClick={() =>
              void (async () => {
                setRosterNote(null);
                setBusy("Family list");
                setError(null);
                try {
                  const r = await guardianKeys.sendRoster();
                  setRosterNote(
                    r.sent.length > 0
                      ? `Shared with ${r.sent.length === 1 ? "the other guardian's phone" : `${r.sent.length} phones`}.`
                      : r.failed.length > 0
                        ? `Couldn't share it: ${r.failed[0].error}`
                        : "Nothing to share with yet: no other guardian's phone is known.",
                  );
                } catch (e) {
                  setError(e instanceof Error ? e.message : String(e));
                } finally {
                  setBusy(null);
                }
              })()
            }
          >
            {busy === "Family list" ? "Sharing…" : "Share the family with the other guardian"}
          </Button>
          <Reason gate={rosterGate} />
          {rosterNote && (
            <p className="card-sub" style={{ margin: 0 }}>
              {rosterNote}
            </p>
          )}

          {otherRoots.map((r) => (
            <div key={r.root} className="stack" style={{ gap: 4 }}>
              <Button
                variant="secondary"
                block
                disabled={!remove.ok}
                onClick={() => goGuardians(`remove/${r.root}`)}
              >
                Remove the other parent ({shortNpub(r.root)})
              </Button>
              <Reason gate={remove} />
            </div>
          ))}

          <Button variant="secondary" block disabled={!move.ok} onClick={() => goGuardians("move")}>
            Change my identity
          </Button>
          <Reason gate={move} />

          <Button variant="secondary" block onClick={() => goGuardians("lost")}>
            Lost a phone?
          </Button>

          {confirmLeave ? (
            <div className="banner banner-warn">
              <p style={{ margin: "0 0 8px" }}>
                You stop being a guardian of every device at once, and this phone stops receiving anything from them.
                The other parent stays in charge; the limits you set stay until they change them.
              </p>
              <div className="stack">
                <Button
                  variant="danger"
                  block
                  disabled={busy !== null}
                  onClick={() => void act("Leaving", () => guardianKeys.leaveFamily()).then(() => setConfirmLeave(false))}
                >
                  {busy === "Leaving" ? "Leaving…" : "Leave the family"}
                </Button>
                <Button variant="ghost" block onClick={() => setConfirmLeave(false)}>
                  Stay
                </Button>
              </div>
            </div>
          ) : (
            <Button variant="ghost" block disabled={!leave.ok} onClick={() => setConfirmLeave(true)}>
              Leave the family
            </Button>
          )}
          <Reason gate={leave} />
        </div>
      </Card>
    </>
  );
}

function DeviceCard({
  ward,
  now,
  busy,
  act,
}: {
  ward: WardRef;
  now: number;
  busy: string | null;
  act: (what: string, fn: () => Promise<ReportLike>) => Promise<void>;
}) {
  const { guardianKeys } = useCharter();
  const me = myRootOf(guardianKeys);
  const r = guardianKeys.childReadiness(ward.childId).wards.find((w) => w.machine === ward.device.devicePubkey);
  const line = r ? readinessLine(r.readiness) : null;
  const pending = pendingFor(ward.device.guardians, guardianKeys.state, me);
  return (
    <Card>
      <div className="row-between">
        <span style={{ fontWeight: 600 }}>
          {ward.device.label}
          <span className="card-sub"> · {ward.childName}</span>
        </span>
        {line ? <Pill tone="warn">needs update</Pill> : <Pill tone="ok">ready</Pill>}
      </div>
      {line && (
        <p className="card-sub" style={{ margin: "4px 0 0" }}>
          {line}
        </p>
      )}
      {pending.map((p) => (
        <PendingRow key={p.d} p={p} me={me} now={now} busy={busy} act={act} />
      ))}
    </Card>
  );
}

function PendingRow({
  p,
  me,
  now,
  busy,
  act,
}: {
  p: PendingView;
  me: string;
  now: number;
  busy: string | null;
  act: (what: string, fn: () => Promise<ReportLike>) => Promise<void>;
}) {
  const { guardianKeys } = useCharter();
  const when: string[] = [];
  if (p.takesEffectAt !== undefined && p.takesEffectAt > now) {
    when.push(`can take effect in ${durationWords(p.takesEffectAt - now)}`);
  }
  when.push(p.expiresAt > now ? `expires in ${durationWords(p.expiresAt - now)}` : "expired");
  const hint =
    p.role === "mine"
      ? p.kind === "coguardian"
        ? "Waiting for them to confirm in person here."
        : "Waiting for you to confirm in person here."
      : p.role === "against-me"
        ? "You can object (see above)."
        : null;
  return (
    <div className="stack" style={{ gap: 4, marginTop: 10, paddingTop: 10, borderTop: "1px solid var(--line)" }}>
      <div className="row-between">
        <span>{pendingTitle(p, me)}</span>
        <Pill tone="warn">pending</Pill>
      </div>
      <p className="card-sub" style={{ margin: 0 }}>
        {when.join(" · ")}
        {hint ? `. ${hint}` : ""}
      </p>
      {p.role === "mine" && (
        <Button
          variant="ghost"
          disabled={busy !== null}
          onClick={() => void act("Cancel", () => guardianKeys.cancelPending(p.d))}
        >
          {busy === "Cancel" ? "Cancelling…" : "Cancel this"}
        </Button>
      )}
    </div>
  );
}

function RemoveParent({ target }: { target: string }) {
  const { state, guardianKeys } = useCharter();
  const wards = familyWards(state.children);
  const names = Object.fromEntries(wards.map((w) => [w.device.devicePubkey, w.device.label]));
  const gate = removeGate(guardianKeys.state);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [report, setReport] = useState<ReportLike | null>(null);
  const valid = /^[0-9a-f]{64}$/.test(target);

  if (report) {
    return (
      <Card>
        <h2 className="card-title">Removal started</h2>
        <SendSummary report={report} names={names} />
        <p className="card-sub">
          Now visit each device and confirm in person. The removal takes effect there 72 hours after you started it,
          or when you confirm, whichever is later, unless they object.
        </p>
        <div className="stack" style={{ marginTop: 12 }}>
          <Button block onClick={() => goGuardians("presence")}>
            Confirm in person
          </Button>
          <Button variant="ghost" block onClick={() => goGuardians()}>
            Done
          </Button>
        </div>
      </Card>
    );
  }

  return (
    <Card>
      <h2 className="card-title">Remove the other parent</h2>
      {!valid ? (
        <p className="card-sub">That guardian isn't on any of your devices.</p>
      ) : !gate.ok ? (
        <p className="card-sub">{gate.reason}</p>
      ) : (
        <div className="stack" style={{ marginTop: 8 }}>
          <p className="card-sub" style={{ margin: 0 }}>
            Removes {shortNpub(target)} as a guardian of every device. It doesn't happen straight away:
          </p>
          <ul className="card-sub" style={{ paddingLeft: 20, margin: 0 }}>
            <li>they get 72 hours' notice, and their phone tells them;</li>
            <li>you must confirm in person at each device;</li>
            <li>they can object, and then you both stay as guardians.</li>
          </ul>
          <p className="card-sub" style={{ margin: 0 }}>
            Your limits are signed again with your own key first, so the rules stay as they are now once they've gone.
          </p>
          {error && <ErrorLine>{error}</ErrorLine>}
          <Button
            variant="danger"
            block
            disabled={busy}
            onClick={() =>
              void (async () => {
                setBusy(true);
                setError(null);
                try {
                  setReport(await guardianKeys.removeCoGuardian(target));
                } catch (e) {
                  setError(e instanceof Error ? e.message : String(e));
                } finally {
                  setBusy(false);
                }
              })()
            }
          >
            {busy ? "Sending…" : "Start removing them"}
          </Button>
        </div>
      )}
      <Button variant="ghost" block disabled={busy} onClick={() => goGuardians()} style={{ marginTop: 12 }}>
        Back
      </Button>
    </Card>
  );
}
