import { Panel } from "../../components/common/Panel";
import { TruthStateNotice } from "../../components/common/TruthStateNotice";
import type { SystemModel } from "../system/types";
import { MetricStripChart } from "../execution/components/MetricStripChart";
import { panelTruthRenderState } from "../system/truthRendering";

export function MetricsScreen({ model }: { model: SystemModel }) {
  const truthState = panelTruthRenderState(model, "metrics");

  if (truthState !== null) {
    return <TruthStateNotice state={truthState} />;
  }

  return (
    <div className="screen-grid desk-screen-grid">
      <div className="desk-panel-grid desk-panel-grid-primary">
        <Panel title={model.metrics.runtime.title} subtitle={model.metrics.runtime.description}>
          <div className="metrics-grid">
            {model.metrics.runtime.series.map((series) => (
              <MetricStripChart key={series.key} series={series} />
            ))}
          </div>
        </Panel>

        <Panel title={model.metrics.execution.title} subtitle={model.metrics.execution.description}>
          <div className="metrics-grid">
            {model.metrics.execution.series.map((series) => (
              <MetricStripChart key={series.key} series={series} />
            ))}
          </div>
        </Panel>
      </div>

      <div className="desk-panel-grid desk-panel-grid-primary">
        <Panel title={model.metrics.fillQuality.title} subtitle={model.metrics.fillQuality.description}>
          <div className="metrics-grid">
            {model.metrics.fillQuality.series.map((series) => (
              <MetricStripChart key={series.key} series={series} />
            ))}
          </div>
        </Panel>

        <Panel title={model.metrics.reconciliation.title} subtitle={model.metrics.reconciliation.description}>
          <div className="metrics-grid">
            {model.metrics.reconciliation.series.map((series) => (
              <MetricStripChart key={series.key} series={series} />
            ))}
          </div>
        </Panel>
      </div>

      <Panel title={model.metrics.riskSafety.title} subtitle={model.metrics.riskSafety.description}>
        <div className="metrics-grid">
          {model.metrics.riskSafety.series.map((series) => (
            <MetricStripChart key={series.key} series={series} />
          ))}
        </div>
      </Panel>
    </div>
  );
}
