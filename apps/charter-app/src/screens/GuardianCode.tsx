import { useEffect, useState } from "react";
import QRCode from "qrcode";
import { Button, Card } from "../components/ui";
import { useCharter } from "../store/store";
import { npubOf } from "./guardiansView";

/**
 * This parent's guardian code: their guardian identity (npub) as a QR code.
 * A parent joining the family scans it on their own phone (Join a family),
 * which is how their phone knows whose family list to trust. A device's
 * report can't tell it that: anyone can make a presence code name any
 * machine (docs/guardian-keys-open-questions.md, "Anchoring a joining
 * parent's trust").
 */
export function GuardianCodeCard() {
  const { guardianKeys } = useCharter();
  const root = guardianKeys.state.root?.pubkey;
  const npub = root ? npubOf(root) : null;
  const [src, setSrc] = useState<string | null>(null);

  useEffect(() => {
    if (!npub) return;
    let alive = true;
    QRCode.toDataURL(`nostr:${npub}`, { width: 480, margin: 1, errorCorrectionLevel: "M" })
      .then((url) => {
        if (alive) setSrc(url);
      })
      .catch(() => {
        if (alive) setSrc(null);
      });
    return () => {
      alive = false;
    };
  }, [npub]);

  if (!npub) {
    return (
      <p className="card-sub" style={{ margin: 0 }}>
        Your guardian identity is kept on this phone, so there's no guardian code to share. Connect My Signet as your
        guardian identity first.
      </p>
    );
  }
  return (
    <div className="stack" style={{ gap: 8 }}>
      <p className="card-sub" style={{ margin: 0 }}>
        The parent joining your family scans this with their phone (Join a family → Scan the other parent's guardian
        code). Show it to them in person.
      </p>
      {src && (
        <img
          src={src}
          alt="Your guardian code"
          style={{ width: "100%", maxWidth: 280, alignSelf: "center", imageRendering: "pixelated" }}
        />
      )}
      <p style={{ overflowWrap: "anywhere", userSelect: "all", fontSize: 13, margin: 0 }}>{npub}</p>
    </div>
  );
}

/** "Show my guardian code", as its own page. */
export default function GuardianCode({ onBack }: { onBack: () => void }) {
  return (
    <Card>
      <h2 className="card-title">My guardian code</h2>
      <GuardianCodeCard />
      <Button variant="ghost" block onClick={onBack} style={{ marginTop: 12 }}>
        Back
      </Button>
    </Card>
  );
}
