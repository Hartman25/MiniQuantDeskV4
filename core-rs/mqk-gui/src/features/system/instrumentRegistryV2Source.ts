// core-rs/mqk-gui/src/features/system/instrumentRegistryV2Source.ts
//
// Read-only helpers for the separate InstrumentRegistryV2 source configured
// via MQK_INSTRUMENT_REGISTRY_V2_PATH. This source is diagnostics-only and
// never proves live/paper trading readiness.

import type { InstrumentRegistryV2SourceStatusResponse } from "./types.ts";

/** One-line operator label for the configured v2 source's truth_state. */
export function instrumentRegistryV2SourceStatusLabel(
  s: InstrumentRegistryV2SourceStatusResponse,
): string {
  switch (s.truth_state) {
    case "not_configured":
      return "Not configured";
    case "configured_valid":
      return "Configured & valid";
    case "registry_unavailable":
      return "Configured — unavailable";
    case "validation_failed":
      return "Configured — validation failed";
    default:
      return s.truth_state;
  }
}

/**
 * Truthful trading-use note, derived from the response's own boolean rather
 * than a hardcoded string. If the backend invariant were ever violated, this
 * surfaces a warning instead of silently claiming read-only behavior.
 */
export function describeInstrumentRegistryV2SourceTradingUse(
  s: InstrumentRegistryV2SourceStatusResponse,
): string {
  if (s.used_for_trading) {
    return "WARNING: backend reports this source IS used for trading — contact engineering before trusting this build.";
  }
  return "Read-only — used only for backtest economics suggestions, never for live or paper trading.";
}

/** Truthful non-equity-disablement note for the configured v2 source. */
export function describeInstrumentRegistryV2SourceNonEquity(
  s: InstrumentRegistryV2SourceStatusResponse,
): string {
  if (!s.non_equity_present) {
    return "No non-equity instruments in this source.";
  }
  return s.non_equity_all_disabled
    ? "Non-equity instruments are present and all disabled."
    : "WARNING: non-equity instruments are present and NOT all disabled.";
}
