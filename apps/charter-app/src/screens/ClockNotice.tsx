import { Banner } from "../components/ui";
import { useCharter } from "../store/store";
import { clockBehindLine, clockBehindWards, familyWards } from "./guardiansView";

/**
 * A device of this child took a rule dated ahead of its own clock (its STATUS
 * clamp counter rose, decision D1). Nothing is refused; it only means "newest
 * wins" follows the order rules arrive in, so the parent is told plainly.
 */
export default function ClockNotice({ childId }: { childId: string }) {
  const { state, guardianKeys } = useCharter();
  const now = Math.floor(Date.now() / 1000);
  const behind = clockBehindWards(familyWards(state.children.filter((c) => c.id === childId)), guardianKeys.state, now);
  if (behind.length === 0) return null;
  return (
    <div className="stack" style={{ marginBottom: 12 }}>
      {behind.map((w) => (
        <Banner key={w.device.id} tone="info">
          {clockBehindLine(w.device.label)}
        </Banner>
      ))}
    </div>
  );
}
