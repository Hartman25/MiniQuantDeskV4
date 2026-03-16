import { DataTable } from "../../components/common/DataTable";
import { Panel } from "../../components/common/Panel";
import { StatCard } from "../../components/common/StatCard";
import { TruthStateNotice } from "../../components/common/TruthStateNotice";
import { formatDateTime } from "../../lib/format";
import { panelTruthRenderState } from "../system/truthRendering";
import type { SystemModel } from "../system/types";

export function ConfigScreen({ model }: { model: SystemModel }) {
  const c = model.configFingerprint;
  const truthState = panelTruthRenderState(model, "config");

  if (truthState !== null) {
    return <TruthStateNotice state={truthState} />;
  }

  return (
    <div className="screen-grid desk-screen-grid">
      <div className="summary-grid summary-grid-four">
        <StatCard title="Config Hash" value={c.config_hash} detail="Current loaded config fingerprint" tone="good" />
        <StatCard title="Risk Policy" value={c.risk_policy_version} detail="Risk policy version" tone="neutral" />
        <StatCard title="Strategy Bundle" value={c.strategy_bundle_version} detail="Strategy bundle version" tone="neutral" />
        <StatCard title="Runtime Generation" value={c.runtime_generation_id} detail={`Restarted ${formatDateTime(c.last_restart_at)}`} tone="neutral" />
      </div>

      <Panel title="Config diffs" subtitle="Recent config and runtime generation changes.">
        <DataTable
          rows={model.configDiffs}
          rowKey={(row) => row.diff_id}
          columns={[
            { key: "when", title: "Changed At", render: (row) => formatDateTime(row.changed_at) },
            { key: "domain", title: "Domain", render: (row) => row.changed_domain },
            { key: "before", title: "Before", render: (row) => row.before_version },
            { key: "after", title: "After", render: (row) => row.after_version },
            { key: "summary", title: "Summary", render: (row) => row.summary },
          ]}
        />
      </Panel>
    </div>
  );
}
