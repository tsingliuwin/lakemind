import { onCleanup, onMount, createSignal, For, Show, createEffect } from "solid-js";
import * as echarts from "echarts";
import { invoke } from "@tauri-apps/api/core";
import type { Segment, SqlResult } from "../lib/types";
import { currentTheme, Theme } from "../lib/theme";

/**
 * Inline chart segment — renders an ECharts visualization from a SqlResult.
 *
 * The agent's `render_chart` tool emits a `chart` segment with a chart type
 * (bar/line/pie/scatter), axis mapping (xField/yFields), and the raw query
 * data. This component converts that into an ECharts option and renders it.
 * The user can switch chart types via the toolbar.
 */

type ChartType = "bar" | "line" | "pie" | "scatter" | "funnel" | "gauge";

/** Chart types that can be freely switched between via the tab bar. Types
 * outside this set (e.g. future specialized charts like heatmap/map) render
 * without a tab bar — the agent's chosen type is shown as-is. */
const SWITCHABLE_TYPES: ChartType[] = ["bar", "line", "pie", "scatter"];

const CHART_TYPES: { type: ChartType; label: string; svg: string }[] = [
  { type: "bar", label: "柱状图", svg: '<rect x="3" y="12" width="4" height="9"/><rect x="10" y="7" width="4" height="14"/><rect x="17" y="4" width="4" height="17"/>' },
  { type: "line", label: "折线图", svg: '<polyline points="3 17 8 11 13 14 21 5" fill="none"/><circle cx="3" cy="17" r="1.5"/><circle cx="8" cy="11" r="1.5"/><circle cx="13" cy="14" r="1.5"/><circle cx="21" cy="5" r="1.5"/>' },
  { type: "pie", label: "饼图", svg: '<circle cx="12" cy="12" r="9"/><path d="M12 3 A9 9 0 0 1 21 12 L12 12 Z" fill="currentColor" stroke="none"/>' },
  { type: "scatter", label: "散点图", svg: '<circle cx="5" cy="18" r="1.8"/><circle cx="10" cy="8" r="1.8"/><circle cx="15" cy="14" r="1.8"/><circle cx="19" cy="5" r="1.8"/><circle cx="8" cy="16" r="1.8"/>' },
];

/** Theme styles helper mapping palette colors, grids, tooltips, and fonts for Dark and Light modes. */
function getThemeStyles(theme: Theme) {
  const isLight = theme === "light";
  return {
    isLight,
    palette: isLight
      ? [
          "#3b82f6", // blue
          "#10b981", // green
          "#f59e0b", // amber
          "#8b5cf6", // purple
          "#f97316", // orange
          "#ef4444", // red
          "#06b6d4", // cyan
          "#d946ef", // pink
        ]
      : [
          "#5b8ff9", // blue
          "#61ddaa", // green
          "#f6bd16", // amber
          "#7262fd", // purple
          "#ff9d4d", // orange
          "#e86452", // red
          "#6dc8ec", // cyan
          "#945fb9", // violet
        ],
    axisLineColor: isLight ? "#d1d5db" : "#3a3a3e",
    axisTickColor: isLight ? "#d1d5db" : "#3a3a3e",
    axisLabelColor: isLight ? "#4b5563" : "#9aa0a6",
    splitLineColor: isLight ? "#e5e7eb" : "#1f1f22",
    tooltipBg: isLight ? "#ffffff" : "#18181b",
    tooltipBorder: isLight ? "#e5e7eb" : "#3a3a3e",
    tooltipText: isLight ? "#111827" : "#e6e7eb",
    textColor: isLight ? "#111827" : "#e6e7eb",
    legendColor: isLight ? "#4b5563" : "#9aa0a6",
    lineStyleColor: isLight ? "#e5e7eb" : "#5c6066",
    gaugeLineColor: isLight ? "#e5e7eb" : "#2a2a2e",
  };
}

function hexToRgba(hex: string, alpha: number): string {
  const r = parseInt(hex.slice(1, 3), 16);
  const g = parseInt(hex.slice(3, 5), 16);
  const b = parseInt(hex.slice(5, 7), 16);
  return `rgba(${r}, ${g}, ${b}, ${alpha})`;
}

export default function ChartSegment(props: { seg: Extract<Segment, { type: "chart" }> }) {
  let container: HTMLDivElement | undefined;
  let chart: echarts.ECharts | undefined;
  const [chartType, setChartType] = createSignal<ChartType>(props.seg.chartType);
  const [isFullScreen, setIsFullScreen] = createSignal(false);
  let fullscreenContainer: HTMLDivElement | undefined;
  let fullscreenChart: echarts.ECharts | undefined;

  function saveAsImage() {
    const activeChart = isFullScreen() ? fullscreenChart : chart;
    if (!activeChart) return;

    const isLight = currentTheme() === "light";
    const url = activeChart.getDataURL({
      type: "png",
      pixelRatio: 2,
      backgroundColor: isLight ? "#ffffff" : "#121214",
    });

    const fileName = `${props.seg.title || "chart"}.png`;
    invoke("save_image_from_base64", {
      base64Data: url,
      defaultName: fileName,
    }).catch((e) => console.error("[chart] save image failed:", e));
  }

  /** Build ECharts option from SqlResult + chart config. */
  function buildOption(type: ChartType, table: SqlResult, xField?: string, yFields?: string[], title?: string): echarts.EChartsOption {
    const cols = table.columns;
    // Determine column indices.
    const xIdx = xField ? cols.indexOf(xField) : findDimensionCol(table.columnTypes);
    const yCols = yFields && yFields.length > 0 ? yFields : findNumericCols(cols, table.columnTypes, xIdx);
    const styles = getThemeStyles(currentTheme());

    const AXIS_STYLE = {
      axisLine: { lineStyle: { color: styles.axisLineColor } },
      axisTick: { show: false, lineStyle: { color: styles.axisTickColor } },
      axisLabel: { color: styles.axisLabelColor, fontSize: 11, fontFamily: "var(--font-sans)" },
      splitLine: { lineStyle: { color: styles.splitLineColor, type: "dashed" as const } },
    };

    const TOOLTIP_STYLE = {
      backgroundColor: styles.tooltipBg,
      borderColor: styles.tooltipBorder,
      borderWidth: 1,
      padding: [8, 12],
      borderRadius: 6,
      shadowColor: "rgba(0, 0, 0, 0.12)",
      shadowBlur: 8,
      textStyle: { color: styles.tooltipText, fontSize: 12, fontFamily: "var(--font-sans)" },
    };

    const TITLE_STYLE = (text: string) => ({
      text, left: "center",
      textStyle: { color: styles.textColor, fontSize: 13, fontWeight: 500, fontFamily: "var(--font-sans)" },
    });

    if (type === "pie") {
      const yIdx = yCols.length > 0 ? cols.indexOf(yCols[0]) : -1;
      const data = table.rows
        .filter((r) => r[xIdx] != null && r[yIdx] != null)
        .map((r) => ({ name: String(r[xIdx]), value: num(r[yIdx]) }));
      return {
        color: styles.palette,
        title: title ? { ...TITLE_STYLE(title), top: 8 } : undefined,
        tooltip: { trigger: "item", formatter: "{b}: {c} ({d}%)", ...TOOLTIP_STYLE },
        legend: { bottom: 2, type: "scroll", textStyle: { color: styles.legendColor, fontSize: 11 }, itemWidth: 8, itemHeight: 8, itemGap: 12 },
        series: [{
          type: "pie",
          radius: ["42%", "68%"],
          center: ["50%", "50%"],
          data,
          label: { color: styles.textColor, fontSize: 11, formatter: "{b}: {d}%", fontWeight: 500 },
          labelLine: { lineStyle: { color: styles.lineStyleColor } },
          itemStyle: { borderRadius: 6, borderColor: styles.tooltipBg, borderWidth: 2 },
        }],
      };
    }

    if (type === "scatter") {
      const yIdx = yCols.length > 0 ? cols.indexOf(yCols[0]) : -1;
      const data = table.rows
        .filter((r) => r[xIdx] != null && r[yIdx] != null)
        .map((r) => [num(r[xIdx]), num(r[yIdx])]);
      return {
        color: styles.palette,
        title: title ? TITLE_STYLE(title) : undefined,
        tooltip: { trigger: "item", ...TOOLTIP_STYLE },
        grid: { left: 60, right: 24, top: title ? 44 : 20, bottom: 32 },
        xAxis: { type: "value", name: xField ?? cols[xIdx] ?? "X", nameTextStyle: { color: styles.axisLabelColor, fontSize: 11 }, scale: true, ...AXIS_STYLE },
        yAxis: { type: "value", name: yCols[0] ?? "Y", nameTextStyle: { color: styles.axisLabelColor, fontSize: 11 }, scale: true, ...AXIS_STYLE },
        series: [{
          type: "scatter",
          data,
          symbolSize: 8,
          itemStyle: {
            opacity: 0.8,
            borderColor: styles.tooltipBg,
            borderWidth: 1.5,
            shadowBlur: 4,
            shadowColor: "rgba(0, 0, 0, 0.15)"
          },
          emphasis: {
            focus: "self",
            itemStyle: { opacity: 1 }
          }
        }],
      };
    }

    if (type === "funnel") {
      const yIdx = yCols.length > 0 ? cols.indexOf(yCols[0]) : -1;
      const data = table.rows
        .filter((r) => r[xIdx] != null && r[yIdx] != null)
        .map((r) => ({ name: String(r[xIdx]), value: num(r[yIdx]) }));
      return {
        color: styles.palette,
        title: title ? { ...TITLE_STYLE(title), top: 8 } : undefined,
        tooltip: { trigger: "item", formatter: "{b}: {c}", ...TOOLTIP_STYLE },
        legend: { bottom: 2, type: "scroll", textStyle: { color: styles.legendColor, fontSize: 11 }, itemWidth: 8, itemHeight: 8, itemGap: 12 },
        series: [{
          type: "funnel",
          data,
          sort: "descending",
          gap: 2,
          label: { color: styles.textColor, fontSize: 11, formatter: "{b}: {c}", fontWeight: 500 },
          itemStyle: { borderRadius: 4, borderColor: styles.tooltipBg, borderWidth: 1.5 },
        }],
      };
    }

    if (type === "gauge") {
      // Gauge shows a single value — use the first row, first numeric column.
      const yIdx = yCols.length > 0 ? cols.indexOf(yCols[0]) : findFirstNumeric(table.columnTypes, xIdx);
      const label = yIdx >= 0 ? cols[yIdx] : (xField ?? "值");
      const value = yIdx >= 0 && table.rows.length > 0 ? num(table.rows[0][yIdx]) : 0;
      return {
        color: styles.palette,
        title: title ? { ...TITLE_STYLE(title), top: 8 } : undefined,
        tooltip: { ...TOOLTIP_STYLE },
        series: [{
          type: "gauge",
          center: ["50%", "58%"],
          radius: "82%",
          min: 0,
          max: (() => {
            // Auto-scale max: round up to a nice number (×1, ×2, ×5 × 10^n).
            if (value <= 0) return 100;
            const mag = Math.pow(10, Math.floor(Math.log10(value)));
            const norm = value / mag;
            const nice = norm <= 1 ? 1 : norm <= 2 ? 2 : norm <= 5 ? 5 : 10;
            return nice * mag;
          })(),
          progress: { show: true, width: 12, itemStyle: { color: styles.palette[0] } },
          axisLine: { lineStyle: { width: 12, color: [[1, styles.gaugeLineColor]] } },
          pointer: { width: 4, itemStyle: { color: styles.axisLabelColor } },
          axisTick: { show: false },
          splitLine: { length: 8, lineStyle: { color: styles.axisLineColor, width: 1.5 } },
          axisLabel: { color: styles.legendColor, fontSize: 10, distance: 16 },
          detail: { valueAnimation: true, color: styles.textColor, fontSize: 18, fontWeight: 600, offsetCenter: [0, "62%"], formatter: `{value}` },
          data: [{ value, name: label }],
        }],
      };
    }

    // bar / line
    const categoryData = table.rows.map((r) => String(r[xIdx >= 0 ? xIdx : 0] ?? ""));
    const rotated = categoryData.length > 8;
    const series = yCols.map((yn, colorOffset) => {
      const yi = cols.indexOf(yn);
      const baseColor = styles.palette[colorOffset % styles.palette.length];
      return {
        name: yn,
        type,
        data: table.rows.map((r) => num(r[yi])),
        smooth: type === "line",
        ...(type === "bar"
          ? {
              itemStyle: {
                borderRadius: [4, 4, 0, 0],
                color: new echarts.graphic.LinearGradient(0, 0, 0, 1, [
                  { offset: 0, color: baseColor },
                  { offset: 1, color: hexToRgba(baseColor, 0.35) }
                ])
              },
              barMaxWidth: 24,
            }
          : {}),
        ...(type === "line"
          ? {
              symbol: "circle",
              symbolSize: 6,
              lineStyle: {
                width: 3,
                shadowColor: hexToRgba(baseColor, 0.2),
                shadowBlur: 6,
                shadowOffsetY: 3
              },
              itemStyle: { color: baseColor },
              areaStyle: {
                color: new echarts.graphic.LinearGradient(0, 0, 0, 1, [
                  { offset: 0, color: hexToRgba(baseColor, 0.15) },
                  { offset: 1, color: hexToRgba(baseColor, 0.01) }
                ])
              }
            }
          : {}),
      };
    });
    return {
      color: styles.palette,
      title: title ? TITLE_STYLE(title) : undefined,
      tooltip: { trigger: "axis", ...TOOLTIP_STYLE },
      legend: { bottom: 2, type: "scroll", textStyle: { color: styles.legendColor, fontSize: 11 }, itemWidth: 8, itemHeight: 8, itemGap: 12 },
      // bottom space: legend (~22px) + X axis label (~18px normal / ~40px rotated) + gaps
      grid: { left: 60, right: 24, top: title ? 44 : 20, bottom: rotated ? 72 : 52 },
      xAxis: { type: "category", data: categoryData, ...AXIS_STYLE, axisLabel: { ...AXIS_STYLE.axisLabel, rotate: rotated ? 30 : 0 } },
      yAxis: { type: "value", ...AXIS_STYLE },
      series,
    };
  }

  function render() {
    if (!chart || !container) return;
    const opt = buildOption(chartType(), props.seg.table, props.seg.xField, props.seg.yFields, props.seg.title);
    chart.setOption(opt, true);
  }

  createEffect(() => {
    // Establish dependency on current theme for automatic re-rendering
    currentTheme();
    render();
  });

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
    // Also update the fullscreen chart if it's open
    if (fullscreenChart) {
      const opt = buildOption(t, props.seg.table, props.seg.xField, props.seg.yFields, props.seg.title);
      fullscreenChart.setOption(opt, true);
    }
  }

  const switchable = SWITCHABLE_TYPES.includes(props.seg.chartType);

  return (
    <div class="chart-seg">
      <div class="chart-seg__toolbar">
        <div class="chart-seg__toolbar-left">
          <Show when={switchable}>
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
          </Show>
        </div>
        <div class="chart-seg__toolbar-right">
          <button
            class="chart-seg__action-btn"
            title="保存为图片"
            onClick={saveAsImage}
          >
            <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round" style="width: 13px; height: 13px;">
              <path d="M21 15v4a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2v-4"></path>
              <polyline points="7 10 12 15 17 10"></polyline>
              <line x1="12" y1="15" x2="12" y2="3"></line>
            </svg>
          </button>
          <button
            class="chart-seg__action-btn"
            title="全屏查看"
            onClick={() => setIsFullScreen(true)}
          >
            <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round" style="width: 13px; height: 13px;">
              <path d="M8 3H5a2 2 0 0 0-2 2v3m18 0V5a2 2 0 0 0-2-2h-3m0 18h3a2 2 0 0 0 2-2v-3M3 16v3a2 2 0 0 0 2 2h3"></path>
            </svg>
          </button>
        </div>
      </div>
      <div ref={container} class="chart-seg__canvas" />

      {/* Fullscreen Overlay Dialog */}
      <Show when={isFullScreen()}>
        <div class="chart-fullscreen-overlay">
          <div class="chart-fullscreen-header">
            <span class="chart-fullscreen-title">{props.seg.title || "图表预览"}</span>
            <Show when={switchable}>
              <div class="chart-fullscreen-tabs">
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
            </Show>
            <div class="chart-fullscreen-actions">
              <button class="chart-fullscreen-btn" onClick={saveAsImage} title="保存为图片">
                <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round" style="width: 15px; height: 15px;">
                  <path d="M21 15v4a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2v-4"></path>
                  <polyline points="7 10 12 15 17 10"></polyline>
                  <line x1="12" y1="15" x2="12" y2="3"></line>
                </svg>
              </button>
              <button class="chart-fullscreen-btn" onClick={() => setIsFullScreen(false)} title="退出全屏">
                <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round" style="width: 15px; height: 15px;">
                  <path d="M4 14h6v6m10-6h-6v6M4 10h6V4m10 6h-6V4"></path>
                </svg>
              </button>
            </div>
          </div>
          <div class="chart-fullscreen-body" onClick={() => setIsFullScreen(false)}>
            <div 
              ref={(el) => {
                if (el) {
                  fullscreenContainer = el;
                  fullscreenChart = echarts.init(el);
                  const opt = buildOption(chartType(), props.seg.table, props.seg.xField, props.seg.yFields, props.seg.title);
                  fullscreenChart.setOption(opt);
                  
                  const ro = new ResizeObserver(() => fullscreenChart?.resize());
                  ro.observe(el);
                  
                  (el as any)._cleanup = () => {
                    ro.disconnect();
                    fullscreenChart?.dispose();
                    fullscreenChart = undefined;
                  };
                } else {
                  if (fullscreenContainer && (fullscreenContainer as any)._cleanup) {
                    (fullscreenContainer as any)._cleanup();
                  }
                  fullscreenContainer = undefined;
                }
              }} 
              class="chart-fullscreen-canvas" 
              onClick={(e) => e.stopPropagation()} 
            />
          </div>
        </div>
      </Show>
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

/** Find the first numeric column index (for gauge's single value). */
function findFirstNumeric(types: string[], excludeIdx: number): number {
  for (let i = 0; i < types.length; i++) {
    if (i === excludeIdx) continue;
    const t = types[i].toUpperCase();
    if (t.includes("INT") || t.includes("FLOAT") || t.includes("DOUBLE") || t.includes("DECIMAL")) return i;
  }
  return -1;
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
