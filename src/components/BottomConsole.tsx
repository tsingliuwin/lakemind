import { For, Show, createMemo, createSignal } from "solid-js";
import type { LogCategory, LogLevel, UnifiedLog } from "../lib/types";
import { t } from "../lib/i18n";

/** The three preset heights for the collapsible bottom console. */
export type ConsoleState = "folded" | "default" | "expanded";

/** Which log category a console tab shows. "all" = no category filter. */
type ConsoleTab = "all" | LogCategory;

/** Tab definition: key + label i18n key + which categories it covers.
 * The "system" tab folds the backend infra categories (sync/duckdb/link/system)
 * together so the user isn't overwhelmed by fine-grained prefixes. */
const TABS: { key: ConsoleTab; labelKey: string; cats?: LogCategory[] }[] = [
  { key: "all", labelKey: "tabAll" },
  { key: "query", labelKey: "tabQuery", cats: ["query"] },
  { key: "import", labelKey: "tabImport", cats: ["import"] },
  { key: "agent", labelKey: "tabAgent", cats: ["agent"] },
  { key: "system", labelKey: "tabSystem", cats: ["sync", "duckdb", "link", "system"] },
];

/**
 * Bottom drawer: the unified log console. Replaces the old SQL-only log with a
 * multi-tab view over the global `UnifiedLog` stream. Every log line from the
 * backend (tracing → SQLite → `app-log` emit) and the frontend (logger.ts →
 * append_log) converges here, newest-first.
 *
 * Tabs: 全部 / 查询 / 导入 / Agent / 系统. The header doubles as a status bar —
 * per-tab ok/error counts and the most recent timing. Query rows keep the old
 * SQL-detail expand + copy affordance. Three heights: folded / default / expanded.
 */
export default function BottomConsole(props: {
  logs: UnifiedLog[];
  state: ConsoleState;
  onCycleState: () => void;
  onClear: () => void;
}) {
  const [activeTab, setActiveTab] = createSignal<ConsoleTab>("all");
  /** Which log row is currently expanded to reveal its detail/error payload. */
  const [expandedId, setExpandedId] = createSignal<number | null>(null);
  /** Which log row is currently showing the "copied" checkmark feedback. */
  const [copiedId, setCopiedId] = createSignal<number | null>(null);

  /** Logs filtered by the active tab's category set. */
  const visibleLogs = createMemo(() => {
    const tab = TABS.find((tb) => tb.key === activeTab());
    const cats = tab?.cats;
    if (!cats || cats.length === 0) return props.logs;
    return props.logs.filter((l) => cats.includes(l.category));
  });

  /** Per-tab header summary: success/error/error-ish counts + last timing. */
  const summary = createMemo(() => {
    let ok = 0;
    let err = 0;
    let lastMs: number | null = null;
    for (const l of visibleLogs()) {
      if (l.level === "error") err++;
      else if (l.level === "warn" || l.level === "info") ok++;
      // Most recent timing: query logs carry detail.elapsedMs.
      if (lastMs === null) {
        const ms = l.detail?.elapsedMs;
        if (typeof ms === "number") lastMs = ms;
      }
    }
    return { ok, err, lastMs, total: visibleLogs().length };
  });

  return (
    <section classList={{ bottom: true, [`state-${props.state}`]: true }}>
      <div class="console-header" onClick={() => props.onCycleState()}>
        <span class="ch-title">{t("consoleTitle")}</span>
        <Show when={summary().total > 0}>
          <span class="ch-summary">
            <span class="ok-n">✓ {summary().ok}</span>
            <Show when={summary().err > 0}>
              <span class="err-n">✗ {summary().err}</span>
            </Show>
            <Show when={summary().lastMs != null}>
              <span class="muted">· {t("latest")} {summary().lastMs}ms</span>
            </Show>
          </span>
        </Show>
        <div class="ch-actions" onClick={(e) => e.stopPropagation()}>
          <Show when={props.logs.length > 0}>
            <button class="icon-btn" title={t("clearLog")} onClick={() => props.onClear()}>
              <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round" style="width: 12px; height: 12px;">
                <line x1="18" y1="6" x2="6" y2="18"></line>
                <line x1="6" y1="6" x2="18" y2="18"></line>
              </svg>
            </button>
          </Show>
          <button
            class="icon-btn"
            title={
              props.state === "folded"
                ? t("expandConsole")
                : props.state === "default"
                  ? t("expandFurther")
                  : t("foldConsole")
            }
            onClick={() => props.onCycleState()}
          >
            <Show when={props.state === "expanded"} fallback={
              <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.5" stroke-linecap="round" stroke-linejoin="round" style="width: 12px; height: 12px;">
                <polyline points="18 15 12 9 6 15"></polyline>
              </svg>
            }>
              <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.5" stroke-linecap="round" stroke-linejoin="round" style="width: 12px; height: 12px;">
                <polyline points="6 9 12 15 18 9"></polyline>
              </svg>
            </Show>
          </button>
        </div>
      </div>

      <Show when={props.state !== "folded"}>
        {/* Tab bar */}
        <div class="console-tabs">
          <For each={TABS}>
            {(tab) => (
              <button
                class="console-tab"
                classList={{ active: activeTab() === tab.key }}
                onClick={() => setActiveTab(tab.key)}
              >
                {t(tab.labelKey as any)}
              </button>
            )}
          </For>
        </div>

        <div class="console-list">
          <Show
            when={visibleLogs().length > 0}
            fallback={
              <div class="console-empty">
                {activeTab() === "all" ? t("consoleAllEmptyHint") : t("consoleEmptyHint")}
              </div>
            }
          >
            <For each={visibleLogs()}>
              {(log) => {
                const title = titleFor(log);
                const rowCount = numFromDetail(log.detail?.rowCount);
                const elapsedMs = numFromDetail(log.detail?.elapsedMs);
                const expanded = () => expandedId() === log.id;
                return (
                  <>
                    <div
                      class="log-row"
                      classList={{ clickable: true, expanded: expanded() }}
                      onClick={() => setExpandedId((id) => (id === log.id ? null : log.id ?? null))}
                    >
                      <span class="log-ts">{formatTs(log.ts)}</span>
                      <span class="log-badge" data-level={log.level} title={levelName(log.level)}>{log.category}</span>
                      <span class="log-title" title={title}>{title}</span>
                      <span class="log-meta">
                        <Show when={rowCount != null}>
                          <span>{rowCount!.toLocaleString()} {t("rowsUnit")}</span>
                        </Show>
                        <Show when={elapsedMs != null}>
                          <span>{elapsedMs}ms</span>
                        </Show>
                      </span>
                      <span class="log-expand" data-open={expanded()}>▸</span>
                    </div>
                    <Show when={expanded()}>
                      <LogDetail log={log} copiedId={copiedId()} onCopied={(id) => setCopiedId(id)} />
                    </Show>
                  </>
                );
              }}
            </For>
          </Show>
        </div>
      </Show>
    </section>
  );
}

function formatTs(ms: number): string {
  const d = new Date(ms);
  return d.toLocaleTimeString("en-GB", { hour12: false });
}

/** Pull a numeric value out of a possibly-undefined detail field. */
function numFromDetail(v: unknown): number | null {
  return typeof v === "number" ? v : null;
}

/** Localized level name for tooltips. */
function levelName(level: LogLevel): string {
  switch (level) {
    case "error": return t("levelError");
    case "warn": return t("levelWarn");
    case "info": return t("levelInfo");
    default: return t("levelDebug");
  }
}

/**
 * Derive the short single-line title shown in each console row. The row must
 * stay scannable, so the title is the human summary — NOT the full payload:
 *   query  → first line of the SQL (collapsed, CSS truncates the width)
 *   other  → the `message` field as-is
 * The full SQL / error / detail is revealed by expanding the row (LogDetail).
 */
function titleFor(log: UnifiedLog): string {
  if (log.category === "query") {
    const sql = log.detail?.sql;
    if (typeof sql === "string" && sql.trim()) {
      // First non-empty line, whitespace collapsed — the SQL "verb + target".
      const firstLine = sql.split("\n").map((l) => l.trim()).find((l) => l.length > 0);
      return collapseSpaces(firstLine ?? sql);
    }
  }
  return collapseSpaces(log.message);
}

function collapseSpaces(s: string): string {
  return s.replace(/\s+/g, " ").trim();
}

/**
 * Expanded detail panel for one log row. Renders the full payload in up to four
 * sections (only those present): the full SQL (code block + copy), the error
 * message, a key/value table for the remaining structured fields, and the
 * precise timestamp. This is where the verbose content lives — the row itself
 * only shows the short title.
 */
function LogDetail(props: { log: UnifiedLog; copiedId: number | null; onCopied: (id: number | null) => void }) {
  const log = props.log;
  const detail = log.detail ?? {};
  const sql = typeof detail.sql === "string" ? detail.sql : null;
  const errorStr = typeof detail.error === "string" && detail.error.length > 0 ? detail.error : null;
  // Remaining structured fields, excluding the ones already rendered as SQL / error.
  const extraFields = Object.entries(detail).filter(([k]) => k !== "sql" && k !== "error");

  const copy = async () => {
    if (!sql) return;
    try {
      await navigator.clipboard.writeText(sql);
      props.onCopied(log.id ?? null);
      setTimeout(() => props.onCopied(null), 1500);
    } catch {}
  };

  return (
    <div class="log-detail">
      <Show when={sql == null}>
        {/* Non-query logs: show the full (untruncated) message so the expanded
         * panel always carries the complete text the row title summarized. */}
        <div class="ld-section">
          <span class="ld-label">{t("levelInfo")}</span>
          <pre class="ld-code">{log.message}</pre>
        </div>
      </Show>

      <Show when={sql != null}>
        <div class="ld-section">
          <div class="ld-section-head">
            <span class="ld-label">SQL</span>
            <button class="ld-copy" title={props.copiedId === log.id ? "已复制" : t("copySql")} onClick={(e) => { e.stopPropagation(); copy(); }}>
              <Show
                when={props.copiedId === log.id}
                fallback={
                  <svg xmlns="http://www.w3.org/2000/svg" width="11" height="11" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.5" stroke-linecap="round" stroke-linejoin="round">
                    <rect x="9" y="9" width="13" height="13" rx="2" ry="2"></rect>
                    <path d="M5 15H4a2 2 0 0 1-2-2V4a2 2 0 0 1 2-2h9a2 2 0 0 1 2 2v1"></path>
                  </svg>
                }
              >
                <svg xmlns="http://www.w3.org/2000/svg" width="11" height="11" viewBox="0 0 24 24" fill="none" stroke="var(--accent-green, #10b981)" stroke-width="3" stroke-linecap="round" stroke-linejoin="round">
                  <polyline points="20 6 9 17 4 12"></polyline>
                </svg>
              </Show>
            </button>
          </div>
          <pre class="ld-code">{sql}</pre>
        </div>
      </Show>

      <Show when={errorStr != null}>
        <div class="ld-section">
          <span class="ld-label ld-label-err">{t("levelError")}</span>
          <pre class="ld-err">{errorStr}</pre>
        </div>
      </Show>

      <Show when={extraFields.length > 0}>
        <div class="ld-section">
          <table class="ld-fields">
            <tbody>
              <For each={extraFields}>
                {([k, v]) => (
                  <tr>
                    <td class="ld-k">{k}</td>
                    <td class="ld-v">{String(v)}</td>
                  </tr>
                )}
              </For>
            </tbody>
          </table>
        </div>
      </Show>

      <div class="ld-foot">
        <span class="ld-ts">{new Date(log.ts).toLocaleString()}</span>
        <Show when={log.workspace}><span class="ld-tag">workspace: {log.workspace}</span></Show>
        <Show when={log.taskId}><span class="ld-tag">task: {log.taskId}</span></Show>
      </div>
    </div>
  );
}
