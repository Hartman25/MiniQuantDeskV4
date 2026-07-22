import type { ScreenKey } from "../../features/screens/screenRegistry";

export const LEFT_RAIL_PRIMARY: readonly ScreenKey[] = [
  "dashboard", "execution", "risk", "portfolio", "reconcile", "ops", "runtime",
];

export const LEFT_RAIL_SECONDARY: readonly ScreenKey[] = [
  "metrics",
  "transport",
  "topology",
  "alerts",
  "incidents",
  "operatorTimeline",
  "session",
  "dailyOperations",
  "config",
  "marketData",
  "ingest",
  "strategy",
  "audit",
  "artifacts",
  "backtests",
  "strategyScanner",
  "settings",
];
