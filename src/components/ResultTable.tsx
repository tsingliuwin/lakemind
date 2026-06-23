import { createMemo, createEffect, Show, For } from "solid-js";
import { createSolidTable, flexRender, getCoreRowModel } from "@tanstack/solid-table";
import { createVirtualizer } from "@tanstack/solid-virtual";
import type { ColumnDef } from "@tanstack/table-core";
import type { JSX } from "solid-js";
import type { JsonValue, SqlResult } from "../lib/types";

// Fixed column widths — must match the CSS (.row-idx 54px, .result-cell 160px).
const ROW_IDX_W = 54;
const CELL_W = 160;

/**
 * Result grid wrapper. The actual virtualized grid lives in a sub-component
 * (`VirtualGrid`) that is only created when `result` is non-null, inside the
 * `<Show>`. This sidesteps the solid-virtual scroll-element bootstrap problem:
 *
 *   `createVirtualizer` sets up its internal `createComputed` + `onMount` at
 *   component creation time. `onMount` calls `_willUpdate()`, which reads
 *   `getScrollElement()` and — if the element exists — attaches a
 *   ResizeObserver and scroll listeners. Those listeners fire `onChange`,
 *   which re-calls `_willUpdate()`, keeping the cycle alive.
 *
 *   If the scroll container is inside a `<Show>` that starts out false (the
 *   common case: ResultTable mounts before any query has run), the `ref` is
 *   never assigned, `getScrollElement()` returns null at `onMount` time, no
 *   observers are attached, `onChange` never fires → deadlock, 0 rows.
 *
 *   By placing the virtualizer in a sub-component that only exists when the
 *   `<Show>` is truthy, the scroll container is unconditionally present at
 *   `onMount` time, and the bootstrap cycle succeeds.
 *
 *   Note: a `signal`-based scrollRef does NOT fix this — `getScrollElement`
 *   is a plain function value, only read inside `_willUpdate()`, and SolidJS
 *   reactivity only tracks synchronous reads in a tracking context. The signal
 *   changes but the function is never called, so nothing happens.
 */
export default function ResultTable(props: {
  result: SqlResult | null;
  /** Compact variant for inline embedding inside chat tool segments:
   *  smaller row height, capped height with internal scrolling. */
  compact?: boolean;
}) {
  return (
    <div class="result-wrap" classList={{ "result-wrap--compact": !!props.compact }}>
      <Show
        when={props.result}
        fallback={<div class="result-empty">执行查询以查看结果。</div>}
      >
        {(result) => <VirtualGrid result={result()} compact={props.compact} />}
      </Show>
    </div>
  );
}

// ─── Sub-component: only created when result is non-null ────────────────────

function VirtualGrid(props: { result: SqlResult; compact?: boolean }) {
  let scrollRef: HTMLDivElement | undefined;

  const columns = createMemo<ColumnDef<Row, unknown>[]>(() => {
    return props.result.columns.map(
      (name, i) =>
        ({
          id: name,
          accessorFn: (row: Row) => row[i],
          header: () => name,
          cell: (info) => renderCell(info.getValue()),
        }) as ColumnDef<Row, unknown>,
    );
  });

  const data = createMemo<Row[]>(() => props.result.rows ?? []);

  const table = createSolidTable({
    get data() {
      return data();
    },
    get columns() {
      return columns();
    },
    getCoreRowModel: getCoreRowModel(),
  });

  const tableWidth = createMemo(() => ROW_IDX_W + CELL_W * props.result.columns.length);

  // At this point scrollRef is still `undefined` (assigned below in JSX).
  // But by the time `onMount` fires inside solid-virtual, the JSX has been
  // evaluated and the ref assigned. `_willUpdate()` reads scrollRef → finds
  // the real element → attaches observers → virtualizer works.
  const rowVirtualizer = createVirtualizer({
    get count() {
      return table.getRowModel().rows.length;
    },
    getScrollElement: () => scrollRef ?? null,
    estimateSize: () => (props.compact ? 24 : 28),
    overscan: props.compact ? 8 : 12,
  });

  // Reset scroll position when result identity changes.
  createEffect(() => {
    props.result;
    scrollRef?.scrollTo({ top: 0, left: 0 });
  });

  return (
    <div class="result-scroll" classList={{ "result-scroll--compact": !!props.compact }} ref={scrollRef}>
      {/* Sticky header */}
      <div class="result-head" role="row" style={{ width: `${tableWidth()}px` }}>
        <div class="result-cell row-idx">#</div>
        <For each={props.result.columns}>
          {(name, i) => (
            <div class="result-cell head-cell" title={props.result.columnTypes[i()]}>
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
          width: `${tableWidth()}px`,
        }}
      >
        <For each={rowVirtualizer.getVirtualItems()}>
          {(vRow) => {
            const row = table.getRowModel().rows[vRow.index];
            if (!row) return null;
            return (
              <div
                class="result-row"
                role="row"
                style={{
                  position: "absolute",
                  top: "0",
                  left: "0",
                  width: `${tableWidth()}px`,
                  height: `${vRow.size}px`,
                  transform: `translateY(${vRow.start}px)`,
                }}
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
