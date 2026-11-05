import { Button } from "../components/ui";
import { useCharter } from "../store/store";
import { KEY_BACKUP_ANCHOR } from "./KeyBackupNudge";
import { goGuardians, PHONE_JOIN_DONE } from "./guardiansView";

/**
 * For a parent who joined with this phone only: what was made, what to back
 * up, and the way to move the identity to My Signet later (decision D5).
 * Shows nothing for any other install.
 */
export default function PhoneJoinNote() {
  const { guardianKeys } = useCharter();
  if (guardianKeys.state.root?.via !== "phone") return null;
  return (
    <div className="stack" style={{ gap: 8, marginTop: 8 }}>
      <p className="card-sub" style={{ margin: 0 }}>
        {PHONE_JOIN_DONE}
      </p>
      <Button
        variant="secondary"
        block
        onClick={() => {
          window.location.hash = `/settings/${KEY_BACKUP_ANCHOR}`;
        }}
      >
        Back up my guardian identity
      </Button>
      <Button variant="ghost" block onClick={() => goGuardians("move")}>
        Move my identity to My Signet
      </Button>
    </div>
  );
}
