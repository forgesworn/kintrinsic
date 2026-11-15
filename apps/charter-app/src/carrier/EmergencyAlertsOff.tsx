import { useEffect, useState } from "react";
import { Banner, Button } from "../components/ui";
import {
  emergencyAlertsOff,
  openEmergencyAlertSettings,
  readEmergencyAlertState,
} from "./emergencyAlerts";

// A persistent warning on the console's home screen when the phone will not
// tell the guardian that a ward used an emergency button. Nothing at all is
// shown in a browser tab or with a shell too old to report it. The guardian
// fixes it in system settings and comes back, so it is checked again whenever
// the page becomes visible.

export default function EmergencyAlertsOff() {
  const [off, setOff] = useState(() => emergencyAlertsOff(readEmergencyAlertState()));

  useEffect(() => {
    const check = () => setOff(emergencyAlertsOff(readEmergencyAlertState()));
    const onVisible = () => {
      if (document.visibilityState === "visible") check();
    };
    check();
    document.addEventListener("visibilitychange", onVisible);
    return () => document.removeEventListener("visibilitychange", onVisible);
  }, []);

  if (!off) return null;
  return (
    <div className="stack" style={{ marginBottom: 12 }}>
      <Banner tone="warn">
        <p style={{ margin: 0 }} role="alert">
          Emergency alerts are off on this phone. Turn them on so you&rsquo;re told when your ward
          uses an emergency button.
        </p>
        <div style={{ marginTop: 8 }}>
          <Button variant="secondary" onClick={() => openEmergencyAlertSettings()}>
            Turn on emergency alerts
          </Button>
        </div>
      </Banner>
    </div>
  );
}
