import { Show, For, createSignal } from "solid-js";
import type { Segment } from "../lib/types";
import ResultTable from "./ResultTable";

const TOOL_LABELS: Record<string, string> = {
  list_tables: "探索数据库",
  describe_table: "分析表结构",
  execute_query: "执行 SQL",
  sample_data: "采样数据",
  create_table: "创建表",
  create_view: "创建视图",
  drop_object: "删除对象",
  render_chart: "生成图表",
  load_okf_block: "读取业务知识",
  write_okf_block: "更新业务知识",
  search_okf_recipes: "检索清洗配方",
  check_source_fingerprint: "校验数据指纹",
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
  /** awaiting 状态下用户点击「确认执行」(true) 或「取消」(false)。 */
  onConfirm?: (approved: boolean) => void;
}) {
  // ToolSegment is only ever rendered for `type === "tool"` segments (filtered
  // by the parent). Narrow once into a local typed variable so the tool-shape
  // fields (status/sql/table/...) resolve.
  const t = () => props.seg.type === "tool" ? props.seg : null;

  const sqlFromArgs = () => {
    const s = t();
    if (!s) return undefined;
    return s.args && typeof s.args === "object" && "sql" in (s.args as any)
      ? String((s.args as any).sql)
      : undefined;
  };

  const tableFromArgs = () => {
    const s = t();
    if (!s) return undefined;
    return s.args && typeof s.args === "object" && "table_name" in (s.args as any)
      ? String((s.args as any).table_name)
      : undefined;
  };

  const tablesList = () => {
    const s = t();
    if (!s || s.tool !== "list_tables" || !s.summary) return [];
    const marker = "张表: ";
    const idx = s.summary.indexOf(marker);
    if (idx < 0) return [];
    return s.summary.substring(idx + marker.length).split(",").map(item => item.trim()).filter(Boolean);
  };

  const hasBody = () => {
    const s = t();
    if (!s) return false;
    return !!(
      s.status === "awaiting" || // awaiting: always show DDL + confirm buttons
      (sqlFromArgs() && s.tool === "execute_query") ||
      (sqlFromArgs() && s.tool === "render_chart") || // chart: show the SQL that produced the data
      (tableFromArgs() && s.tool !== "execute_query") ||
      s.sql ||
      s.table ||
      (s.tool === "list_tables" && tablesList().length > 0) ||
      s.args ||
      s.result
    );
  };

  // awaiting 状态点击确认/取消后，本地置灰，等待后端 tool_result 覆盖状态。
  const [confirmResolved, setConfirmResolved] = createSignal(false);
  const handleConfirm = (approved: boolean) => {
    if (confirmResolved()) return;
    setConfirmResolved(true);
    props.onConfirm?.(approved);
  };

  return (
    <div
      class={`tool-seg tool-seg--${t()?.status}`}
      classList={{ "tool-seg--open": props.expanded && hasBody() }}
    >
      <div
        class="tool-seg__summary"
        classList={{ "tool-seg__summary--clickable": hasBody() && t()?.status !== "running" && t()?.status !== "awaiting" }}
        onClick={() => {
          if (hasBody() && t()?.status !== "running" && t()?.status !== "awaiting") {
            props.onToggle(t()!.id);
          }
        }}
      >
        <span class="tool-seg__status">
          {t()?.status === "running" ? (
            <span class="tool-seg__spinner" />
          ) : t()?.status === "awaiting" ? (
            <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round" style="width: 14px; height: 14px; color: var(--accent-orange, #f59e0b);">
              <circle cx="12" cy="12" r="10"></circle>
              <line x1="10" y1="9" x2="10" y2="15"></line>
              <line x1="14" y1="9" x2="14" y2="15"></line>
            </svg>
          ) : t()?.status !== "ok" ? (
            <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round" style="width: 14px; height: 14px; color: var(--accent-red);">
              <path d="M10.29 3.86L1.82 18a2 2 0 0 0 1.71 3h16.94a2 2 0 0 0 1.71-3L13.71 3.86a2 2 0 0 0-3.42 0z"></path>
              <line x1="12" y1="9" x2="12" y2="13"></line>
              <line x1="12" y1="17" x2="12.01" y2="17"></line>
            </svg>
          ) : (t()?.tool === "list_tables" || t()?.tool === "describe_table") ? (
            <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.2" stroke-linecap="round" stroke-linejoin="round" style="width: 14px; height: 14px;">
              <circle cx="11" cy="11" r="8"></circle>
              <line x1="21" y1="21" x2="16.65" y2="16.65"></line>
            </svg>
          ) : t()?.tool === "execute_query" ? (
            <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.2" stroke-linecap="round" stroke-linejoin="round" style="width: 14px; height: 14px;">
              <ellipse cx="12" cy="5" rx="9" ry="3"></ellipse>
              <path d="M3 5V19A9 3 0 0 0 21 19V5"></path>
              <path d="M3 12A9 3 0 0 0 21 12"></path>
            </svg>
          ) : t()?.tool === "sample_data" ? (
            <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.2" stroke-linecap="round" stroke-linejoin="round" style="width: 14px; height: 14px;">
              <line x1="18" y1="20" x2="18" y2="10"></line>
              <line x1="12" y1="20" x2="12" y2="4"></line>
              <line x1="6" y1="20" x2="6" y2="14"></line>
            </svg>
          ) : t()?.tool === "render_chart" ? (
            <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.2" stroke-linecap="round" stroke-linejoin="round" style="width: 14px; height: 14px;">
              <line x1="18" y1="20" x2="18" y2="10" />
              <line x1="12" y1="20" x2="12" y2="4" />
              <line x1="6" y1="20" x2="6" y2="14" />
              <line x1="3" y1="20" x2="21" y2="20" />
            </svg>
          ) : t()?.tool === "load_okf_block" ? (
            <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.2" stroke-linecap="round" stroke-linejoin="round" style="width: 14px; height: 14px; color: var(--accent-blue, #60a5fa);">
              <path d="M4 19.5A2.5 2.5 0 0 1 6.5 17H20"></path>
              <path d="M6.5 2H20v20H6.5A2.5 2.5 0 0 1 4 19.5v-15A2.5 2.5 0 0 1 6.5 2z"></path>
            </svg>
          ) : t()?.tool === "write_okf_block" ? (
            <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.2" stroke-linecap="round" stroke-linejoin="round" style="width: 14px; height: 14px; color: var(--accent-green, #10b981);">
              <path d="M11 4H4a2 2 0 0 0-2 2v14a2 2 0 0 0 2 2h14a2 2 0 0 0 2-2v-7"></path>
              <path d="M18.5 2.5a2.121 2.121 0 1 1 3 3L12 15l-4 1 1-4 9.5-9.5z"></path>
            </svg>
          ) : t()?.tool === "search_okf_recipes" ? (
            <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.2" stroke-linecap="round" stroke-linejoin="round" style="width: 14px; height: 14px; color: var(--accent-purple, #a78bfa);">
              <circle cx="11" cy="11" r="8"></circle>
              <line x1="21" y1="21" x2="16.65" y2="16.65"></line>
              <line x1="8" y1="11" x2="14" y2="11"></line>
            </svg>
          ) : t()?.tool === "check_source_fingerprint" ? (
            <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.2" stroke-linecap="round" stroke-linejoin="round" style="width: 14px; height: 14px; color: var(--accent-orange, #f59e0b);">
              <path d="M12 22s8-4 8-10V5l-8-3-8 3v7c0 6 8 10 8 10z"></path>
            </svg>
          ) : (
            <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.2" stroke-linecap="round" stroke-linejoin="round" style="width: 14px; height: 14px;">
              <circle cx="12" cy="12" r="3"></circle>
              <path d="M19.4 15a1.65 1.65 0 0 0 .33 1.82l.06.06a2 2 0 1 1-2.83 2.83l-.06-.06a1.65 1.65 0 0 0-1.82-.33 1.65 1.65 0 0 0-1 1.51V21a2 2 0 0 1-4 0v-.09A1.65 1.65 0 0 0 9 19.4a1.65 1.65 0 0 0-1.82.33l-.06.06a2 2 0 1 1-2.83-2.83l.06-.06a1.65 1.65 0 0 0 .33-1.82 1.65 1.65 0 0 0-1.51-1H3a2 2 0 0 1 0-4h.09A1.65 1.65 0 0 0 4.6 9a1.65 1.65 0 0 0-.33-1.82l-.06-.06a2 2 0 1 1 2.83-2.83l.06.06a1.65 1.65 0 0 0 1.82.33H9a1.65 1.65 0 0 0 1-1.51V3a2 2 0 0 1 4 0v.09a1.65 1.65 0 0 0 1 1.51 1.65 1.65 0 0 0 1.82-.33l.06-.06a2 2 0 1 1 2.83 2.83l-.06.06a1.65 1.65 0 0 0-.33 1.82V9a1.65 1.65 0 0 0 1.51 1H21a2 2 0 0 1 0 4h-.09a1.65 1.65 0 0 0-1.51 1z"></path>
            </svg>
          )}
        </span>
        <span class="tool-seg__name">{TOOL_LABELS[t()!.tool] ?? t()!.tool}</span>
        <Show when={t()?.elapsedMs != null}>
          <span class="tool-seg__meta">· {fmtMs(t()!.elapsedMs!)}</span>
        </Show>
        <Show when={t()?.summary}>
          <span class="tool-seg__summary-text">· {t()!.summary}</span>
        </Show>
        <Show when={t()?.status !== "running" && t()?.status !== "awaiting" && hasBody()}>
          <span class="tool-seg__chevron" classList={{ "tool-seg__chevron--open": props.expanded }}>
            <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.5" style="width: 10px; height: 10px; transition: transform 0.15s ease;">
              <polyline points="9 18 15 12 9 6"></polyline>
            </svg>
          </span>
        </Show>
      </div>

      <Show when={props.expanded && hasBody()}>
        <div class="tool-seg__body">
          {/* Awaiting confirmation: show the DDL to be run + confirm/cancel. */}
          <Show when={t()?.status === "awaiting"}>
            <div class="tool-seg__confirm">
              <div class="tool-seg__confirm-hint">即将执行以下变更，请确认：</div>
              <Show when={t()?.sql}>
                <SqlBlock sql={t()!.sql!} onCopy onOpenInSqlPanel={props.onOpenInSqlPanel} />
              </Show>
              <div class="tool-seg__confirm-actions">
                <button
                  class="tool-seg__confirm-btn tool-seg__confirm-btn--ok"
                  disabled={confirmResolved()}
                  onClick={(e) => { e.stopPropagation(); handleConfirm(true); }}
                >确认执行</button>
                <button
                  class="tool-seg__confirm-btn tool-seg__confirm-btn--cancel"
                  disabled={confirmResolved()}
                  onClick={(e) => { e.stopPropagation(); handleConfirm(false); }}
                >取消</button>
              </div>
            </div>
          </Show>

          {/* Tables list from list_tables */}
          <Show when={t()?.tool === "list_tables" && tablesList().length > 0}>
            <div style="display: flex; flex-wrap: wrap; gap: 6px; padding: 2px 0;">
              <span style="font-size: 11.5px; color: var(--text-dim); margin-right: 2px; align-self: center;">数据表:</span>
              <For each={tablesList()}>
                {(tableName: string) => (
                  <code style="background: var(--bg-active); padding: 1.5px 5px; border-radius: 3px; font-size: 11px; color: var(--text-normal); border: 1px solid var(--border-faint); font-family: inherit;">
                    {tableName}
                  </code>
                )}
              </For>
            </div>
          </Show>

          {/* Args / SQL preview */}
          <Show when={sqlFromArgs() && (t()?.tool === "execute_query" || t()?.tool === "render_chart")}>
            <SqlBlock sql={sqlFromArgs()!} onCopy onOpenInSqlPanel={props.onOpenInSqlPanel} />
          </Show>
          <Show when={tableFromArgs() && t()?.tool !== "execute_query"}>
            <div class="tool-seg__arg">表: <code>{tableFromArgs()}</code></div>
          </Show>
          {/* SQL from the result (execute_query success carries it). awaiting DDL is shown in the confirm block above. */}
          <Show when={t()?.sql && !sqlFromArgs() && t()?.status !== "awaiting"}>
            <SqlBlock sql={t()!.sql!} onCopy onOpenInSqlPanel={props.onOpenInSqlPanel} />
          </Show>

          {/* Inline result table */}
          <Show when={t()?.table}>
            <div class="tool-seg__table">
              <ResultTable result={t()!.table!} compact />
            </div>
          </Show>

          {/* OKF tools parameters and results */}
          <Show when={t()?.tool && ["load_okf_block", "write_okf_block", "search_okf_recipes", "check_source_fingerprint"].includes(t()!.tool)}>
            <div class="tool-seg__okf-details" style="display: flex; flex-direction: column; gap: 10px; font-size: 12px; color: var(--text-normal); background: var(--bg-hover); padding: 10px 12px; border-radius: 6px; border: 1px solid var(--border-faint); margin: 6px 0;">
              <Show when={t()?.args && typeof t()!.args === "object"}>
                <div style="display: flex; flex-direction: column; gap: 5px;">
                  <div style="font-weight: 600; color: var(--text-dim); font-size: 10.5px; text-transform: uppercase; letter-spacing: 0.05em;">调用参数</div>
                  <For each={Object.entries(t()!.args as Record<string, any>)}>
                    {([key, val]) => (
                      <div style="display: flex; gap: 6px; align-items: flex-start; font-family: monospace; line-height: 1.4;">
                        <span style="color: var(--accent-blue, #60a5fa); font-weight: 500; min-width: 70px;">{key}:</span>
                        <Show when={key === "content" || key === "query" || key === "file_path"} fallback={<span style="color: var(--text-normal);">{String(val)}</span>}>
                          <pre style="margin: 0; background: var(--bg-active); padding: 4px 8px; border-radius: 4px; border: 1px solid var(--border-faint); font-family: inherit; font-size: 11.5px; white-space: pre-wrap; word-break: break-all; flex: 1;">{String(val)}</pre>
                        </Show>
                      </div>
                    )}
                  </For>
                </div>
              </Show>
              <Show when={t()?.result}>
                <div style="display: flex; flex-direction: column; gap: 5px; border-top: 1px solid var(--border-faint); padding-top: 8px;">
                  <div style="font-weight: 600; color: var(--text-dim); font-size: 10.5px; text-transform: uppercase; letter-spacing: 0.05em;">返回结果</div>
                  <pre style="margin: 0; background: var(--bg-active); padding: 8px 10px; border-radius: 4px; border: 1px solid var(--border-faint); font-family: monospace; font-size: 11.5px; white-space: pre-wrap; word-break: break-all;">{t()!.result}</pre>
                </div>
              </Show>
            </div>
          </Show>
        </div>
      </Show>
    </div>
  );
}

function SqlBlock(props: { sql: string; onCopy?: boolean; onOpenInSqlPanel?: (sql: string) => void }) {
  const [copied, setCopied] = createSignal(false);

  return (
    <div style="position: relative;">
      <pre class="tool-seg__code">{props.sql}</pre>
      <div class="tool-seg__code-actions">
        <Show when={props.onOpenInSqlPanel}>
          <button
            class="tool-seg__open"
            title="在 SQL 面板打开"
            onClick={(e) => {
              e.stopPropagation();
              props.onOpenInSqlPanel?.(props.sql);
            }}
          >
            <svg xmlns="http://www.w3.org/2000/svg" width="12" height="12" viewBox="0 0 24 24" fill="currentColor">
              <polygon points="5 3 19 12 5 21 5 3"></polygon>
            </svg>
          </button>
        </Show>
        <Show when={props.onCopy}>
          <button
            class="tool-seg__copy"
            title={copied() ? "已复制" : "复制"}
            onClick={async (e) => {
              e.stopPropagation();
              try {
                await navigator.clipboard.writeText(props.sql);
                setCopied(true);
                setTimeout(() => setCopied(false), 1500);
              } catch {}
            }}
          >
            <Show
              when={copied()}
              fallback={
                <svg xmlns="http://www.w3.org/2000/svg" width="12" height="12" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.5" stroke-linecap="round" stroke-linejoin="round">
                  <rect x="9" y="9" width="13" height="13" rx="2" ry="2"></rect>
                  <path d="M5 15H4a2 2 0 0 1-2-2V4a2 2 0 0 1 2-2h9a2 2 0 0 1 2 2v1"></path>
                </svg>
              }
            >
              <svg xmlns="http://www.w3.org/2000/svg" width="12" height="12" viewBox="0 0 24 24" fill="none" stroke="var(--accent-green, #10b981)" stroke-width="3" stroke-linecap="round" stroke-linejoin="round">
                <polyline points="20 6 9 17 4 12"></polyline>
              </svg>
            </Show>
          </button>
        </Show>
      </div>
    </div>
  );
}

function fmtMs(ms: number): string {
  if (ms < 1000) return `${ms}ms`;
  return `${(ms / 1000).toFixed(1)}s`;
}
