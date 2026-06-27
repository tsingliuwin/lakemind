import { onCleanup, onMount, createSignal, For } from "solid-js";
import * as echarts from "echarts";
import type { Segment, SqlResult } from "../lib/types";

/**
 * Inline chart segment — renders an ECharts visualization from a SqlResult.
 *
 * The agent's `render_chart` tool emits a `chart` segment with a chart type
 * (bar/line/pie/scatter), axis mapping (xField/yFields), and the raw query
 * data. This component converts that into an ECharts option and renders it.
 * The user can switch chart types via the toolbar.
 */

type ChartType = "bar" | "line" | "pie" | "scatter";

const CHART_TYPES: { type: ChartType; label: string; svg: string }[] = [
  { type: "bar", label: "柱状图", svg: '<rect x="3" y="12" width="4" height="9"/><rect x="10" y="7" width="4" height="14"/><rect x="17" y="4" width="4" height="17"/>' },
  { type: "line", label: "折线图", svg: '<polyline points="3 17 8 11 13 14 21 5" fill="none"/><circle cx="3" cy="17" r="1.5"/><circle cx="8" cy="11" r="1.5"/><circle cx="13" cy="14" r="1.5"/><circle cx="21" cy="5" r="1.5"/>' },
  { type: "pie", label: "饼图", svg: '<circle cx="12" cy="12" r="9"/><path d="M12 3 A9 9 0 0 1 21 12 L12 12 Z" fill="currentColor" stroke="none"/>' },
  { type: "scatter", label: "散点图", svg: '<circle cx="5" cy="18" r="1.8"/><circle cx="10" cy="8" r="1.8"/><circle cx="15" cy="14" r="1.8"/><circle cx="19" cy="5" r="1.8"/><circle cx="8" cy="16" r="1.8"/>' },
];

export default function ChartSegment(props: { seg: Extract<Segment, { type: "chart" }> }) {
  let container: HTMLDivElement | undefined;
  let chart: echarts.ECharts | undefined;
  const [chartType, setChartType] = createSignal<ChartType>(props.seg.chartType);

  /** Build ECharts option from SqlResult + chart config. */
  function buildOption(type: ChartType, table: SqlResult, xField?: string, yFields?: string[], title?: string): echarts.EChartsOption {
    const cols = table.columns;
    // Determine column indices.
    const xIdx = xField ? cols.indexOf(xField) : findDimensionCol(table.columnTypes);
    const yCols = yFields && yFields.length > 0 ? yFields : findNumericCols(cols, table.columnTypes, xIdx);

    if (type === "pie") {
      const yIdx = yCols.length > 0 ? cols.indexOf(yCols[0]) : -1;
      const data = table.rows
        .filter((r) => r[xIdx] != null && r[yIdx] != null)
        .map((r) => ({ name: String(r[xIdx]), value: num(r[yIdx]) }));
      return {
        title: title ? { text: title, left: "center", textStyle: { fontSize: 13 } } : undefined,
        tooltip: { trigger: "item", formatter: "{b}: {c} ({d}%)" },
        legend: { bottom: 0, type: "scroll" },
        series: [{
          type: "pie",
          radius: ["35%", "65%"],
          data,
          label: { formatter: "{b}: {d}%" },
        }],
      };
    }

    if (type === "scatter") {
      const yIdx = yCols.length > 0 ? cols.indexOf(yCols[0]) : -1;
      const data = table.rows
        .filter((r) => r[xIdx] != null && r[yIdx] != null)
        .map((r) => [num(r[xIdx]), num(r[yIdx])]);
      return {
        title: title ? { text: title, left: "center", textStyle: { fontSize: 13 } } : undefined,
        tooltip: { trigger: "item" },
        xAxis: { type: "value", name: xField ?? cols[xIdx] ?? "X", scale: true },
        yAxis: { type: "value", name: yCols[0] ?? "Y", scale: true },
        series: [{ type: "scatter", data, symbolSize: 8 }],
      };
    }

    // bar / line
    const categoryData = table.rows.map((r) => String(r[xIdx >= 0 ? xIdx : 0] ?? ""));
    const series = yCols.map((yn) => {
      const yi = cols.indexOf(yn);
      return {
        name: yn,
        type,
        data: table.rows.map((r) => num(r[yi])),
        smooth: type === "line",
      };
    });
    return {
      title: title ? { text: title, left: "center", textStyle: { fontSize: 13 } } : undefined,
      tooltip: { trigger: "axis" },
      legend: { bottom: 0, type: "scroll" },
      grid: { left: "8%", right: "5%", top: title ? 45 : 20, bottom: 40 },
      xAxis: { type: "category", data: categoryData, axisLabel: { rotate: categoryData.length > 8 ? 30 : 0 } },
      yAxis: { type: "value" },
      series,
    };
  }

  function render() {
    if (!chart || !container) return;
    const opt = buildOption(chartType(), props.seg.table, props.seg.xField, props.seg.yFields, props.seg.title);
    chart.setOption(opt, true);
  }

  onMount(() => {
    if (!container) return;
    chart = echarts.init(container);
    render();
    const ro = new ResizeObserver(() => chart?.resize());
    ro.observe(container);
    onCleanup(() => ro.disconnect());
  });

  onCleanup(() => {
    chart?.dispose();
  });

  function switchType(t: ChartType) {
    if (t === chartType()) return;
    setChartType(t);
    render();
  }

  return (
    <div class="chart-seg">
      <div class="chart-seg__toolbar">
        <For each={CHART_TYPES}>
          {(ct) => (
            <button
              class="chart-seg__type-btn"
              classList={{ active: chartType() === ct.type }}
              title={ct.label}
              onClick={() => switchType(ct.type)}
            >
              <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round" style="width: 12px; height: 12px; display: inline-block; vertical-align: middle;" innerHTML={ct.svg} />
              <span>{ct.label}</span>
            </button>
          )}
        </For>
      </div>
      <div ref={container} class="chart-seg__canvas" />
    </div>
  );
}

// ── helpers ──

function num(v: unknown): number {
  if (typeof v === "number") return v;
  if (typeof v === "string") {
    const n = parseFloat(v);
    return isNaN(n) ? 0 : n;
  }
  if (typeof v === "boolean") return v ? 1 : 0;
  return 0;
}

/** Find the first non-numeric column (dimension: string/time/category). */
function findDimensionCol(types: string[]): number {
  for (let i = 0; i < types.length; i++) {
    const t = types[i].toUpperCase();
    if (!t.includes("INT") && !t.includes("FLOAT") && !t.includes("DOUBLE") && !t.includes("DECIMAL")) {
      return i;
    }
  }
  return 0;
}

/** Find numeric columns (excluding the dimension). */
function findNumericCols(cols: string[], types: string[], excludeIdx: number): string[] {
  const out: string[] = [];
  for (let i = 0; i < cols.length; i++) {
    if (i === excludeIdx) continue;
    const t = types[i]?.toUpperCase() ?? "";
    if (t.includes("INT") || t.includes("FLOAT") || t.includes("DOUBLE") || t.includes("DECIMAL")) {
      out.push(cols[i]);
    }
  }
  if (out.length === 0 && cols.length > 0) {
    // No typed numeric columns — return all columns except the dimension.
    return cols.filter((_, i) => i !== excludeIdx);
  }
  return out;
}
