// core-rs/mqk-gui/src/features/execution/components/ExecutionFlowPanel.tsx
//
// FLOW-04 / FLOW-05: Read-only execution flow panel.
//
// Displays a unified, time-ordered table of execution lifecycle events
// assembled from oms_outbox, oms_order_lifecycle_events, and
// fill_quality_telemetry.
//
// Hard constraints:
// - Read-only. No action buttons, no approval/retry controls.
// - Does not claim authoritative truth when backend is unavailable.
// - Shows explicit unavailable / empty / present states.
// - No synthetic broker events.

import { useEffect, useState } from "react";
import { DataTable } from "../../../components/common/DataTable";
import { Panel } from "../../../components/common/Panel";
import { formatDateTime } from "../../../lib/format";
import { fetchExecutionFlow } from "../../system/api";
import type { ExecutionFlowRow, ExecutionFlowSurface } from "../../system/types";

// ---------------------------------------------------------------------------
// Truth state notice
// ---------------------------------------------------------------------------

function flowUnavailableNotice(surface: ExecutionFlowSurface): string | null {
  switch (surface.truth_state) {
    case "no_db":
      return "No database connection — execution flow unavailable. Connect a DB to view the order lifecycle timeline.";
    case "no_active_run":
      return "No active run — execution flow is only available during or after an active run. Use the order_id filter to look up a specific order from a prior run.";
    default:
      return null;
  }
}

// ---------------------------------------------------------------------------
// Severity badge
// ---------------------------------------------------------------------------

function SeverityBadge({ severity }: { severity: string }) {
  const cls =
    severity === "error"
      ? "severity-badge severity-error"
      : severity === "warn"
        ? "severity-badge severity-warn"
        : "severity-badge severity-info";
  return <span className={cls}>{severity}</span>;
}

// ---------------------------------------------------------------------------
// Filters (FLOW-05)
// ---------------------------------------------------------------------------

interface FlowFilters {
  orderId: string;
  limit: number;
}

const LIMIT_OPTIONS = [25, 50, 100, 200] as const;

// ---------------------------------------------------------------------------
// Main panel
// ---------------------------------------------------------------------------

/**
 * Read-only execution flow panel.
 *
 * Shows a compact vertical table of lifecycle events assembled from
 * existing durable tables. Filters: order_id text, limit selector.
 *
 * Props:
 * - `runId`    — current active run UUID (from SystemModel or parent state).
 *               Pass `null` when no active run exists.
 * - `orderId`  — optional pre-selected order ID (when an order row is selected
 *               in the in-flight orders table above this panel).
 */
export function ExecutionFlowPanel({
  runId,
  orderId: externalOrderId,
}: {
  runId: string | null;
  orderId?: string | null;
}) {
  const [surface, setSurface] = useState<ExecutionFlowSurface | null>(null);
  const [loading, setLoading] = useState(false);
  const [filters, setFilters] = useState<FlowFilters>({
    orderId: externalOrderId ?? "",
    limit: 100,
  });

  // Sync external order selection into local filter.
  useEffect(() => {
    if (externalOrderId != null && externalOrderId !== filters.orderId) {
      setFilters((f) => ({ ...f, orderId: externalOrderId }));
    }
  }, [externalOrderId]);

  // Fetch whenever run, order filter, or limit changes.
  useEffect(() => {
    let cancelled = false;
    setLoading(true);

    const params: { runId?: string; orderId?: string; limit?: number } = {
      limit: filters.limit,
    };
    if (runId) params.runId = runId;
    if (filters.orderId.trim()) params.orderId = filters.orderId.trim();

    fetchExecutionFlow(params).then((result) => {
      if (!cancelled) {
        setSurface(result);
        setLoading(false);
      }
    });

    return () => {
      cancelled = true;
    };
  }, [runId, filters.orderId, filters.limit]);

  // ---------------------------------------------------------------------------
  // Render
  // ---------------------------------------------------------------------------

  const unavailableNotice = surface ? flowUnavailableNotice(surface) : null;
  const rows: ExecutionFlowRow[] = surface?.rows ?? [];

  const subtitle = surface
    ? surface.truth_state === "active"
      ? `Run ${surface.run_id ?? "unknown"} · ${surface.backend}`
      : `truth_state: ${surface.truth_state}`
    : loading
      ? "Loading…"
      : "Awaiting fetch";

  return (
    <Panel
      title="Execution flow"
      subtitle={subtitle}
    >
      {/* FLOW-05: Filters */}
      <div className="flow-filter-bar">
        <label className="flow-filter-field">
          <span>Order / Intent ID</span>
          <input
            type="text"
            className="flow-filter-input"
            placeholder="Filter by order_id or idempotency_key"
            value={filters.orderId}
            onChange={(e) => setFilters((f) => ({ ...f, orderId: e.target.value }))}
          />
        </label>
        <label className="flow-filter-field">
          <span>Limit</span>
          <select
            className="flow-filter-select"
            value={filters.limit}
            onChange={(e) =>
              setFilters((f) => ({ ...f, limit: Number(e.target.value) }))
            }
          >
            {LIMIT_OPTIONS.map((n) => (
              <option key={n} value={n}>
                {n}
              </option>
            ))}
          </select>
        </label>
      </div>

      {/* Unavailable state: explicit notice, not empty table */}
      {unavailableNotice ? (
        <div className="unavailable-notice">{unavailableNotice}</div>
      ) : surface === null && !loading ? (
        <div className="unavailable-notice">
          Execution flow backend unavailable — no response from daemon.
        </div>
      ) : loading ? (
        <div className="empty-state">Loading execution flow…</div>
      ) : rows.length === 0 ? (
        <div className="empty-state">
          No flow events recorded yet for this run.
          {filters.orderId
            ? ` (filtered by order: ${filters.orderId})`
            : ""}
        </div>
      ) : (
        <DataTable
          rows={rows}
          rowKey={(row) => row.row_id}
          columns={[
            {
              key: "ts",
              title: "Time",
              render: (row) => formatDateTime(row.ts_utc),
            },
            {
              key: "stage",
              title: "Stage",
              render: (row) => row.stage,
            },
            {
              key: "severity",
              title: "Sev",
              render: (row) => <SeverityBadge severity={row.severity} />,
            },
            {
              key: "order",
              title: "Order / Key",
              render: (row) => row.internal_order_id ?? "—",
            },
            {
              key: "broker",
              title: "Broker Order",
              render: (row) => row.broker_order_id ?? "—",
            },
            {
              key: "symbol",
              title: "Symbol",
              render: (row) => row.symbol ?? "—",
            },
            {
              key: "message",
              title: "Message",
              render: (row) => row.message,
            },
            {
              key: "source",
              title: "Source",
              render: (row) => row.source_table,
            },
          ]}
        />
      )}
    </Panel>
  );
}
