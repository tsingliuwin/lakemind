import { Show } from "solid-js";
import type { Segment } from "../lib/types";
import ResultTable from "./ResultTable";

const TOOL_LABELS: Record<string, string> = {
  list_tables: "探索数据库",
  describe_table: "分析表结构",
  execute_query: "执行 SQL",
  sample_data: "采样数据",
  step: "步骤",
};

/**
 * 一个工具调用段（tool_call + tool_result 合并）。混合折叠：
 * - running：展开，显示工具名 + SQL 预览（若有）。
 * - ok / error：默认收成一行摘要「✓ 执行 SQL · 0.3s · 12 行」；点击展开
 *   看 SQL + 内联结果表 + 「在 SQL 面板打开」。
 *
 * 「展开 / 收起」由父级 ChatView 的 expandedSegmentIds 驱动：最新 running
 * 的工具自动展开，拿到结果后自动收起；历史工具默认收起。
 */
export default function ToolSegment(props: {
  seg: Segment;
  expanded: boolean;
  onToggle: (id: string) => void;
  onOpenInSqlPanel: (sql: string) => void;
}) {
  // ToolSegment is only ever rendered for `type === "tool"` segments (filtered
  // by the parent). Narrow once into a local typed variable so the tool-shape
  // fields (status/sql/table/...) resolve.
  const t = props.seg.type === "tool" ? props.seg : null;
  if (!t) return null;

  const sqlFromArgs =
    t.args && typeof t.args === "object" && "sql" in (t.args as any)
      ? String((t.args as any).sql)
      : undefined;
  const tableFromArgs =
    t.args && typeof t.args === "object" && "table_name" in (t.args as any)
      ? String((t.args as any).table_name)
      : undefined;

  return (
    <div
      class={`tool-seg tool-seg--${t.status}`}
      classList={{ "tool-seg--open": props.expanded }}
    >
      <div class="tool-seg__summary" onClick={() => props.onToggle(t.id)}>
        <span class="tool-seg__status">
          {t.status === "running" ? <span class="tool-seg__spinner" /> : t.status === "ok" ? "✓" : "✗"}
        </span>
        <span class="tool-seg__name">{TOOL_LABELS[t.tool] ?? t.tool}</span>
        <Show when={t.elapsedMs != null}>
          <span class="tool-seg__meta">· {fmtMs(t.elapsedMs!)}</span>
        </Show>
        <Show when={t.summary}>
          <span class="tool-seg__summary-text">· {t.summary}</span>
        </Show>
        <Show when={t.status !== "running"}>
          <span class="tool-seg__chevron">{props.expanded ? "▾" : "▸"}</span>
        </Show>
      </div>

      <Show when={props.expanded}>
        <div class="tool-seg__body">
          {/* Args / SQL preview */}
          <Show when={sqlFromArgs && t.tool === "execute_query"}>
            <SqlBlock sql={sqlFromArgs!} onCopy />
          </Show>
          <Show when={tableFromArgs && t.tool !== "execute_query"}>
            <div class="tool-seg__arg">表: <code>{tableFromArgs}</code></div>
          </Show>
          {/* SQL from the result (execute_query success carries it) */}
          <Show when={t.sql && !sqlFromArgs}>
            <SqlBlock sql={t.sql!} onCopy />
          </Show>

          {/* Inline result table */}
          <Show when={t.table}>
            <div class="tool-seg__table">
              <ResultTable result={t.table!} compact />
            </div>
          </Show>

          {/* Actions */}
          <Show when={t.sql}>
            <div class="tool-seg__actions">
              <button class="tool-seg__open" onClick={() => props.onOpenInSqlPanel(t.sql!)}>
                ▶ 在 SQL 面板打开
              </button>
            </div>
          </Show>
        </div>
      </Show>
    </div>
  );
}

function SqlBlock(props: { sql: string; onCopy?: boolean }) {
  return (
    <div style="position: relative;">
      <pre class="tool-seg__code">{props.sql}</pre>
      <Show when={props.onCopy}>
        <button
          class="tool-seg__copy"
          onClick={async (e) => {
            e.stopPropagation();
            try {
              await navigator.clipboard.writeText(props.sql);
              const btn = e.currentTarget;
              const old = btn.innerText;
              btn.innerText = "✓ 已复制";
              setTimeout(() => (btn.innerText = old), 1500);
            } catch {}
          }}
        >
          复制
        </button>
      </Show>
    </div>
  );
}

function fmtMs(ms: number): string {
  if (ms < 1000) return `${ms}ms`;
  return `${(ms / 1000).toFixed(1)}s`;
}
