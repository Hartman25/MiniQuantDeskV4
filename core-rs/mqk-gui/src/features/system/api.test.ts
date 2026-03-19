import test from "node:test";
import assert from "node:assert/strict";
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
