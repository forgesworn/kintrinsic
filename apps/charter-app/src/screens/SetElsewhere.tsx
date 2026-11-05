import { useState, type ReactNode } from "react";
import { Button, Card } from "../components/ui";
import type { Child } from "../domain/types";
import { namesList } from "./guardiansView";
import { useSetElsewhere } from "./setElsewhereView";

// A rule another guardian's phone set that this phone's screen can't show as
// it is (decision 3, lossy seeding). The screen stays read-only, says who set
// it, and offers Replace: this parent sets the kind afresh. Until then the
// other phone's clause stays the rule in force, and a re-send sends it as is.

/** "Set from Robin": the read-only stand-in for a rule page, with Replace. */
export function SetElsewhereCard({ child, names, onReplace }: { child: Child; names: string[]; onReplace: () => void }) {
  const who = namesList(names);
  return (
    <Card>
      <p className="card-title" style={{ marginBottom: 6 }}>
        Set from {who}
      </p>
      <p className="card-sub" style={{ marginBottom: 14 }}>
        {who} set this rule in a way this screen can’t show. It stays in force on {child.name}’s devices until you
        replace it.
      </p>
      <Button block onClick={onReplace}>
        Replace
      </Button>
    </Card>
  );
}

/**
 * A rule page, or, while a rule it signs was set on another phone in a way
 * this phone can't show, the read-only card with Replace. Replace opens the
 * page as this phone has it; saving there sets the kind afresh.
 */
export function SetElsewhereGate({ child, kinds, children }: { child: Child; kinds: readonly string[]; children: ReactNode }) {
  const { rules, names } = useSetElsewhere(child, kinds);
  const [replacing, setReplacing] = useState(false);
  if (rules.length === 0 || replacing) return <>{children}</>;
  return <SetElsewhereCard child={child} names={names} onReplace={() => setReplacing(true)} />;
}
