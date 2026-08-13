import { Card, SectionLabel } from "../components/ui";
import { carrierNeedsUpdate } from "./bridge";

// The "get Kintrinsic on your phone" card (spec D5). Points at the ONE canonical
// download place — charter.signet.you/download — so there's a single, stable
// spot to get and update the app (resolves the app-inside-app circularity:
// updating Kintrinsic used to mean opening Kintrinsic). Hidden when already
// running INSIDE the carrier shell — there, the app IS the download.

const DOWNLOAD_URL = "https://charter.signet.you/download.html#android";

export default function CarrierDownload() {
  // …EXCEPT when the shell itself is too old to name a ward. That case used to
  // fall through the gap below: outside the shell you are told to download,
  // inside it you are told nothing, so a stale app could never announce that
  // it was stale. The notification naming lives in the shell's Kotlin, so no
  // amount of updating this page fixes it — only a new APK does, and the
  // guardian has to be told that in those words.
  if (carrierNeedsUpdate()) {
    return (
      <>
        <SectionLabel>On your phone</SectionLabel>
        <Card>
          <p className="card-title" style={{ marginBottom: 2 }}>
            Update the Kintrinsic app
          </p>
          <p className="card-sub">
            Your notifications still say &ldquo;Your ward&rdquo; instead of
            naming who it was and which device. That naming lives in the app
            itself, so updating this page cannot fix it — you need the newer
            Kintrinsic.
          </p>
          <p className="card-sub" style={{ marginTop: 8 }}>
            <a href={DOWNLOAD_URL} target="_blank" rel="noopener noreferrer">
              Update Kintrinsic for Android →
            </a>
          </p>
        </Card>
      </>
    );
  }

  // Inside the carrier shell this card is noise — the app IS the download.
  if (window.CharterCarrier) return null;

  return (
    <>
      <SectionLabel>On your phone</SectionLabel>
      <Card>
        <p className="card-title" style={{ marginBottom: 2 }}>
          Get the Kintrinsic app
        </p>
        <p className="card-sub">
          Requests from your wards reach your phone the moment they happen —
          even with the screen off. No Google services needed.
        </p>
        <p className="card-sub" style={{ marginTop: 8 }}>
          <a href={DOWNLOAD_URL} target="_blank" rel="noopener noreferrer">
            Download Kintrinsic for Android →
          </a>
        </p>
      </Card>
    </>
  );
}
