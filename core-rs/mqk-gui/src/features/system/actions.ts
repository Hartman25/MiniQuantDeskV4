// core-rs/mqk-gui/src/features/system/actions.ts
//
// Operator action dispatch: canonical → legacy fallback chain.
// Exports invokeOperatorAction and the two helper functions it depends on.

import { postJson, type EndpointPostResult } from "./http";
import { legacyActionPaths } from "./legacy";
import type { OperatorActionReceipt, SystemStatus } from "./types";

// ---------------------------------------------------------------------------
// Private daemon response shapes used only in this module
// ---------------------------------------------------------------------------

interface LegacyDaemonStatusSnapshot {
  daemon_uptime_secs: number;
  active_run_id: string | null;
  state: string;
  notes?: string | null;
  integrity_armed: boolean;
}

interface LegacyIntegrityResponse {
  armed: boolean;
  active_run_id: string | null;
  state: string;
}

interface DaemonOperatorActionResponse {
  requested_action: string;
  accepted: boolean;
  disposition: string;
  warnings?: string[];
  environment?: SystemStatus["environment"];
  audit?: {
    audit_event_id?: string | null;
  };
}

// ---------------------------------------------------------------------------
// Helper: map any daemon action response shape to OperatorActionReceipt
// ---------------------------------------------------------------------------

function mapLegacyOperatorActionResponse(
  actionKey: string,
  response: EndpointPostResult<unknown>,
): OperatorActionReceipt | null {
  if (!response.ok) return null;

  const payload = response.data as
    | Partial<OperatorActionReceipt & LegacyDaemonStatusSnapshot & LegacyIntegrityResponse>
    | DaemonOperatorActionResponse
    | undefined;
  if (!payload || typeof payload !== "object") {
    return {
      ok: true,
      action_key: actionKey,
      environment: "paper",
      live_routing_enabled: false,
      result_state: "accepted",
      warnings: ["Operator action completed but returned no JSON payload."],
      audit_reference: null,
      blocking_failures: [],
    };
  }

  if ("requested_action" in payload || "disposition" in payload) {
    const operatorPayload = payload as DaemonOperatorActionResponse;
    return {
      ok: operatorPayload.accepted ?? true,
      action_key: operatorPayload.requested_action ?? actionKey,
      environment: operatorPayload.environment ?? "paper",
      live_routing_enabled: false,
      result_state: operatorPayload.disposition ?? "accepted",
      warnings: operatorPayload.warnings ?? [],
      audit_reference: operatorPayload.audit?.audit_event_id ?? null,
      blocking_failures: [],
    };
  }

  if ("action_key" in payload || "result_state" in payload) {
    return {
      ok: payload.ok ?? true,
      action_key: payload.action_key ?? actionKey,
      environment: payload.environment ?? "paper",
      live_routing_enabled: payload.live_routing_enabled ?? false,
      result_state: payload.result_state ?? "accepted",
      warnings: payload.warnings ?? [],
      audit_reference: payload.audit_reference ?? null,
      blocking_failures: payload.blocking_failures ?? [],
    };
  }

  if ("armed" in payload || "active_run_id" in payload || "state" in payload) {
    return {
      ok: true,
      action_key: actionKey,
      environment: "paper",
      live_routing_enabled: false,
      result_state: String(payload.state ?? "accepted"),
      warnings: [],
      audit_reference: null,
      blocking_failures: [],
    };
  }

  return null;
}

// ---------------------------------------------------------------------------
// Helper: construct a failed receipt from a post error
// ---------------------------------------------------------------------------

function failedOperatorActionReceipt(
  actionKey: string,
  failure: EndpointPostResult<unknown>,
  targetEnvironment: SystemStatus["environment"] = "paper",
): OperatorActionReceipt {
  const blockingFailures: string[] = [];
  const warnings: string[] = [];
  let resultState = "unavailable";

  if (failure.status === 401) {
    resultState = "unauthorized";
    blockingFailures.push("Daemon refused operator action: valid Bearer token required.");
  } else if (failure.status === 403) {
    resultState = "refused";
    blockingFailures.push("Daemon refused operator action at the gate.");
  } else if (failure.status === 404) {
    blockingFailures.push(`Operator action endpoint missing for ${actionKey}.`);
  } else if (failure.error) {
    blockingFailures.push(`Operator action failed: ${failure.error}`);
  } else {
    blockingFailures.push(`Operator action failed for ${actionKey}.`);
  }

  warnings.push(`Last attempted endpoint: ${failure.endpoint}`);

  return {
    ok: false,
    action_key: actionKey,
    environment: targetEnvironment,
    live_routing_enabled: false,
    result_state: resultState,
    warnings,
    audit_reference: null,
    blocking_failures: blockingFailures,
  };
}

// ---------------------------------------------------------------------------
// Public: invoke an operator action
// ---------------------------------------------------------------------------

export async function invokeOperatorAction(
  actionKey: string,
  params: Record<string, unknown>,
): Promise<OperatorActionReceipt> {
  // Try the canonical dispatcher first. A 400/403/409 from canonical is a
  // definitive daemon decision and MUST NOT fall through to legacy paths.
  // Only fall back to legacy when canonical was unreachable (network error,
  // status === undefined) or explicitly absent (status === 404).
  const canonicalResult = await postJson<Partial<OperatorActionReceipt> | LegacyDaemonStatusSnapshot | LegacyIntegrityResponse>(
    ["/api/v1/ops/action"],
    { action_key: actionKey, ...params },
  );

  const canonicalDefinitive = canonicalResult.ok || (canonicalResult.status !== undefined && canonicalResult.status !== 404);
  if (canonicalDefinitive) {
    const mapped = mapLegacyOperatorActionResponse(actionKey, canonicalResult);
    if (mapped) return mapped;
    return failedOperatorActionReceipt(actionKey, canonicalResult);
  }

  // Canonical was not found (404) or was unreachable (no status = network error).
  // Fall back to legacy action paths for older daemon versions.
  const legacyPaths = legacyActionPaths(actionKey);
  if (legacyPaths.length === 0) {
    return failedOperatorActionReceipt(actionKey, canonicalResult);
  }

  const legacyResult = await postJson<Partial<OperatorActionReceipt> | LegacyDaemonStatusSnapshot | LegacyIntegrityResponse>(
    legacyPaths,
    { action_key: actionKey, ...params },
  );

  const mapped = mapLegacyOperatorActionResponse(actionKey, legacyResult);
  if (mapped) return mapped;
  return failedOperatorActionReceipt(actionKey, legacyResult);
}
