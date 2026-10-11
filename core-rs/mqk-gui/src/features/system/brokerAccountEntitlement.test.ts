// core-rs/mqk-gui/src/features/system/brokerAccountEntitlement.test.ts
//
// Contract and presentation proofs for the preflight broker-account fields.
// Bodies here are fixtures of the daemon wire shape: they prove the GUI maps
// daemon truth faithfully, never that any real account is entitled.

import test from "node:test";
import assert from "node:assert/strict";
import {
  brokerAccountChecks,
  brokerReadinessFieldsStructurallyValid,
  isBrokerAccountEntitlement,
} from "./brokerAccountEntitlement.ts";
import type { BrokerAccountEntitlement, BrokerStartBlocker } from "./types.ts";

function ent(overrides: Partial<BrokerAccountEntitlement> = {}): BrokerAccountEntitlement {
  return {
    state: "entitled",
    asset_class: "equity",
    code: null,
    detail: null,
    provider_account_id: "904837e3-3b76-47ec-b432-046db621571b",
    observed_at_utc: "2026-10-11T14:00:00Z",
    ...overrides,
  };
}

const check = (e: BrokerAccountEntitlement | null | undefined, blockers?: BrokerStartBlocker[]) =>
  brokerAccountChecks({ broker_account_entitlement: e, broker_start_blockers: blockers });

test("entitled is the only ok state and it is not worded as a provider guarantee", () => {
  const [c] = check(ent());
  assert.equal(c.tone, "ok");
  assert.equal(c.kind, "entitlement");
  assert.match(c.detail, /Fresh daemon evidence is bound to this run/);
  assert.doesNotMatch(c.detail, /guarantee|verified with the provider/i);
});

test("every non-entitled state is distinguishable and never ok", () => {
  const tones: Record<string, string> = {};
  for (const state of ["denied", "stale", "unbound", "unknown", "not_observed"] as const) {
    const [c] = check(ent({ state, code: state === "denied" ? "account_trading_blocked" : null }));
    tones[state] = c.tone;
    assert.notEqual(c.tone, "ok", state);
  }
  assert.equal(tones["denied"], "blocked");
  assert.equal(tones["stale"], "warning");
  assert.equal(tones["unbound"], "warning");
  assert.equal(tones["unknown"], "unknown");
  assert.equal(tones["not_observed"], "unknown");
});

test("structured codes are retained verbatim and identity refusal is its own kind", () => {
  const [denied] = check(ent({ state: "denied", code: "account_trading_blocked", detail: "provider reports trading_blocked=true" }));
  assert.equal(denied.kind, "entitlement");
  assert.equal(denied.code, "account_trading_blocked");
  assert.match(denied.detail, /trading_blocked=true/);

  const [drift] = check(ent({ state: "denied", code: "account_identity_drift" }));
  assert.equal(drift.kind, "identity");
  assert.equal(drift.title, "Broker account identity refused");
  assert.equal(drift.code, "account_identity_drift");
});

test("endpoint, credential and identity start refusals are separate checks; duplicates of the entitlement code are not repeated", () => {
  const checks = check(ent({ state: "denied", code: "account_trading_blocked" }), [
    { code: "account_trading_blocked", message: "broker account entitlement is denied [account_trading_blocked]: x" },
    { code: "runtime.start_refused.alpaca_paper_base_url_not_paper", message: "ALPACA_PAPER_BASE_URL does not target the Alpaca Paper API host" },
    { code: "runtime.start_refused.alpaca_creds_missing", message: "broker 'alpaca' requires a non-empty ALPACA_API_KEY_PAPER environment variable" },
    { code: "runtime.start_refused.alpaca_creds_malformed", message: "ALPACA_API_SECRET_PAPER is not a valid credential token; value not shown" },
    { code: "runtime.start_refused.broker_account_identity_conflict", message: "registered under another mode" },
    { code: "runtime.start_refused.something_else", message: "other" },
  ]);
  assert.deepEqual(
    checks.map((c) => c.kind),
    ["entitlement", "endpoint", "credentials", "credentials", "identity", "start_blocker"],
  );
  assert.ok(checks.slice(1).every((c) => c.tone === "blocked"));
});

test("no entitlement object means no entitlement check; blockers still show", () => {
  assert.deepEqual(check(null), []);
  assert.deepEqual(check(undefined), []);
  const only = check(null, [{ code: "runtime.start_refused.alpaca_creds_missing", message: "m" }]);
  assert.equal(only.length, 1);
  assert.equal(only[0].kind, "credentials");
});

test("structural validation: absent/null are fine, malformed present fields fail closed", () => {
  assert.equal(brokerReadinessFieldsStructurallyValid({}), true);
  assert.equal(brokerReadinessFieldsStructurallyValid({ broker_account_entitlement: null, broker_start_blockers: [] }), true);
  assert.equal(brokerReadinessFieldsStructurallyValid({ broker_account_entitlement: ent() }), true);
  assert.equal(brokerReadinessFieldsStructurallyValid({ broker_account_entitlement: { ...ent(), state: "ready" } }), false);
  assert.equal(brokerReadinessFieldsStructurallyValid({ broker_account_entitlement: { state: "entitled" } }), false);
  assert.equal(brokerReadinessFieldsStructurallyValid({ broker_account_entitlement: "entitled" }), false);
  assert.equal(brokerReadinessFieldsStructurallyValid({ broker_start_blockers: [{ code: 1, message: "m" }] }), false);
  assert.equal(brokerReadinessFieldsStructurallyValid({ broker_start_blockers: "x" }), false);
  assert.equal(isBrokerAccountEntitlement(ent({ code: "c", detail: "d" })), true);
});
