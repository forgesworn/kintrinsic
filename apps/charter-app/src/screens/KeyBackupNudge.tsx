import { useEffect, useState } from "react";
import { Banner, Button } from "../components/ui";
import { useCharter } from "../store/store";
import { guardianPubkeyHex } from "../signer/guardianKey";
import {
  BACKUP_STATUS_EVENT,
  dismissBackupNudge,
  isBackupNudgeDismissed,
  isKeyBackedUp,
} from "../signer/backupStatus";

/** The element id of the "Back up your guardian key" section on Settings. */
export const KEY_BACKUP_ANCHOR = "key-backup";

function backedUp(): boolean {
  try {
    return isKeyBackedUp(guardianPubkeyHex());
  } catch {
    // No readable key yet: say nothing rather than nag about something we
    // cannot back up.
    return true;
  }
}

/**
 * A reminder, near the top of Home and Settings, to save a backup of the parent key.
 *
 * Only for the key that lives on this phone (parent approval "on this phone"):
 * with Signet or Heartwood the key is held elsewhere and there is nothing here
 * to back up. Shown until a backup has been exported; "Not now" hides it for
 * the rest of the session only.
 */
export default function KeyBackupNudge() {
  const { state } = useCharter();
  const [, bump] = useState(0);

  useEffect(() => {
    const refresh = () => bump((n) => n + 1);
    window.addEventListener(BACKUP_STATUS_EVENT, refresh);
    return () => window.removeEventListener(BACKUP_STATUS_EVENT, refresh);
  }, []);

  const { connected, kind } = state.signer;
  if (!connected || kind !== "local") return null;
  if (isBackupNudgeDismissed() || backedUp()) return null;

  return (
    <div style={{ marginBottom: 16 }}>
      <Banner tone="warn">
        <div className="stack" style={{ width: "100%" }}>
          <span>
            Save a backup of your parent key — without it, a lost phone means
            setting every device up again.
          </span>
          <div className="row-between" style={{ gap: 8 }}>
            <Button
              block
              onClick={() => {
                const el = document.getElementById(KEY_BACKUP_ANCHOR);
                // Off the page that holds the section (Home shows this nudge
                // too): go to it, and Settings scrolls it into view.
                if (el) el.scrollIntoView?.({ behavior: "smooth", block: "start" });
                else window.location.hash = `/settings/${KEY_BACKUP_ANCHOR}`;
              }}
            >
              Back up now
            </Button>
            <Button variant="ghost" block onClick={dismissBackupNudge}>
              Not now
            </Button>
          </div>
        </div>
      </Banner>
    </div>
  );
}
