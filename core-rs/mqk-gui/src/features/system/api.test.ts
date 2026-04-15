import test from "node:test";
import assert from "node:assert/strict";
import React from "react";
import { renderToStaticMarkup } from "react-dom/server";
import { ConfigScreen } from "../config/ConfigScreen.tsx";
import { RiskScreen } from "../risk/RiskScreen.tsx";
import { StrategyScreen } from "../strategy/StrategyScreen.tsx";
import { fetchOperatorModel } from "./api.ts";
import { panelTruthRenderState } from "./truthRendering.ts";
import { DEFAULT_PREFLIGHT, DEFAULT_STATUS } from "./types.ts";

function jsonResponse(body: unknown, status = 200): Response {
  return {
    ok: status >= 200 && status < 300,
    status,
    async json() {
      return body;
    },
  } as Response;
}

function notFoundResponse(): Response {
  return jsonResponse({ error: "not found" }, 404);
}

test("operator-history backend_unavailable stays fail-closed through fetch/model/render", async () => {
  const originalFetch = globalThis.fetch;

  globalThis.fetch = (async (input: string | URL | Request) => {
    const raw = typeof input === "string" ? input : input instanceof URL ? input.toString() : input.url;
    const path = new URL(raw).pathname;

    switch (path) {
      case "/api/v1/system/status":
        return jsonResponse({
          ...DEFAULT_STATUS,
          runtime_status: "running",
          daemon_reachable: true,
          last_heartbeat: new Date().toISOString(),
        });

      case "/api/v1/system/preflight":
        return jsonResponse({
          ...DEFAULT_PREFLIGHT,
          daemon_reachable: true,
        });

      case "/api/v1/audit/operator-actions":
      case "/api/v1/audit/artifacts":
      case "/api/v1/ops/operator-timeline":
        return jsonResponse({
          canonical_route: path,
          truth_state: "backend_unavailable",
          backend: "unavailable",
          rows: [],
        });

      default:
        return notFoundResponse();
    }
  }) as typeof fetch;

  try {
    const model = await fetchOperatorModel();

    // Fallback row containers stay empty, but they are NOT treated as authoritative truth.
    assert.equal(model.auditActions.length, 0);
    assert.equal(model.artifactRegistry.artifacts.length, 0);
    assert.equal(model.operatorTimeline.length, 0);

    // The three durable-history surfaces must be marked as missing truth.
    assert.ok(model.dataSource.missingEndpoints.includes("/api/v1/audit/operator-actions"));
    assert.ok(model.dataSource.missingEndpoints.includes("/api/v1/audit/artifacts"));
    assert.ok(model.dataSource.missingEndpoints.includes("/api/v1/ops/operator-timeline"));

    // Unavailable durable truth must NOT be relabeled as placeholder/unimplemented.
    assert.equal(model.panelSources.audit, "unknown");
    assert.equal(model.panelSources.artifacts, "unknown");
    assert.equal(model.panelSources.operatorTimeline, "unknown");

    // Existing truth rendering then fail-closes the panels correctly.
    assert.equal(panelTruthRenderState(model, "audit"), "no_snapshot");
    assert.equal(panelTruthRenderState(model, "artifacts"), "no_snapshot");
    assert.equal(panelTruthRenderState(model, "operatorTimeline"), "no_snapshot");
  } finally {
    globalThis.fetch = originalFetch;
  }
});


test("explicit not_wired truth wrappers stay mounted and render honest GUI copy", async () => {
  const originalFetch = globalThis.fetch;

  globalThis.fetch = (async (input: string | URL | Request) => {
    const raw = typeof input === "string" ? input : input instanceof URL ? input.toString() : input.url;
    const path = new URL(raw).pathname;

    switch (path) {
      case "/api/v1/system/status":
        return jsonResponse({
          ...DEFAULT_STATUS,
          runtime_status: "running",
          daemon_reachable: true,
          last_heartbeat: new Date().toISOString(),
        });
      case "/api/v1/system/preflight":
        return jsonResponse({
          ...DEFAULT_PREFLIGHT,
          daemon_reachable: true,
        });
      case "/api/v1/system/config-fingerprint":
        return jsonResponse({
          config_hash: "cfg-123",
          risk_policy_version: "risk-v1",
          strategy_bundle_version: "bundle-v1",
          build_version: "build-v1",
          environment_profile: "paper",
          runtime_generation_id: "gen-123",
          last_restart_at: new Date().toISOString(),
        });
      case "/api/v1/system/runtime-leadership":
        return jsonResponse({
          leader_node: "daemon-a",
          leader_lease_state: "held",
          generation_id: "gen-123",
          restart_count_24h: 0,
          last_restart_at: new Date().toISOString(),
          post_restart_recovery_state: "complete",
          recovery_checkpoint: "ready",
          checkpoints: [],
        });
      case "/api/v1/risk/summary":
        return jsonResponse({
          gross_exposure: 0,
          net_exposure: 0,
          concentration_pct: 0,
          daily_pnl: 0,
          drawdown_pct: 0,
          loss_limit_utilization_pct: 0,
          kill_switch_active: false,
          active_breaches: 0,
        });
      case "/api/v1/risk/denials":
        return jsonResponse({ truth_state: "active", snapshot_at_utc: new Date().toISOString(), denials: [] });
      case "/api/v1/system/config-diffs":
        return jsonResponse({ truth_state: "not_wired", backend: "not_wired", rows: [] });
      case "/api/v1/strategy/suppressions":
        return jsonResponse({ truth_state: "not_wired", backend: "not_wired", rows: [] });
      case "/api/v1/strategy/summary":
        return jsonResponse({ truth_state: "not_wired", backend: "not_wired", rows: [] });
      default:
        return notFoundResponse();
    }
  }) as typeof fetch;

  try {
    const model = await fetchOperatorModel();

    assert.equal(model.strategySummaryTruth.truth_state, "not_wired");
    assert.equal(model.strategySuppressionsTruth.truth_state, "not_wired");
    assert.equal(model.configDiffsTruth.truth_state, "not_wired");

    assert.ok(model.dataSource.realEndpoints.includes("/api/v1/strategy/summary"));
    assert.ok(model.dataSource.realEndpoints.includes("/api/v1/strategy/suppressions"));
    assert.ok(model.dataSource.realEndpoints.includes("/api/v1/system/config-diffs"));

    assert.ok(!model.dataSource.mockSections.includes("strategies"));
    assert.ok(!model.dataSource.mockSections.includes("strategySuppressions"));
    assert.ok(!model.dataSource.mockSections.includes("configDiffs"));

    assert.equal(panelTruthRenderState(model, "strategy"), "not_wired");

    const configHtml = renderToStaticMarkup(React.createElement(ConfigScreen, { model }));
    const strategyHtml = renderToStaticMarkup(React.createElement(StrategyScreen, { model }));
    const riskHtml = renderToStaticMarkup(React.createElement(RiskScreen, { model }));

    assert.match(configHtml, /Config-diff truth is mounted but not wired/i);
    assert.doesNotMatch(configHtml, /No config diffs recorded/i);

    assert.match(strategyHtml, /This daemon truth surface is mounted but not wired/i);
    assert.doesNotMatch(strategyHtml, /Configured strategy engines/i);

    assert.match(riskHtml, /Strategy suppression truth is mounted but not wired/i);
    assert.doesNotMatch(riskHtml, /<div class="empty-state">No active suppressions\.<\/div>/i);
  } finally {
    globalThis.fetch = originalFetch;
  }
});

test("authoritative active-empty truth stays distinct from not_wired wrappers", async () => {
  const originalFetch = globalThis.fetch;

  globalThis.fetch = (async (input: string | URL | Request) => {
    const raw = typeof input === "string" ? input : input instanceof URL ? input.toString() : input.url;
    const path = new URL(raw).pathname;

    switch (path) {
      case "/api/v1/system/status":
        return jsonResponse({
          ...DEFAULT_STATUS,
          runtime_status: "running",
          daemon_reachable: true,
          last_heartbeat: new Date().toISOString(),
        });
      case "/api/v1/system/preflight":
        return jsonResponse({
          ...DEFAULT_PREFLIGHT,
          daemon_reachable: true,
        });
      case "/api/v1/system/config-fingerprint":
        return jsonResponse({
          config_hash: "cfg-123",
          risk_policy_version: "risk-v1",
          strategy_bundle_version: "bundle-v1",
          build_version: "build-v1",
          environment_profile: "paper",
          runtime_generation_id: "gen-123",
          last_restart_at: new Date().toISOString(),
        });
      case "/api/v1/system/runtime-leadership":
        return jsonResponse({
          leader_node: "daemon-a",
          leader_lease_state: "held",
          generation_id: "gen-123",
          restart_count_24h: 0,
          last_restart_at: new Date().toISOString(),
          post_restart_recovery_state: "complete",
          recovery_checkpoint: "ready",
          checkpoints: [],
        });
      case "/api/v1/risk/summary":
        return jsonResponse({
          gross_exposure: 0,
          net_exposure: 0,
          concentration_pct: 0,
          daily_pnl: 0,
          drawdown_pct: 0,
          loss_limit_utilization_pct: 0,
          kill_switch_active: false,
          active_breaches: 0,
        });
      case "/api/v1/risk/denials":
        return jsonResponse({ truth_state: "active", snapshot_at_utc: new Date().toISOString(), denials: [] });
      case "/api/v1/system/config-diffs":
        return jsonResponse({ truth_state: "active", backend: "sqlite", rows: [] });
      case "/api/v1/strategy/suppressions":
        return jsonResponse({ truth_state: "active", backend: "sqlite", rows: [] });
      case "/api/v1/strategy/summary":
        return jsonResponse({ truth_state: "active", backend: "runtime", rows: [] });
      default:
        return notFoundResponse();
    }
  }) as typeof fetch;

  try {
    const model = await fetchOperatorModel();

    assert.equal(model.strategySummaryTruth.truth_state, "active");
    assert.equal(model.strategySuppressionsTruth.truth_state, "active");
    assert.equal(model.configDiffsTruth.truth_state, "active");
    assert.equal(panelTruthRenderState(model, "strategy"), null);

    const configHtml = renderToStaticMarkup(React.createElement(ConfigScreen, { model }));
    const strategyHtml = renderToStaticMarkup(React.createElement(StrategyScreen, { model }));
    const riskHtml = renderToStaticMarkup(React.createElement(RiskScreen, { model }));

    assert.match(configHtml, /No config diffs recorded/i);
    assert.doesNotMatch(configHtml, /not wired/i);

    assert.match(strategyHtml, /No strategy summary rows reported/i);
    assert.doesNotMatch(strategyHtml, /not wired/i);

    assert.match(riskHtml, /No active suppressions/i);
    assert.doesNotMatch(riskHtml, /not wired/i);
  } finally {
    globalThis.fetch = originalFetch;
  }
});

test("strategy summary no_db fails closed: endpoint in missingEndpoints and panel blocks with no_snapshot", async () => {
  // B2B-GUI-01: When the daemon returns truth_state="no_db" for /strategy/summary,
  // the GUI must treat the probe as failed (ok:false) so:
  //   1. The endpoint lands in missingEndpoints (not realEndpoints).
  //   2. isMissingPanelTruth fires for the strategy panel.
  //   3. panelTruthRenderState returns "no_snapshot" (not null).
  //   4. StrategyScreen renders a TruthStateNotice — not "No strategy summary rows reported."
  //
  // This proves "no_db" is never treated as authoritative active-empty truth.
  const originalFetch = globalThis.fetch;

  globalThis.fetch = (async (input: string | URL | Request) => {
    const raw = typeof input === "string" ? input : input instanceof URL ? input.toString() : input.url;
    const path = new URL(raw).pathname;

    switch (path) {
      case "/api/v1/system/status":
        return jsonResponse({
          ...DEFAULT_STATUS,
          runtime_status: "running",
          daemon_reachable: true,
          last_heartbeat: new Date().toISOString(),
        });
      case "/api/v1/system/preflight":
        return jsonResponse({ ...DEFAULT_PREFLIGHT, daemon_reachable: true });
      case "/api/v1/strategy/summary":
        // Daemon reports DB unavailable — registry truth absent.
        return jsonResponse({
          canonical_route: "/api/v1/strategy/summary",
          truth_state: "no_db",
          backend: "postgres.sys_strategy_registry",
          runtime_execution_mode: "single_strategy",
          configured_fleet_size: 1,
          rows: [],
        });
      default:
        return notFoundResponse();
    }
  }) as typeof fetch;

  try {
    const model = await fetchOperatorModel();

    // Probe must be fail-closed: endpoint in missingEndpoints, not realEndpoints.
    assert.ok(
      model.dataSource.missingEndpoints.some((e) => e.includes("/strategy/summary")),
      "no_db: /strategy/summary must land in missingEndpoints",
    );
    assert.ok(
      !model.dataSource.realEndpoints.includes("/api/v1/strategy/summary"),
      "no_db: /strategy/summary must NOT be in realEndpoints",
    );

    // Strategy panel must block with no_snapshot — not render.
    assert.equal(
      panelTruthRenderState(model, "strategy"),
      "no_snapshot",
      "no_db: panelTruthRenderState must return no_snapshot for strategy panel",
    );

    // Screen must render TruthStateNotice, not the empty rows fallback copy.
    const strategyHtml = renderToStaticMarkup(React.createElement(StrategyScreen, { model }));
    assert.doesNotMatch(
      strategyHtml,
      /No strategy summary rows reported/i,
      "no_db: empty-rows copy must not appear when registry truth is absent",
    );
  } finally {
    globalThis.fetch = originalFetch;
  }
});

test("DESKTOP-10: connected daemon with unavailable preflight — safety-state checks fail-closed", async () => {
  // DESKTOP-10: when the daemon is reachable (status responds) but /api/v1/system/preflight
  // is unavailable (404), the unavailablePreflight fallback must NOT inherit the
  // "assume-safe" true defaults from DEFAULT_PREFLIGHT for safety-state checks.
  // Presenting runtime_idle/strategy_disarmed/execution_disarmed/live_routing_disabled
  // as true would cause PreflightGate to render them as "✓ Ready" — treating partial
  // daemon reachability as if safety states were canonically confirmed from the daemon.
  // All four must be false so the gate shows "Review required" (fail-closed).
  const originalFetch = globalThis.fetch;

  globalThis.fetch = (async (input: string | URL | Request) => {
    const raw = typeof input === "string" ? input : input instanceof URL ? input.toString() : input.url;
    const path = new URL(raw).pathname;

    if (path === "/api/v1/system/status") {
      return jsonResponse({
        ...DEFAULT_STATUS,
        daemon_reachable: true,
        last_heartbeat: new Date().toISOString(),
      });
    }
    // /api/v1/system/preflight intentionally not handled — falls through to 404.
    return notFoundResponse();
  }) as typeof fetch;

  try {
    const model = await fetchOperatorModel();

    // Daemon is reachable — status probe succeeded.
    assert.equal(model.connected, true, "model must be connected when status probe succeeds");

    // Preflight endpoint failed → unavailablePreflight is the fallback.
    // daemon_reachable must reflect actual connectivity (true).
    assert.equal(model.preflight.daemon_reachable, true, "daemon_reachable must reflect reachability");

    // Safety-state checks must be false — not inherited as true from DEFAULT_PREFLIGHT.
    // Each false means "this check is NOT confirmed" — fail-closed, not "condition is active".
    assert.equal(model.preflight.runtime_idle, false, "runtime_idle must fail-closed when preflight truth is unavailable");
    assert.equal(model.preflight.strategy_disarmed, false, "strategy_disarmed must fail-closed when preflight truth is unavailable");
    assert.equal(model.preflight.execution_disarmed, false, "execution_disarmed must fail-closed when preflight truth is unavailable");
    assert.equal(model.preflight.live_routing_disabled, false, "live_routing_disabled must fail-closed when preflight truth is unavailable");

    // Blocker must explicitly name the condition so the operator knows preflight is absent.
    assert.ok(
      model.preflight.blockers.some((b) => b.toLowerCase().includes("preflight truth unavailable")),
      "blocker must name that preflight truth is unavailable",
    );
  } finally {
    globalThis.fetch = originalFetch;
  }
});
