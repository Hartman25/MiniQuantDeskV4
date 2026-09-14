import test from "node:test";
import assert from "node:assert/strict";
import {
  OPERATOR_AUTHORITY_ROLE,
  containsDisallowedOperatorSingleton,
  filterPanelIdsForRole,
  isOperatorSingletonPanelId,
  panelAllowedInRole,
} from "../operatorSingletonGuard.ts";
import { OPERATOR_SINGLETON_PANEL_IDS } from "../panelRegistry.ts";

test("OPERATOR_AUTHORITY_ROLE is control, matching today's sole operator-singleton panel (ops)", () => {
  assert.equal(OPERATOR_AUTHORITY_ROLE, "control");
});

test("isOperatorSingletonPanelId matches exactly OPERATOR_SINGLETON_PANEL_IDS", () => {
  assert.equal(isOperatorSingletonPanelId("ops"), true);
  assert.equal(isOperatorSingletonPanelId("portfolio"), false);
  assert.equal(isOperatorSingletonPanelId("not-a-real-panel"), false);
  assert.deepEqual([...OPERATOR_SINGLETON_PANEL_IDS], ["ops"]);
});

test("panelAllowedInRole permits an operator-singleton panel only in the control/operator authority window", () => {
  assert.equal(panelAllowedInRole("ops", "control"), true);
  assert.equal(panelAllowedInRole("ops", "execution"), false);
  assert.equal(panelAllowedInRole("ops", "oversight"), false);
});

test("panelAllowedInRole permits every read-only panel in every role", () => {
  for (const role of ["control", "execution", "oversight"] as const) {
    assert.equal(panelAllowedInRole("portfolio", role), true, `portfolio must be allowed in ${role}`);
  }
});

// Negative control: execution/oversight layout cannot restore ops from localStorage.
test("filterPanelIdsForRole drops ops from an execution/oversight panel list but keeps it for control", () => {
  const ids = ["controlStation", "ops", "portfolio"];
  assert.deepEqual(filterPanelIdsForRole(ids, "execution"), ["controlStation", "portfolio"]);
  assert.deepEqual(filterPanelIdsForRole(ids, "oversight"), ["controlStation", "portfolio"]);
  assert.deepEqual(filterPanelIdsForRole(ids, "control"), ["controlStation", "ops", "portfolio"]);
});

// Negative control: execution/oversight preset/event cannot inject ops.
test("containsDisallowedOperatorSingleton flags ops for execution/oversight but not control", () => {
  assert.equal(containsDisallowedOperatorSingleton(["ops", "portfolio"], "execution"), true);
  assert.equal(containsDisallowedOperatorSingleton(["ops", "portfolio"], "oversight"), true);
  assert.equal(containsDisallowedOperatorSingleton(["ops", "portfolio"], "control"), false);
  assert.equal(containsDisallowedOperatorSingleton(["portfolio", "risk"], "execution"), false);
});
