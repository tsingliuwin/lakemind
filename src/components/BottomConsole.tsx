import { For, Show, createMemo, createSignal } from "solid-js";
import type { LogEntry } from "../lib/types";
import { t } from "../lib/i18n";

/** The three preset heights for the collapsible bottom console. */
export type ConsoleState = "folded" | "default" | "expanded";

/**
 * Bottom drawer: the SQL execution log. Every run (success or failure) pushes
 * a [`LogEntry`] here, so the user has a durable trail. The header doubles as
 * a status bar — it shows running totals (✓ ok · ✗ failed) and the most
 * recent timing. Failures expand inline with the raw DuckDB error.
 *
 * Three states: folded (32px header only), default (160px), expanded (320px).
 */
export default function BottomConsole(props: {
  logs: LogEntry[];
  state: ConsoleState;
  onCycleState: () => void;
  onClear: () => void;
}) {
  const [expandedId, setExpandedId] = createSignal<number | null>(null);

  const summary = createMemo(() => {
    let ok = 0;
    let err = 0;
    let lastMs: number | null = null;
    for (const l of props.logs) {
      if (l.status === "ok") ok++;
      else err++;
      if (l.elapsedMs != null) lastMs = l.elapsedMs;
    }
    return { ok, err, lastMs };
  });

  return (
    <section classList={{ bottom: true, [`state-${props.state}`]: true }}>
      <div class="console-header" onClick={() => props.onCycleState()}>
        <span class="ch-title">{t("consoleTitle")}</span>
        <Show when={props.logs.length > 0}>
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
        <div class="console-list">
          <Show
            when={props.logs.length > 0}
            fallback={<div class="console-empty">{t("consoleEmptyHint")}</div>}
          >
            <For each={props.logs}>
              {(log) => (
                <>
                  <div
                    class="log-row"
                    classList={{ clickable: log.status === "error" }}
                    onClick={() =>
                      log.status === "error" &&
                      setExpandedId((id) => (id === log.id ? null : log.id))
                    }
                  >
                    <span class="log-ts">{formatTs(log.ts)}</span>
                    <span classList={{ "log-status": true, ok: log.status === "ok", err: log.status === "error" }}>
                      {log.status === "ok" ? "✓" : "✗"}
                    </span>
                    <span class="log-sql">{summarizeSql(log.sql)}</span>
                    <span class="log-meta">
                      <Show when={log.rowCount != null}>
                        <span>{log.rowCount!.toLocaleString()} {t("rowsUnit")}</span>
                      </Show>
                      <Show when={log.elapsedMs != null}>
                        <span>{log.elapsedMs}ms</span>
                      </Show>
                      <Show when={log.truncated}>
                        <span title="truncated">⚠</span>
                      </Show>
                    </span>
                  </div>
                  <Show when={log.status === "error" && expandedId() === log.id && log.error}>
                    <div class="log-err">{log.error}</div>
                  </Show>
                </>
              )}
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

function summarizeSql(sql: string): string {
  const one = sql.replace(/\s+/g, " ").trim();
  return one.length > 60 ? one.slice(0, 60) + "…" : one;
}
