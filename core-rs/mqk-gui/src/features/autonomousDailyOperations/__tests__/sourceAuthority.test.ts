import test from "node:test";
import assert from "node:assert/strict";
import { classifyDailyOperationsSourceAuthority } from "../../system/sourceAuthority.ts";
import type {
  AutonomousDailyOperationSurface,
  AutonomousDailyOperationsSurface,
} from "../../system/types.ts";

const AVAILABLE_SINGLE: AutonomousDailyOperationSurface = {
  transport_state: "available",
  canonical_route: "/api/v1/autonomous/daily-operation",
  truth_state: "active",
  operation: null,
  message: null,
};

const UNAVAILABLE_SINGLE: AutonomousDailyOperationSurface = {
  transport_state: "endpoint_unavailable",
  canonical_route: null,
  truth_state: null,
  operation: null,
  message: null,
};

const AVAILABLE_HISTORY: AutonomousDailyOperationsSurface = {
  transport_state: "available",
  canonical_route: "/api/v1/autonomous/daily-operations",
  truth_state: "active",
  requested_limit: 20,
  effective_limit: 20,
  rows: [],
  message: null,
};

const UNAVAILABLE_HISTORY: AutonomousDailyOperationsSurface = {
  transport_state: "endpoint_unavailable",
  canonical_route: null,
  truth_state: null,
  requested_limit: null,
  effective_limit: null,
  rows: [],
  message: null,
};

// F1.10 / F1.11 item 18: both available -> both sections may render.
test("classifyDailyOperationsSourceAuthority: both surfaces available", () => {
  const authority = classifyDailyOperationsSourceAuthority(AVAILABLE_SINGLE, AVAILABLE_HISTORY);
  assert.equal(authority.current, "available");
  assert.equal(authority.history, "available");
  assert.equal(authority.bothUnavailable, false);
});

test("classifyDailyOperationsSourceAuthority: single available, history unavailable", () => {
  const authority = classifyDailyOperationsSourceAuthority(AVAILABLE_SINGLE, UNAVAILABLE_HISTORY);
  assert.equal(authority.current, "available");
  assert.equal(authority.history, "unavailable");
  assert.equal(authority.bothUnavailable, false);
});

test("classifyDailyOperationsSourceAuthority: single unavailable, history available", () => {
  const authority = classifyDailyOperationsSourceAuthority(UNAVAILABLE_SINGLE, AVAILABLE_HISTORY);
  assert.equal(authority.current, "unavailable");
  assert.equal(authority.history, "available");
  assert.equal(authority.bothUnavailable, false);
});

// F1.10: both unavailable -> screen-level fail-closed notice.
test("classifyDailyOperationsSourceAuthority: both unavailable", () => {
  const authority = classifyDailyOperationsSourceAuthority(UNAVAILABLE_SINGLE, UNAVAILABLE_HISTORY);
  assert.equal(authority.current, "unavailable");
  assert.equal(authority.history, "unavailable");
  assert.equal(authority.bothUnavailable, true);
});
