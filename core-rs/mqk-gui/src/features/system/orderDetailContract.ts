import type { OrderTraceResponse, OrderReplayResponse, OrderChartResponse, OrderCausalityResponse, ExecutionFlowSurface } from "./types";
import { canonicalHistory } from "./historyContract";

type RecordValue = Record<string, unknown>;
const object = (value: unknown): value is RecordValue => !!value && typeof value === "object" && !Array.isArray(value);
const text = (value: unknown) => typeof value === "string";
const nullableText = (value: unknown) => value === null || text(value);
const finite = (value: unknown) => typeof value === "number" && Number.isFinite(value);
const nullableNumber = (value: unknown) => value === null || finite(value);
const quantity = (value: unknown) => value === null || typeof value === "number" && Number.isSafeInteger(value) && value >= 0;
const time = (value: unknown) => typeof value === "string" && /(?:Z|[+-]\d{2}:\d{2})$/.test(value) && Number.isFinite(Date.parse(value));
const nullableTime = (value: unknown) => value === null || time(value);
const texts = (value: unknown) => Array.isArray(value) && value.every(text);
const fields = (value: RecordValue, keys: string[], check: (value: unknown) => boolean) => keys.every((key) => check(value[key]));
const rows = (value: unknown, identity: string, check: (row: RecordValue) => boolean) => Array.isArray(value) &&
  value.every((row) => object(row) && text(row[identity]) && check(row)) && new Set(value.map((row) => row[identity])).size === value.length;

/** Validate the actual canonical order DTO before any drill-down can become evidence. */
export function parseOrderDetail(kind: "trace", value: unknown, orderId: string): OrderTraceResponse | null;
export function parseOrderDetail(kind: "replay", value: unknown, orderId: string): OrderReplayResponse | null;
export function parseOrderDetail(kind: "chart", value: unknown, orderId: string): OrderChartResponse | null;
export function parseOrderDetail(kind: "causality", value: unknown, orderId: string): OrderCausalityResponse | null;
export function parseOrderDetail(kind: "trace" | "replay" | "chart" | "causality", value: unknown, orderId: string): OrderTraceResponse | OrderReplayResponse | OrderChartResponse | OrderCausalityResponse | null {
  if (!object(value) || value.order_id !== orderId || value.canonical_route !== `/api/v1/execution/orders/${orderId}/${kind}` || !text(value.backend)) return null;
  if (kind === "trace") {
    if (!["active", "filled_without_fill_quality_telemetry", "no_fills_yet", "no_order", "no_db"].includes(String(value.truth_state)) ||
      !fields(value, ["broker_order_id", "symbol", "current_status", "current_stage", "outbox_status", "outbox_lifecycle_stage"], nullableText) ||
      !fields(value, ["requested_qty", "filled_qty"], quantity) || !nullableTime(value.last_event_at) ||
      !rows(value.rows, "event_id", (row) => time(row.ts_utc) && fields(row, ["stage", "source"], text) &&
        fields(row, ["detail", "side", "provenance_ref"], nullableText) && quantity(row.fill_qty) &&
        fields(row, ["fill_price_micros", "slippage_bps", "submit_to_fill_ms"], nullableNumber) && nullableTime(row.submit_ts_utc)) ||
      value.truth_state !== "active" && (value.rows as unknown[]).length !== 0) return null;
    return value as unknown as OrderTraceResponse;
  }
  if (kind === "replay") {
    if (!["active", "no_fills_yet", "no_order", "no_db"].includes(String(value.truth_state)) || value.replay_id !== orderId ||
      value.replay_scope !== "single_order" || value.source !== "fill_quality_telemetry" || !text(value.title) ||
      !rows(value.frames, "frame_id", (row) => time(row.timestamp) && fields(row, ["subsystem", "event_type", "state_delta", "message_digest",
        "order_execution_state", "oms_state", "risk_state", "reconcile_state", "queue_status"], text) &&
        quantity(row.filled_qty) && row.filled_qty !== null && quantity(row.open_qty) && texts(row.anomaly_tags) && texts(row.boundary_tags)) ||
      !Number.isSafeInteger(value.current_frame_index) || (value.current_frame_index as number) < 0 ||
      (value.current_frame_index as number) > Math.max(0, (value.frames as unknown[]).length - 1) ||
      value.truth_state !== "active" && (value.frames as unknown[]).length !== 0) return null;
    return value as unknown as OrderReplayResponse;
  }
  if (kind === "chart") {
    // The mounted route supplies identity and an explanation only; no chart source exists.
    if (!["no_bars", "no_order", "no_db"].includes(String(value.truth_state)) || !nullableText(value.symbol) || !text(value.comment) ||
      value.bars !== undefined && (!Array.isArray(value.bars) || value.bars.length !== 0) ||
      value.overlays !== undefined && (!Array.isArray(value.overlays) || value.overlays.length !== 0)) return null;
    return value as unknown as OrderChartResponse;
  }
  if (!["partial", "no_fills_yet", "no_order", "no_db"].includes(String(value.truth_state)) || !nullableText(value.symbol) || !text(value.comment) ||
    !texts(value.proven_lanes) || !texts(value.unproven_lanes) ||
    (value.proven_lanes as string[]).some((lane) => (value.unproven_lanes as string[]).includes(lane)) ||
    !rows(value.nodes, "node_key", (row) => fields(row, ["node_type", "title", "status", "subsystem", "summary"], text) &&
      nullableText(row.linked_id) && nullableTime(row.timestamp) && nullableNumber(row.elapsed_from_prev_ms) && texts(row.anomaly_tags)) ||
    value.truth_state !== "partial" && ((value.nodes as unknown[]).length !== 0 || (value.proven_lanes as string[]).length !== 0)) return null;
  return value as unknown as OrderCausalityResponse;
}

export function parseExecutionFlow(value: unknown, scope?: { runId?: string; orderId?: string }): ExecutionFlowSurface | null {
  if (!object(value) || value.canonical_route !== "/api/v1/execution/flow" || !text(value.backend) || !nullableText(value.run_id) ||
    !["active", "no_active_run", "no_db"].includes(String(value.truth_state)) ||
    !rows(value.rows, "row_id", (row) => time(row.ts_utc) && fields(row, ["stage", "run_id", "message", "source_table"], text) &&
      ["info", "warn", "error"].includes(String(row.severity)) && fields(row, ["internal_order_id", "broker_order_id", "symbol"], nullableText) &&
      row.run_id === value.run_id && (!scope?.orderId || row.internal_order_id === scope.orderId)) ||
    value.truth_state === "active" && (!text(value.run_id) || scope?.runId !== undefined && scope.runId !== value.run_id) ||
    value.truth_state !== "active" && (value.rows as unknown[]).length !== 0) return null;
  const surface = value as unknown as ExecutionFlowSurface;
  const ordered = canonicalHistory(surface.rows, (row) => row.row_id, (row) => row.ts_utc);
  return ordered === null ? null : { ...surface, rows: ordered.reverse() };
}
