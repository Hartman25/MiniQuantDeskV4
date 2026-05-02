import type {
  BacktestManifest,
  BacktestMetrics,
  EquityCurveRow,
  FillRow,
  OrderRow,
  ParsedCsvResult,
} from "./types.ts";

export function parseManifest(json: string): BacktestManifest {
  const obj: unknown = JSON.parse(json);
  if (!obj || typeof obj !== "object" || Array.isArray(obj)) {
    throw new Error("manifest.json: expected a JSON object");
  }
  const m = obj as Record<string, unknown>;
  if (typeof m.run_id !== "string") throw new Error("manifest.json: missing run_id");
  if (typeof m.strategy_name !== "string") throw new Error("manifest.json: missing strategy_name");
  if (typeof m.created_at_utc !== "string") throw new Error("manifest.json: missing created_at_utc");
  return obj as BacktestManifest;
}

export function parseMetrics(json: string): BacktestMetrics {
  const obj: unknown = JSON.parse(json);
  if (!obj || typeof obj !== "object" || Array.isArray(obj)) {
    throw new Error("metrics.json: expected a JSON object");
  }
  const m = obj as Record<string, unknown>;
  if (typeof m.run_id !== "string") throw new Error("metrics.json: missing run_id");
  if (typeof m.bars !== "number") throw new Error("metrics.json: missing bars");
  return obj as BacktestMetrics;
}

export function parseCsvRows(csv: string): { headers: string[]; rows: Record<string, string>[] } {
  const lines = csv.split(/\r?\n/).filter((line) => line.trim() !== "");
  if (lines.length === 0) return { headers: [], rows: [] };
  const headers = lines[0].split(",").map((h) => h.trim());
  const rows: Record<string, string>[] = [];
  for (let i = 1; i < lines.length; i++) {
    const cells = lines[i].split(",");
    const row: Record<string, string> = {};
    for (let j = 0; j < headers.length; j++) {
      row[headers[j]] = cells[j]?.trim() ?? "";
    }
    rows.push(row);
  }
  return { headers, rows };
}

export function parseEquityCurve(csv: string): ParsedCsvResult<EquityCurveRow> {
  const { rows } = parseCsvRows(csv);
  let malformed = 0;
  const parsed: EquityCurveRow[] = [];
  for (const r of rows) {
    const equity = Number(r.equity);
    if (!Object.prototype.hasOwnProperty.call(r, "equity") || Number.isNaN(equity)) {
      malformed++;
      continue;
    }
    parsed.push({ ts_utc: r.ts_utc ?? "", equity });
  }
  return { rows: parsed, malformed };
}

export function parseOrders(csv: string): ParsedCsvResult<OrderRow> {
  const { rows } = parseCsvRows(csv);
  let malformed = 0;
  const parsed: OrderRow[] = [];
  for (const r of rows) {
    if (!r.order_id) {
      malformed++;
      continue;
    }
    parsed.push({
      ts_utc: r.ts_utc ?? "",
      order_id: r.order_id,
      symbol: r.symbol ?? "",
      side: r.side ?? "",
      qty: r.qty ?? "",
      order_type: r.order_type ?? "",
      limit_price: r.limit_price ?? "",
      stop_price: r.stop_price ?? "",
      status: r.status ?? "",
    });
  }
  return { rows: parsed, malformed };
}

export function parseFills(csv: string): ParsedCsvResult<FillRow> {
  const { rows } = parseCsvRows(csv);
  let malformed = 0;
  const parsed: FillRow[] = [];
  for (const r of rows) {
    if (!r.fill_id) {
      malformed++;
      continue;
    }
    parsed.push({
      ts_utc: r.ts_utc ?? "",
      fill_id: r.fill_id,
      order_id: r.order_id ?? "",
      symbol: r.symbol ?? "",
      side: r.side ?? "",
      qty: r.qty ?? "",
      price: r.price ?? "",
      fee: r.fee ?? "",
    });
  }
  return { rows: parsed, malformed };
}

export function microsToUsd(micros: number): number {
  return micros / 1_000_000;
}

export function formatMicrosAsDollars(micros: number | null | undefined): string {
  if (micros == null || Number.isNaN(micros)) return "—";
  const usd = microsToUsd(micros);
  return usd.toLocaleString(undefined, {
    style: "currency",
    currency: "USD",
    maximumFractionDigits: 2,
  });
}

export function formatNullableNumber(value: number | null | undefined, digits = 4): string {
  if (value == null || Number.isNaN(value)) return "—";
  return value.toFixed(digits);
}

export function formatNullablePercent(value: number | null | undefined): string {
  if (value == null || Number.isNaN(value)) return "—";
  return `${value.toFixed(2)}%`;
}
