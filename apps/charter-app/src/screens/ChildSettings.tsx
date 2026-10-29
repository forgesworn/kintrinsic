import { SectionLabel } from "../components/ui";
import type { Child } from "../domain/types";
import { ChildRules } from "./RuleEditor";
import { ChildDevices, ChildDeviceSettings, ChildManage } from "./ChildAdmin";

// A child's settings (`#/ward/<id>/settings`): what is set once and changed
// rarely. The child page keeps the daily things; this page is the rest.
//
//   Rules -> Devices (each row opens that device's own page)
//   -> child-wide device settings -> Manage (rename, colour, remove)
//
// Each section keeps its id so a link (and the "devices" scroll after set-up)
// can land on it.

export default function ChildSettings({ child }: { child: Child }) {
  return (
    <>
      <section id="rules" className="scroll-anchor" aria-label="Rules">
        <SectionLabel>Rules</SectionLabel>
        <ChildRules child={child} />
      </section>

      <section id="devices" className="scroll-anchor" aria-label="Devices">
        <SectionLabel>Devices</SectionLabel>
        <ChildDevices child={child} />
      </section>

      <ChildDeviceSettings child={child} />

      <section id="manage" className="scroll-anchor" aria-label="Manage">
        <ChildManage child={child} />
      </section>
    </>
  );
}
