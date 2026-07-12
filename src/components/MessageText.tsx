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
 * When no marker is present this is exactly MarkdownRenderer (zero regression).
 * When a marker can't be resolved (e.g. the referenced chart isn't in the
 * message), it shows a small badge instead of leaking raw `{{...}}` braces.
 */
export default function MessageText(props: { text: string; segments: Segment[] }) {
  const chunks = createMemo<ResolvedChunk[]>(() => {
    // Touch props.segments so the memo re-resolves when a chart segment
    // arrives after the marker text (streaming-order edge case).
    const segs = props.segments;
    return splitTextByChartRefs(props.text).map<ResolvedChunk>((p) =>
      p.kind === "chartRef"
        ? { kind: "chartRef", ref: p.ref, chart: findChartSegment(segs, p.ref) }
        : { kind: "text", content: p.content },
    );
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
