// core-rs/mqk-gui/src/features/system/assetCapability.ts
//
// ASSET-CAPABILITY-MATRIX-GUI-VISIBILITY-01: pure, read-only helpers for
// rendering the static asset-capability matrix carried on
// /api/v1/system/metadata (`MetadataSummary.asset_capability_matrix`).
//
// These helpers never fabricate truth: when the matrix is absent (daemon
// unreachable, or an older daemon build that predates this field), every
// helper here reports an explicit "unknown" state rather than a friendly
// default — see CLAUDE.md's "no fabricated truth" / "unavailable vs empty
// vs present" GUI rules.

import type { AssetCapabilityEntry, AssetCapabilityMatrix } from "./types.ts";

export const EQUITY_ASSET_CLASS = "us_equities";

/** One-line label for the matrix's overall truth/availability. */
export function assetCapabilityMatrixStatusLabel(
  matrix: AssetCapabilityMatrix | undefined,
): string {
  if (!matrix || !Array.isArray(matrix.entries)) {
    return "Asset capability matrix truth unavailable.";
  }
  const enabledCount = matrix.default_enabled_asset_classes?.length ?? 0;
  const totalCount = matrix.entries.length;
  return `${enabledCount} of ${totalCount} asset classes enabled.`;
}

export type AssetCapabilityNonEquityState = "all_disabled" | "some_enabled" | "unknown";

/**
 * Independently verifies (from `entries`, not from the backend's own
 * `disabled_asset_classes` summary array) whether every non-equity asset
 * class is disabled. Returns `"unknown"` when the matrix is absent or
 * carries no non-equity entries to check — never defaults to "all_disabled"
 * on missing data.
 */
export function nonEquityAssetClassState(
  matrix: AssetCapabilityMatrix | undefined,
): AssetCapabilityNonEquityState {
  if (!matrix || !Array.isArray(matrix.entries)) return "unknown";
  const nonEquity = matrix.entries.filter((e) => e.asset_class !== EQUITY_ASSET_CLASS);
  if (nonEquity.length === 0) return "unknown";
  return nonEquity.every((e) => !e.enabled) ? "all_disabled" : "some_enabled";
}

export function describeNonEquityAssetClassState(state: AssetCapabilityNonEquityState): string {
  switch (state) {
    case "all_disabled":
      return "All non-equity asset classes are disabled.";
    case "some_enabled":
      return "WARNING: a non-equity asset class is enabled.";
    case "unknown":
      return "Non-equity disablement truth unavailable.";
  }
}

/** Concise per-row label for one capability-matrix entry. */
export function describeAssetCapabilityEntry(entry: AssetCapabilityEntry): string {
  const enabled = entry.enabled ? "enabled" : "disabled";
  const paper = entry.paper_ready ? "paper-ready" : "paper not ready";
  const live = entry.live_ready ? "live-ready" : "live not ready";
  return `${entry.asset_class} — ${enabled} · ${paper} · ${live} · adapter: ${entry.broker_adapter}`;
}
