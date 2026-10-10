import type { ReactNode } from "react";

export interface DataTableColumn<T> {
  key: string;
  title: string;
  render: (row: T) => ReactNode;
}

export function DataTable<T>({ rows, columns, rowKey }: { rows: T[]; columns: DataTableColumn<T>[]; rowKey: (row: T) => string }) {
  const wideGrid = columns.length > 10 ? { gridTemplateColumns: `repeat(${columns.length}, minmax(100px, 1fr))` } : undefined;
  return (
    <div className={`table-grid table-cols-${columns.length}`}>
      <div className="table-row table-head" style={wideGrid}>
        {columns.map((column) => (
          <span key={column.key}>{column.title}</span>
        ))}
      </div>
      {rows.map((row) => (
        <div className="table-row" key={rowKey(row)} style={wideGrid}>
          {columns.map((column) => (
            <span key={column.key}>{column.render(row)}</span>
          ))}
        </div>
      ))}
    </div>
  );
}
