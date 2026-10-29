import { useEffect, useState, type CSSProperties } from "react";
import QRCode from "qrcode";
import { Banner, Button, SectionLabel } from "../components/ui";
import { useCharter } from "../store/store";
import type { Child, Device } from "../domain/types";
import { guardianPubkeyHex } from "../signer/guardianKey";
import { pairingSas } from "../domain/pairingSas";
import { guardianPairingBunkerUri, guardianPairingQrContent } from "../signer/guardianPairing";
import { DEFAULT_RELAYS } from "../signer/config";
import { parseDevicePairingCode, parsePairInvite } from "../wire/deviceCode";
import { QRScanner } from "../components/QRScanner";
import SetupPhoneQr from "./SetupPhoneQr";


/**
 * Set-up flow. The PHONE path is one scan: this screen shows a QR (the
 * guardian's pairing link + a one-time token); the parent scans it with the
 * CHILD phone's camera; the Kintrinsic app opens, pairs, and echoes the token on
 * its heartbeat — and the device appears here by itself, real pubkey bound,
 * ready to name. Typing the device code stays as the no-camera fallback, and
 * remains the primary path for computers.
 */
export default function SetupComputer({ child, onClose }: { child: Child; onClose: () => void }) {
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
  // QR setup from a factory reset (docs/qr-setup.md): its own token owner,
  // exclusive with the pairing QR and the cable exactly as cableMode is.
  const [qrSetupMode, setQrSetupMode] = useState(false);
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
  const phoneScanMode = kind === "android" && !manualEntry && step === "scan" && !cableMode && !qrSetupMode;

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
      // QR setup mints its own token on mount, and a child's effects run
      // before this one: cancelling here would wipe it in the same commit.
      // SetupPhoneQr cancels its own token when it unmounts.
      if (!qrSetupMode) cancelPhonePairing();
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
  }, [phoneScanMode, haveLocalSigner, child.id, cableMode, qrSetupMode]);

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
    if (next !== "android") {
      setCableMode(false);
      setQrSetupMode(false);
    }
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
          ) : kind === "android" && qrSetupMode && haveLocalSigner ? (
            <SetupPhoneQr
              child={child}
              onBack={() => setQrSetupMode(false)}
              onUseCable={() => {
                setQrSetupMode(false);
                setCableMode(true);
              }}
            />
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
                    <Button block onClick={() => setQrSetupMode(true)}>
                      Set up with a QR code
                    </Button>
                    <div className="card-sub" style={{ textAlign: "center" }}>
                      Recommended for a new or factory-reset phone. Already has
                      the Kintrinsic ward app? Scan the code below instead.
                    </div>
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
                <>
                  <SectionLabel>Other ways</SectionLabel>
                  <Button variant="ghost" block onClick={() => setCableMode(true)}>
                    🔌 Set up with a cable (no scanning)
                  </Button>
                </>
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
      const { bytes, sha256, versionName } = await fetchReleaseApk();
      setDetail(
        `Checked Kintrinsic ${versionName} (SHA-256 ${sha256.slice(0, 16)}…${sha256.slice(-8)}).`,
      );

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
