import { useEffect, useState } from "react";
import QRCode from "qrcode";
import { Banner, Button } from "../components/ui";
import { useCharter } from "../store/store";
import type { Child } from "../domain/types";
import { guardianPairingBunkerUri } from "../signer/guardianPairing";
import { DEFAULT_RELAYS } from "../signer/config";
import { pairingSas } from "../domain/pairingSas";
import { fetchApkFeed } from "../provision/apkFeed";
import {
  QrPayloadError,
  qrPayloadText,
  type QrPayloadProblem,
  type WifiSecurity,
} from "../provision/qrPayload";

type Feed = { status: "loading" } | { status: "ready"; feed: unknown } | { status: "error" };

/**
 * QR setup for a ward phone (docs/qr-setup.md): Android's own setup wizard
 * scans this code from a factory reset, downloads the ward app, checks its
 * checksum, makes it Device Owner and hands it the pairing link —
 * no computer, no cable, no developer options.
 *
 * It owns the one-time pairing token while it is on screen, exactly as the
 * cable setup does: minted on mount, cancelled on unmount. The phone echoes
 * the token on its STATUS heartbeat, and the set-up flow around this screen
 * (`SetupComputer`) moves to the naming step by itself, as it does for the
 * pairing QR.
 *
 * Wi-Fi details live in this component's state and nowhere else: never
 * stored, gone when the screen closes (unmounts) — nothing here writes them
 * to storage or to the store.
 */
export default function SetupPhoneQr({
  child,
  onUseCable,
  onBack,
}: {
  child: Child;
  /** Leave for the cable setup — offered when no code can be shown. */
  onUseCable: () => void;
  onBack: () => void;
}) {
  const { beginPhonePairing, cancelPhonePairing, guardianKeys } = useCharter();
  const [feed, setFeed] = useState<Feed>({ status: "loading" });
  const [attempt, setAttempt] = useState(0);
  const [link, setLink] = useState<string | null>(null);
  const [sas, setSas] = useState<string | null>(null);
  const [showWifi, setShowWifi] = useState(false);
  const [ssid, setSsid] = useState("");
  const [password, setPassword] = useState("");
  const [security, setSecurity] = useState<WifiSecurity>("WPA");
  const [payload, setPayload] = useState<string | null>(null);
  const [qrSrc, setQrSrc] = useState<string | null>(null);
  /** Why no code can be shown; "render" when the QR itself failed to draw. */
  const [problem, setProblem] = useState<QrPayloadProblem | "render" | null>(null);

  // The one-time pairing token: this screen's for as long as it is shown.
  useEffect(() => {
    const token = beginPhonePairing(child.id);
    // The key the ward pins: the family's root (see Family's phone QR).
    const pk = guardianKeys.rootPubkey();
    setLink(guardianPairingBunkerUri(pk, DEFAULT_RELAYS, token));
    let alive = true;
    void pairingSas(pk, token).then((code) => {
      if (alive) setSas(code);
    });
    return () => {
      alive = false;
      cancelPhonePairing();
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [child.id]);

  // The release feed, read fresh so the code names the current release.
  useEffect(() => {
    let alive = true;
    setFeed({ status: "loading" });
    fetchApkFeed()
      .then((f) => {
        if (alive) setFeed({ status: "ready", feed: f });
      })
      .catch(() => {
        if (alive) setFeed({ status: "error" });
      });
    return () => {
      alive = false;
    };
  }, [attempt]);

  useEffect(() => {
    setQrSrc(null);
    setPayload(null);
    setProblem(null);
    if (feed.status !== "ready" || !link) return;
    let text: string;
    try {
      // No password means an open network, whatever the picker says.
      const wifi = ssid !== "" ? { ssid, password, security: password ? security : ("NONE" as const) } : undefined;
      text = qrPayloadText(feed.feed, link, wifi);
    } catch (e) {
      if (e instanceof QrPayloadError) {
        setProblem(e.problem);
        return;
      }
      throw e;
    }
    setPayload(text);
    let alive = true;
    QRCode.toDataURL(text, { width: 560, margin: 1, errorCorrectionLevel: "L" })
      .then((url) => {
        if (alive) setQrSrc(url);
      })
      .catch(() => {
        if (alive) setProblem("render");
      });
    return () => {
      alive = false;
    };
  }, [feed, link, ssid, password, security]);

  if (feed.status === "error" || problem) {
    const oldRelease = problem === "old-release";
    return (
      <div className="stack">
        <Banner tone="warn">
          {oldRelease
            ? "QR setup needs the next ward release; use the cable setup for now."
            : "Kintrinsic couldn't read the details of the current ward app, so there is no setup code to show. A code that can't pin the exact app the phone will install is never shown. Try again in a moment, or set the phone up with a cable instead."}
        </Banner>
        {!oldRelease && (
          <Button block onClick={() => setAttempt((n) => n + 1)}>
            Try again
          </Button>
        )}
        <Button variant="secondary" block onClick={onUseCable}>
          🔌 Set up with a cable instead
        </Button>
        <Button variant="ghost" block onClick={onBack}>
          Back
        </Button>
      </div>
    );
  }

  return (
    <div className="stack">
      <Banner tone="info">
        <span>
          Set up {child.name}'s phone straight from its Welcome screen — no
          computer, no cable, no Google account.
        </span>
      </Banner>
      <ol className="card-sub" style={{ margin: 0, paddingLeft: 20 }}>
        <li>
          Factory-reset the phone (Settings → System → Reset), or start with a
          new one.
        </li>
        <li>On the first Welcome screen, tap the same blank spot six times.</li>
        <li>Connect to Wi-Fi if it asks, then scan this code.</li>
      </ol>

      {qrSrc && payload ? (
        <img
          src={qrSrc}
          data-provisioning-payload={payload}
          alt="Setup code — scan it from the phone's Welcome screen"
          style={{
            width: "100%",
            // About 1 KB of payload: a dense code, so give it all the width there is.
            maxWidth: 480,
            margin: "0 auto",
            borderRadius: "var(--radius-card)",
            background: "#fff",
            padding: 8,
          }}
        />
      ) : (
        <div className="card-sub" style={{ textAlign: "center" }} aria-live="polite">
          Getting the setup code ready…
        </div>
      )}

      {sas && (
        <div style={{ textAlign: "center" }}>
          <div
            style={{
              fontSize: 28,
              fontWeight: 700,
              letterSpacing: "0.12em",
              fontFamily: "ui-monospace, SFMono-Regular, Menlo, monospace",
            }}
          >
            {sas}
          </div>
          <div className="card-sub">
            Kintrinsic opens on the phone; if it doesn't, open Kintrinsic from
            the app list. It shows this code. Check it matches before tapping <b>Pair with guardian</b>.
            It will then ask for <b>Usage access</b>: one switch, so time limits
            can count.
          </div>
        </div>
      )}

      <div className="card-sub" style={{ textAlign: "center" }} aria-live="polite">
        Waiting for the phone…
      </div>

      {showWifi ? (
        <>
          <div className="card-sub">
            Only needed if the phone can't find your Wi-Fi itself. Kept on this
            screen only, never saved. A WPA3-only network can't be set here:
            connect the phone to it on its own Wi-Fi screen before scanning.
          </div>
          <label className="field" htmlFor="qr-wifi-ssid" style={{ margin: 0 }}>
            <span className="field-label">Wi-Fi name</span>
            <input
              id="qr-wifi-ssid"
              className="input"
              autoComplete="off"
              spellCheck={false}
              value={ssid}
              onChange={(e) => setSsid(e.target.value)}
            />
          </label>
          <label className="field" htmlFor="qr-wifi-password" style={{ margin: 0 }}>
            <span className="field-label">Wi-Fi password</span>
            <input
              id="qr-wifi-password"
              className="input"
              type="password"
              autoComplete="off"
              value={password}
              onChange={(e) => setPassword(e.target.value)}
            />
          </label>
          <label className="field" htmlFor="qr-wifi-security" style={{ margin: 0 }}>
            <span className="field-label">Security</span>
            <select
              id="qr-wifi-security"
              className="input"
              value={security}
              onChange={(e) => setSecurity(e.target.value as WifiSecurity)}
            >
              <option value="WPA">WPA / WPA2</option>
              <option value="WEP">WEP</option>
              <option value="NONE">None (open network)</option>
            </select>
          </label>
        </>
      ) : (
        <Button variant="ghost" block onClick={() => setShowWifi(true)}>
          Add Wi-Fi details
        </Button>
      )}

      <Button variant="ghost" block onClick={onBack}>
        Back
      </Button>
    </div>
  );
}
