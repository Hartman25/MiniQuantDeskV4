// Pure path-classification and path-building helpers for the Backtest GUI.
// No DOM, no Tauri, no side effects — safe to unit-test in Node.

export type ArtifactPathKind =
  | { kind: "blank" }
  | { kind: "csv_file" }
  | { kind: "folder" };

/**
 * Classify operator input for the artifact folder field.
 * Returns "blank" for empty input, "csv_file" if the path ends with .csv
 * (case-insensitive), and "folder" otherwise.
 */
export function classifyArtifactPathInput(raw: string): ArtifactPathKind {
  const trimmed = raw.trim();
  if (!trimmed) return { kind: "blank" };
  if (/\.csv$/i.test(trimmed)) return { kind: "csv_file" };
  return { kind: "folder" };
}

/**
 * Join a repo root with path segments using the separator inferred from the
 * root (backslash on Windows paths, forward slash otherwise).
 * Returns null when repoRoot is null or empty.
 */
export function buildRepoRelativePath(
  repoRoot: string | null,
  ...segments: string[]
): string | null {
  if (!repoRoot) return null;
  const sep = repoRoot.includes("\\") ? "\\" : "/";
  return [repoRoot, ...segments].join(sep);
}

// Canonical segment lists for well-known repo-relative paths.
export const FIXTURE_BARS_SEGMENTS: string[] = [
  "core-rs",
  "crates",
  "mqk-backtest",
  "tests",
  "fixtures",
  "bkt4_bars.csv",
];
export const BACKTEST_OUT_SEGMENTS: string[] = ["exports", "backtests"];
export const MD_BACKUP_1D_SEGMENTS: string[] = ["exports", "md_backup", "1D"];

/**
 * Build the expected CSV path for a symbol in the md_backup/1D folder.
 * Naming convention: <SYMBOL>_1D.csv (uppercase symbol).
 * Returns null when repoRoot is null/empty or symbol is blank.
 *
 * Example: buildMd1DSymbolPath("C:\\repo", "AAPL")
 *   → "C:\\repo\\exports\\md_backup\\1D\\AAPL_1D.csv"
 */
export function buildMd1DSymbolPath(
  repoRoot: string | null,
  symbol: string,
): string | null {
  const trimmedSymbol = symbol.trim().toUpperCase();
  if (!trimmedSymbol) return null;
  const dir = buildRepoRelativePath(repoRoot, ...MD_BACKUP_1D_SEGMENTS);
  if (!dir) return null;
  const sep = dir.includes("\\") ? "\\" : "/";
  return `${dir}${sep}${trimmedSymbol}_1D.csv`;
}
