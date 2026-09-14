// OT-MQD-01: Interactive Evidence Chart — presentation layer over
// evidenceChartModel.ts. Renders equity + derived drawdown as line series,
// order/fill/cost markers on a shared time-fraction events strip, hover
// time-inspection, layer toggles, and a provenance detail panel. Never
// renders a marker or region that the model did not already resolve from a
// literal artifact row.

import { useMemo, useState, type MouseEvent } from "react";
import { Panel } from "../common/Panel";
import {
  timeFraction,
  type EvidenceChartModel,
  type EvidenceChartTimeRange,
  type EvidenceMarker,
  type LaneStatus,
} from "./evidenceChartModel.ts";

const WIDTH = 640;
const EQUITY_HEIGHT = 90;
const DRAWDOWN_HEIGHT = 46;
const EVENTS_HEIGHT = 36;

type LayerKey = "equity" | "drawdown" | "orders" | "fills" | "costs";

const LAYER_LABELS: Record<LayerKey, string> = {
  equity: "Equity",
  drawdown: "Drawdown",
  orders: "Order intents",
  fills: "Fills",
  costs: "Costs",
};

function statusLabel(status: LaneStatus): string {
  switch (status) {
    case "authoritative":
      return "AUTHORITATIVE";
    case "authoritative_empty":
      return "EMPTY (authoritative)";
    case "not_wired":
      return "NOT WIRED";
    case "unavailable":
      return "UNAVAILABLE";
    case "idle":
      return "IDLE";
    case "loading":
      return "LOADING";
  }
}

function statusToneClass(status: LaneStatus): string {
  switch (status) {
    case "authoritative":
      return "good";
    case "authoritative_empty":
      return "neutral";
    case "not_wired":
      return "neutral";
    case "unavailable":
      return "bad";
    default:
      return "neutral";
  }
}

function buildPolyline(
  points: { tsMs: number | null; value: number }[],
  range: EvidenceChartTimeRange,
  height: number,
): { path: string; min: number; max: number } | null {
  const withFraction = points
    .map((p) => ({ f: timeFraction(range, p.tsMs), value: p.value }))
    .filter((p): p is { f: number; value: number } => p.f != null);
  if (withFraction.length === 0) return null;

  const values = withFraction.map((p) => p.value);
  const min = Math.min(...values);
  const max = Math.max(...values);
  const spread = max - min;
  const flat = spread === 0;

  const path = withFraction
    .map((p) => {
      const x = p.f * WIDTH;
      const y = flat ? height / 2 : ((max - p.value) / spread) * height;
      return `${x.toFixed(1)},${y.toFixed(1)}`;
    })
    .join(" ");
  return { path, min, max };
}

const MARKER_COLOR: Record<EvidenceMarker["kind"], string> = {
  order_intent: "var(--accent)",
  fill: "var(--success)",
  cost: "var(--warning)",
};

export function EvidenceChart({ model }: { model: EvidenceChartModel }) {
  const [layers, setLayers] = useState<Record<LayerKey, boolean>>({
    equity: true,
    drawdown: true,
    orders: true,
    fills: true,
    costs: true,
  });
  const [hoverFraction, setHoverFraction] = useState<number | null>(null);
  const [selectedMarker, setSelectedMarker] = useState<EvidenceMarker | null>(null);

  const toggleLayer = (key: LayerKey) => setLayers((prev) => ({ ...prev, [key]: !prev[key] }));

  const equityLine = useMemo(
    () => buildPolyline(model.equitySeries.points, model.timeRange, EQUITY_HEIGHT),
    [model.equitySeries.points, model.timeRange],
  );
  const drawdownLine = useMemo(
    () => buildPolyline(model.drawdownSeries.points, model.timeRange, DRAWDOWN_HEIGHT),
    [model.drawdownSeries.points, model.timeRange],
  );

  const allMarkers: EvidenceMarker[] = useMemo(() => {
    const out: EvidenceMarker[] = [];
    if (layers.orders) out.push(...model.orderIntentMarkers.markers);
    if (layers.fills) out.push(...model.fillMarkers.markers);
    if (layers.costs) out.push(...model.costEvents.markers);
    return out;
  }, [layers.orders, layers.fills, layers.costs, model.orderIntentMarkers.markers, model.fillMarkers.markers, model.costEvents.markers]);

  const knownMarkers = allMarkers.filter((m) => m.timeStatus === "known");
  const unknownMarkers = allMarkers.filter((m) => m.timeStatus === "unknown");
  // A known timestamp is not automatically plottable: timeFraction fails
  // closed for a value genuinely outside the authoritative range. This is a
  // distinct truth from "unknown/unparsable" and must never be conflated
  // with it or silently dropped — it gets its own notice below.
  const plottableMarkers = knownMarkers.filter((m) => timeFraction(model.timeRange, m.tsMs) != null);
  const outOfRangeMarkers = knownMarkers.filter((m) => timeFraction(model.timeRange, m.tsMs) == null);

  const handleMouseMove = (e: MouseEvent<HTMLDivElement>) => {
    const rect = e.currentTarget.getBoundingClientRect();
    if (rect.width === 0) return;
    const fraction = Math.min(1, Math.max(0, (e.clientX - rect.left) / rect.width));
    setHoverFraction(fraction);
  };

  const hoverEquityValue = useMemo(() => {
    if (hoverFraction == null || model.timeRange.startMs == null || model.timeRange.endMs == null) return null;
    const targetMs = model.timeRange.startMs + hoverFraction * (model.timeRange.endMs - model.timeRange.startMs);
    let nearest: { tsMs: number; value: number } | null = null;
    for (const p of model.equitySeries.points) {
      if (p.tsMs == null) continue;
      if (nearest == null || Math.abs(p.tsMs - targetMs) < Math.abs(nearest.tsMs - targetMs)) {
        nearest = { tsMs: p.tsMs, value: p.value };
      }
    }
    return nearest;
  }, [hoverFraction, model.timeRange, model.equitySeries.points]);

  const priceNotice = `PRICE: ${model.priceSeries.reason}`;

  return (
    <Panel
      title="Interactive evidence chart"
      subtitle="Equity, derived drawdown, and execution markers on a shared time axis — every layer traces to a literal artifact row or is labeled NOT WIRED / UNAVAILABLE."
    >
      <div className="evidence-chart-identity timeline-meta-grid">
        <div>
          <span>Run ID</span>
          <strong className="bt-mono-wrap" style={{ fontFamily: "monospace", fontSize: "0.8rem" }}>
            {model.identity.runIdConflict ? "IDENTITY CONFLICT" : model.identity.runId ?? "not reported"}
          </strong>
        </div>
        <div>
          <span>Strategy</span>
          <strong>
            {model.identity.strategyNameConflict ? "IDENTITY CONFLICT" : model.identity.strategyName ?? "not reported"}
          </strong>
        </div>
        <div>
          <span>Symbol(s)</span>
          <strong>{model.identity.symbols.length > 0 ? model.identity.symbols.join(", ") : "not reported"}</strong>
        </div>
        <div>
          <span>Timeframe</span>
          <strong>{model.identity.timeframeLabel ?? "not reported"}</strong>
        </div>
      </div>

      <div className="evidence-chart-layer-toggles">
        {(Object.keys(LAYER_LABELS) as LayerKey[]).map((key) => (
          <label key={key} className="evidence-chart-toggle">
            <input type="checkbox" checked={layers[key]} onChange={() => toggleLayer(key)} />
            {LAYER_LABELS[key]}
          </label>
        ))}
      </div>

      {(equityLine || drawdownLine) && (
        <div className="bt-equity-meta" style={{ marginTop: 6 }}>
          {equityLine && (
            <span>
              <span className="eyebrow">equity range</span>{" "}
              {(equityLine.min / 1_000_000).toLocaleString(undefined, { style: "currency", currency: "USD", maximumFractionDigits: 0 })}
              {" – "}
              {(equityLine.max / 1_000_000).toLocaleString(undefined, { style: "currency", currency: "USD", maximumFractionDigits: 0 })}
            </span>
          )}
          {drawdownLine && (
            <span>
              <span className="eyebrow">max drawdown (window)</span> {drawdownLine.max.toFixed(2)}%
            </span>
          )}
        </div>
      )}

      {(model.identity.runIdConflict || model.identity.strategyNameConflict) && (
        <div className="unavailable-notice" style={{ marginTop: 8 }}>
          IDENTITY CONFLICT: manifest and metrics report different{" "}
          {[
            model.identity.runIdConflict ? "run_id" : null,
            model.identity.strategyNameConflict ? "strategy_name" : null,
          ]
            .filter(Boolean)
            .join(" and ")}{" "}
          values for this run — neither is treated as authoritative provenance.
        </div>
      )}

      <div className="unavailable-notice" style={{ marginTop: 8 }}>
        {priceNotice}
      </div>

      {model.timeRange.status !== "authoritative" ? (
        <div className="empty-state" style={{ marginTop: 8 }}>
          {model.timeRange.status === "authoritative_empty"
            ? "Equity curve reported zero bars — no time range to chart."
            : "No authoritative time range available (equity_curve.csv unavailable) — chart cannot be rendered."}
        </div>
      ) : (
        <div className="evidence-chart-wrap" onMouseMove={handleMouseMove} onMouseLeave={() => setHoverFraction(null)}>
          {layers.equity && (
            <svg
              viewBox={`0 0 ${WIDTH} ${EQUITY_HEIGHT}`}
              preserveAspectRatio="none"
              className="evidence-chart-svg"
              aria-label="Equity (interactive)"
            >
              <rect x="0" y="0" width={WIDTH} height={EQUITY_HEIGHT} className="chart-bg" />
              {equityLine ? (
                <polyline points={equityLine.path} fill="none" stroke="var(--accent)" strokeWidth="1.5" vectorEffect="non-scaling-stroke" />
              ) : null}
              {hoverFraction != null ? (
                <line x1={hoverFraction * WIDTH} x2={hoverFraction * WIDTH} y1={0} y2={EQUITY_HEIGHT} className="chart-gridline" stroke="var(--accent)" />
              ) : null}
            </svg>
          )}

          {layers.drawdown && (
            <svg
              viewBox={`0 0 ${WIDTH} ${DRAWDOWN_HEIGHT}`}
              preserveAspectRatio="none"
              className="evidence-chart-svg"
              aria-label="Drawdown (derived, interactive)"
            >
              <rect x="0" y="0" width={WIDTH} height={DRAWDOWN_HEIGHT} className="chart-bg" />
              {drawdownLine ? (
                <polyline points={drawdownLine.path} fill="none" stroke="var(--critical)" strokeWidth="1.5" vectorEffect="non-scaling-stroke" />
              ) : null}
              {hoverFraction != null ? (
                <line x1={hoverFraction * WIDTH} x2={hoverFraction * WIDTH} y1={0} y2={DRAWDOWN_HEIGHT} className="chart-gridline" stroke="var(--accent)" />
              ) : null}
            </svg>
          )}

          <svg viewBox={`0 0 ${WIDTH} ${EVENTS_HEIGHT}`} preserveAspectRatio="none" className="evidence-chart-svg evidence-chart-events" aria-label="Order/fill/cost events">
            <rect x="0" y="0" width={WIDTH} height={EVENTS_HEIGHT} className="chart-bg" />
            <line x1={0} x2={WIDTH} y1={EVENTS_HEIGHT / 2} y2={EVENTS_HEIGHT / 2} className="chart-gridline" />
            {plottableMarkers.map((m) => {
              const f = timeFraction(model.timeRange, m.tsMs);
              if (f == null) return null;
              const x = f * WIDTH;
              const isSelected = selectedMarker?.id === m.id;
              return (
                <circle
                  key={m.id}
                  cx={x}
                  cy={EVENTS_HEIGHT / 2}
                  r={isSelected ? 5.5 : 3.5}
                  fill={MARKER_COLOR[m.kind]}
                  stroke="rgba(226,232,240,0.6)"
                  strokeWidth={1}
                  style={{ cursor: "pointer" }}
                  onClick={() => setSelectedMarker(m)}
                >
                  <title>{`${m.kind} · ${m.tsUtc} · ${m.label}`}</title>
                </circle>
              );
            })}
            {hoverFraction != null ? (
              <line x1={hoverFraction * WIDTH} x2={hoverFraction * WIDTH} y1={0} y2={EVENTS_HEIGHT} className="chart-gridline" stroke="var(--accent)" />
            ) : null}
          </svg>
        </div>
      )}

      {hoverEquityValue && (
        <div className="evidence-chart-hover-readout">
          <span className="eyebrow">cursor</span>{" "}
          {new Date(hoverEquityValue.tsMs).toISOString()} · equity{" "}
          {(hoverEquityValue.value / 1_000_000).toLocaleString(undefined, { style: "currency", currency: "USD", maximumFractionDigits: 0 })}
        </div>
      )}

      <div className="evidence-chart-legend">
        {(
          [
            ["Price", model.priceSeries.status],
            ["Equity", model.equitySeries.status],
            ["Drawdown", model.drawdownSeries.status],
            ["Signals", model.signalMarkers.status],
            ["Order intents", model.orderIntentMarkers.status],
            ["Fills", model.fillMarkers.status],
            ["Entry/exit", model.entryExitMarkers.status],
            ["Fold regions", model.foldRegions.status],
            ["OOS regions", model.oosRegions.status],
            ["Costs", model.costEvents.status],
          ] as [string, LaneStatus][]
        ).map(([label, status]) => (
          <span key={label} className={`legend-pill status-${statusToneClass(status)}`}>
            {label}: {statusLabel(status)}
          </span>
        ))}
      </div>

      {unknownMarkers.length > 0 && (
        <div className="unavailable-notice" style={{ marginTop: 8 }}>
          {unknownMarkers.length} event(s) have a missing/unparsable timestamp and are not placed on the
          chart (never guessed): {unknownMarkers.map((m) => m.id).join(", ")}
        </div>
      )}

      {outOfRangeMarkers.length > 0 && (
        <div className="unavailable-notice" style={{ marginTop: 8 }}>
          {outOfRangeMarkers.length} event(s) have a known timestamp outside the plotted time range and
          are not placed on the chart (never relocated to the start/end): {outOfRangeMarkers.map((m) => m.id).join(", ")}
        </div>
      )}

      {selectedMarker && (
        <div className="evidence-chart-provenance timeline-meta-grid" style={{ marginTop: 10 }}>
          <div>
            <span>Marker</span>
            <strong className="bt-mono-wrap" style={{ fontFamily: "monospace" }}>{selectedMarker.id}</strong>
          </div>
          <div>
            <span>Kind</span>
            <strong>{selectedMarker.kind}</strong>
          </div>
          <div>
            <span>Timestamp</span>
            <strong>{selectedMarker.tsUtc || "unknown"}</strong>
          </div>
          <div>
            <span>Symbol</span>
            <strong>{selectedMarker.symbol ?? "—"}</strong>
          </div>
          <div>
            <span>Side / Qty</span>
            <strong>{selectedMarker.side ?? "—"} / {selectedMarker.qty ?? "—"}</strong>
          </div>
          <div>
            <span>Price</span>
            <strong>{selectedMarker.price != null ? selectedMarker.price.toFixed(2) : "—"}</strong>
          </div>
          <div>
            <span>Fee</span>
            <strong>{selectedMarker.feeUsd != null ? selectedMarker.feeUsd.toFixed(2) : "—"}</strong>
          </div>
          <div>
            <span>Source artifact</span>
            <strong className="bt-mono-wrap" style={{ fontFamily: "monospace" }}>{selectedMarker.provenance.artifact}</strong>
          </div>
          <div>
            <span>Run ID</span>
            <strong className="bt-mono-wrap" style={{ fontFamily: "monospace", fontSize: "0.78rem" }}>
              {selectedMarker.provenance.runIdConflict ? "IDENTITY CONFLICT" : selectedMarker.provenance.runId ?? "—"}
            </strong>
          </div>
        </div>
      )}
    </Panel>
  );
}
