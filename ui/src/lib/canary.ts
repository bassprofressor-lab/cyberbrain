import type { CanaryReport } from "@/api/types";
import type { State } from "@/components/design";

/**
 * How the decoy card reads. No decoys is not a fault, only nothing to watch (idle). A decoy
 * that was used is the one thing on this card that needs a person (danger); decoys that sit
 * untouched are the card working (ok).
 */
export function canaryState(r: Pick<CanaryReport, "summary">): State {
  if (r.summary.count === 0) return "idle";
  return r.summary.hits > 0 ? "danger" : "ok";
}
