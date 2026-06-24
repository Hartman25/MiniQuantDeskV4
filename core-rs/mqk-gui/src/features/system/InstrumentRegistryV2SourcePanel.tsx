import { useEffect, useState } from "react";
import { Panel } from "../../components/common/Panel";
import { getInstrumentRegistryV2SourceStatus } from "./api";
import {
  describeInstrumentRegistryV2SourceNonEquity,
  describeInstrumentRegistryV2SourceTradingUse,
  instrumentRegistryV2SourceStatusLabel,
} from "./instrumentRegistryV2Source";
import type { InstrumentRegistryV2SourceStatusResponse } from "./types";

export function InstrumentRegistryV2SourcePanel() {
  const [status, setStatus] = useState<InstrumentRegistryV2SourceStatusResponse | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [loading, setLoading] = useState(true);

  useEffect(() => {
    let cancelled = false;
    setLoading(true);
    void getInstrumentRegistryV2SourceStatus().then((result) => {
      if (cancelled) return;
      setLoading(false);
      if (!result.ok || !result.data) {
        setError(result.error ?? "Instrument registry v2 source status unavailable.");
        setStatus(null);
        return;
      }
      setError(null);
      setStatus(result.data);
    });
    return () => {
      cancelled = true;
    };
  }, []);

  return (
    <Panel
      title="Instrument registry v2 source"
      subtitle="Read-only status of the separate v2 registry configured via MQK_INSTRUMENT_REGISTRY_V2_PATH. Suggestion-only — never used for live or paper trading."
    >
      {loading && <div className="unavailable-notice">Checking instrument registry v2 source…</div>}
      {!loading && error && (
        <div className="unavailable-notice unavailable-critical">
          <strong>Instrument registry v2 source status unavailable:</strong> {error}
        </div>
      )}
      {!loading && !error && status && (
        <div className="metric-list">
          <div><span>Status</span><strong>{instrumentRegistryV2SourceStatusLabel(status)}</strong></div>
          <div><span>Configured path</span><strong style={{ wordBreak: "break-all" }}>{status.path ?? "—"}</strong></div>
          <div><span>Trading use</span><strong>{describeInstrumentRegistryV2SourceTradingUse(status)}</strong></div>
          {status.truth_state === "configured_valid" && (
            <>
              <div><span>Total instruments</span><strong>{status.total_instruments}</strong></div>
              <div>
                <span>By asset class</span>
                <strong>
                  {Object.entries(status.asset_class_counts)
                    .map(([assetClass, count]) => `${assetClass}: ${count}`)
                    .join(", ") || "—"}
                </strong>
              </div>
              <div>
                <span>Enabled / paper / live</span>
                <strong>
                  {status.enabled_counts.enabled} / {status.enabled_counts.paper_trading_enabled} /{" "}
                  {status.enabled_counts.live_trading_enabled}
                </strong>
              </div>
              <div><span>Non-equity</span><strong>{describeInstrumentRegistryV2SourceNonEquity(status)}</strong></div>
              <div>
                <span>Economics metadata</span>
                <strong>{status.has_economics_metadata ? "present" : "not present"}</strong>
              </div>
              <div>
                <span>Sample symbols</span>
                <strong>{status.sample_symbols.length > 0 ? status.sample_symbols.join(", ") : "—"}</strong>
              </div>
            </>
          )}
          {status.validation_errors.length > 0 && (
            <div><span>Validation errors</span><strong>{status.validation_errors.join("; ")}</strong></div>
          )}
          <div><span>Message</span><strong>{status.message}</strong></div>
        </div>
      )}
    </Panel>
  );
}
