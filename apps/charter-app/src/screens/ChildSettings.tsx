import { SectionLabel } from "../components/ui";
import type { Child } from "../domain/types";
import { ChildRules } from "./RuleEditor";
import { ChildDevices, ChildDeviceSettings, ChildManage } from "./ChildAdmin";
import { TimesSection } from "./Times";

// A child's settings (`#/ward/<id>/settings`): what is set once and changed
// rarely. The child page keeps the daily things; this page is the rest.
//
//   Times -> Free time's rules -> Devices (each row opens that device's own page)
//   -> child-wide device settings -> Manage (rename, colour, remove)
//
// Each section keeps its id so a link (and the "devices" scroll after set-up)
// can land on it.

export default function ChildSettings({ child }: { child: Child }) {
  return (
    <>
      <section id="times" className="scroll-anchor" aria-label="Times">
        <SectionLabel>Times</SectionLabel>
        <TimesSection child={child} />
      </section>

      {/* The Rules ARE Free time's. A named time starts from them and departs
          where it says so, so a ban added here next month reaches every time
          that inherits Free time's bans without being re-entered. */}
      <section id="rules" className="scroll-anchor" aria-label="Free time’s rules">
        <SectionLabel>Free time’s rules</SectionLabel>
        <p className="card-sub" style={{ margin: "0 4px 10px" }}>
          What’s banned in Free time. Other times start from these and change what
          they need to. Single-app limits, Always available, Lifeline and Hotspot apply in every time.
        </p>
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
