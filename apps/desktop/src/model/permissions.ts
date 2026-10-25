// How each tool's permission reads (ADR-POLICY-001). The Host decides and
// enforces; these only say in plain words what it returned.

import type { PermissionView } from "@nexus/contracts";

/** What the tool does now, as the user reads it. */
export function permissionText(p: PermissionView): string {
  if (!p.enabled) return "Off: Nexus is never offered this tool";
  if (p.max_auto !== null) {
    return p.max_auto === 0
      ? "Asks you before every change; a step's changes wait together"
      : `Up to ${p.max_auto} changes per reply without asking; the rest wait for you together`;
  }
  if (p.effect_class === "Read") return "Runs without asking: it only reads";
  if (p.effect_class === "ExternalReversible" || p.effect_class === "ExternalIrreversible") {
    return "Asks you each time; Windows asks you to confirm";
  }
  return p.decision === "Ask" ? "Asks you each time" : "Runs without asking";
}

/** Whether a change loosens the policy, and so goes through the Windows
 *  dialog: a tool back on, or more register writes without asking. */
export function loosens(p: PermissionView, enabled: boolean, maxAuto: number | null): boolean {
  return (enabled && !p.enabled) || (enabled && maxAuto !== null && p.max_auto !== null && maxAuto > p.max_auto);
}
