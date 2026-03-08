import { Panel } from "../../components/common/Panel";
import { MetricStripChart } from "../execution/components/MetricStripChart";
import type { SystemModel, MetricsSection } from "../system/types";

function Section({ section }: { section: MetricsSection }) {
  return (
    <Panel title={section.title} subtitle={section.description}>
      <div className="metrics-grid">
        {section.series.map((series) => (
          <MetricStripChart key={series.key} series={series} />
        ))}
      </div>
    </Panel>
  );
}

export function MetricsScreen({ model }: { model: SystemModel }) {
  const { runtime, execution, fillQuality, reconciliation, riskSafety } = model.metrics;

  return (
    <div className="screen-grid">
      <Section section={runtime} />
      <Section section={execution} />
      <Section section={fillQuality} />
      <Section section={reconciliation} />
      <Section section={riskSafety} />
    </div>
  );
}
