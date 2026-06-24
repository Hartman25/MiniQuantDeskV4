import { useEffect, useState } from "react";
import { Panel } from "../../components/common/Panel";
import { fetchJsonCandidate } from "./http";
import {
  assetCapabilityMatrixStatusLabel,
  describeAssetCapabilityEntry,
  describeNonEquityAssetClassState,
  nonEquityAssetClassState,
} from "./assetCapability";
import type { AssetCapabilityMatrix, MetadataSummary } from "./types";

export function AssetCapabilityMatrixPanel() {
  const [matrix, setMatrix] = useState<AssetCapabilityMatrix | undefined>(undefined);
  const [unavailable, setUnavailable] = useState(false);
  const [loading, setLoading] = useState(true);

  useEffect(() => {
    let cancelled = false;
    setLoading(true);
    void fetchJsonCandidate<MetadataSummary>("/api/v1/system/metadata").then((result) => {
      if (cancelled) return;
      setLoading(false);
      if (!result.ok || !result.data) {
        setUnavailable(true);
        setMatrix(undefined);
        return;
      }
      setUnavailable(false);
      setMatrix(result.data.asset_capability_matrix);
    });
    return () => {
      cancelled = true;
    };
  }, []);

  return (
    <Panel
      title="Asset capability matrix"
      subtitle="Read-only, statically-defined per-asset-class trading capability. Never used for order routing."
    >
      {loading && <div className="unavailable-notice">Checking asset capability matrix…</div>}
      {!loading && unavailable && (
        <div className="unavailable-notice unavailable-critical">
          <strong>Asset capability matrix unavailable:</strong> daemon unreachable or metadata route failed.
        </div>
      )}
      {!loading && !unavailable && (
        <div className="metric-list">
          <div><span>Status</span><strong>{assetCapabilityMatrixStatusLabel(matrix)}</strong></div>
          <div>
            <span>Non-equity classes</span>
            <strong>{describeNonEquityAssetClassState(nonEquityAssetClassState(matrix))}</strong>
          </div>
        </div>
      )}
      {!loading && !unavailable && matrix && (
        <ul style={{ marginTop: 10 }}>
          {matrix.entries.map((entry) => (
            <li key={entry.asset_class} style={{ fontSize: "0.83rem", marginBottom: 4 }}>
              {describeAssetCapabilityEntry(entry)}
            </li>
          ))}
        </ul>
      )}
    </Panel>
  );
}
