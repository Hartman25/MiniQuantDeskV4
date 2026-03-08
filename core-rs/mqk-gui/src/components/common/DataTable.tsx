import type { ReactNode } from "react";

export interface DataTableColumn<T> {
  key: string;
  title: string;
  render: (row: T) => ReactNode;
}

export function DataTable<T>({ rows, columns, rowKey }: { rows: T[]; columns: DataTableColumn<T>[]; rowKey: (row: T) => string }) {
  return (
    <div className="table-grid">
      <div className="table-row table-head">
        {columns.map((column) => (
          <span key={column.key}>{column.title}</span>
        ))}
      </div>
      {rows.map((row) => (
        <div className="table-row" key={rowKey(row)}>
          {columns.map((column) => (
            <span key={column.key}>{column.render(row)}</span>
          ))}
        </div>
      ))}
    </div>
  );
}
