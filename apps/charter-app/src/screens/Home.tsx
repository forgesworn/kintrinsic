import { UnclaimedPhones } from "../components/RecoveryBanners";
import Onboarding from "./Onboarding";
import KeyBackupNudge from "./KeyBackupNudge";
import CarrierDownload from "../carrier/CarrierDownload";

// Home — the getting-started page, for a family with no children yet. Once
// there is a child there is no Home: the app opens on the first child's page
// (see routes.ts), and the family-wide notices below sit at the top of every
// child page (FamilyNotices). Top to bottom:
//
//   1. getting started (until a computer is connected)
//   2. recovery nudges, only while they hold: key backup, an unclaimed phone
//   3. the app download nudge
//
// The model is device-only (Stage 0/1): a child is a name + a colour + a
// governed computer. There is no child account yet — requests come FROM the
// device, and the guardian approves specific apps one at a time (Stage 1).

export default function Home() {
  return (
    <>
      {/* Getting started — self-hides once a computer is connected. */}
      <Onboarding />

      {/* Things that mean the family is quietly not working. Each renders only
          while it holds, so a healthy household sees none of them. */}
      <KeyBackupNudge />
      <UnclaimedPhones />

      <CarrierDownload />
    </>
  );
}
