import { useEffect, useRef, useState } from "react";
import type { UpdateManifest } from "../wire/types";
import { pickInstallUrl } from "../release/releaseEvent";
import {
  MYCHARTER_DOWNLOAD_URL,
  fetchKintrinsicManifest,
  myCharterVersionState,
  readCarrierVersion,
  type KintrinsicVersionState,
} from "./version";

/**
 * The app's own version, at the foot of Family — and a way out when it is
 * behind.
 *
 * This exists because the shell and the page update independently: deploying
 * the PWA changes what is inside the app, while the app's own Kotlin (which
 * writes notifications) changes only on a new APK. There was previously
 * nowhere at all to see which shell you were running, so "I updated Kintrinsic"
 * and "Kintrinsic is up to date" could both be true while the thing you were
 * waiting for sat in an APK you had never installed.
 *
 * Quiet by default: a current app is a footnote, not an announcement. It only
 * raises its voice when there is something to do about it.
 */
export default function AppVersion() {
  const [state, setState] = useState<KintrinsicVersionState>({ kind: "not-carrier" });

  useEffect(() => {
    let live = true;
    const inCarrier = Boolean(window.CharterCarrier);
    const reading = readCarrierVersion();
    // Outside the app there is no shell to check, so don't reach for the
    // network at all.
    if (!inCarrier) {
      setState({ kind: "not-carrier" });
      return;
    }
    void fetchKintrinsicManifest().then((latest) => {
      if (live) setState(myCharterVersionState(true, reading, latest));
    });
    return () => {
      live = false;
    };
  }, []);

  if (state.kind === "not-carrier") return null;

  const line = (text: string, action?: string) => (
    <p className="card-sub" style={{ textAlign: "center", marginTop: 24, opacity: 0.75 }}>
      {text}
      {action && (
        <>
          {" "}
          <a href={MYCHARTER_DOWNLOAD_URL} target="_blank" rel="noopener noreferrer">
            {action}
          </a>
        </>
      )}
    </p>
  );

  switch (state.kind) {
    case "current":
      return line(`Kintrinsic ${state.installed.versionName} — up to date`);
    case "installed-unreadable":
      // The shell HAS the version method, so it is at least the release that
      // added it — it just would not answer. Say that, and never "out of
      // date", which is the one thing we know is false here.
      return line("Kintrinsic — couldn't read this app's version");
    case "unknown":
      // Offline, or the manifest did not answer. Say so plainly rather than
      // implying either "current" or "behind".
      return line(`Kintrinsic ${state.installed.versionName} — couldn't check for updates`);
    case "behind":
      return (
        <BehindNotice installed={state.installed.versionName} latest={state.latest}>
          {line}
        </BehindNotice>
      );
    case "too-old-to-say":
      // No `version()` on the bridge: this shell predates version reporting,
      // so it is genuinely behind even though it cannot say by how much.
      return line("Your Kintrinsic app is out of date.", "Update →");
  }
}

type LineFn = (text: string, action?: string) => JSX.Element;

/** What the polled bridge installState answers deserialize to. */
interface InstallState {
  phase: string;
  error: string | null;
}

function readInstallState(): InstallState | null {
  const carrier = window.CharterCarrier;
  if (!carrier || typeof carrier.installState !== "function") return null;
  try {
    // Call ON the object — a detached reference throws (2026-08-06 lesson).
    const parsed: unknown = JSON.parse(carrier.installState());
    if (typeof parsed !== "object" || parsed === null) return null;
    const o = parsed as Record<string, unknown>;
    if (typeof o.phase !== "string") return null;
    return { phase: o.phase, error: typeof o.error === "string" ? o.error : null };
  } catch {
    return null;
  }
}

/**
 * The `behind` footer. When the shell can self-install (D2 bridge methods)
 * AND the manifest is a relay announcement carrying Blossom mirrors, offer
 * the real in-app update; otherwise the legacy download-page link. "Method
 * absent" (old shell) is the fallback, "call failed" is an error — the two
 * must never collapse into each other.
 */
function BehindNotice({
  installed,
  latest,
  children: line,
}: {
  installed: string;
  latest: UpdateManifest;
  children: LineFn;
}) {
  const [phase, setPhase] = useState<InstallState>({ phase: "idle", error: null });
  const polling = useRef<number | null>(null);

  useEffect(
    () => () => {
      if (polling.current !== null) window.clearInterval(polling.current);
    },
    [],
  );

  const carrier = window.CharterCarrier;
  // A relay ReleaseManifest carries `urls[]`; the origin-JSON fallback carries
  // a single Blossom `url` (D3). Either supplies the self-install source —
  // the canonical Blossom address, never a CDN redirect target (2026-08-27).
  const rm = latest as Partial<{ urls: string[]; url: string }>;
  const mirrors =
    rm.urls && rm.urls.length > 0 ? rm.urls : rm.url ? [rm.url] : [];
  const installUrl = pickInstallUrl(mirrors, latest.apkSha256);
  const canSelfInstall =
    Boolean(carrier && typeof carrier.installUpdate === "function") && installUrl !== null;

  if (!canSelfInstall) {
    return line(`Kintrinsic ${installed} — ${latest.versionName} is available.`, "Update →");
  }

  const start = () => {
    const answer = carrier!.installUpdate!(installUrl!, latest.apkSha256);
    if (answer !== "started" && answer !== "busy") {
      setPhase({ phase: "failed", error: answer });
      return;
    }
    setPhase({ phase: "downloading", error: null });
    if (polling.current !== null) window.clearInterval(polling.current);
    polling.current = window.setInterval(() => {
      const s = readInstallState();
      if (!s) return;
      setPhase(s);
      if ((s.phase === "failed" || s.phase === "done") && polling.current !== null) {
        window.clearInterval(polling.current);
        polling.current = null;
      }
    }, 1000);
  };

  const label: Record<string, string> = {
    downloading: "Downloading the update…",
    verifying: "Checking the download…",
    "waiting-user": "Confirm the install in the Android dialog.",
    done: "Installed — reopening…",
  };
  if (phase.phase in label) {
    return line(`Kintrinsic ${installed} — ${label[phase.phase]}`);
  }
  return (
    <p className="card-sub" style={{ textAlign: "center", marginTop: 24, opacity: 0.75 }}>
      {phase.phase === "failed"
        ? `The update didn't install${phase.error ? ` (${phase.error})` : ""}. `
        : `Kintrinsic ${installed} — ${latest.versionName} is available. `}
      <button type="button" className="linklike" onClick={start}>
        {phase.phase === "failed" ? "Try again" : "Update now"}
      </button>
      {phase.phase === "failed" && (
        <>
          {" or "}
          <a href={MYCHARTER_DOWNLOAD_URL} target="_blank" rel="noopener noreferrer">
            get it from the download page
          </a>
        </>
      )}
    </p>
  );
}
