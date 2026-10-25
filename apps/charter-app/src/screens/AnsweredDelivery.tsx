import { useMemo } from "react";
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
  const lines = useMemo(() => {
    const nameOf = (rec: LocalWrapRecord): string => {
      for (const c of state.children) {
        const d = c.devices.find((dev) => dev.devicePubkey === rec.machine);
        if (d) return d.label;
      }
      return "their device";
    };
    return answeredLines(state.requests, recordsOf(state.localDeliveries), nameOf, Date.now());
  }, [state.requests, state.localDeliveries, state.children]);

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
