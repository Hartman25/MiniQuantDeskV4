import { Panel } from "../../components/common/Panel";
import { StatCard } from "../../components/common/StatCard";
import { TruthStateNotice } from "../../components/common/TruthStateNotice";
import { formatDateTime } from "../../lib/format";
import type { SystemModel } from "../system/types";
import { MetricStripChart } from "../execution/components/MetricStripChart";
import { isTruthHardBlock, panelTruthRenderState } from "../system/truthRendering";

export function MetricsScreen({ model }: { model: SystemModel }) {
  const truthState = panelTruthRenderState(model, "metrics");
  if (truthState !== null && isTruthHardBlock(truthState)) return <TruthStateNotice state={truthState} />;
  const { metrics } = model;
  const sections = [metrics.runtime, metrics.execution, metrics.portfolio, metrics.fillQuality, metrics.reconciliation, metrics.riskSafety].filter((section) => section !== undefined);
  return (
    <div className="screen-grid desk-screen-grid">
      {truthState !== null && <TruthStateNotice state={truthState} />}
      <Panel title="Operational metric evidence" subtitle="GET /api/v1/metrics/dashboards · current snapshot only">
        <p>Browser observation: {formatDateTime(model.lastUpdatedAt)}. Snapshot capture time and historical samples are unavailable from this endpoint.</p>
        <p>Missing measurements remain unavailable. A measured count or a clear kill switch does not establish subsystem readiness.</p>
      </Panel>
      <div className="summary-grid summary-grid-five">
        {sections.map((section) => <StatCard key={section.key} title={section.title}
          value={section.operational_state ?? (section.truth_state === "active" ? "Observed" : "Unavailable")}
          detail={section.description}
          tone={section.operational_state === "HALTED" || section.operational_state === "DIRTY" ? "bad" : section.operational_state === "STALE" ? "warn" : "neutral"} />)}
      </div>
      {sections.map((section) => (
        <Panel key={section.key} title={section.title} subtitle={section.description}>
          {section.series.length === 0 ? <div className="empty-state">Measurements unavailable; health is not established.</div> :
            <div className="metrics-grid">{section.series.map((series) => <MetricStripChart key={series.key} series={series} />)}</div>}
        </Panel>
      ))}
    </div>
  );
}
