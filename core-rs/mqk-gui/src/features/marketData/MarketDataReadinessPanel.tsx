import { useCallback, useEffect, useRef, useState } from "react";
import { DailyDataReadinessPanel } from "../ingest/DailyDataReadinessPanel";
import { fetchDailyDataReadiness, type FetchDailyDataReadinessResult } from "../ingest/api";
import { getDaemonUrl } from "../../config";
import { LatestRequest } from "../system/latestRequest";

export function MarketDataReadinessPanel() {
  const [result, setResult] = useState<FetchDailyDataReadinessResult | null>(null);
  const [loading, setLoading] = useState(true);
  const requests = useRef(new LatestRequest());
  const refresh = useCallback(async () => {
    const daemonUrl = getDaemonUrl();
    setLoading(true);
    setResult(null);
    await requests.current.run(fetchDailyDataReadiness, (next) => {
      if (daemonUrl !== getDaemonUrl()) return;
      setResult(next); setLoading(false);
    }, () => { setResult({ ok: false, error: "Market-data readiness unavailable" }); setLoading(false); });
  }, []);
  useEffect(() => { void refresh(); return () => requests.current.invalidate(); }, [refresh]);
  return <DailyDataReadinessPanel response={result?.data ?? null} loading={loading}
    error={result && !result.ok ? result.error ?? "Readiness unavailable" : null} onRefresh={() => { if (!requests.current.busy) void refresh(); }} />;
}
