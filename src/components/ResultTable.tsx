import { createMemo, Show, For } from "solid-js";
import { createSolidTable, flexRender, getCoreRowModel } from "@tanstack/solid-table";
import type { ColumnDef } from "@tanstack/table-core";
import type { JSX } from "solid-js";
import type { JsonValue, SqlResult } from "../lib/types";

// Fixed column widths — must match the CSS (.row-idx 54px, .result-cell 160px).
// We size head/row to a COMPUTED total width (not min-width:max-content, which
// sums cell content widths and overshoots, leaving empty scroll space).
const ROW_IDX_W = 54;
const CELL_W = 160;

/**
 * The result grid. Rows are rendered directly (no virtualization) — simple and
 * avoids the virtualizer's scrollRef/height init bugs that left the body empty.
 * Row caps keep this safe (default 10k; even 1m renders, just slower). If very
 * large results need 60fps later, reintroduce virtualization carefully.
 */
export default function ResultTable(props: { result: SqlResult | null }) {
  const columns = createMemo<ColumnDef<Row, unknown>[]>(() => {
    const r = props.result;
    if (!r) return [];
    return r.columns.map(
      (name, i) =>
        ({
          id: name,
          accessorFn: (row: Row) => row[i],
          header: () => name,
          cell: (info) => renderCell(info.getValue()),
        }) as ColumnDef<Row, unknown>,
    );
  });

  const data = createMemo<Row[]>(() => props.result?.rows ?? []);

  const table = createSolidTable({
    get data() {
      return data();
    },
    get columns() {
      return columns();
    },
    getCoreRowModel: getCoreRowModel(),
  });

  const tableWidth = createMemo(() => {
    const r = props.result;
    if (!r) return 0;
    return ROW_IDX_W + CELL_W * r.columns.length;
  });

  return (
    <div class="result-wrap">
      <Show
        when={props.result}
        fallback={<div class="result-empty">执行查询以查看结果。</div>}
      >
        <div class="result-scroll">
          {/* Sticky header */}
          <div class="result-head" role="row" style={{ width: `${tableWidth()}px` }}>
            <div class="result-cell row-idx">#</div>
            <For each={props.result!.columns}>
              {(name, i) => (
                <div class="result-cell head-cell" title={props.result!.columnTypes[i()]}>
                  {name}
                </div>
              )}
            </For>
          </div>
          {/* Body — direct render, no virtualizer */}
          <For each={table.getRowModel().rows}>
            {(row, i) => (
              <div class="result-row" role="row" style={{ width: `${tableWidth()}px` }}>
                <div class="result-cell row-idx">{i() + 1}</div>
                <For each={row.getVisibleCells()}>
                  {(cell) => (
                    <div class="result-cell">
                      {flexRender(cell.column.columnDef.cell, cell.getContext())}
                    </div>
                  )}
                </For>
              </div>
            )}
          </For>
        </div>
      </Show>
    </div>
  );
}

/** 一行 = 一个单元格 JSON 值数组（直接对齐后端 SqlResult.rows）。 */
type Row = JsonValue[];

/** Render a JSON cell value into something compact and readable. */
function renderCell(v: unknown): JSX.Element {
  if (v === null || v === undefined) return <span class="cell-null">NULL</span>;
  if (typeof v === "boolean") return v ? "true" : "false";
  if (typeof v === "number") return Number.isFinite(v) ? v : String(v);
  if (typeof v === "string") return v;
  if (Array.isArray(v) || typeof v === "object") return JSON.stringify(v);
  return String(v);
}
