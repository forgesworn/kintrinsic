import { Banner, Button } from "../components/ui";
import { UnclaimedPhones } from "../components/RecoveryBanners";
import { useCharter } from "../store/store";
import { familyWards, goGuardians, myRootOf, removalsAgainstMe } from "./guardiansView";
import Onboarding from "./Onboarding";
import KeyBackupNudge from "./KeyBackupNudge";
import AnsweredDelivery from "./AnsweredDelivery";

/** A pending removal of this parent: there are 72 hours to object to it. */
function RemovalAgainstYou() {
  const { state, guardianKeys } = useCharter();
  const against = removalsAgainstMe(familyWards(state.children), guardianKeys.state, myRootOf(guardianKeys));
  if (against.length === 0) return null;
  return (
    <div className="stack" style={{ marginBottom: 12 }}>
      <Banner tone="warn">The other parent has asked to remove you as a guardian.</Banner>
      <Button block onClick={() => goGuardians()}>
        Open Guardians to object
      </Button>
    </div>
  );
}

/**
 * The family-wide notices, at the top of each child's page (there is no Home to
 * hold them once there is a child). Each renders only while it holds, so a
 * healthy household sees none of them. The answers shown are this child's own.
 */
export default function FamilyNotices({ childId }: { childId: string }) {
  return (
    <>
      <Onboarding />
      <KeyBackupNudge />
      <UnclaimedPhones />
      <RemovalAgainstYou />
      <AnsweredDelivery childId={childId} />
    </>
  );
}
