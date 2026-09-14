import test from "node:test";
import assert from "node:assert/strict";
import {
  connectivityDisplayState,
  incidentDisplayState,
  killSwitchDisplayState,
  liveRoutingDisplayState,
} from "../GlobalStatusBar.tsx";
import { DEFAULT_STATUS, type SystemStatus } from "../../../features/system/types.ts";

// WAVE-02-FINAL-REPAIR-01 R1: negative controls proving the fixed Tier-0
// safety surface fails closed rather than rendering a misleadingly clear
// state when the daemon is unreachable. DEFAULT_STATUS deliberately sets
// kill_switch_active/live_routing_enabled/has_critical/has_warning to
// "safe-looking" false defaults while unreachable (see DEFAULT_STATUS
// comment) — these tests prove the GlobalStatusBar derivation does not
// pass those defaults through as confirmed truth.

function unreachableStatus(overrides: Partial<SystemStatus> = {}): SystemStatus {
  return { ...DEFAULT_STATUS, daemon_reachable: false, ...overrides };
}

function reachableStatus(overrides: Partial<SystemStatus> = {}): SystemStatus {
  return { ...DEFAULT_STATUS, daemon_reachable: true, ...overrides };
}

test("unknown kill-switch state must not render clear/off", () => {
  const status = unreachableStatus({ kill_switch_active: false });
  assert.equal(killSwitchDisplayState(status), "unknown");
});

test("kill-switch state renders true/false only once the daemon is confirmed reachable", () => {
  assert.equal(killSwitchDisplayState(reachableStatus({ kill_switch_active: true })), "active");
  assert.equal(killSwitchDisplayState(reachableStatus({ kill_switch_active: false })), "inactive");
});

test("unknown Live-routing state must not render disabled", () => {
  const status = unreachableStatus({ live_routing_enabled: false });
  assert.equal(liveRoutingDisplayState(status), "unknown");
});

test("live-routing state renders true/false only once the daemon is confirmed reachable", () => {
  assert.equal(liveRoutingDisplayState(reachableStatus({ live_routing_enabled: true })), "enabled");
  assert.equal(liveRoutingDisplayState(reachableStatus({ live_routing_enabled: false })), "disabled");
});

test("unavailable incident source must not render 'no incidents' (clear)", () => {
  // has_critical/has_warning both false — if reachability were ignored this
  // would incorrectly resolve to "clear".
  const status = unreachableStatus({ has_critical: false, has_warning: false });
  assert.equal(incidentDisplayState(status), "unknown");
  assert.notEqual(incidentDisplayState(status), "clear");
});

test("incident indication distinguishes critical/warning/clear once reachable", () => {
  assert.equal(incidentDisplayState(reachableStatus({ has_critical: true, has_warning: false })), "critical");
  assert.equal(incidentDisplayState(reachableStatus({ has_critical: false, has_warning: true })), "warning");
  assert.equal(incidentDisplayState(reachableStatus({ has_critical: false, has_warning: false })), "clear");
});

test("offline/stale remains visibly distinguishable from healthy", () => {
  assert.equal(connectivityDisplayState(unreachableStatus()), "offline");
  assert.equal(connectivityDisplayState(reachableStatus()), "online");
  assert.notEqual(connectivityDisplayState(unreachableStatus()), connectivityDisplayState(reachableStatus()));
});
