import { Button } from "../components/ui";

/**
 * How a parent turns on parent approval.
 *
 * "Set up on this phone" is the plain, recommended choice: the key is made and
 * kept right here, with no other app. My Signet and Heartwood (a Heartwood is
 * reached through My Signet: the same bunker link) are for the few people who
 * already run one of them, so they sit under a collapsed "Other ways" section
 * instead of competing for the same attention. Either becomes the guardian
 * root and approves this phone's working key once.
 */
export default function ApprovalSetupChoices({
  busy,
  onLocal,
  onSignet,
  onHeartwood,
}: {
  busy: boolean;
  onLocal: () => void;
  onSignet: () => void;
  onHeartwood: () => void;
}) {
  return (
    <div className="stack">
      <Button block disabled={busy} onClick={onLocal}>
        Set up on this phone
      </Button>
      <details>
        <summary style={{ cursor: "pointer", fontWeight: 600 }}>
          Other ways (for people who already use Signet or Heartwood)
        </summary>
        <div className="stack" style={{ marginTop: 12 }}>
          <p className="card-sub" style={{ margin: 0 }}>
            My Signet is a separate app that keeps your guardian identity.
            It approves this phone once, and this phone then signs for you.
          </p>
          <Button variant="secondary" block disabled={busy} onClick={onSignet}>
            Set up with Signet
          </Button>
          <p className="card-sub" style={{ margin: 0 }}>
            Heartwood keeps your guardian identity on a separate device,
            reached through My Signet. Pick it only if you already have one
            set up.
          </p>
          <Button variant="secondary" block disabled={busy} onClick={onHeartwood}>
            Set up with Heartwood
          </Button>
        </div>
      </details>
    </div>
  );
}
