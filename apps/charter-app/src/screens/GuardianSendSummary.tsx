/** What a guardian-keys send reports: who a relay or the home network took it for. */
export interface ReportLike {
  sent: string[];
  held: string[];
  failed: { to: string; error: string }[];
}

/**
 * Where a statement, PIN or confirmation went, by device name. A send that
 * reached nobody says so plainly rather than reading as success.
 */
export function SendSummary({ report, names }: { report: ReportLike; names: Record<string, string> }) {
  const name = (m: string) => names[m] ?? "a device";
  const list = (ms: string[]) => [...new Set(ms.map(name))].join(", ");
  return (
    <div className="stack" style={{ gap: 6 }}>
      {report.sent.length > 0 && <p className="card-sub" style={{ margin: 0 }}>Sent to {list(report.sent)}.</p>}
      {report.held.length > 0 && (
        <p className="card-sub" style={{ margin: 0 }}>
          Waiting for the home network: {list(report.held)}. It goes as soon as this phone is home.
        </p>
      )}
      {report.failed.map((f) => (
        <p key={f.to} role="alert" className="card-sub" style={{ margin: 0, color: "var(--danger, #B23B30)" }}>
          {name(f.to)}: {f.error}
        </p>
      ))}
      {report.sent.length + report.held.length + report.failed.length === 0 && (
        <p className="card-sub" style={{ margin: 0 }}>Nothing was sent: there was no device to send it to.</p>
      )}
    </div>
  );
}
