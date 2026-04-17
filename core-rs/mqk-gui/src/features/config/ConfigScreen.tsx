import { DataTable } from "../../components/common/DataTable";
import { Panel } from "../../components/common/Panel";
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

  const diffsActive = model.configDiffsTruth.truth_state === "active";
  const changedDomains: string[] = diffsActive
    ? [...new Set(model.configDiffs.map((d) => d.changed_domain))]
    : [];
  const recentDiffs = diffsActive ? model.configDiffs.slice(0, 3) : [];

  return (
    <div className="screen-grid desk-screen-grid">

      {/* Loaded config fingerprint — complete identity of what is active right now.
          All five fields required to verify the loaded artifact matches the promoted artifact.
          This panel owns the question: what exactly is running in this generation? */}
      <Panel
        title="Loaded config fingerprint"
        subtitle="The config identity active in the current runtime generation. Verify these match the expected promoted artifact before starting execution."
      >
        <div className="metric-list">
          <div><span>Config hash</span><strong>{c.config_hash}</strong></div>
          <div><span>Runtime generation</span><strong>{c.runtime_generation_id}</strong></div>
          <div><span>Risk policy version</span><strong>{c.risk_policy_version}</strong></div>
          <div><span>Strategy bundle version</span><strong>{c.strategy_bundle_version}</strong></div>
          <div><span>Generation started</span><strong>{formatDateTime(c.last_restart_at)}</strong></div>
        </div>
      </Panel>

      {/* Changed domains — rendered only when diffs are active and present.
          Answers: which domains changed, how many changes per domain,
          and what changed most recently. Use domain count to prioritise
          the first place to look when investigating a generation boundary. */}
      {diffsActive && model.configDiffs.length > 0 && (
        <Panel
          title="Changed domains — current generation"
          subtitle={`${model.configDiffs.length} diff${model.configDiffs.length === 1 ? "" : "s"} across ${changedDomains.length} domain${changedDomains.length === 1 ? "" : "s"}. Inspect the domain most relevant to the current operator concern.`}
        >
          <div className="timeline-category-strip">
            {changedDomains.map((domain) => {
              const count = model.configDiffs.filter((d) => d.changed_domain === domain).length;
              return (
                <div key={domain} className="timeline-category-pill">
                  <span className="timeline-category-label">{domain.replace(/_/g, " ")}</span>
                  <span className="timeline-category-count">{count}</span>
                </div>
              );
            })}
          </div>

          <div className="operator-timeline-stack" style={{ marginTop: "12px" }}>
            {recentDiffs.map((diff) => (
              <div key={diff.diff_id} className="operator-timeline-card severity-info">
                <div className="operator-timeline-head">
                  <strong>{diff.changed_domain.replace(/_/g, " ")} — {diff.summary}</strong>
                  <span className="operator-timeline-meta">{formatDateTime(diff.changed_at)}</span>
                </div>
                <div className="operator-timeline-meta">
                  <span>Before: {diff.before_version}</span>
                  <span>After: {diff.after_version}</span>
                </div>
              </div>
            ))}
          </div>
        </Panel>
      )}

      {/* Config diff ledger — full record, demoted to investigative body.
          Existing unavailable / not_wired truth notices preserved exactly. */}
      <Panel
        title="Config diff ledger — full record"
        subtitle="Complete history of config and generation changes. Newest first."
      >
        {model.configDiffsTruth.truth_state === "not_wired" ? (
          <div className="unavailable-notice">
            Config-diff truth is mounted but not wired. Empty rows do not mean there are no config diffs.
          </div>
        ) : model.configDiffsTruth.truth_state !== "active" ? (
          <div className="unavailable-notice">
            Config-diff truth is currently unavailable. Do not treat the empty row set as authoritative.
          </div>
        ) : model.configDiffs.length === 0 ? (
          <div className="empty-state">No config diffs recorded.</div>
        ) : (
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
        )}
      </Panel>
    </div>
  );
}
