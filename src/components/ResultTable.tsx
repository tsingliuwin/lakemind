import { createMemo, createEffect, Show, For } from "solid-js";
import { createSolidTable, flexRender, getCoreRowModel } from "@tanstack/solid-table";
import { createVirtualizer } from "@tanstack/solid-virtual";
import type { ColumnDef } from "@tanstack/table-core";
import type { JSX } from "solid-js";
import type { JsonValue, SqlResult } from "../lib/types";

/**
 * The virtualized result grid. Columns come from `SqlResult.columns` and rows
 * from `SqlResult.rows` (already JSON-decoded). Only the visible window is
 * rendered, so 100k rows scroll at 60fps even though the whole array is in
 * memory. This mirrors TanStack's official solid/virtualized-rows example.
 *
 * Layout: a sticky header row plus a scrollable body whose rows are absolutely
 * positioned inside a spacer sized to the total virtual height.
 */
export default function ResultTable(props: { result: SqlResult | null }) {
  let scrollRef: HTMLDivElement | undefined;

  const columns = createMemo<ColumnDef<Row, unknown>[]>(() => {
    const r = props.result;
    if (!r) return [];
    return r.columns.map(
      (name, i) =>
        ({
          id: name,
          accessorFn: (row: Row) => row.values[i],
          header: () => name,
          cell: (info) => renderCell(info.getValue()),
        }) as ColumnDef<Row, unknown>,
    );
  });

  const data = createMemo<Row[]>(() => (props.result?.rows ?? []).map((values) => ({ values })));

  const table = createSolidTable({
    get data() {
      return data();
    },
    get columns() {
      return columns();
    },
    getCoreRowModel: getCoreRowModel(),
  });

  const rowVirtualizer = createVirtualizer({
    get count() {
      return table.getRowModel().rows.length;
    },
    getScrollElement: () => scrollRef ?? null,
    estimateSize: () => 28,
    overscan: 12,
  });

  createEffect(() => {
    props.result; // track result identity
    scrollRef?.scrollTo({ top: 0 });
  });

  return (
    <div class="result-wrap">
      <Show
        when={props.result}
        fallback={<div class="result-empty">执行查询以查看结果。</div>}
      >
        <div class="result-scroll" ref={scrollRef}>
          {/* Sticky header */}
          <div class="result-head" role="row">
            <div class="result-cell row-idx">#</div>
            <For each={props.result!.columns}>
              {(name, i) => (
                <div class="result-cell head-cell" title={props.result!.columnTypes[i()]}>
                  {name}
                </div>
              )}
            </For>
          </div>
          {/* Virtualized body */}
          <div
            style={{
              height: `${rowVirtualizer.getTotalSize()}px`,
              position: "relative",
              width: "100%",
            }}
          >
            <For each={rowVirtualizer.getVirtualItems()}>
              {(vRow) => {
                const row = table.getRowModel().rows[vRow.index];
                if (!row) return null;
                return (
                  <div
                    class="result-row"
                    style={{
                      position: "absolute",
                      top: "0",
                      left: "0",
                      width: "100%",
                      height: `${vRow.size}px`,
                      transform: `translateY(${vRow.start}px)`,
                    }}
                    role="row"
                  >
                    <div class="result-cell row-idx">{vRow.index + 1}</div>
                    <For each={row.getVisibleCells()}>
                      {(cell) => (
                        <div class="result-cell">
                          {flexRender(cell.column.columnDef.cell, cell.getContext())}
                        </div>
                      )}
                    </For>
                  </div>
                );
              }}
            </For>
          </div>
        </div>
      </Show>
    </div>
  );
}

interface Row {
  values: JsonValue[];
}

/** Render a JSON cell value into something compact and readable. */
function renderCell(v: unknown): JSX.Element {
  if (v === null || v === undefined) return <span class="cell-null">NULL</span>;
  if (typeof v === "boolean") return v ? "true" : "false";
  if (typeof v === "number") return Number.isFinite(v) ? v : String(v);
  if (typeof v === "string") return v;
  if (Array.isArray(v) || typeof v === "object") return JSON.stringify(v);
  return String(v);
}
