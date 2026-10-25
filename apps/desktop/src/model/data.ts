// What data may leave for which AI provider (ADR-PROVIDER-002), in plain
// words. The Host decides and enforces; these only say what it returned.

import type { ConnectionDataView, DataChange, DataPolicyView } from "@nexus/contracts";

/** What a connection may receive. */
export function receiveText(canReceive: string): string {
  switch (canReceive) {
    case "Business":
      return "Your own data and other people's";
    case "Personal":
      return "Your own data, not other people's";
    default:
      return "Only public data";
  }
}

/** Whether a change lets more data leave, and so goes through the Windows
 *  dialog (the Host refuses it from the window). */
export function letsMoreLeave(now: DataPolicyView, change: DataChange): boolean {
  if ("InEea" in change) return change.InEea.in_eea && !now.in_eea;
  if ("Connection" in change) {
    const c = now.connections.find((x) => x.connection_id === change.Connection.connection_id);
    if (!c) return false;
    return (change.Connection.dpa_signed && !c.dpa_signed) || (change.Connection.allows_training && !c.allows_training);
  }
  const s = now.sources.find((x) => x.kind === change.Source.kind && x.id === change.Source.id);
  return change.Source.data_class === "Personal" && s?.data_class === "Business";
}

/** Consent to training is offered where it changes something: a
 *  connection that may receive only public data, or one already given it. */
export function offersConsent(c: ConnectionDataView): boolean {
  return c.can_receive === "Public" || c.allows_training;
}
