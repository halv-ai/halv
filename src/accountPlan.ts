import type { HalvPlan } from "./types";

export function accountPlanLabel(plan: HalvPlan | null | undefined) {
  switch (plan) {
    case "beta_free":
      return "Beta Free";
    case "basic":
      return "Basic";
    case "lifetime":
      return "Lifetime";
    case "unlimited":
      return "Unlimited";
    default:
      return "Unknown";
  }
}
