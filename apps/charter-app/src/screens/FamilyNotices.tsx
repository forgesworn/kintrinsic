import { UnclaimedPhones } from "../components/RecoveryBanners";
import Onboarding from "./Onboarding";
import KeyBackupNudge from "./KeyBackupNudge";
import AnsweredDelivery from "./AnsweredDelivery";

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
      <AnsweredDelivery childId={childId} />
    </>
  );
}
