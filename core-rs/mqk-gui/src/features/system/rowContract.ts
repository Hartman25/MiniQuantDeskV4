/** Validate container and displayed scalar columns before a wrapper becomes real evidence. */
export function hasRows(value: unknown, key: string, strings: readonly string[], numbers: readonly string[] = []): boolean {
  if (!value || typeof value !== "object") return false;
  const rows = (value as Record<string, unknown>)[key];
  return Array.isArray(rows) && rows.every((row: unknown) => !!row && typeof row === "object" &&
    strings.every((field) => typeof (row as Record<string, unknown>)[field] === "string") &&
    numbers.every((field) => typeof (row as Record<string, unknown>)[field] === "number" && Number.isFinite((row as Record<string, unknown>)[field])));
}
