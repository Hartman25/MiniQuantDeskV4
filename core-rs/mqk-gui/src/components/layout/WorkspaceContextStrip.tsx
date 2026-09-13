// OT-MQD-02: persistent, compact indicator of the current global linked
// workspace identity. Presentation only — Reset clears local React context
// state, nothing else.

import { useWorkspaceContext } from "../../features/workspace/WorkspaceContext.tsx";

export function WorkspaceContextStrip() {
  const { linked, resetLinked } = useWorkspaceContext();
  const hasIdentity =
    linked.symbol != null ||
    linked.strategyId != null ||
    linked.runId != null ||
    linked.timeframe != null ||
    linked.executionDomain != null;

  return (
    <div className="workspace-context-strip" role="status" aria-label="Workspace linked context">
      <span className="legend-pill status-neutral">LINKED</span>
      <span className="workspace-context-field">{linked.symbol ?? "—"}</span>
      <span className="workspace-context-sep">·</span>
      <span className="workspace-context-field">{linked.strategyId ?? "—"}</span>
      <span className="workspace-context-sep">·</span>
      <span className="workspace-context-field">{linked.timeframe ?? "—"}</span>
      <span className="workspace-context-sep">·</span>
      <span className="workspace-context-field">
        {linked.executionDomain ?? "—"}
        {linked.runId ? ` ${linked.runId.slice(0, 12)}` : ""}
      </span>
      {hasIdentity && (
        <button type="button" className="workspace-context-reset" onClick={resetLinked}>
          Reset
        </button>
      )}
    </div>
  );
}
