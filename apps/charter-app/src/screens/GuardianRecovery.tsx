import { useState } from "react";
import { Banner, Button, Card } from "../components/ui";
import { useCharter } from "../store/store";
import { clearGuardianKey } from "../signer/guardianKey";

// Shown full-screen when the guardian key stored on this phone is PRESENT but
// undecodable (see `readGuardianKey`). The app must NOT boot past this: every
// screen reads the key on its render path, and — the whole reason this exists —
// a corrupt value must never be silently overwritten with a fresh, unpaired
// identity. The one safe exit is restoring the encrypted backup, which brings
// the SAME key (and therefore every existing pairing) back with no re-pairing.
export default function GuardianRecovery({ onResolved }: { onResolved: () => void }) {
  const { restoreGuardianKey } = useCharter();
  const [blob, setBlob] = useState("");
  const [pass, setPass] = useState("");
  const [error, setError] = useState<string | null>(null);
  const [working, setWorking] = useState(false);
  const [confirmFresh, setConfirmFresh] = useState(false);

  async function doRestore() {
    setError(null);
    setWorking(true);
    try {
      // Lazy-load the crypto (mirrors Family's backup flow) — off the boot path.
      const { decryptGuardianKey } = await import("../signer/keyBackup");
      const secret = await decryptGuardianKey(blob.trim(), pass);
      await restoreGuardianKey(secret); // overwrites the corrupt value with the
      onResolved(); //                     restored one, then reconnects.
    } catch (e) {
      setError(e instanceof Error ? e.message : "Couldn't restore that backup.");
    } finally {
      setWorking(false);
    }
  }

  function startFresh() {
    // No backup, and the parent accepts losing the pairings. We CLEAR the
    // corrupt value (never overwrite it in place): the next "Set up on this
    // phone" mints a fresh identity — a deliberate, eyes-open choice, not the
    // silent orphaning this screen exists to prevent.
    clearGuardianKey();
    onResolved();
  }

  return (
    <div className="app-shell">
      <header className="app-header">
        <h1 className="app-title">Restore your guardian key</h1>
      </header>

      <main className="app-main">
        <Card>
          <Banner tone="warn">
            The guardian key saved on this phone couldn't be read — it looks
            corrupted. Don't set up again yet: that would mint a brand-new
            identity and unpair every device. Restore your encrypted backup to
            bring the same key — and every pairing — back.
          </Banner>

          <div className="stack" style={{ marginTop: 16 }}>
            <p className="card-sub" style={{ marginTop: 2 }}>
              Paste your backup and its passphrase. This restores the key on this
              phone; your paired devices come back with no re-pairing.
            </p>
            <textarea
              className="input"
              rows={3}
              placeholder="CHARTER-KEYBAK.1.…"
              aria-label="Backup blob"
              style={{ fontFamily: "monospace", fontSize: 11 }}
              value={blob}
              onChange={(e) => setBlob(e.target.value)}
            />
            <input
              className="input"
              type="password"
              placeholder="Passphrase"
              aria-label="Restore passphrase"
              value={pass}
              onChange={(e) => setPass(e.target.value)}
            />
            {error && <Banner tone="warn">{error}</Banner>}
            <Button block disabled={working || !blob.trim() || !pass} onClick={doRestore}>
              {working ? "Restoring…" : "Restore key"}
            </Button>
          </div>

          <div className="stack" style={{ marginTop: 24 }}>
            {!confirmFresh ? (
              <Button
                variant="ghost"
                block
                disabled={working}
                onClick={() => setConfirmFresh(true)}
              >
                I don't have a backup
              </Button>
            ) : (
              <>
                <Banner tone="warn">
                  Starting over abandons the old guardian identity for good. Every
                  device you've paired will need to be paired again. Only do this
                  if your backup is truly gone.
                </Banner>
                <Button variant="danger" block disabled={working} onClick={startFresh}>
                  Start over without my devices
                </Button>
                <Button
                  variant="ghost"
                  block
                  disabled={working}
                  onClick={() => setConfirmFresh(false)}
                >
                  Keep trying to restore
                </Button>
              </>
            )}
          </div>
        </Card>
      </main>
    </div>
  );
}
