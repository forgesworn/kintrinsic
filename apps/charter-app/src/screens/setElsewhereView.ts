import { useMemo } from "react";
import type { Child } from "../domain/types";
import { setElsewhere, type SetElsewhere } from "../domain/receivedRules";
import { ruleInForceIn } from "../keys/guardianKeys";
import { useCharter } from "../store/store";
import { myRootOf, ruleSetterName } from "./guardiansView";
import type { SectionId } from "./Limits";

// Which rules a page signs, and which of them were set on another phone in a
// way this phone can't show (decision 3, lossy seeding). See SetElsewhere.tsx.

/** The clause kinds each rule page signs. */
export const SECTION_KINDS: Record<SectionId, readonly string[]> = {
  schedule: ["schedule"],
  budget: ["budget"],
  // App allowances compile into learning, buckets and the apps clause.
  "named-times": ["learning", "buckets", "apps"],
  listening: ["listening"],
  web: ["content"],
  // The apps page also holds the singled-out games and apps.
  apps: ["apps", "apprules"],
  "always-available": ["alwaysavailable"],
  lifeline: ["lifeline"],
  tethering: ["tethering"],
  connections: ["connections"],
};

/** The rules among `kinds` set on another phone that this phone can't show, with who set them. */
export function useSetElsewhere(child: Child, kinds: readonly string[]): { rules: (SetElsewhere & { name: string })[]; names: string[] } {
  const { state, guardianKeys } = useCharter();
  const keys = guardianKeys.state;
  const me = myRootOf(guardianKeys);
  return useMemo(() => {
    const subject = child.rosterId;
    if (!subject) return { rules: [], names: [] };
    const rules = setElsewhere(child, kinds, (k, m) => ruleInForceIn(keys, subject, k, m)).map((r) => ({
      ...r,
      name: ruleSetterName(r.author, keys, state.children, me),
    }));
    return { rules, names: [...new Set(rules.map((r) => r.name))] };
  }, [child, keys, state.children, me, kinds]);
}

/** The kinds the times editor signs: the time itself, and the hours it runs. */
export const TIME_KINDS: readonly string[] = ["times", "schedule"];

/** The kinds a singled-out game or app's page signs. */
export const APP_RULE_KINDS: readonly string[] = ["apprules"];

/** Every kind a rule page signs, for the rules list. */
export const ALL_RULE_KINDS: readonly string[] = [...new Set([...Object.values(SECTION_KINDS).flat(), ...TIME_KINDS, ...APP_RULE_KINDS])];
