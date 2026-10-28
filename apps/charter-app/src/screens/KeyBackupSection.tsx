import { useState } from "react";
import { Banner, Button } from "../components/ui";
import { useCharter } from "../store/store";
import { guardianPubkeyHex } from "../signer/guardianKey";
import { markKeyBackedUp } from "../signer/backupStatus";

/**
 * Back up (and restore) the guardian key. This is the ONE piece of recovery a
 * family can't do without: the guardian secret lives only in this browser, so
 * a lost phone would orphan every paired device. The backup is passphrase-
 * encrypted; restoring the same key on a new device revives every pairing with
 * no re-pairing (the devices pinned the guardian PUBKEY, which is unchanged).
 */
export default function KeyBackupSection({ busy }: { busy: boolean }) {
  const { restoreGuardianKey } = useCharter();
  const [mode, setMode] = useState<null | "backup" | "restore">(null);
  const [pass, setPass] = useState("");
  const [pass2, setPass2] = useState("");
  const [blob, setBlob] = useState("");
  const [restoreBlob, setRestoreBlob] = useState("");
  const [error, setError] = useState<string | null>(null);
  const [ok, setOk] = useState<string | null>(null);
  const [working, setWorking] = useState(false);

  function reset() {
    setMode(null);
    setPass("");
    setPass2("");
    setBlob("");
    setRestoreBlob("");
    setError(null);
    setOk(null);
  }

  async function makeBackup() {
    setError(null);
    if (pass.length < 8) {
      setError("Use a passphrase of at least 8 characters.");
      return;
    }
    if (pass !== pass2) {
      setError("The two passphrases don't match.");
      return;
    }
    setWorking(true);
    try {
      const { encryptGuardianBackup } = await import("../signer/keyBackup");
      const { loadOrCreateGuardianKey } = await import("../signer/guardianKey");
      const { exportPersistedState } = await import("../store/store");
      setBlob(await encryptGuardianBackup(loadOrCreateGuardianKey(), exportPersistedState(), pass));
    } catch (e) {
      setError(e instanceof Error ? e.message : "Couldn't make a backup.");
    } finally {
      setWorking(false);
    }
  }

  // A backup only counts once it has left the screen: copied or downloaded.
  // Making the blob alone is not enough (the parent may close the page).
  function noteExported() {
    try {
      markKeyBackedUp(guardianPubkeyHex());
    } catch {
      // No readable key: nothing to record.
    }
  }

  function copyBackup() {
    navigator.clipboard?.writeText(blob);
    noteExported();
  }

  function downloadBackup() {
    noteExported();
    const file = new Blob([blob], { type: "text/plain" });
    const url = URL.createObjectURL(file);
    const a = document.createElement("a");
    a.href = url;
    a.download = "charter-guardian-backup.txt";
    a.click();
    URL.revokeObjectURL(url);
  }

  async function doRestore() {
    setError(null);
    setWorking(true);
    try {
      const { decryptGuardianBackup } = await import("../signer/keyBackup");
      const backup = await decryptGuardianBackup(restoreBlob, pass);
      const pk = await restoreGuardianKey(backup.secret);
      // They hold a backup of exactly this key — no reminder needed.
      markKeyBackedUp(pk);
      if (backup.stateJson) {
        // The v2 blob carries the household — install it and boot from it.
        const { importPersistedState } = await import("../store/store");
        importPersistedState(backup.stateJson);
        setOk(`Restored guardian ${pk.slice(0, 8)}… and your family — reloading.`);
        setTimeout(() => window.location.reload(), 1500);
      } else {
        setOk(`Restored guardian ${pk.slice(0, 8)}…. Your paired devices are back.`);
      }
      setRestoreBlob("");
      setPass("");
    } catch (e) {
      setError(e instanceof Error ? e.message : "Couldn't restore that backup.");
    } finally {
      setWorking(false);
    }
  }

  if (mode === null) {
    return (
      <div className="stack" style={{ marginTop: 16 }}>
        <div style={{ fontWeight: 600 }}>Back up your guardian key</div>
        <p className="card-sub" style={{ marginTop: 2 }}>
          Your key and family setup live only on this device. One backup
          carries both — restore it on a new phone or in the Kintrinsic app and
          everything comes back: children, devices, rules, no re-pairing.
        </p>
        <div className="row-between" style={{ gap: 8 }}>
          <Button variant="secondary" block disabled={busy} onClick={() => setMode("backup")}>
            Back up key
          </Button>
          <Button variant="ghost" block disabled={busy} onClick={() => setMode("restore")}>
            Restore from backup
          </Button>
        </div>
      </div>
    );
  }

  if (mode === "backup") {
    return (
      <div className="stack" style={{ marginTop: 16 }}>
        <div style={{ fontWeight: 600 }}>Back up your guardian key</div>
        {!blob ? (
          <>
            <p className="card-sub" style={{ marginTop: 2 }}>
              Choose a passphrase. You'll need it to restore — we can't recover
              it for you, so keep it somewhere safe.
            </p>
            <input
              className="input"
              type="password"
              placeholder="Passphrase (8+ characters)"
              aria-label="Backup passphrase"
              value={pass}
              onChange={(e) => setPass(e.target.value)}
            />
            <input
              className="input"
              type="password"
              placeholder="Repeat passphrase"
              aria-label="Repeat backup passphrase"
              value={pass2}
              onChange={(e) => setPass2(e.target.value)}
            />
            {error && <Banner tone="warn">{error}</Banner>}
            <Button block disabled={working} onClick={makeBackup}>
              {working ? "Encrypting…" : "Create backup"}
            </Button>
            <Button variant="ghost" block onClick={reset}>
              Cancel
            </Button>
          </>
        ) : (
          <>
            <Banner tone="ok">
              Encrypted. Save this somewhere safe — a password manager, or the
              downloaded file. Anyone with it AND your passphrase can act as the
              guardian.
            </Banner>
            <code
              data-backup-blob
              style={{ wordBreak: "break-all", fontSize: 11, opacity: 0.85 }}
            >
              {blob}
            </code>
            <div className="row-between" style={{ gap: 8 }}>
              <Button
                variant="secondary"
                block
                onClick={copyBackup}
              >
                Copy
              </Button>
              <Button variant="secondary" block onClick={downloadBackup}>
                Download .txt
              </Button>
            </div>
            <Button variant="ghost" block onClick={reset}>
              Done
            </Button>
          </>
        )}
      </div>
    );
  }

  // restore
  return (
    <div className="stack" style={{ marginTop: 16 }}>
      <div style={{ fontWeight: 600 }}>Restore your guardian key</div>
      {ok ? (
        <>
          <Banner tone="ok">{ok}</Banner>
          <Button variant="ghost" block onClick={reset}>
            Done
          </Button>
        </>
      ) : (
        <>
          <p className="card-sub" style={{ marginTop: 2 }}>
            Paste your backup and its passphrase. This replaces the key on this
            phone with the backed-up one.
          </p>
          <textarea
            className="input"
            rows={3}
            placeholder="CHARTER-KEYBAK.…"
            aria-label="Backup blob"
            style={{ fontFamily: "monospace", fontSize: 11 }}
            value={restoreBlob}
            onChange={(e) => setRestoreBlob(e.target.value)}
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
          <Button
            block
            disabled={working || !restoreBlob.trim() || !pass}
            onClick={doRestore}
          >
            {working ? "Restoring…" : "Restore key"}
          </Button>
          <Button variant="ghost" block onClick={reset}>
            Cancel
          </Button>
        </>
      )}
    </div>
  );
}
