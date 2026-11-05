import { useEffect, useState } from "react";
import { Banner, Button } from "../components/ui";
import { useCharter } from "../store/store";
import { bunkerRoot } from "../keys/state";
import {
  durationWords,
  familyWards,
  goGuardians,
  myRootOf,
  namesList,
  parentName,
  removalsAgainstMe,
  type RemovalAgainstMe,
} from "./guardiansView";

function useNowUnix(): number {
  const [now, setNow] = useState(() => Math.floor(Date.now() / 1000));
  useEffect(() => {
    const t = setInterval(() => setNow(Math.floor(Date.now() / 1000)), 30_000);
    return () => clearInterval(t);
  }, []);
  return now;
}

/**
 * Another parent asked to remove this one (decision Q1). This phone objects
 * on its own as soon as a device reports the request (`noteWardStatus`), so
 * the notice says so and offers the way out by choice: leaving. Where it
 * can't object on its own (it can't tell who asked, or a device hasn't taken
 * this phone's day-to-day key yet), it says why and offers an Object button.
 * Shown while a device's current report still lists the request.
 */
export default function RemovalNotice() {
  const { state, guardianKeys } = useCharter();
  const now = useNowUnix();
  const keys = guardianKeys.state;
  const against = removalsAgainstMe(familyWards(state.children), keys, myRootOf(guardianKeys), now, guardianKeys.workingPubkey);
  if (against.length === 0) return null;
  return (
    <div className="stack" style={{ marginBottom: 12 }}>
      {against.map((r) => (
        <Banner key={r.d} tone="warn">
          {r.remover ? parentName(r.remover, keys) : "Another parent"} asked to remove you from{" "}
          {namesList(r.children)}'s devices.{" "}
          {r.objected
            ? "We've objected on your behalf, so you stay a guardian."
            : r.blocked === "remover"
              ? "This phone can't tell which parent asked, so it couldn't object for you on its own."
              : r.blocked === "key"
                ? "Your devices haven't taken this phone's day-to-day key yet, so it couldn't object for you on its own."
                : "This phone objects for you as soon as it reaches the device."}
          {!r.objected &&
            r.takesEffectAt !== undefined &&
            (r.takesEffectAt > now
              ? ` Without an objection it can take effect in ${durationWords(r.takesEffectAt - now)}, once they confirm in person at the child's device.`
              : " Without an objection it takes effect as soon as they confirm in person at the child's device.")}
          {r.blocked && <ObjectNow removal={r} />}
          <div style={{ marginTop: 8 }}>
            <Button variant="ghost" onClick={() => goGuardians("leave")}>
              Leave the family instead
            </Button>
          </div>
        </Banner>
      ))}
    </div>
  );
}

/** The manual objection: signed by this phone's identity (My Signet asks first, where it holds it). */
function ObjectNow({ removal }: { removal: RemovalAgainstMe }) {
  const { guardianKeys } = useCharter();
  const keys = guardianKeys.state;
  const [busy, setBusy] = useState(false);
  const [note, setNote] = useState<string | null>(null);
  const prompt = bunkerRoot(keys) ? ` ${keys.root?.via === "heartwood" ? "Your Heartwood" : "My Signet"} will ask you to approve it.` : "";
  async function object(remover?: string) {
    setBusy(true);
    setNote(null);
    try {
      const report = await guardianKeys.objectToRemoval(removal.d, remover);
      setNote(
        report.sent.length + report.held.length > 0
          ? "Objection sent. You stay a guardian at each device once it has it."
          : "The objection couldn't be sent yet. Try again when you're online.",
      );
    } catch (e) {
      setNote(e instanceof Error ? e.message : String(e));
    } finally {
      setBusy(false);
    }
  }
  // Who asked must be named in the objection: one button per other parent when it isn't known.
  const choices = removal.remover ? [removal.remover] : removal.candidates;
  return (
    <div className="stack" style={{ marginTop: 8, gap: 6 }}>
      <span>
        {choices.length > 1 ? "Choose who asked, to object now." : "You can object now."}
        {prompt}
      </span>
      {choices.map((c) => (
        <Button key={c} disabled={busy} onClick={() => void object(c)}>
          {busy ? "Objecting…" : choices.length > 1 ? `Object: ${parentName(c, keys)} asked` : "Object"}
        </Button>
      ))}
      {note && <span role="status">{note}</span>}
    </div>
  );
}
