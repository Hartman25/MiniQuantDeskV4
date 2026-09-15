// R2 — UNIFIED-MQD-EVIDENCE-CHART-01: the Unified Evidence / Forensics panel.
// Strictly READ ONLY — no operator/trading action of any kind. Follows the
// currently-linked WorkspaceIdentity's runId (see WorkspaceContext.tsx,
// corrected for late window join by R1) and fetches the real, wired
// execution-flow evidence for exactly that run, never a server-side
// "whatever's active" default — an unlinked workspace shows an explicit
// prompt rather than silently picking a run.
//
// Backtest Results keeps its own separate EvidenceChart integration
// (folder-based artifact loading, decoupled from workspace identity) — this
// screen is the SECOND real usage of the same EvidenceChart component,
// proving the model/component genuinely generalizes across lifecycle
// sources (see executionEvidenceModel.ts).

import { useEffect, useState } from "react";
import { EvidenceChart } from "./EvidenceChart.tsx";
import { buildExecutionEvidenceChartModel } from "./executionEvidenceModel.ts";
import { fetchExecutionFlow } from "../../features/system/api.ts";
import type { ExecutionFlowSurface } from "../../features/system/types/execution.ts";
import { useWorkspaceContext } from "../../features/workspace/WorkspaceContext.tsx";

type FetchState =
  | { kind: "idle" }
  | { kind: "loading" }
  | { kind: "loaded"; surface: ExecutionFlowSurface }
  | { kind: "error" };

export function EvidenceForensicsScreen() {
  const { linked } = useWorkspaceContext();
  const runId = linked.runId;
  const [state, setState] = useState<FetchState>({ kind: "idle" });

  useEffect(() => {
    if (runId == null) {
      setState({ kind: "idle" });
      return;
    }
    let cancelled = false;
    setState({ kind: "loading" });
    void fetchExecutionFlow({ runId }).then((surface) => {
      // A stale response for a run the operator has since switched away from
      // must never overwrite state for the (by then) different linked run —
      // this is workspace isolation enforced at the fetch race, not just the
      // model layer.
      if (cancelled) return;
      setState(surface == null ? { kind: "error" } : { kind: "loaded", surface });
    });
    return () => {
      cancelled = true;
    };
  }, [runId]);

  if (runId == null) {
    return (
      <div className="screen-grid desk-screen-grid">
        <div className="empty-state">
          No run is linked in the current workspace. Link a run (e.g. from Backtest Results' "Link to workspace"
          control) to view unified execution evidence here.
        </div>
      </div>
    );
  }

  if (state.kind === "idle" || state.kind === "loading") {
    return (
      <div className="screen-grid desk-screen-grid">
        <div className="empty-state">Loading execution evidence for run {runId}…</div>
      </div>
    );
  }

  if (state.kind === "error") {
    return (
      <div className="screen-grid desk-screen-grid">
        <div className="unavailable-notice">
          Execution evidence request failed (network or backend unavailable) for run {runId}.
        </div>
      </div>
    );
  }

  const model = buildExecutionEvidenceChartModel(state.surface, runId);
  return (
    <div className="screen-grid desk-screen-grid">
      <EvidenceChart model={model} />
    </div>
  );
}
