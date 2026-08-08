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
          truth_state: "active",
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
          truth_state: "active",
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

// ---------------------------------------------------------------------------
// STRATEGY-DECISION-OBSERVABILITY-01: strategy decision diagnostics wired through model
// ---------------------------------------------------------------------------

test("SDOBS-01: strategy_decision_diagnostics from autonomous/readiness is surfaced in model", async () => {
  const originalFetch = globalThis.fetch;

  const diagFixture = {
    strategy_id: "intraday_scalper",
    symbol: "AAPL",
    timeframe: "5m",
    lookback_bars: 5,
    threshold_bps: 20,
    latest_bar_ts: 1780688700,
    latest_close_micros: 308110000,
    lookback_bar_ts: 1780687500,
    lookback_close_micros: 308870000,
    move_bps: -24,
    abs_move_bps: 24,
    gap_to_threshold_bps: -4,
    raw_direction: -1,
    decision: "flat_due_to_negative_direction",
    reason: "bearish displacement; long-only strategy maps to flat",
  };

  globalThis.fetch = (async (input: string | URL | Request) => {
    const raw = typeof input === "string" ? input : input instanceof URL ? input.toString() : input.url;
    const path = new URL(raw).pathname;

    if (path === "/api/v1/system/status") {
      return jsonResponse({ ...DEFAULT_STATUS, daemon_reachable: true, last_heartbeat: new Date().toISOString() });
    }
    if (path === "/api/v1/system/preflight") {
      return jsonResponse({ ...DEFAULT_PREFLIGHT, daemon_reachable: true });
    }
    if (path === "/api/v1/autonomous/readiness") {
      return jsonResponse({
        truth_state: "active",
        session_in_window: true,
        session_window_state: "in_window",
        now_utc: new Date().toISOString(),
        session_start_utc: "13:30 UTC",
        session_stop_utc: "20:00 UTC",
        session_window_source: "fixed_window_override",
        arm_state: "armed",
        nyse_market_session: "regular",
        bar_ticker_gate: "open",
        bar_tick_dispatch_count: 1,
        last_bar_signal_qty: 0,
        bar_context_source: "db_loaded",
        bar_context_bars_loaded: 30,
        blockers: ["NO_SIGNAL_GENERATED: strategy conditions not met"],
        overall_ready: false,
        strategy_decision_diagnostics: diagFixture,
      });
    }
    // Provide enough for StrategyScreen to pass truthRendering check.
    if (path === "/api/v1/strategy/summary") {
      return jsonResponse({ truth_state: "active", backend: "runtime", rows: [] });
    }
    if (path === "/api/v1/strategy/suppressions") {
      return jsonResponse({ truth_state: "active", backend: "postgres", rows: [] });
    }
    return notFoundResponse();
  }) as typeof fetch;

  try {
    const model = await fetchOperatorModel();

    // Decision diagnostics must be wired through the model.
    assert.ok(model.strategyDecisionDiagnostics !== null, "strategyDecisionDiagnostics must be non-null when readiness is active");
    assert.equal(model.strategyDecisionDiagnostics?.strategy_id, "intraday_scalper");
    assert.equal(model.strategyDecisionDiagnostics?.symbol, "AAPL");
    assert.equal(model.strategyDecisionDiagnostics?.decision, "flat_due_to_negative_direction");
    assert.equal(model.strategyDecisionDiagnostics?.move_bps, -24);
    assert.equal(model.strategyDecisionDiagnostics?.threshold_bps, 20);
    assert.equal(model.strategyDecisionDiagnostics?.gap_to_threshold_bps, -4);

    // Bar dispatch context must be wired.
    assert.equal(model.autonomousBarTickCount, 1);
    assert.equal(model.autonomousLastSignalQty, 0);
    assert.equal(model.autonomousBarContextSource, "db_loaded");

    // Blockers must be wired.
    assert.equal(model.autonomousBlockers.length, 1);
    assert.ok(model.autonomousBlockers[0].includes("NO_SIGNAL_GENERATED"), "blocker must include NO_SIGNAL_GENERATED");

    // StrategyScreen must render the decision panel when autonomousBarTickCount is non-null.
    const strategyHtml = renderToStaticMarkup(React.createElement(StrategyScreen, { model }));
    assert.match(strategyHtml, /Last bar signal decision/i, "decision panel title must appear");
    assert.match(strategyHtml, /flat_due_to_negative_direction/i, "decision value must render");
    assert.match(strategyHtml, /AAPL/i, "symbol must render in decision panel");
    assert.match(strategyHtml, /Autonomous readiness blockers/i, "blockers panel title must appear");
    assert.match(strategyHtml, /NO_SIGNAL_GENERATED/i, "blocker text must render");
  } finally {
    globalThis.fetch = originalFetch;
  }
});

test("SDOBS-02: absent autonomous/readiness keeps strategyDecisionDiagnostics null and hides panel", async () => {
  const originalFetch = globalThis.fetch;

  globalThis.fetch = (async (input: string | URL | Request) => {
    const raw = typeof input === "string" ? input : input instanceof URL ? input.toString() : input.url;
    const path = new URL(raw).pathname;

    if (path === "/api/v1/system/status") {
      return jsonResponse({ ...DEFAULT_STATUS, daemon_reachable: true, last_heartbeat: new Date().toISOString() });
    }
    if (path === "/api/v1/system/preflight") {
      return jsonResponse({ ...DEFAULT_PREFLIGHT, daemon_reachable: true });
    }
    // autonomous/readiness intentionally absent (not paper+alpaca) — returns 404.
    return notFoundResponse();
  }) as typeof fetch;

  try {
    const model = await fetchOperatorModel();

    // When readiness is unavailable, all fields must be null/empty.
    assert.equal(model.strategyDecisionDiagnostics, null, "strategyDecisionDiagnostics must be null when readiness is absent");
    assert.equal(model.autonomousBarTickCount, null, "autonomousBarTickCount must be null");
    assert.equal(model.autonomousLastSignalQty, null);
    assert.equal(model.autonomousBarContextSource, null);
    assert.deepEqual(model.autonomousBlockers, [], "autonomousBlockers must be empty");

    // StrategyScreen must NOT render the decision panel when autonomousBarTickCount is null.
    const strategyHtml = renderToStaticMarkup(React.createElement(StrategyScreen, { model }));
    assert.doesNotMatch(strategyHtml, /Last bar signal decision/i, "decision panel must not appear when readiness is absent");
    assert.doesNotMatch(strategyHtml, /Autonomous readiness blockers/i, "blockers panel must not appear when readiness is absent");
  } finally {
    globalThis.fetch = originalFetch;
  }
});

// ---------------------------------------------------------------------------
// GUI-PAPER-STATUS-VISIBILITY-01: Read-only visibility for autonomous paper
// status, watchlist status, and watchlist admission-check (dry-run advisory).
// ---------------------------------------------------------------------------

const PSV_STRATEGY_FIXTURES: Record<string, unknown> = {
  "/api/v1/strategy/summary": { truth_state: "active", backend: "runtime", rows: [] },
  "/api/v1/strategy/suppressions": { truth_state: "active", backend: "postgres", rows: [] },
};

test("PSV-01: autonomous paper status surfaces readiness/next-action/flatten/live-routing/watchlist honestly (no fabricated truth)", async () => {
  const originalFetch = globalThis.fetch;
  const nowUtc = new Date().toISOString();

  const paperStatusFixture = {
    canonical_route: "/api/v1/autonomous/paper-status",
    truth_state: "active",
    mode: "paper",
    live_routing_enabled: false,
    runtime_status: "running",
    arm_state: "armed",
    kill_switch_active: false,
    deadman_status: "ok",
    ws_continuity: "live",
    reconcile_status: "clean",
    mismatch_count: 0,
    open_order_count: 0,
    position_count: 0,
    current_symbol: null,
    current_position_qty: null,
    target_qty: null,
    computed_delta_qty: null,
    no_order_reason: "NO_SIGNAL_GENERATED: strategy conditions not met",
    last_strategy_decision: "flat",
    flatten_available: false,
    flatten_blockers: [
      "NO_OPEN_POSITION: position_count is zero — nothing to flatten",
      "RUNTIME_NOT_ARMED_FOR_FLATTEN: flatten requires running+armed+paper",
    ],
    watchlist_outcome: "approved",
    watchlist_approved: true,
    readiness_classification: "READY_FOR_SUPERVISED_PAPER",
    blockers: [],
    next_operator_action: "Monitor for first signal-driven order this session.",
    autonomous_session_state: "active",
    now_utc: nowUtc,
  };

  const watchlistStatusFixture = {
    configured_path: "research-py/watchlists/active.json",
    status: "approved",
    approved_for_autonomous_paper: true,
    approved_for_live: false,
    symbols: ["MSFT", "NVDA"],
    top_symbol: "MSFT",
    strategy_assignments: { MSFT: "intraday_scalper", NVDA: "intraday_scalper" },
    max_symbols_to_trade: 2,
    max_concurrent_positions: 1,
    failure_reasons: [],
    checked_at_utc: nowUtc,
  };

  globalThis.fetch = (async (input: string | URL | Request) => {
    const raw = typeof input === "string" ? input : input instanceof URL ? input.toString() : input.url;
    const path = new URL(raw).pathname;

    if (path === "/api/v1/system/status") {
      return jsonResponse({ ...DEFAULT_STATUS, daemon_reachable: true, last_heartbeat: new Date().toISOString() });
    }
    if (path === "/api/v1/system/preflight") {
      return jsonResponse({ ...DEFAULT_PREFLIGHT, daemon_reachable: true });
    }
    if (path === "/api/v1/autonomous/paper-status") {
      return jsonResponse(paperStatusFixture);
    }
    if (path === "/api/v1/watchlist/status") {
      return jsonResponse(watchlistStatusFixture);
    }
    if (path in PSV_STRATEGY_FIXTURES) {
      return jsonResponse(PSV_STRATEGY_FIXTURES[path]);
    }
    // autonomous/readiness intentionally absent — no strategy_id source, so
    // admission-check must land in "not_checked" (never invents a default pair).
    return notFoundResponse();
  }) as typeof fetch;

  try {
    const model = await fetchOperatorModel();

    // Proof point 1: readiness_classification + next_operator_action render.
    assert.equal(model.autonomousPaperStatus.truth_state, "active");
    assert.equal(model.autonomousPaperStatus.readiness_classification, "READY_FOR_SUPERVISED_PAPER");
    assert.equal(model.autonomousPaperStatus.next_operator_action, "Monitor for first signal-driven order this session.");

    // Proof point 2: flatten_available=false + blockers visible.
    assert.equal(model.autonomousPaperStatus.flatten_available, false);
    assert.deepEqual(model.autonomousPaperStatus.flatten_blockers, paperStatusFixture.flatten_blockers);

    // Proof point 3: live_routing_enabled=false visible (not rendered as an error).
    assert.equal(model.autonomousPaperStatus.live_routing_enabled, false);

    // Proof point 4 (part 1): watchlist status honest "active" state with real fields.
    assert.equal(model.watchlistStatus.truth_state, "active");
    assert.equal(model.watchlistStatus.status, "approved");
    assert.deepEqual(model.watchlistStatus.symbols, ["MSFT", "NVDA"]);
    assert.equal(model.watchlistStatus.strategy_assignment_count, 2);

    // Proof point 5: approved_for_live=false visible on the watchlist status surface.
    assert.equal(model.watchlistStatus.approved_for_live, false);

    // No real strategy_id source (autonomous/readiness absent) → admission-check must
    // be "not_checked" and must NOT invent a default symbol/strategy pair.
    assert.equal(model.admissionCheck.state, "not_checked");
    assert.equal(model.admissionCheck.symbol, null);
    assert.equal(model.admissionCheck.strategy_id, null);

    const html = renderToStaticMarkup(React.createElement(StrategyScreen, { model }));

    // Readiness/next-action render as text (proof point 1).
    assert.match(html, /READY_FOR_SUPERVISED_PAPER/);
    assert.match(html, /Monitor for first signal-driven order this session\./);

    // Flatten availability + blockers render, explicitly marked visibility-only (proof point 2).
    assert.match(html, /Flatten availability \(visibility only\)/);
    assert.match(html, /Flatten available/);
    assert.match(html, /NO_OPEN_POSITION: position_count is zero/);
    assert.match(html, /cannot invoke flatten-paper-positions/);

    // live_routing_enabled=false renders as an honest positive-tone fact, not an
    // "unavailable"/error notice (proof point 3).
    assert.match(html, /Live routing enabled/);
    assert.doesNotMatch(html, /Autonomous paper status is currently unavailable/);
    assert.doesNotMatch(html, /Autonomous paper status is not applicable/);

    // Watchlist status renders honest "active" data, including approved_for_live=false
    // (proof points 4 + 5).
    assert.match(html, /Watchlist status/);
    assert.match(html, /Approved for live/);
    assert.doesNotMatch(html, /Watchlist status is currently unavailable/);

    // Admission-check renders the "not_checked" honest state — never a fabricated default.
    assert.match(html, /Admission-check not run/);
    assert.match(html, /never substitutes a default symbol or strategy/);
  } finally {
    globalThis.fetch = originalFetch;
  }
});

test("PSV-02: watchlist admission-check runs against a real backend-sourced symbol/strategy pair and renders the dry-run advisory note (never hardcodes a default symbol)", async () => {
  const originalFetch = globalThis.fetch;
  const nowUtc = new Date().toISOString();
  let admissionCheckRequested: URL | null = null;

  const paperStatusFixture = {
    canonical_route: "/api/v1/autonomous/paper-status",
    truth_state: "active",
    mode: "paper",
    live_routing_enabled: false,
    runtime_status: "running",
    arm_state: "armed",
    kill_switch_active: false,
    deadman_status: "ok",
    ws_continuity: "live",
    reconcile_status: "clean",
    mismatch_count: 0,
    open_order_count: 1,
    position_count: 1,
    // Deliberately NOT "AAPL" — proves the GUI derives the symbol from real backend
    // truth (current_symbol) rather than any hardcoded default.
    current_symbol: "MSFT",
    current_position_qty: 10,
    target_qty: 10,
    computed_delta_qty: 0,
    no_order_reason: null,
    last_strategy_decision: "signal_long",
    flatten_available: true,
    flatten_blockers: [],
    watchlist_outcome: "approved",
    watchlist_approved: true,
    readiness_classification: "READY_FOR_SUPERVISED_PAPER",
    blockers: [],
    next_operator_action: "Continue monitoring the active paper session.",
    autonomous_session_state: "active",
    now_utc: nowUtc,
  };

  const watchlistStatusFixture = {
    configured_path: "research-py/watchlists/active.json",
    status: "approved",
    approved_for_autonomous_paper: true,
    approved_for_live: false,
    symbols: ["MSFT"],
    top_symbol: "MSFT",
    strategy_assignments: { MSFT: "intraday_scalper" },
    max_symbols_to_trade: 1,
    max_concurrent_positions: 1,
    failure_reasons: [],
    checked_at_utc: nowUtc,
  };

  const admissionCheckFixture = {
    allowed: true,
    reason: "symbol_and_strategy_approved",
    status: "approved",
    approved_for_autonomous_paper: true,
    approved_for_live: false,
    symbol: "MSFT",
    strategy_id: "intraday_scalper",
    top_symbol: "MSFT",
    strategy_assignments: { MSFT: "intraday_scalper" },
    note: "dry_run_only_not_enforced",
    checked_at_utc: nowUtc,
  };

  globalThis.fetch = (async (input: string | URL | Request) => {
    const raw = typeof input === "string" ? input : input instanceof URL ? input.toString() : input.url;
    const url = new URL(raw);
    const path = url.pathname;

    if (path === "/api/v1/system/status") {
      return jsonResponse({ ...DEFAULT_STATUS, daemon_reachable: true, last_heartbeat: new Date().toISOString() });
    }
    if (path === "/api/v1/system/preflight") {
      return jsonResponse({ ...DEFAULT_PREFLIGHT, daemon_reachable: true });
    }
    if (path === "/api/v1/autonomous/paper-status") {
      return jsonResponse(paperStatusFixture);
    }
    if (path === "/api/v1/watchlist/status") {
      return jsonResponse(watchlistStatusFixture);
    }
    if (path === "/api/v1/autonomous/readiness") {
      return jsonResponse({
        truth_state: "active",
        session_in_window: true,
        session_window_state: "in_window",
        now_utc: nowUtc,
        session_start_utc: "13:30 UTC",
        session_stop_utc: "20:00 UTC",
        session_window_source: "fixed_window_override",
        arm_state: "armed",
        nyse_market_session: "regular",
        bar_ticker_gate: "open",
        bar_tick_dispatch_count: 3,
        last_bar_signal_qty: 10,
        bar_context_source: "db_loaded",
        bar_context_bars_loaded: 30,
        blockers: [],
        overall_ready: true,
        strategy_decision_diagnostics: {
          strategy_id: "intraday_scalper",
          symbol: "MSFT",
          timeframe: "5m",
          lookback_bars: 5,
          threshold_bps: 20,
          latest_bar_ts: 1780688700,
          latest_close_micros: 308110000,
          lookback_bar_ts: 1780687500,
          lookback_close_micros: 308870000,
          move_bps: 28,
          abs_move_bps: 28,
          gap_to_threshold_bps: 8,
          raw_direction: 1,
          decision: "signal_long",
          reason: "bullish displacement exceeds threshold",
        },
      });
    }
    if (path === "/api/v1/watchlist/admission-check") {
      admissionCheckRequested = url;
      return jsonResponse(admissionCheckFixture);
    }
    if (path in PSV_STRATEGY_FIXTURES) {
      return jsonResponse(PSV_STRATEGY_FIXTURES[path]);
    }
    return notFoundResponse();
  }) as typeof fetch;

  try {
    const model = await fetchOperatorModel();

    // The admission-check fetch must have been made against the real backend-sourced
    // pair (current_symbol + strategy_decision_diagnostics.strategy_id) — never AAPL,
    // never an invented default.
    assert.ok(admissionCheckRequested !== null, "admission-check must be invoked when a real symbol/strategy pair exists");
    assert.equal(admissionCheckRequested?.searchParams.get("symbol"), "MSFT");
    assert.equal(admissionCheckRequested?.searchParams.get("strategy_id"), "intraday_scalper");

    assert.equal(model.admissionCheck.state, "checked");
    assert.equal(model.admissionCheck.symbol, "MSFT");
    assert.equal(model.admissionCheck.strategy_id, "intraday_scalper");
    assert.equal(model.admissionCheck.note, "dry_run_only_not_enforced");
    assert.equal(model.admissionCheck.approved_for_live, false);

    const html = renderToStaticMarkup(React.createElement(StrategyScreen, { model }));

    // Proof point 6: admission-check dry_run note is visible, with the symbol/strategy
    // sourced from real backend truth (MSFT/intraday_scalper — not a hardcoded "AAPL").
    assert.match(html, /Watchlist admission-check \(dry-run, advisory only\)/);
    assert.match(html, /dry_run_only_not_enforced/);
    assert.match(html, /advisory dry-run only/);
    assert.match(html, /must never be used to trigger trading behavior/);
    assert.match(html, />MSFT</);
    assert.match(html, />intraday_scalper</);
    assert.doesNotMatch(html, />AAPL</, "admission-check must never render a hardcoded AAPL default");
  } finally {
    globalThis.fetch = originalFetch;
  }
});

test("PSV-03: flatten-paper-positions stays excluded from the invokable action catalog and StrategyScreen renders zero invocation controls", async () => {
  const originalFetch = globalThis.fetch;
  const nowUtc = new Date().toISOString();

  globalThis.fetch = (async (input: string | URL | Request) => {
    const raw = typeof input === "string" ? input : input instanceof URL ? input.toString() : input.url;
    const path = new URL(raw).pathname;

    if (path === "/api/v1/system/status") {
      return jsonResponse({ ...DEFAULT_STATUS, daemon_reachable: true, last_heartbeat: new Date().toISOString() });
    }
    if (path === "/api/v1/system/preflight") {
      return jsonResponse({ ...DEFAULT_PREFLIGHT, daemon_reachable: true });
    }
    if (path === "/api/v1/autonomous/paper-status") {
      return jsonResponse({
        canonical_route: "/api/v1/autonomous/paper-status",
        truth_state: "active",
        mode: "paper",
        live_routing_enabled: false,
        runtime_status: "running",
        arm_state: "armed",
        kill_switch_active: false,
        deadman_status: "ok",
        ws_continuity: "live",
        reconcile_status: "clean",
        mismatch_count: 0,
        open_order_count: 0,
        position_count: 1,
        current_symbol: "MSFT",
        current_position_qty: 10,
        target_qty: 0,
        computed_delta_qty: -10,
        no_order_reason: null,
        last_strategy_decision: "signal_flat",
        // Daemon truth: flatten IS available here — proves the GUI hides the
        // invocation control regardless of backend availability state.
        flatten_available: true,
        flatten_blockers: [],
        watchlist_outcome: "approved",
        watchlist_approved: true,
        readiness_classification: "READY_FOR_SUPERVISED_PAPER",
        blockers: [],
        next_operator_action: "Operator may flatten via the authorized control surface.",
        autonomous_session_state: "active",
        now_utc: nowUtc,
      });
    }
    if (path === "/api/v1/watchlist/status") {
      return notFoundResponse();
    }
    if (path === "/api/v1/ops/catalog") {
      return jsonResponse({
        canonical_route: "/api/v1/ops/catalog",
        actions: [
          {
            action_key: "arm-execution",
            label: "Arm execution",
            level: 1,
            description: "Arm the execution engine.",
            requires_reason: false,
            confirm_text: "Arm execution?",
            enabled: true,
          },
          {
            // Confirmed present in the daemon catalog (routes/control_plane.rs); the GUI
            // must keep it out of the invokable action set in this patch.
            action_key: "flatten-paper-positions",
            label: "Flatten paper positions",
            level: 3,
            description: "Flatten all open paper positions immediately.",
            requires_reason: true,
            confirm_text: "Flatten all open paper positions?",
            enabled: true,
          },
        ],
      });
    }
    if (path in PSV_STRATEGY_FIXTURES) {
      return jsonResponse(PSV_STRATEGY_FIXTURES[path]);
    }
    return notFoundResponse();
  }) as typeof fetch;

  try {
    const model = await fetchOperatorModel();

    // Proof point 7: flatten-paper-positions must never reach the invokable catalog,
    // even though the daemon-authoritative catalog includes it.
    assert.equal(
      model.actionCatalog.some((a) => (a.action_key as string) === "flatten-paper-positions"),
      false,
      "flatten-paper-positions must never appear in the invokable action catalog",
    );
    assert.equal(model.autonomousPaperStatus.flatten_available, true, "fixture sanity: backend truth says flatten IS available");

    const html = renderToStaticMarkup(React.createElement(StrategyScreen, { model }));

    // StrategyScreen is a read-only visibility screen — it must render zero buttons,
    // so there is structurally no path by which it could invoke flatten (or anything else).
    // (The action key string itself MAY appear in the panel's own disclosure text — see below —
    // so the proof is the absence of any control element, not the absence of the name.)
    assert.doesNotMatch(html, /<button/i, "StrategyScreen must render no invocation controls of any kind");
    assert.doesNotMatch(html, /onClick/i, "StrategyScreen must wire no click handlers");

    // The visibility panel still honestly reports that flatten is available per backend truth —
    // it just provides no way to invoke it, and says so by name.
    assert.match(html, /Flatten available/);
    assert.match(html, /cannot invoke flatten-paper-positions/);
  } finally {
    globalThis.fetch = originalFetch;
  }
});

test("PSV-04: watchlist status renders an honest unavailable notice when the backend probe fails (never a fake healthy default)", async () => {
  const originalFetch = globalThis.fetch;
  const nowUtc = new Date().toISOString();

  globalThis.fetch = (async (input: string | URL | Request) => {
    const raw = typeof input === "string" ? input : input instanceof URL ? input.toString() : input.url;
    const path = new URL(raw).pathname;

    if (path === "/api/v1/system/status") {
      return jsonResponse({ ...DEFAULT_STATUS, daemon_reachable: true, last_heartbeat: new Date().toISOString() });
    }
    if (path === "/api/v1/system/preflight") {
      return jsonResponse({ ...DEFAULT_PREFLIGHT, daemon_reachable: true });
    }
    if (path === "/api/v1/autonomous/paper-status") {
      return jsonResponse({
        canonical_route: "/api/v1/autonomous/paper-status",
        truth_state: "not_applicable",
        mode: "paper",
        live_routing_enabled: false,
        runtime_status: "unknown",
        arm_state: "unknown",
        kill_switch_active: false,
        deadman_status: "unknown",
        ws_continuity: "not_applicable",
        reconcile_status: "unknown",
        mismatch_count: 0,
        open_order_count: 0,
        position_count: 0,
        current_symbol: null,
        current_position_qty: null,
        target_qty: null,
        computed_delta_qty: null,
        no_order_reason: "NOT_PAPER_ALPACA_DEPLOYMENT",
        last_strategy_decision: null,
        flatten_available: false,
        flatten_blockers: ["NOT_APPLICABLE: deployment is not paper+alpaca"],
        watchlist_outcome: "not_applicable",
        watchlist_approved: false,
        readiness_classification: "not_applicable",
        blockers: ["NOT_APPLICABLE: deployment is not paper+alpaca"],
        next_operator_action: "No action — autonomous paper trading is not applicable to this deployment.",
        autonomous_session_state: "not_applicable",
        now_utc: nowUtc,
      });
    }
    // watchlist/status mounted but unreachable in this scenario (e.g. network error / 404).
    if (path === "/api/v1/watchlist/status") {
      return notFoundResponse();
    }
    if (path in PSV_STRATEGY_FIXTURES) {
      return jsonResponse(PSV_STRATEGY_FIXTURES[path]);
    }
    return notFoundResponse();
  }) as typeof fetch;

  try {
    const model = await fetchOperatorModel();

    assert.equal(model.autonomousPaperStatus.truth_state, "not_applicable");
    assert.equal(model.watchlistStatus.truth_state, "unavailable");
    // Unavailable must never collapse into fabricated "approved"/healthy defaults.
    assert.equal(model.watchlistStatus.approved_for_autonomous_paper, false);
    assert.equal(model.watchlistStatus.approved_for_live, false);
    assert.equal(model.watchlistStatus.status, "unknown");

    const html = renderToStaticMarkup(React.createElement(StrategyScreen, { model }));

    // Honest "not_applicable" rendering for paper status — distinct from "active" or "unavailable".
    assert.match(html, /Autonomous paper status is not applicable for the current deployment mode/);
    // Honest "unavailable" rendering for watchlist status — never a fake healthy table.
    assert.match(html, /Watchlist status is currently unavailable/);
    assert.doesNotMatch(html, /MSFT|NVDA|AAPL/, "no symbol data should render when watchlist status is unavailable");
  } finally {
    globalThis.fetch = originalFetch;
  }
});

// ---------------------------------------------------------------------------
// MULTI-SYMBOL-OMS-OVERVIEW-AND-GUI-01: Multi-symbol dispatch summary panel
// (Patch 10). Read-only — reuses the Patch-9 route
// GET /api/v1/strategy/multi-symbol-dispatch-summary
// (MultiSymbolDispatchSummaryResponse / PerSymbolDispatchRow are unchanged by
// this patch). The panel must never gain order submit/cancel/replace/flatten
// controls or any approved_for_live/live-routing surface.
// ---------------------------------------------------------------------------

const MULTI_SYMBOL_DISPATCH_SUMMARY_ROUTE = "/api/v1/strategy/multi-symbol-dispatch-summary";

function notApplicablePaperStatusFixture(nowUtc: string) {
  return {
    canonical_route: "/api/v1/autonomous/paper-status",
    truth_state: "not_applicable",
    mode: "paper",
    live_routing_enabled: false,
    runtime_status: "unknown",
    arm_state: "unknown",
    kill_switch_active: false,
    deadman_status: "unknown",
    ws_continuity: "not_applicable",
    reconcile_status: "unknown",
    mismatch_count: 0,
    open_order_count: 0,
    position_count: 0,
    current_symbol: null,
    current_position_qty: null,
    target_qty: null,
    computed_delta_qty: null,
    no_order_reason: "NOT_PAPER_ALPACA_DEPLOYMENT",
    last_strategy_decision: null,
    flatten_available: false,
    flatten_blockers: ["NOT_APPLICABLE: deployment is not paper+alpaca"],
    watchlist_outcome: "not_applicable",
    watchlist_approved: false,
    readiness_classification: "not_applicable",
    blockers: ["NOT_APPLICABLE: deployment is not paper+alpaca"],
    next_operator_action: "No action — autonomous paper trading is not applicable to this deployment.",
    autonomous_session_state: "not_applicable",
    now_utc: nowUtc,
  };
}

function buildMultiSymbolDispatchSummaryFetchMock(multiSymbolResponse: () => Response): typeof fetch {
  const nowUtc = new Date().toISOString();
  return (async (input: string | URL | Request) => {
    const raw = typeof input === "string" ? input : input instanceof URL ? input.toString() : input.url;
    const path = new URL(raw).pathname;

    if (path === "/api/v1/system/status") {
      return jsonResponse({ ...DEFAULT_STATUS, daemon_reachable: true, last_heartbeat: new Date().toISOString() });
    }
    if (path === "/api/v1/system/preflight") {
      return jsonResponse({ ...DEFAULT_PREFLIGHT, daemon_reachable: true });
    }
    if (path === "/api/v1/autonomous/paper-status") {
      return jsonResponse(notApplicablePaperStatusFixture(nowUtc));
    }
    if (path === "/api/v1/watchlist/status") {
      return notFoundResponse();
    }
    if (path === MULTI_SYMBOL_DISPATCH_SUMMARY_ROUTE) {
      return multiSymbolResponse();
    }
    if (path in PSV_STRATEGY_FIXTURES) {
      return jsonResponse(PSV_STRATEGY_FIXTURES[path]);
    }
    return notFoundResponse();
  }) as typeof fetch;
}

const ACTIVE_MULTI_SYMBOL_DISPATCH_SUMMARY_FIXTURE = {
  canonical_route: MULTI_SYMBOL_DISPATCH_SUMMARY_ROUTE,
  backend: "daemon.runtime_state",
  truth_state: "active",
  runtime_execution_mode: "single_symbol",
  configured_symbol_count: 2,
  per_symbol: [
    {
      symbol: "AAPL",
      strategy_id: "swing_momentum",
      current_qty: 0,
      target_qty: 10,
      delta: 10,
      no_order_reason: "order_will_be_submitted",
      last_decision_id: "decision-001",
      last_decision_disposition: "accepted",
      day_order_count: 1,
      day_order_limit: 5,
      bar_staleness_secs: 12,
    },
    {
      symbol: "MSFT",
      strategy_id: "swing_momentum",
      current_qty: 3,
      target_qty: 3,
      delta: 0,
      no_order_reason: "max_new_orders_per_tick_reached",
      last_decision_id: null,
      last_decision_disposition: null,
      day_order_count: 5,
      day_order_limit: 5,
      bar_staleness_secs: null,
    },
  ],
};

const NO_SNAPSHOT_MULTI_SYMBOL_DISPATCH_SUMMARY_FIXTURE = {
  canonical_route: MULTI_SYMBOL_DISPATCH_SUMMARY_ROUTE,
  backend: "daemon.runtime_state",
  truth_state: "no_snapshot",
  runtime_execution_mode: "unknown",
  configured_symbol_count: 0,
  per_symbol: [],
};

test("GUIT01: fetchOperatorModel parses a normal multi-symbol-dispatch-summary response", async () => {
  const originalFetch = globalThis.fetch;
  globalThis.fetch = buildMultiSymbolDispatchSummaryFetchMock(() => jsonResponse(ACTIVE_MULTI_SYMBOL_DISPATCH_SUMMARY_FIXTURE));

  try {
    const model = await fetchOperatorModel();

    assert.equal(model.multiSymbolDispatchSummary.truth_state, "active");
    assert.equal(model.multiSymbolDispatchSummary.backend, "daemon.runtime_state");
    assert.equal(model.multiSymbolDispatchSummary.runtime_execution_mode, "single_symbol");
    assert.equal(model.multiSymbolDispatchSummary.configured_symbol_count, 2);
    assert.equal(model.multiSymbolDispatchSummary.per_symbol.length, 2);
    assert.equal(model.multiSymbolDispatchSummary.per_symbol[0].symbol, "AAPL");
    assert.equal(model.multiSymbolDispatchSummary.per_symbol[1].symbol, "MSFT");
  } finally {
    globalThis.fetch = originalFetch;
  }
});

test("GUIT02: StrategyScreen renders top-level multi-symbol dispatch summary status", async () => {
  const originalFetch = globalThis.fetch;
  globalThis.fetch = buildMultiSymbolDispatchSummaryFetchMock(() => jsonResponse(ACTIVE_MULTI_SYMBOL_DISPATCH_SUMMARY_FIXTURE));

  try {
    const model = await fetchOperatorModel();
    const html = renderToStaticMarkup(React.createElement(StrategyScreen, { model }));

    assert.match(html, /Multi-symbol dispatch summary/);
    assert.match(html, /Truth state<\/span><strong>active<\/strong>/);
    assert.match(html, /Backend<\/span><strong>daemon\.runtime_state<\/strong>/);
    assert.match(html, /Runtime execution mode<\/span><strong>single_symbol<\/strong>/);
    assert.match(html, /Configured symbol count<\/span><strong>2<\/strong>/);
  } finally {
    globalThis.fetch = originalFetch;
  }
});

test("GUIT03: StrategyScreen renders one per-symbol row each for AAPL and MSFT", async () => {
  const originalFetch = globalThis.fetch;
  globalThis.fetch = buildMultiSymbolDispatchSummaryFetchMock(() => jsonResponse(ACTIVE_MULTI_SYMBOL_DISPATCH_SUMMARY_FIXTURE));

  try {
    const model = await fetchOperatorModel();
    const html = renderToStaticMarkup(React.createElement(StrategyScreen, { model }));

    assert.match(html, /AAPL/);
    assert.match(html, /MSFT/);
    assert.match(html, /swing_momentum/);
    assert.match(html, /decision-001/);
    assert.match(html, /accepted/);
  } finally {
    globalThis.fetch = originalFetch;
  }
});

test("GUIT04: no_order_reason values render verbatim, including max_new_orders_per_tick_reached", async () => {
  const originalFetch = globalThis.fetch;
  globalThis.fetch = buildMultiSymbolDispatchSummaryFetchMock(() => jsonResponse(ACTIVE_MULTI_SYMBOL_DISPATCH_SUMMARY_FIXTURE));

  try {
    const model = await fetchOperatorModel();

    const reasons = model.multiSymbolDispatchSummary.per_symbol.map((row) => row.no_order_reason);
    assert.deepEqual(reasons, ["order_will_be_submitted", "max_new_orders_per_tick_reached"]);

    const html = renderToStaticMarkup(React.createElement(StrategyScreen, { model }));
    assert.match(html, /order_will_be_submitted/);
    assert.match(html, /max_new_orders_per_tick_reached/);
  } finally {
    globalThis.fetch = originalFetch;
  }
});

test("GUIT05: StrategyScreen shows the empty-state message when truth_state is no_snapshot with no per_symbol rows", async () => {
  const originalFetch = globalThis.fetch;
  globalThis.fetch = buildMultiSymbolDispatchSummaryFetchMock(() => jsonResponse(NO_SNAPSHOT_MULTI_SYMBOL_DISPATCH_SUMMARY_FIXTURE));

  try {
    const model = await fetchOperatorModel();

    assert.equal(model.multiSymbolDispatchSummary.truth_state, "no_snapshot");
    assert.equal(model.multiSymbolDispatchSummary.per_symbol.length, 0);

    const html = renderToStaticMarkup(React.createElement(StrategyScreen, { model }));
    assert.match(html, /No multi-symbol dispatch snapshot yet\./);
    assert.doesNotMatch(html, /AAPL|MSFT/, "no per-symbol rows should render for an empty no_snapshot response");
  } finally {
    globalThis.fetch = originalFetch;
  }
});

test("GUIT06: a failed multi-symbol-dispatch-summary probe degrades to fail-soft unavailable without blocking the rest of the screen", async () => {
  const originalFetch = globalThis.fetch;
  globalThis.fetch = buildMultiSymbolDispatchSummaryFetchMock(() => notFoundResponse());

  try {
    const model = await fetchOperatorModel();

    assert.equal(model.multiSymbolDispatchSummary.truth_state, "unavailable");
    assert.equal(model.multiSymbolDispatchSummary.per_symbol.length, 0);
    // Fetch failure on this endpoint must not hard-block the Strategy panel.
    assert.equal(panelTruthRenderState(model, "strategy"), null);

    const html = renderToStaticMarkup(React.createElement(StrategyScreen, { model }));
    assert.match(html, /Multi-symbol dispatch summary is currently unavailable/);
    // The rest of the screen still renders (engine posture panel heading present).
    assert.match(html, /Engine posture/);
  } finally {
    globalThis.fetch = originalFetch;
  }
});

test("GUIT07: multi-symbol dispatch summary panel carries no approved_for_live or live-routing surface", async () => {
  const originalFetch = globalThis.fetch;
  globalThis.fetch = buildMultiSymbolDispatchSummaryFetchMock(() => jsonResponse(ACTIVE_MULTI_SYMBOL_DISPATCH_SUMMARY_FIXTURE));

  try {
    const model = await fetchOperatorModel();

    assert.equal("approved_for_live" in model.multiSymbolDispatchSummary, false);
    assert.equal("live_routing_enabled" in model.multiSymbolDispatchSummary, false);

    const html = renderToStaticMarkup(React.createElement(StrategyScreen, { model }));
    assert.doesNotMatch(html, /approved_for_live/i);
    assert.doesNotMatch(html, /live[_-]routing/i);
  } finally {
    globalThis.fetch = originalFetch;
  }
});

test("GUIT08: multi-symbol dispatch summary panel contains no order submit/cancel/replace/flatten controls", async () => {
  const originalFetch = globalThis.fetch;
  globalThis.fetch = buildMultiSymbolDispatchSummaryFetchMock(() => jsonResponse(ACTIVE_MULTI_SYMBOL_DISPATCH_SUMMARY_FIXTURE));

  try {
    const model = await fetchOperatorModel();
    const html = renderToStaticMarkup(React.createElement(StrategyScreen, { model }));

    // Scope the assertion to the multi-symbol dispatch summary panel itself —
    // it is the last section on the screen, so slicing from its heading to
    // the end of the markup isolates its HTML from unrelated panels (e.g. the
    // "Flatten availability" panel's subtitle mentions flatten-paper-positions
    // as prose, not as a control).
    const panelStart = html.indexOf("Multi-symbol dispatch summary");
    assert.notEqual(panelStart, -1, "multi-symbol dispatch summary panel should be present");
    const panelHtml = html.slice(panelStart);

    // The panel is read-only visibility — no buttons or click handlers.
    assert.doesNotMatch(panelHtml, /<button/i);
    assert.doesNotMatch(panelHtml, /onClick/i);
    assert.doesNotMatch(panelHtml, /submit-order|cancel-order|replace-order|flatten-paper-positions/i);
  } finally {
    globalThis.fetch = originalFetch;
  }
});

// ---------------------------------------------------------------------------
// MULTI-STRATEGY-DRY-RUN-GUI-01: Multi-strategy dry-run diagnostics panel.
// Read-only — reuses the already-CLOSED route
// GET /api/v1/strategy/dry-run/status (MULTI-STRATEGY-DRY-RUN-STATUS-01).
// submitted is always false on every row. The panel must never gain order
// submit/cancel/replace controls, must never enable secondary-strategy order
// submission, and must never enable live shorting.
// ---------------------------------------------------------------------------

const DRY_RUN_STRATEGY_STATUS_ROUTE = "/api/v1/strategy/dry-run/status";

function buildDryRunStrategyStatusFetchMock(dryRunResponse: () => Response): typeof fetch {
  const nowUtc = new Date().toISOString();
  return (async (input: string | URL | Request) => {
    const raw = typeof input === "string" ? input : input instanceof URL ? input.toString() : input.url;
    const path = new URL(raw).pathname;

    if (path === "/api/v1/system/status") {
      return jsonResponse({ ...DEFAULT_STATUS, daemon_reachable: true, last_heartbeat: new Date().toISOString() });
    }
    if (path === "/api/v1/system/preflight") {
      return jsonResponse({ ...DEFAULT_PREFLIGHT, daemon_reachable: true });
    }
    if (path === "/api/v1/autonomous/paper-status") {
      return jsonResponse(notApplicablePaperStatusFixture(nowUtc));
    }
    if (path === "/api/v1/watchlist/status") {
      return notFoundResponse();
    }
    if (path === DRY_RUN_STRATEGY_STATUS_ROUTE) {
      return dryRunResponse();
    }
    if (path in PSV_STRATEGY_FIXTURES) {
      return jsonResponse(PSV_STRATEGY_FIXTURES[path]);
    }
    return notFoundResponse();
  }) as typeof fetch;
}

const NOT_CONFIGURED_DRY_RUN_STATUS_FIXTURE = {
  canonical_route: DRY_RUN_STRATEGY_STATUS_ROUTE,
  backend: "daemon.runtime_state",
  truth_state: "not_configured",
  configured_dry_run_strategy_ids: [],
  dry_run_strategy_diagnostics: [],
};

const ACTIVE_DRY_RUN_STATUS_FIXTURE = {
  canonical_route: DRY_RUN_STRATEGY_STATUS_ROUTE,
  backend: "daemon.runtime_state",
  truth_state: "active",
  configured_dry_run_strategy_ids: ["intraday_short_scalper"],
  dry_run_strategy_diagnostics: [
    {
      strategy_id: "intraday_short_scalper",
      symbol: "AAPL",
      timeframe_secs: 300,
      current_qty: 0,
      target_qty: -5,
      delta_qty: -5,
      decision: "order_would_be_submitted",
      reason: "bearish displacement exceeds threshold; would open short",
      would_classify_as: "ShortOpen",
      would_b5_block: true,
      would_policy_block: true,
      policy_reason_code: "short_entries_disabled",
      submitted: false,
      evaluated_at_utc: "2026-06-19T13:35:00+00:00",
    },
  ],
};

test("DRGT01: fetchOperatorModel parses not_configured dry-run status with empty diagnostics", async () => {
  const originalFetch = globalThis.fetch;
  globalThis.fetch = buildDryRunStrategyStatusFetchMock(() => jsonResponse(NOT_CONFIGURED_DRY_RUN_STATUS_FIXTURE));

  try {
    const model = await fetchOperatorModel();

    assert.equal(model.dryRunStrategyStatus.truth_state, "not_configured");
    assert.deepEqual(model.dryRunStrategyStatus.configured_dry_run_strategy_ids, []);
    assert.equal(model.dryRunStrategyStatus.dry_run_strategy_diagnostics.length, 0);
  } finally {
    globalThis.fetch = originalFetch;
  }
});

test("DRGT02: fetchOperatorModel parses active dry-run status with one intraday_short_scalper diagnostic row", async () => {
  const originalFetch = globalThis.fetch;
  globalThis.fetch = buildDryRunStrategyStatusFetchMock(() => jsonResponse(ACTIVE_DRY_RUN_STATUS_FIXTURE));

  try {
    const model = await fetchOperatorModel();

    assert.equal(model.dryRunStrategyStatus.truth_state, "active");
    assert.deepEqual(model.dryRunStrategyStatus.configured_dry_run_strategy_ids, ["intraday_short_scalper"]);
    assert.equal(model.dryRunStrategyStatus.dry_run_strategy_diagnostics.length, 1);

    const row = model.dryRunStrategyStatus.dry_run_strategy_diagnostics[0];
    assert.equal(row.strategy_id, "intraday_short_scalper");
    assert.equal(row.symbol, "AAPL");
    assert.equal(row.target_qty, -5);
    assert.equal(row.would_b5_block, true);
    assert.equal(row.would_policy_block, true);
    assert.equal(row.policy_reason_code, "short_entries_disabled");
    assert.equal(row.submitted, false);
  } finally {
    globalThis.fetch = originalFetch;
  }
});

test("DRGT03: StrategyScreen renders the DRY RUN ONLY safety label", async () => {
  const originalFetch = globalThis.fetch;
  globalThis.fetch = buildDryRunStrategyStatusFetchMock(() => jsonResponse(ACTIVE_DRY_RUN_STATUS_FIXTURE));

  try {
    const model = await fetchOperatorModel();
    const html = renderToStaticMarkup(React.createElement(StrategyScreen, { model }));

    assert.match(html, /DRY RUN ONLY/);
  } finally {
    globalThis.fetch = originalFetch;
  }
});

test("DRGT04: StrategyScreen renders the configured intraday_short_scalper strategy id", async () => {
  const originalFetch = globalThis.fetch;
  globalThis.fetch = buildDryRunStrategyStatusFetchMock(() => jsonResponse(ACTIVE_DRY_RUN_STATUS_FIXTURE));

  try {
    const model = await fetchOperatorModel();
    const html = renderToStaticMarkup(React.createElement(StrategyScreen, { model }));

    assert.match(html, /intraday_short_scalper/);
  } finally {
    globalThis.fetch = originalFetch;
  }
});

test("DRGT05: StrategyScreen renders the negative target qty", async () => {
  const originalFetch = globalThis.fetch;
  globalThis.fetch = buildDryRunStrategyStatusFetchMock(() => jsonResponse(ACTIVE_DRY_RUN_STATUS_FIXTURE));

  try {
    const model = await fetchOperatorModel();
    const html = renderToStaticMarkup(React.createElement(StrategyScreen, { model }));

    assert.match(html, />-5</);
  } finally {
    globalThis.fetch = originalFetch;
  }
});

test("DRGT06: StrategyScreen renders submitted=false in the safety banner and the row value", async () => {
  const originalFetch = globalThis.fetch;
  globalThis.fetch = buildDryRunStrategyStatusFetchMock(() => jsonResponse(ACTIVE_DRY_RUN_STATUS_FIXTURE));

  try {
    const model = await fetchOperatorModel();
    const html = renderToStaticMarkup(React.createElement(StrategyScreen, { model }));

    assert.match(html, /submitted=false/);
    assert.match(html, /val-positive">false/);
  } finally {
    globalThis.fetch = originalFetch;
  }
});

test("DRGT07: StrategyScreen renders the B5 block status", async () => {
  const originalFetch = globalThis.fetch;
  globalThis.fetch = buildDryRunStrategyStatusFetchMock(() => jsonResponse(ACTIVE_DRY_RUN_STATUS_FIXTURE));

  try {
    const model = await fetchOperatorModel();
    const html = renderToStaticMarkup(React.createElement(StrategyScreen, { model }));

    assert.match(html, /B5 Block/);
    assert.match(html, /val-warn">true/);
  } finally {
    globalThis.fetch = originalFetch;
  }
});

test("DRGT08: StrategyScreen renders the short_entries_disabled policy block reason", async () => {
  const originalFetch = globalThis.fetch;
  globalThis.fetch = buildDryRunStrategyStatusFetchMock(() => jsonResponse(ACTIVE_DRY_RUN_STATUS_FIXTURE));

  try {
    const model = await fetchOperatorModel();
    const html = renderToStaticMarkup(React.createElement(StrategyScreen, { model }));

    assert.match(html, /Policy Block/);
    assert.match(html, /short_entries_disabled/);
  } finally {
    globalThis.fetch = originalFetch;
  }
});

test("DRGT09: StrategyScreen shows the empty-state message when no dry-run strategies are configured", async () => {
  const originalFetch = globalThis.fetch;
  globalThis.fetch = buildDryRunStrategyStatusFetchMock(() => jsonResponse(NOT_CONFIGURED_DRY_RUN_STATUS_FIXTURE));

  try {
    const model = await fetchOperatorModel();
    const html = renderToStaticMarkup(React.createElement(StrategyScreen, { model }));

    assert.match(html, /No dry-run strategies configured\. Set MQK_DRY_RUN_STRATEGY_IDS to enable diagnostics\./);
    assert.doesNotMatch(
      html,
      /intraday_short_scalper/,
      "no configured strategy id should render when the daemon reports not_configured",
    );
  } finally {
    globalThis.fetch = originalFetch;
  }
});

test("DRGT10: a failed dry-run-status probe degrades to fail-soft unavailable without blocking the rest of the screen", async () => {
  const originalFetch = globalThis.fetch;
  globalThis.fetch = buildDryRunStrategyStatusFetchMock(() => notFoundResponse());

  try {
    const model = await fetchOperatorModel();

    assert.equal(model.dryRunStrategyStatus.truth_state, "unavailable");
    assert.equal(model.dryRunStrategyStatus.dry_run_strategy_diagnostics.length, 0);
    // Fetch failure on this endpoint must not hard-block the Strategy panel.
    assert.equal(panelTruthRenderState(model, "strategy"), null);

    const html = renderToStaticMarkup(React.createElement(StrategyScreen, { model }));
    assert.match(html, /Dry-run strategy diagnostics are currently unavailable/);
    // The rest of the screen still renders (engine posture panel heading present).
    assert.match(html, /Engine posture/);
  } finally {
    globalThis.fetch = originalFetch;
  }
});

test("DRGT11: dry-run diagnostics panel contains no order submit/cancel/replace controls", async () => {
  const originalFetch = globalThis.fetch;
  globalThis.fetch = buildDryRunStrategyStatusFetchMock(() => jsonResponse(ACTIVE_DRY_RUN_STATUS_FIXTURE));

  try {
    const model = await fetchOperatorModel();
    const html = renderToStaticMarkup(React.createElement(StrategyScreen, { model }));

    const panelStart = html.indexOf("Multi-strategy dry-run diagnostics");
    assert.notEqual(panelStart, -1, "dry-run diagnostics panel should be present");
    const panelHtml = html.slice(panelStart);

    // The panel is read-only visibility — no buttons or click handlers, and no
    // path to submit, cancel, or replace an order from this surface.
    assert.doesNotMatch(panelHtml, /<button/i);
    assert.doesNotMatch(panelHtml, /onClick/i);
    assert.doesNotMatch(panelHtml, /submit-order|cancel-order|replace-order/i);
  } finally {
    globalThis.fetch = originalFetch;
  }
});
