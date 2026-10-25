import { test } from "node:test";
import assert from "node:assert/strict";
import type { PermissionView } from "@nexus/contracts";
import { loosens, permissionText } from "../src/model/permissions.ts";

function tool(extra: Partial<PermissionView>): PermissionView {
  return {
    tool_id: "WebRead",
    label: "Read public web pages",
    effect_class: "Read",
    sends: "the address to the site",
    enabled: true,
    decision: "Auto",
    max_auto: null,
    default_max_auto: null,
    changed_at_ms: null,
    ...extra,
  };
}

test("each tool says in plain words what it does now", () => {
  assert.equal(permissionText(tool({})), "Runs without asking: it only reads");
  assert.equal(permissionText(tool({ enabled: false })), "Off: Nexus is never offered this tool");
  assert.equal(
    permissionText(tool({ effect_class: "ExternalIrreversible", decision: "Ask" })),
    "Asks you each time; Windows asks you to confirm",
  );
  assert.equal(permissionText(tool({ effect_class: "InternalReversible", decision: "Ask" })), "Asks you each time");
  const writes = tool({ effect_class: "InternalReversible", max_auto: 10, default_max_auto: 10 });
  assert.equal(permissionText(writes), "Up to 10 changes per reply without asking; the rest wait for you together");
  assert.equal(permissionText({ ...writes, max_auto: 0 }), "Asks you before every change; a step's changes wait together");
});

test("turning a tool back on or letting more writes run loosens the policy", () => {
  assert.equal(loosens(tool({ enabled: false }), true, null), true);
  assert.equal(loosens(tool({}), false, null), false, "turning off tightens");
  const writes = tool({ max_auto: 10 });
  assert.equal(loosens(writes, true, 20), true);
  assert.equal(loosens(writes, true, 5), false);
  assert.equal(loosens(writes, true, 10), false);
});
