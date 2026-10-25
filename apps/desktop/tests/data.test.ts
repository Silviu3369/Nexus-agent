import { test } from "node:test";
import assert from "node:assert/strict";
import type { DataPolicyView } from "@nexus/contracts";
import { letsMoreLeave, offersConsent, receiveText } from "../src/model/data.ts";

function policy(): DataPolicyView {
  return {
    in_eea: false,
    terms_checked: "2026-10-25",
    connections: [
      {
        connection_id: "c-1",
        display_name: "Gemini",
        provider: "Gemini",
        free_tier: true,
        can_receive: "Public",
        reason: "",
        terms_url: "",
        dpa: "Included",
        dpa_signed: false,
        allows_training: false,
      },
    ],
    sources: [{ kind: "Folder", id: "f-1", name: "Clients", data_class: "Business" }],
  };
}

test("what a connection may receive reads in plain words", () => {
  assert.equal(receiveText("Business"), "Your own data and other people's");
  assert.equal(receiveText("Personal"), "Your own data, not other people's");
  assert.equal(receiveText("Public"), "Only public data");
});

test("letting more data leave goes through Windows, keeping more in does not", () => {
  const now = policy();
  assert.equal(letsMoreLeave(now, { InEea: { in_eea: true } }), true);
  assert.equal(letsMoreLeave(now, { InEea: { in_eea: false } }), false);
  assert.equal(letsMoreLeave(now, { Connection: { connection_id: "c-1", dpa_signed: false, allows_training: true } }), true);
  assert.equal(letsMoreLeave(now, { Source: { kind: "Folder", id: "f-1", data_class: "Personal" } }), true);
  assert.equal(letsMoreLeave(now, { Source: { kind: "Folder", id: "f-1", data_class: "Business" } }), false);
  assert.equal(offersConsent(now.connections[0]), true, "a public-only connection can be given consent");
  assert.equal(offersConsent({ ...now.connections[0], can_receive: "Personal" }), false);
});
