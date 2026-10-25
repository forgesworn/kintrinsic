import { useEffect, useMemo, useState } from "react";
import { Banner } from "../components/ui";
import { useCharter } from "../store/store";
import { answeredLines, recordsOf, type LocalWrapRecord } from "../store/localDelivery";

/**
 * What became of the answers a guardian just gave, when they travelled over the
 * home network: "Applied on {device}" once the ward's own acknowledgement says
 * so, or a plain warning if it refused, failed or found the ask expired. Renders
 * nothing for answers that went by relay only — there is nothing honest to add.
 */
export default function AnsweredDelivery() {
  const { state } = useCharter();
  // A queued answer can pass its ask's 24 h window while the screen sits open:
  // a slow clock lets "waiting" turn into "Expired before it reached…" without
  // needing any other change to wake the render.
  const [now, setNow] = useState(() => Date.now());
  useEffect(() => {
    const id = setInterval(() => setNow(Date.now()), 60_000);
    return () => clearInterval(id);
  }, []);
  const lines = useMemo(() => {
    const nameOf = (rec: LocalWrapRecord): string => {
      for (const c of state.children) {
        const d = c.devices.find((dev) => dev.devicePubkey === rec.machine);
        if (d) return d.label;
      }
      return "their device";
    };
    return answeredLines(state.requests, recordsOf(state.localDeliveries), nameOf, now);
  }, [state.requests, state.localDeliveries, state.children, now]);

  if (lines.length === 0) return null;
  return (
    <section aria-label="Your recent answers" className="stack">
      {lines.map(({ req, line }) => (
        <Banner key={req.id} tone={line.tone}>
          <span>
            <strong>{req.title}</strong> — {line.text}
          </span>
        </Banner>
      ))}
    </section>
  );
}
