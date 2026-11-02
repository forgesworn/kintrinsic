// Said before a guardian gives a stopped ward more time. A stand-down caps the
// day after every pool of minutes, so time given now is banked but opens
// nothing until the stand-down is lifted; the action that does is "Allow back
// on", after which the banked time applies.
//
// Not a block: the guardian may still send, for tomorrow's sake or because the
// device status is a few minutes old. The notice only makes sure nobody is
// surprised, which an approval that silently does nothing would be.

import { Button } from "./ui";

function stoppedNoticeText(name: string): string {
  return `${name} is stopped for today, so this won't take effect until you allow them back on.`;
}

export function StoppedNotice({
  name,
  onAllowBack,
}: {
  name: string;
  /** Offered when the existing "Allow back on" action can be reached from here. */
  onAllowBack?: () => void;
}) {
  return (
    <div style={{ margin: "0 0 10px" }} role="status">
      <p className="card-sub" style={{ margin: 0, color: "var(--warn, #9a6516)" }}>
        {stoppedNoticeText(name)}
      </p>
      {onAllowBack && (
        <Button variant="secondary" style={{ marginTop: 8 }} onClick={onAllowBack}>
          Allow back on
        </Button>
      )}
    </div>
  );
}
