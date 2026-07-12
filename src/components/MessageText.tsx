import { For, Match, Show, Switch, createMemo } from "solid-js";
import type { Segment } from "../lib/types";
import { splitTextByChartRefs, findChartSegment } from "../lib/chartRef";
import MarkdownRenderer from "./MarkdownRenderer";
import ChartSegment from "./ChartSegment";

type ChartSeg = Extract<Segment, { type: "chart" }>;

/** A text chunk after splitting, with any chart reference resolved to its
 *  segment (or `undefined` if unresolvable). */
type ResolvedChunk =
  | { kind: "text"; content: string }
  | { kind: "chartRef"; ref: string; chart: ChartSeg | undefined };

/**
 * Renders a markdown text segment that may contain inline chart-reference
 * markers (`{{chart:<id>}}` — returned to the model by the `render_chart`
 * tool). Text between markers is rendered with MarkdownRenderer; each marker
 * is replaced in-place by the corresponding interactive ChartSegment, looked
 * up by id among the message's segments.
 *
 * Streaming stability (the important part): while streaming, `appendDelta`
 * produces a fresh segments-array reference on every token, so this memo
 * re-runs per token and `.map()` yields brand-new chunk objects. SolidJS
 * `<For>` is **reference-keyed** (`mapArray`), so naively it would dispose +
 * recreate every inline ChartSegment on every token — i.e. echarts.init /
 * dispose on every token = severe jank.
 *
 * Fix: we REUSE the previous chart-ref chunk object when its `ref` and
 * resolved `chart` are unchanged. The chart segment object keeps a stable
 * reference across array reshuffles (shallow `[...segments]` copy preserves
 * element identities), so the cache hits on every token after the marker
 * closes, and `<For>` leaves the mounted ChartSegment (and its echarts
 * instance) alone. Only the (cheap, echarts-free) text chunks rebuild per
 * token — whose total parse cost matches the original single-MarkdownRenderer
 * behavior. Charts appear as soon as their marker closes, not at stream end.
 *
 * An unclosed `{{chart:` mid-stream stays in a trailing text chunk and shows
 * as a 📊 badge via MarkdownRenderer's fallback until it closes. When no
 * marker is present this is exactly MarkdownRenderer (zero regression).
 */
export default function MessageText(props: { text: string; segments: Segment[] }) {
  // Resolved chart-ref chunks from the previous run, keyed by ref. Reusing
  // the same object reference lets <For> skip rebuilding the (echarts-backed)
  // ChartSegment on every text delta.
  let chartChunkCache = new Map<string, ResolvedChunk>();

  const chunks = createMemo<ResolvedChunk[]>(() => {
    // Touch props.segments so the memo re-resolves if a chart segment
    // arrives after the marker text (streaming-order edge case).
    const segs = props.segments;
    const nextCache = new Map<string, ResolvedChunk>();
    const out: ResolvedChunk[] = splitTextByChartRefs(props.text).map((p) => {
      if (p.kind === "chartRef") {
        const chart = findChartSegment(segs, p.ref);
        const cached = chartChunkCache.get(p.ref);
        if (cached && cached.kind === "chartRef" && cached.chart === chart) {
          nextCache.set(p.ref, cached);
          return cached;
        }
        const fresh: ResolvedChunk = { kind: "chartRef", ref: p.ref, chart };
        nextCache.set(p.ref, fresh);
        return fresh;
      }
      return { kind: "text", content: p.content };
    });
    chartChunkCache = nextCache;
    return out;
  });
  const hasRefs = createMemo(() => chunks().some((c) => c.kind === "chartRef"));

  return (
    <Show when={hasRefs()} fallback={<MarkdownRenderer content={props.text} />}>
      <For each={chunks()}>
        {(c) => (
          <Switch>
            <Match when={c.kind === "text"}>
              <MarkdownRenderer content={c.kind === "text" ? c.content : ""} />
            </Match>
            <Match when={c.kind === "chartRef" && c.chart}>
              {(chart) => <ChartSegment seg={chart()} />}
            </Match>
            <Match when={c.kind === "chartRef"}>
              <span class="chart-ref-missing">📊 图表引用未找到</span>
            </Match>
          </Switch>
        )}
      </For>
    </Show>
  );
}
