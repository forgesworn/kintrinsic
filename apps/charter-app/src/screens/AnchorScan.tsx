import { useState } from "react";
import { Button } from "../components/ui";
import { QRScanner } from "../components/QRScanner";
import { useCharter } from "../store/store";
import { parseGuardianKey } from "../keys/guardianKeys";
import { stripNostrPrefix } from "./guardiansView";

/**
 * Scan (or paste) another parent's guardian code, and anchor it: the root
 * whose family list this phone trusts (docs/guardian-keys-open-questions.md,
 * "Anchoring a joining parent's trust"). A wrong or garbled code shows why.
 */
export function AnchorScanFields({ onAnchored }: { onAnchored?: (root: string) => void }) {
  const { guardianKeys } = useCharter();
  const [scanning, setScanning] = useState(false);
  const [code, setCode] = useState("");
  const [error, setError] = useState<string | null>(null);

  const take = (text: string): boolean => {
    try {
      const root = guardianKeys.anchorRoot(stripNostrPrefix(text));
      setError(null);
      setScanning(false);
      onAnchored?.(root);
      return true;
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e));
      return false;
    }
  };

  return (
    <>
      {scanning ? (
        <>
          <QRScanner
            active={scanning}
            // A stray QR keeps the camera scanning, without an error each frame.
            onScan={(data) => (parseGuardianKey(stripNostrPrefix(data)) ? take(data) : false)}
          />
          <Button variant="ghost" block onClick={() => setScanning(false)}>
            Stop scanning
          </Button>
        </>
      ) : (
        <Button block onClick={() => setScanning(true)}>
          Scan the other parent's guardian code
        </Button>
      )}
      <label className="field">
        <span className="field-label">Or paste their guardian code</span>
        <input
          className="input"
          placeholder="npub1…"
          value={code}
          autoCapitalize="none"
          autoCorrect="off"
          spellCheck={false}
          onChange={(e) => {
            setCode(e.target.value);
            setError(null);
          }}
        />
      </label>
      {error && (
        <p role="alert" className="card-sub" style={{ margin: 0, color: "var(--danger, #B23B30)" }}>
          {error}
        </p>
      )}
      <Button variant="secondary" block disabled={!code.trim()} onClick={() => take(code)}>
        Use this code
      </Button>
    </>
  );
}
