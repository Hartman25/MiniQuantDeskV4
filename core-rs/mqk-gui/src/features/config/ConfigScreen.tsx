import { DataTable } from "../../components/common/DataTable";
import { Panel } from "../../components/common/Panel";
import { StatCard } from "../../components/common/StatCard";
import { formatDateTime, formatLabel } from "../../lib/format";
import type { SystemModel } from "../system/types";

export function ConfigScreen({ model }: { model: SystemModel }) {
  const c = model.configFingerprint;
  return (
    <div className="screen-grid">
      <div className="summary-grid summary-grid-four">
        <StatCard title="Config Hash" value={c.config_hash} tone="neutral" />
        <StatCard title="Risk Policy" value={c.risk_policy_version} tone="neutral" />
        <StatCard title="Bundle Version" value={c.strategy_bundle_version} tone="neutral" />
        <StatCard title="Runtime Generation" value={c.runtime_generation_id} tone="neutral" />
      </div>
      <Panel title="Config and policy fingerprint" subtitle="Proves exactly which build, profile, policy, and runtime generation the desk is supervising.">
        <div className="metric-list compact-list">
          <div><span>Environment profile</span><strong>{c.environment_profile}</strong></div>
          <div><span>Build version</span><strong>{c.build_version}</strong></div>
          <div><span>Last restart</span><strong>{formatDateTime(c.last_restart_at)}</strong></div>
        </div>
      </Panel>
      <Panel title="Recent config and policy changes" subtitle="Enough diff visibility to know what changed before you blame the broker.">
        <DataTable
          rows={model.configDiffs}
          rowKey={(row) => row.diff_id}
          columns={[
            { key: "id", title: "Diff", render: (row) => row.diff_id },
            { key: "at", title: "Changed At", render: (row) => formatDateTime(row.changed_at) },
            { key: "domain", title: "Domain", render: (row) => formatLabel(row.changed_domain) },
            { key: "before", title: "Before", render: (row) => row.before_version },
            { key: "after", title: "After", render: (row) => row.after_version },
            { key: "summary", title: "Summary", render: (row) => row.summary },
          ]}
        />
      </Panel>
    </div>
  );
}
