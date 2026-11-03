import { useState } from "react";

export default function GuardianBattery() {
  const carrier = window.CharterCarrier;
  const supported = !!carrier?.batterySettings && !!carrier?.setBatterySettings;
  const [cap, setCap] = useState(() => {
    try {
      const value = (JSON.parse(carrier?.batterySettings?.() ?? "{}") as { reconnectCapSecs?: number }).reconnectCapSecs;
      return value && [60, 300, 600].includes(value) ? value : 300;
    } catch { return 300; }
  });
  const [error, setError] = useState("");
  function save(value: number) {
    try {
      carrier?.setBatterySettings?.(JSON.stringify({ reconnectCapSecs: value }));
      setCap(value); setError("");
    } catch { setError("Could not save. Try opening Kintrinsic again."); }
  }
  return <details className="card"><summary>Battery &amp; responsiveness — this guardian phone</summary>
    {!supported ? <p>{carrier ? "Update the guardian APK to configure background reconnection." : "Browser background delivery follows your browser. Install the guardian Android app for background request notifications."}</p> : <>
      <label>Response preference<select className="input" value={cap} onChange={(e) => save(Number(e.target.value))}>
        <option value={600}>Longer battery life</option><option value={300}>Balanced (recommended)</option><option value={60}>Faster responses</option>
      </select></label>
      <p>Connections stay open for background requests and emergency notifications. During an outage, retries back off up to {cap} seconds; requests and emergency notifications can be delayed until reconnection. Existing queued retries keep their deadline.</p>
      <details><summary>Advanced controls</summary>
        <label>Maximum reconnect gap<select className="input" value={cap} onChange={(e) => save(Number(e.target.value))}>
          {[60, 300, 600].map((n) => <option key={n} value={n}>{n} seconds</option>)}
        </select></label>
        <p>The connection keepalive stays at four minutes. This option changes outage retries only.</p>
      </details>
      <button className="btn" onClick={() => save(300)}>Restore recommended defaults</button>
      {error && <p role="alert">{error}</p>}
    </>}
  </details>;
}
