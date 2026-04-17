import test from "node:test";
import assert from "node:assert/strict";
import React from "react";
import { renderToStaticMarkup } from "react-dom/server";
import { GlobalStatusBar } from "../../components/status/GlobalStatusBar.tsx";
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

    assert.match(strategyHtml, /No strategy engines reported/i);
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
  //   4. StrategyScreen renders a TruthStateNotice — not "No strategy engines reported."
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
      /No strategy engines reported/i,
      "no_db: empty-rows copy must not appear when registry truth is absent",
    );
  } finally {
    globalThis.fetch = originalFetch;
  }
});

test("DESKTOP-11: HTTP 200 with partial preflight body missing blockers/warnings — falls back to unavailablePreflight", async () => {
  // DESKTOP-11 gap: objectOrFallback passes any non-null object through.
  // A partial HTTP 200 body with positive safety-state booleans but no blockers/warnings
  // would (a) show false-positive "✓ Ready" safety checks and (b) crash PreflightGate
  // at preflight.blockers.length (TypeError on undefined).
  // isStructurallyValidPreflight must reject this and apply unavailablePreflight.
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
    if (path === "/api/v1/system/preflight") {
      // Partial body: safety-state booleans present (and positive), but blockers/warnings absent.
      return jsonResponse({
        daemon_reachable: true,
        runtime_idle: true,
        strategy_disarmed: true,
        execution_disarmed: true,
        live_routing_disabled: true,
        // blockers and warnings intentionally omitted — structural guard must reject this.
      });
    }
    return notFoundResponse();
  }) as typeof fetch;

  try {
    const model = await fetchOperatorModel();

    // Structural guard must reject the partial body — unavailablePreflight is applied.
    // Safety-state checks must be false (fail-closed), not the partial body's true values.
    assert.equal(model.preflight.runtime_idle, false, "runtime_idle must be false from unavailablePreflight, not partial body true");
    assert.equal(model.preflight.strategy_disarmed, false, "strategy_disarmed must be false from unavailablePreflight");
    assert.equal(model.preflight.execution_disarmed, false, "execution_disarmed must be false from unavailablePreflight");
    assert.equal(model.preflight.live_routing_disabled, false, "live_routing_disabled must be false from unavailablePreflight");

    // blockers must be a real array (from unavailablePreflight) — not undefined.
    assert.ok(Array.isArray(model.preflight.blockers), "blockers must be an array (no crash risk)");
    assert.ok(model.preflight.blockers.length > 0, "unavailablePreflight blocker must be present");
    assert.ok(Array.isArray(model.preflight.warnings), "warnings must be an array (no crash risk)");

    // daemon_reachable reflects actual connectivity (true — status probe succeeded).
    assert.equal(model.preflight.daemon_reachable, true, "daemon_reachable must reflect actual connectivity");
  } finally {
    globalThis.fetch = originalFetch;
  }
});

test("DESKTOP-11: HTTP 200 with empty object preflight body — falls back to unavailablePreflight", async () => {
  // A completely empty object ({}) is structurally incomplete.
  // isStructurallyValidPreflight must reject it — unavailablePreflight is applied.
  const originalFetch = globalThis.fetch;

  globalThis.fetch = (async (input: string | URL | Request) => {
    const raw = typeof input === "string" ? input : input instanceof URL ? input.toString() : input.url;
    const path = new URL(raw).pathname;

    if (path === "/api/v1/system/status") {
      return jsonResponse({ ...DEFAULT_STATUS, daemon_reachable: true, last_heartbeat: new Date().toISOString() });
    }
    if (path === "/api/v1/system/preflight") {
      return jsonResponse({}); // Empty object — no fields at all.
    }
    return notFoundResponse();
  }) as typeof fetch;

  try {
    const model = await fetchOperatorModel();

    assert.equal(model.preflight.runtime_idle, false, "empty body: runtime_idle must be false (unavailablePreflight)");
    assert.ok(Array.isArray(model.preflight.blockers), "empty body: blockers must be a real array");
    assert.ok(model.preflight.blockers.length > 0, "empty body: unavailablePreflight blocker must be present");
  } finally {
    globalThis.fetch = originalFetch;
  }
});

test("DESKTOP-11: structurally complete HTTP 200 preflight is accepted — not replaced by unavailablePreflight", async () => {
  // Positive proof: a structurally complete daemon response (has blockers and warnings as arrays)
  // must pass through as-is. isStructurallyValidPreflight must accept it.
  // This verifies the guard does not over-reject valid full responses.
  const originalFetch = globalThis.fetch;

  globalThis.fetch = (async (input: string | URL | Request) => {
    const raw = typeof input === "string" ? input : input instanceof URL ? input.toString() : input.url;
    const path = new URL(raw).pathname;

    if (path === "/api/v1/system/status") {
      return jsonResponse({ ...DEFAULT_STATUS, daemon_reachable: true, last_heartbeat: new Date().toISOString() });
    }
    if (path === "/api/v1/system/preflight") {
      // Structurally complete: all required fields present including blockers/warnings arrays.
      return jsonResponse({
        ...DEFAULT_PREFLIGHT,
        daemon_reachable: true,
        db_reachable: true,
        runtime_idle: true,
        strategy_disarmed: true,
        execution_disarmed: true,
        live_routing_disabled: true,
        blockers: [],
        warnings: [],
      });
    }
    return notFoundResponse();
  }) as typeof fetch;

  try {
    const model = await fetchOperatorModel();

    // Daemon-confirmed safety state must be used — NOT overridden by unavailablePreflight.
    assert.equal(model.preflight.runtime_idle, true, "daemon-confirmed runtime_idle must be accepted");
    assert.equal(model.preflight.strategy_disarmed, true, "daemon-confirmed strategy_disarmed must be accepted");
    assert.equal(model.preflight.execution_disarmed, true, "daemon-confirmed execution_disarmed must be accepted");
    assert.equal(model.preflight.live_routing_disabled, true, "daemon-confirmed live_routing_disabled must be accepted");

    // Daemon-confirmed empty blockers must be used — not overridden with unavailable blocker.
    assert.ok(Array.isArray(model.preflight.blockers), "blockers must be an array");
    assert.equal(model.preflight.blockers.length, 0, "daemon-confirmed empty blockers must not be replaced");
  } finally {
    globalThis.fetch = originalFetch;
  }
});

test("DESKTOP-11: HTTP 200 partial body with four positive safety booleans plus empty arrays but missing daemon_reachable — falls back to unavailablePreflight", async () => {
  // Exact gap closed by DESKTOP-11 tightened guard:
  // A body with blockers:[] + warnings:[] (passing the old array-only guard) plus all four
  // safety-state booleans set to true, but missing daemon_reachable, can no longer pass.
  // Without daemon_reachable, the partial body has no proven daemon authority but
  // the old guard would have accepted it, surfacing runtime_idle=true etc. as if confirmed.
  const originalFetch = globalThis.fetch;

  globalThis.fetch = (async (input: string | URL | Request) => {
    const raw = typeof input === "string" ? input : input instanceof URL ? input.toString() : input.url;
    const path = new URL(raw).pathname;

    if (path === "/api/v1/system/status") {
      return jsonResponse({ ...DEFAULT_STATUS, daemon_reachable: true, last_heartbeat: new Date().toISOString() });
    }
    if (path === "/api/v1/system/preflight") {
      // Partial body: four positive safety booleans + both empty arrays, daemon_reachable absent.
      // Old guard (only checked arrays): this body passed through — false start-capable state.
      // New guard (requires all 5 booleans + arrays): this body must be rejected.
      return jsonResponse({
        runtime_idle: true,
        strategy_disarmed: true,
        execution_disarmed: true,
        live_routing_disabled: true,
        blockers: [],
        warnings: [],
        // daemon_reachable intentionally omitted — structural guard must reject this.
      });
    }
    return notFoundResponse();
  }) as typeof fetch;

  try {
    const model = await fetchOperatorModel();

    // Guard must reject — unavailablePreflight is applied, not the partial body.
    assert.equal(model.preflight.runtime_idle, false, "runtime_idle must be false (unavailablePreflight), not partial body true");
    assert.equal(model.preflight.strategy_disarmed, false, "strategy_disarmed must be false (unavailablePreflight)");
    assert.equal(model.preflight.execution_disarmed, false, "execution_disarmed must be false (unavailablePreflight)");
    assert.equal(model.preflight.live_routing_disabled, false, "live_routing_disabled must be false (unavailablePreflight)");

    // blockers must come from unavailablePreflight — not the partial body's empty array.
    assert.ok(Array.isArray(model.preflight.blockers), "blockers must be an array");
    assert.ok(model.preflight.blockers.length > 0, "unavailablePreflight blocker must be present — not partial body empty []");

    // daemon_reachable reflects actual connectivity from the status probe.
    assert.equal(model.preflight.daemon_reachable, true, "daemon_reachable must reflect actual connectivity");
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

// ---------------------------------------------------------------------------
// DESKTOP-12: WS continuity pill in GlobalStatusBar
//
// broker_status alone conflates REST reachability with WS event-stream continuity.
// For paper+alpaca, cold_start_unproven and gap_detected block execution start
// even when broker_status is "healthy". The WS Continuity pill must be:
//   - absent when alpaca_ws_continuity === "not_applicable" (synthetic/paper broker)
//   - present with warning tone for cold_start_unproven
//   - present with critical+loud for gap_detected (start-blocking terminal state)
//   - present with info tone for live (proven continuity)
// ---------------------------------------------------------------------------

test("DESKTOP-12: WS continuity pill absent when not_applicable (synthetic broker / paper+paper)", () => {
  const html = renderToStaticMarkup(
    React.createElement(GlobalStatusBar, {
      status: { ...DEFAULT_STATUS, alpaca_ws_continuity: "not_applicable" },
    }),
  );
  assert.doesNotMatch(html, /WS Continuity/i, "not_applicable: WS Continuity pill must not appear for synthetic broker");
});

test("DESKTOP-12: WS continuity pill present with warning tone for cold_start_unproven", () => {
  const html = renderToStaticMarkup(
    React.createElement(GlobalStatusBar, {
      status: { ...DEFAULT_STATUS, alpaca_ws_continuity: "cold_start_unproven" },
    }),
  );
  assert.match(html, /WS Continuity/i, "cold_start_unproven: WS Continuity pill must be present");
  assert.match(html, /tone-warning/, "cold_start_unproven: pill must carry warning tone");
  // cold_start_unproven is a warning, not loud — operator must see it but it is not terminal.
  assert.doesNotMatch(html, /emphasis-loud.*Cold Start Unproven|Cold Start Unproven.*emphasis-loud/, "cold_start_unproven: pill must not carry loud emphasis");
});

test("DESKTOP-12: WS continuity pill present with critical+loud emphasis for gap_detected", () => {
  const html = renderToStaticMarkup(
    React.createElement(GlobalStatusBar, {
      status: { ...DEFAULT_STATUS, alpaca_ws_continuity: "gap_detected" },
    }),
  );
  assert.match(html, /WS Continuity/i, "gap_detected: WS Continuity pill must be present");
  // gap_detected carries both critical tone and loud emphasis — it is a terminal start-blocking state.
  assert.match(html, /tone-critical/, "gap_detected: pill must carry critical tone");
  assert.match(html, /emphasis-loud/, "gap_detected: pill must carry loud emphasis");
});

test("DESKTOP-12: WS continuity pill present with info tone for live (proven continuity)", () => {
  const html = renderToStaticMarkup(
    React.createElement(GlobalStatusBar, {
      status: { ...DEFAULT_STATUS, alpaca_ws_continuity: "live" },
    }),
  );
  assert.match(html, /WS Continuity/i, "live: WS Continuity pill must be present");
  assert.match(html, /tone-info/, "live: pill must carry info tone (proven state)");
});
