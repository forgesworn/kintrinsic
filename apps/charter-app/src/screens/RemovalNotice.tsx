import { useEffect, useState } from "react";
import { Banner, Button } from "../components/ui";
import { useCharter } from "../store/store";
import { durationWords, familyWards, goGuardians, myRootOf, namesList, parentName, removalsAgainstMe } from "./guardiansView";

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
 * the notice says so and offers only the way out by choice: leaving. Shown
 * while a device's current report still lists the request.
 */
export default function RemovalNotice() {
  const { state, guardianKeys } = useCharter();
  const now = useNowUnix();
  const keys = guardianKeys.state;
  const against = removalsAgainstMe(familyWards(state.children), keys, myRootOf(guardianKeys), now);
  if (against.length === 0) return null;
  return (
    <div className="stack" style={{ marginBottom: 12 }}>
      {against.map((r) => (
        <Banner key={r.d} tone="warn">
          {r.remover ? parentName(r.remover, keys) : "Another parent"} asked to remove you from{" "}
          {namesList(r.children)}'s devices.{" "}
          {r.objected
            ? "We've objected on your behalf, so you stay a guardian."
            : r.remover
              ? "This phone objects for you as soon as it reaches the device."
              : "This phone can't tell which parent asked, so it can't object for you."}
          {!r.objected &&
            r.takesEffectAt !== undefined &&
            (r.takesEffectAt > now
              ? ` Without an objection it can take effect in ${durationWords(r.takesEffectAt - now)}, once they confirm in person at the child's device.`
              : " Without an objection it takes effect as soon as they confirm in person at the child's device.")}
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
