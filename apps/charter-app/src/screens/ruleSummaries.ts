import type { Policy } from "../domain/types";
import { decompose } from "../domain/namedTimes";
import {
  alwaysAvailableSummary,
  appsSummary,
  budgetSummary,
  buildAlwaysAvailable,
  buildApps,
  buildBudget,
  buildLifeline,
  buildListening,
  buildSchedule,
  buildTethering,
  buildWeb,
  lifelineSummary,
  listeningSummary,
  namedTimesSummary,
  scheduleSummary,
  tetheringSummary,
  webSummary,
  type SectionId,
} from "./Limits";

/**
 * The one-line summary each row shows, computed from the SAVED policy with the
 * same helpers the Limits accordion uses. Where Limits summarises its draft,
 * a clean draft equals the saved policy, so the two agree.
 */
export function ruleSummaries(policy: Policy, nowUnix: number): Record<SectionId, string> {
  return {
    schedule: scheduleSummary(buildSchedule(policy.schedule), policy.schedule !== undefined),
    budget: budgetSummary(buildBudget(policy.budget)),
    "named-times": namedTimesSummary(
      decompose(
        { learning: policy.learning, buckets: policy.buckets, apps: policy.apps },
        policy.freeGroups,
      ).groups,
    ),
    listening: listeningSummary(buildListening(policy.listening)),
    "always-available": alwaysAvailableSummary(buildAlwaysAvailable(policy.alwaysAvailable), nowUnix),
    web: webSummary(buildWeb(policy.web)),
    apps: appsSummary(buildApps(policy.apps), nowUnix),
    lifeline: lifelineSummary(buildLifeline(policy.lifeline)),
    tethering: tetheringSummary(buildTethering(policy.tethering)),
  };
}
