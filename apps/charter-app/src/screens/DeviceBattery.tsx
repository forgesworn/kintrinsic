import { useState } from "react";
import type { Child, Device } from "../domain/types";
import { recommendedBattery, type BatterySettings } from "../domain/battery";
import { DEFAULT_RELAYS } from "../signer/config";
import { useCharter } from "../store/store";
import BatteryControls from "./BatteryControls";

export default function DeviceBattery({ child, device }: { child: Child; device: Device }) {
  const { createRelayCode } = useCharter();
  const [value, setValue] = useState<BatterySettings>({ ...recommendedBattery });
  const [code, setCode] = useState("");
  const [error, setError] = useState("");
  const [busy, setBusy] = useState(false);
  return <details className="card device-section"><summary>Battery &amp; responsiveness — {device.label}</summary>
    <p className="card-sub">For this ward phone only. Your guardian phone has separate settings. These are proposed values until the ward confirms applying the signed code. Requires an updated ward APK.</p>
    <p className="card-sub">On Longer battery life, while your child&rsquo;s phone is asleep, a change you send can take up to 5 minutes to arrive. Times you have already set, such as bedtime, still start on time. The phone also checks straight away when the screen comes on.</p>
    <BatteryControls value={value} onChange={(v) => { setValue(v); setCode(""); }} />
    <p className="card-sub">This code also confirms the relay list. Check it matches the phone before signing: {(device.relays ?? DEFAULT_RELAYS).join(", ")}. If its relay list differs, update that list in Relays first.</p>
    <button className="btn" disabled={busy || device.pairing !== "paired"} onClick={async () => {
      setBusy(true); setError(""); setCode("");
      try { setCode(await createRelayCode(child.id, device.id, device.relays ?? DEFAULT_RELAYS, value)); }
      catch (err) { setError(err instanceof Error ? err.message : "Could not sign device configuration."); }
      finally { setBusy(false); }
    }}>{busy ? "Signing…" : "Create signed battery code"}</button>
    {error && <p role="alert">{error}</p>}
    {code && <><p>Within ten minutes, on {device.label}, open Kintrinsic → About this device → Change relays / battery. Paste this code and Apply. Signing does not confirm receipt or application. Older ward apps reject the code safely.</p>
      <textarea className="input" aria-label={`Signed battery code for ${device.label}`} readOnly rows={5} value={code} />
    </>}
  </details>;
}
