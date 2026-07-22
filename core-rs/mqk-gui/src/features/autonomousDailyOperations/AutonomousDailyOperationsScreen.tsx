// core-rs/mqk-gui/src/features/autonomousDailyOperations/AutonomousDailyOperationsScreen.tsx
//
// AUTONOMOUS-DAILY-PAPER-OPERATIONS-01F1-GUI-DAILY-OPERATION-TRUTH-PROJECTION
//
// Read-only operator screen for the accepted E4 daily-operation API
// (GET /api/v1/autonomous/daily-operation[s]). Consumes, never reinterprets,
// the daemon's projected truth: no classifier/finalizer logic is re-derived
// here, no evidence count is ever displayed as "0" when the daemon reported
// it as unavailable (null), and no control on this screen can mutate
// operation state, trigger a trade, or invoke a privileged action route.

import { Panel } from "../../components/common/Panel";
import { StatCard } from "../../components/common/StatCard";
import { formatDateTime } from "../../lib/format";
import { classifyDailyOperationsSourceAuthority } from "../system/sourceAuthority";
import type {
  AutonomousDailyOperationApiRow,
  AutonomousDailyOperationSurface,
  AutonomousDailyOperationsSurface,
  SystemModel,
} from "../system/types";
import { formatDailyOperationCount } from "./formatDailyOperationCount";

type Tone = "neutral" | "good" | "warn" | "bad";

interface TruthNotice {
  title: string;
  detail: string;
  tone: Tone;
}

// Maps each surface's own transport_state/truth_state pair to an operator
// notice. Returns null when the surface carries renderable truth (active, or
// an authoritative not_found the caller renders itself). A GUI-side
// transport failure (endpoint_unavailable) is always reported distinctly
// from the daemon's own backend_unavailable/query_failed truth states.
function truthNoticeFor(
  surfaceLabel: string,
  transportState: "available" | "endpoint_unavailable",
  truthState: string | null,
): TruthNotice | null {
  if (transportState === "endpoint_unavailable") {
    return {
      title: "Endpoint unavailable",
      detail: `${surfaceLabel} could not be reached. This is a GUI-side transport failure, not an authoritative daemon truth_state.`,
      tone: "bad",
    };
  }
  if (truthState === "backend_unavailable") {
    return {
      title: "Backend unavailable",
      detail: `${surfaceLabel}: the daemon has no database pool configured.`,
      tone: "bad",
    };
  }
  if (truthState === "query_failed") {
    return {
      title: "Query failed",
      detail: `${surfaceLabel}: a required database read failed on the daemon.`,
      tone: "bad",
    };
  }
  return null;
}

function DailyOperationTruthNotice({ title, detail, tone }: TruthNotice) {
  return (
    <div className={`empty-state truth-state-notice truth-state-daily-op-${tone}`} role="status" aria-live="polite">
      <strong>{title}</strong>
      <div>{detail}</div>
    </div>
  );
}

function finalizationTone(row: AutonomousDailyOperationApiRow): Tone {
  switch (row.finalization_status) {
    case "finalized":
      return row.evidence_state === "complete" ? "good" : "warn";
    case "blocked_insufficient_evidence":
      return "warn";
    case "awaiting_finalization":
    case "not_yet_eligible":
    default:
      return "neutral";
  }
}

function evidenceTone(evidenceState: string): Tone {
  switch (evidenceState) {
    case "complete":
      return "good";
    case "degraded":
    case "unavailable":
      return "warn";
    case "pending":
    default:
      return "neutral";
  }
}

function outcomeLabel(row: AutonomousDailyOperationApiRow): string {
  if (row.outcome_class == null) return "—";
  return row.outcome_reason_code ? `${row.outcome_class} (${row.outcome_reason_code})` : row.outcome_class;
}

function CurrentOperationPanel({ surface }: { surface: AutonomousDailyOperationSurface }) {
  const notice = truthNoticeFor("Current daily operation", surface.transport_state, surface.truth_state);

  if (notice) {
    return (
      <Panel
        title="Current Operation"
        subtitle="Durable outcome truth for the current canonical market-date slot."
      >
        <DailyOperationTruthNotice {...notice} />
      </Panel>
    );
  }

  if (surface.truth_state === "not_found" || surface.operation == null) {
    return (
      <Panel
        title="Current Operation"
        subtitle="Durable outcome truth for the current canonical market-date slot."
      >
        <div className="empty-state" role="status">
          No autonomous daily operation exists for the current canonical slot.
        </div>
      </Panel>
    );
  }

  const row = surface.operation;
  const fTone = finalizationTone(row);
  const eTone = evidenceTone(row.evidence_state);

  return (
    <Panel
      title="Current Operation"
      subtitle="Durable outcome truth for the current canonical market-date slot."
    >
      <div className="summary-grid summary-grid-four">
        <StatCard title="Finalization" value={row.finalization_status} tone={fTone} />
        <StatCard title="Outcome" value={outcomeLabel(row)} tone={fTone} />
        <StatCard title="Evidence" value={row.evidence_state} tone={eTone} />
        <StatCard title="Operation state" value={row.state} tone="neutral" />
      </div>

      {row.evidence_blockers.length > 0 && (
        <div className="list-stack" style={{ marginTop: 12 }}>
          {row.evidence_blockers.map((blocker) => (
            <div key={blocker} className="list-row">
              <strong style={{ color: "var(--warning)" }}>Evidence blocker</strong>
              <span>{blocker}</span>
            </div>
          ))}
        </div>
      )}

      <div className="metric-list" style={{ marginTop: 12 }}>
        <div>
          <span>Market date</span>
          <strong>{row.market_date}</strong>
        </div>
        <div>
          <span>Deployment mode</span>
          <strong>{row.deployment_mode}</strong>
        </div>
        <div>
          <span>Adapter</span>
          <strong>{row.adapter_id}</strong>
        </div>
        <div>
          <span>Operation ID</span>
          <strong>{row.operation_id}</strong>
        </div>
        <div>
          <span>Run ID</span>
          <strong>{row.run_id ?? "—"}</strong>
        </div>
        <div>
          <span>State reason</span>
          <strong>{row.state_reason_code ?? "—"}</strong>
        </div>
        <div>
          <span>Bars observed</span>
          <strong>{row.bars_observed}</strong>
        </div>
        <div>
          <span>Bars dispatched</span>
          <strong>{row.bars_dispatched}</strong>
        </div>
        <div>
          <span>Last completed bar</span>
          <strong>{row.last_completed_bar_ts ?? "—"}</strong>
        </div>
        <div>
          <span>Last dispatched bar</span>
          <strong>{row.last_dispatched_bar_ts ?? "—"}</strong>
        </div>
        <div>
          <span>Strategy evaluations</span>
          <strong>{formatDailyOperationCount(row.strategy_evaluation_count)}</strong>
        </div>
        <div>
          <span>Order activity</span>
          <strong>{formatDailyOperationCount(row.order_activity_count)}</strong>
        </div>
        <div>
          <span>Fills</span>
          <strong>{formatDailyOperationCount(row.fill_count)}</strong>
        </div>
        <div>
          <span>Created</span>
          <strong>{formatDateTime(row.created_at_utc)}</strong>
        </div>
        <div>
          <span>Updated</span>
          <strong>{formatDateTime(row.updated_at_utc)}</strong>
        </div>
        <div>
          <span>Finalized</span>
          <strong>{row.finalized_at_utc ? formatDateTime(row.finalized_at_utc) : "—"}</strong>
        </div>
      </div>
    </Panel>
  );
}

function HistoryPanel({ surface }: { surface: AutonomousDailyOperationsSurface }) {
  const notice = truthNoticeFor("Recent operations", surface.transport_state, surface.truth_state);
  // A top-level query_failed still returns bounded partial rows per the E4
  // contract — the panel must keep them visible, clearly labeled partial,
  // rather than hard-blocking on the same notice a fully unavailable surface
  // would show.
  const isPartial = surface.transport_state === "available" && surface.truth_state === "query_failed";

  if (notice && !isPartial) {
    return (
      <Panel title="Recent History" subtitle="Most recent autonomous daily operations, daemon order preserved.">
        <DailyOperationTruthNotice {...notice} />
      </Panel>
    );
  }

  return (
    <Panel
      title="Recent History"
      subtitle={`Most recent autonomous daily operations (limit ${surface.effective_limit ?? "—"}).`}
    >
      {isPartial && (
        <DailyOperationTruthNotice
          title="Partial — some rows unavailable"
          detail="A downstream read failed for at least one row. Rows below with unavailable evidence are not fully verified; this list is not a complete authoritative history."
          tone="warn"
        />
      )}
      {surface.rows.length === 0 ? (
        <div className="empty-state" role="status">
          {surface.truth_state === "active"
            ? "No autonomous daily operations recorded yet."
            : "History unavailable."}
        </div>
      ) : (
        <div className="table-grid table-cols-9">
          <div className="table-row table-head">
            <span>Market date</span>
            <span>State</span>
            <span>Finalization</span>
            <span>Outcome</span>
            <span>Evidence</span>
            <span>Evaluations</span>
            <span>Order activity</span>
            <span>Fills</span>
            <span>Updated</span>
          </div>
          {surface.rows.map((row) => (
            <div className="table-row" key={row.operation_id}>
              <span>{row.market_date}</span>
              <span>{row.state}</span>
              <span>{row.finalization_status}</span>
              <span>{outcomeLabel(row)}</span>
              <span>{row.evidence_state}</span>
              <span>{formatDailyOperationCount(row.strategy_evaluation_count)}</span>
              <span>{formatDailyOperationCount(row.order_activity_count)}</span>
              <span>{formatDailyOperationCount(row.fill_count)}</span>
              <span>{formatDateTime(row.finalized_at_utc ?? row.updated_at_utc)}</span>
            </div>
          ))}
        </div>
      )}
    </Panel>
  );
}

export function AutonomousDailyOperationsScreen({ model }: { model: SystemModel }) {
  const single = model.autonomousDailyOperation;
  const history = model.autonomousDailyOperations;
  const authority = classifyDailyOperationsSourceAuthority(single, history);

  if (authority.bothUnavailable) {
    const notice = truthNoticeFor("Daily operation truth", single.transport_state, single.truth_state) ?? {
      title: "Unavailable",
      detail: "Autonomous daily operation truth is currently unavailable.",
      tone: "bad" as Tone,
    };
    return (
      <div className="screen-grid desk-screen-grid">
        <Panel
          title="Daily Operations"
          subtitle="Durable autonomous paper-day state, final outcome, evidence posture, and recent history."
        >
          <DailyOperationTruthNotice {...notice} />
        </Panel>
      </div>
    );
  }

  return (
    <div className="screen-grid desk-screen-grid">
      <CurrentOperationPanel surface={single} />
      <HistoryPanel surface={history} />
    </div>
  );
}
