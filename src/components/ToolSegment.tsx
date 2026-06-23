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
          {t.status === "running" ? (
            <span class="tool-seg__spinner" />
          ) : t.status !== "ok" ? (
            <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round" style="width: 14px; height: 14px; color: var(--accent-red);">
              <path d="M10.29 3.86L1.82 18a2 2 0 0 0 1.71 3h16.94a2 2 0 0 0 1.71-3L13.71 3.86a2 2 0 0 0-3.42 0z"></path>
              <line x1="12" y1="9" x2="12" y2="13"></line>
              <line x1="12" y1="17" x2="12.01" y2="17"></line>
            </svg>
          ) : (t.tool === "list_tables" || t.tool === "describe_table") ? (
            <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.2" stroke-linecap="round" stroke-linejoin="round" style="width: 14px; height: 14px;">
              <circle cx="11" cy="11" r="8"></circle>
              <line x1="21" y1="21" x2="16.65" y2="16.65"></line>
            </svg>
          ) : t.tool === "execute_query" ? (
            <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.2" stroke-linecap="round" stroke-linejoin="round" style="width: 14px; height: 14px;">
              <ellipse cx="12" cy="5" rx="9" ry="3"></ellipse>
              <path d="M3 5V19A9 3 0 0 0 21 19V5"></path>
              <path d="M3 12A9 3 0 0 0 21 12"></path>
            </svg>
          ) : t.tool === "sample_data" ? (
            <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.2" stroke-linecap="round" stroke-linejoin="round" style="width: 14px; height: 14px;">
              <line x1="18" y1="20" x2="18" y2="10"></line>
              <line x1="12" y1="20" x2="12" y2="4"></line>
              <line x1="6" y1="20" x2="6" y2="14"></line>
            </svg>
          ) : (
            <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.2" stroke-linecap="round" stroke-linejoin="round" style="width: 14px; height: 14px;">
              <circle cx="12" cy="12" r="3"></circle>
              <path d="M19.4 15a1.65 1.65 0 0 0 .33 1.82l.06.06a2 2 0 1 1-2.83 2.83l-.06-.06a1.65 1.65 0 0 0-1.82-.33 1.65 1.65 0 0 0-1 1.51V21a2 2 0 0 1-4 0v-.09A1.65 1.65 0 0 0 9 19.4a1.65 1.65 0 0 0-1.82.33l-.06.06a2 2 0 1 1-2.83-2.83l.06-.06a1.65 1.65 0 0 0 .33-1.82 1.65 1.65 0 0 0-1.51-1H3a2 2 0 0 1 0-4h.09A1.65 1.65 0 0 0 4.6 9a1.65 1.65 0 0 0-.33-1.82l-.06-.06a2 2 0 1 1 2.83-2.83l.06.06a1.65 1.65 0 0 0 1.82.33H9a1.65 1.65 0 0 0 1-1.51V3a2 2 0 0 1 4 0v.09a1.65 1.65 0 0 0 1 1.51 1.65 1.65 0 0 0 1.82-.33l.06-.06a2 2 0 1 1 2.83 2.83l-.06.06a1.65 1.65 0 0 0-.33 1.82V9a1.65 1.65 0 0 0 1.51 1H21a2 2 0 0 1 0 4h-.09a1.65 1.65 0 0 0-1.51 1z"></path>
            </svg>
          )}
        </span>
        <span class="tool-seg__name">{TOOL_LABELS[t.tool] ?? t.tool}</span>
        <Show when={t.elapsedMs != null}>
          <span class="tool-seg__meta">· {fmtMs(t.elapsedMs!)}</span>
        </Show>
        <Show when={t.summary}>
          <span class="tool-seg__summary-text">· {t.summary}</span>
        </Show>
        <Show when={t.status !== "running"}>
          <span class="tool-seg__chevron" classList={{ "tool-seg__chevron--open": props.expanded }}>
            <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.5" style="width: 10px; height: 10px; transition: transform 0.15s ease;">
              <polyline points="9 18 15 12 9 6"></polyline>
            </svg>
          </span>
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
