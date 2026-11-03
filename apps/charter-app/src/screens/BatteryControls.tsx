import { batteryPreset, batteryPresets, recommendedBattery, type BatterySettings } from "../domain/battery";

export default function BatteryControls({ value, onChange }: { value: BatterySettings; onChange: (v: BatterySettings) => void }) {
  return <div className="stack">
    <label>Battery &amp; responsiveness
      <select className="input" value={batteryPreset(value)} onChange={(e) => onChange({ ...batteryPresets[e.target.value] })}>
        {Object.keys(batteryPresets).map((name) => <option key={name}>{name}</option>)}
        {batteryPreset(value) === "Custom" && <option disabled>Custom</option>}
      </select>
    </label>
    <p className="card-sub">Relay checks use fresh connections. Longer gaps can delay answers and changes. Local delivery and enforcement continue independently. These are fallback delays, not delivery deadlines.</p>
    <details><summary>Advanced controls</summary><div style={{ display: "grid", gap: 12, marginTop: 12 }}>
      {([ ["screenOffSecs", "Screen-off fallback", [120, 300, 600]], ["ordinarySecs", "Ordinary-use fallback", [30, 60, 120]], ["boostSecs", "Fast-check window after a request", [60, 120, 180]] ] as const).map(([key, label, steps]) =>
        <label key={key}>{label}<select className="input" value={value[key]} onChange={(e) => onChange({ ...value, [key]: Number(e.target.value) })}>
          {steps.map((n) => <option key={n} value={n}>{n} seconds</option>)}
        </select></label>)}
      <label><input type="checkbox" checked={value.requestBoost} onChange={(e) => onChange({ ...value, requestBoost: e.target.checked })} /> Fast checks after submitting a request</label>
      <label><input type="checkbox" checked={value.openingBoost} onChange={(e) => onChange({ ...value, openingBoost: e.target.checked })} /> Brief fast checks when opening Kintrinsic</label>
      <p className="card-sub">Fast checks pause with the screen off. An unexpired window resumes on wake; it never renews because a request is still pending. Opening checks last 15 seconds, at most once a minute.</p>
    </div></details>
    <button type="button" className="btn" onClick={() => onChange({ ...recommendedBattery })}>Restore recommended defaults</button>
  </div>;
}
