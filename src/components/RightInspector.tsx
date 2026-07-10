import { For, Show, createSignal, createEffect } from "solid-js";
import { invoke } from "@tauri-apps/api/core";
import { typeFamily, type SourceTable, type DepInfo } from "../lib/types";
import { logError } from "../lib/logger";
import { t } from "../lib/i18n";

/**
 * Right drawer: Schema/column detail inspector for the currently selected
 * SOURCE table. Shows the table's kind, row-count estimate, Hive partition
 * keys, and a clickable list of columns (name + type-colored badge + null).
 *
 * Clicking a column injects `SELECT col FROM "t" LIMIT 1000` into the editor;
 * the Preview button runs `SELECT * FROM "t" LIMIT 50` directly.
 */

/** True if a dependency name is a lake table/view (clickable), false if it's a
 * file name (not clickable — files aren't selectable in the data tree). */
function isLakeObject(name: string): boolean {
  return /^(s_|t_|v_|tmp_|tmp_v_)/.test(name);
}

export default function RightInspector(props: {
  table: SourceTable | null;
  busy: boolean;
  onInjectSql: (sql: string) => void;
  onPreview: (table: SourceTable) => void;
  /** Upstream + downstream dependencies for the selected table. */
  deps?: DepInfo | null;
  /** Click a dependency name to select that table. */
  onSelectDep?: (name: string) => void;
}) {
  const [copied, setCopied] = createSignal(false);
  const [copiedDdl, setCopiedDdl] = createSignal(false);
  const [ddl, setDdl] = createSignal<string | null>(null);
  const [_ddlLoading, setDdlLoading] = createSignal(false);
  const [ddlOpen, setDdlOpen] = createSignal(false);

  createEffect(() => {
    const table = props.table;
    if (!table) {
      setDdl(null);
      setDdlOpen(false);
      return;
    }
    setDdlLoading(true);
    setDdl(null);
    setDdlOpen(false);

    invoke<string>("get_table_ddl", { tableName: table.name })
      .then((res) => {
        setDdl(res);
      })
      .catch((err) => {
        logError("ui", "Failed to load DDL", err);
        setDdl(null);
      })
      .finally(() => {
        setDdlLoading(false);
      });
  });

  return (
    <aside class="right">
      <Show
        when={props.table}
        fallback={<div class="ri-empty">{t("inspectorEmptyHint")}</div>}
      >
        {(tVal) => (
          <>
            <div class="ri-header">
              <div class="ri-title">
                <span class="kind-badge" data-kind={tVal().kind}>{tVal().kind}</span>
                <span class="ri-name" title={tVal().name}>{tVal().name}</span>
                <button
                  class="ri-copy-btn"
                  title={copied() ? "已复制" : "复制表名"}
                  onClick={async (e) => {
                    e.stopPropagation();
                    try {
                      await navigator.clipboard.writeText(tVal().name);
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
              </div>
              <div class="ri-meta">
                <Show when={tVal().rowCountEstimate != null}>
                  <span class="rows">{formatCount(tVal().rowCountEstimate!)} {t("rowsUnit")}</span>
                </Show>
                <Show when={tVal().rowCountEstimate == null}>
                  <span>— {t("rowsUnit")}</span>
                </Show>
                <span>· {tVal().columns.length} {t("colsUnit")}</span>
              </div>
              <Show when={tVal().partitionKeys.length > 0}>
                <div class="ri-partitions">
                  <For each={tVal().partitionKeys}>
                    {(k) => <span class="part-pill">🗂 {k}</span>}
                  </For>
                </div>
              </Show>
            </div>

            {/* Dependency info: upstream (what this depends on) + downstream
                (what depends on this). Shown above the field list so the user
                sees relationships before diving into columns. */}
            <Show when={props.deps && (props.deps.upstreams.length > 0 || props.deps.downstreams.length > 0)}>
              <div class="ri-section-label">依赖关系</div>
              <div class="ri-deps">
                <Show when={props.deps!.upstreams.length > 0}>
                  <div class="ri-dep-group">
                    <span class="ri-dep-label">↑ 上游</span>
                    <div class="ri-dep-items">
                      <For each={props.deps!.upstreams}>
                        {(name) => {
                          const clickable = isLakeObject(name);
                          return (
                            <div class="ri-dep-row">
                              <Show
                                when={clickable}
                                fallback={
                                  <div class="ri-dep-chip ri-dep-chip--static" title={name}>
                                    <span class="ri-dep-chip-name">{name}</span>
                                  </div>
                                }
                              >
                                <button class="ri-dep-chip" title={`查看 ${name}`} onClick={() => props.onSelectDep?.(name)}>
                                  <span class="ri-dep-chip-name">{name}</span>
                                </button>
                              </Show>
                              <button class="ri-dep-copy" title="复制" onClick={() => void navigator.clipboard.writeText(name)}>
                                <svg xmlns="http://www.w3.org/2000/svg" width="10" height="10" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.5" stroke-linecap="round" stroke-linejoin="round">
                                  <rect x="9" y="9" width="13" height="13" rx="2" ry="2"></rect>
                                  <path d="M5 15H4a2 2 0 0 1-2-2V4a2 2 0 0 1 2-2h9a2 2 0 0 1 2 2v1"></path>
                                </svg>
                              </button>
                            </div>
                          );
                        }}
                      </For>
                    </div>
                  </div>
                </Show>
                <Show when={props.deps!.downstreams.length > 0}>
                  <div class="ri-dep-group">
                    <span class="ri-dep-label ri-dep-label--down">↓ 下游</span>
                    <div class="ri-dep-items">
                      <For each={props.deps!.downstreams}>
                        {(name) => (
                          <div class="ri-dep-row">
                            <button class="ri-dep-chip ri-dep-chip--down" title={`查看 ${name}`} onClick={() => props.onSelectDep?.(name)}>
                              <span class="ri-dep-chip-name">{name}</span>
                            </button>
                            <button class="ri-dep-copy" title="复制" onClick={() => void navigator.clipboard.writeText(name)}>
                              <svg xmlns="http://www.w3.org/2000/svg" width="10" height="10" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.5" stroke-linecap="round" stroke-linejoin="round">
                                <rect x="9" y="9" width="13" height="13" rx="2" ry="2"></rect>
                                <path d="M5 15H4a2 2 0 0 1-2-2V4a2 2 0 0 1 2-2h9a2 2 0 0 1 2 2v1"></path>
                              </svg>
                            </button>
                          </div>
                        )}
                      </For>
                    </div>
                  </div>
                </Show>
              </div>
            </Show>

            {/* Collapsible SQL Definition */}
            <Show when={ddl()}>
              <div class="ri-ddl-section">
                <button
                  class="ri-ddl-toggle-btn"
                  onClick={() => setDdlOpen(!ddlOpen())}
                >
                  <span class="ri-ddl-toggle-arrow">{ddlOpen() ? "▼" : "▶"}</span>
                  <span>SQL 定义</span>
                </button>
                <Show when={ddlOpen()}>
                  <div class="ri-ddl-content">
                    <pre class="ri-ddl-code">
                      <code>{ddl()}</code>
                    </pre>
                    <button
                      class="ri-ddl-copy-btn"
                      onClick={async (e) => {
                        e.stopPropagation();
                        try {
                          await navigator.clipboard.writeText(ddl() || "");
                          setCopiedDdl(true);
                          setTimeout(() => setCopiedDdl(false), 1500);
                        } catch {}
                      }}
                    >
                      {copiedDdl() ? "已复制" : "复制 SQL"}
                    </button>
                  </div>
                </Show>
              </div>
            </Show>

            <div class="ri-section-label">{t("fieldsLabel")}</div>
            <div class="ri-cols">
              <For each={tVal().columns}>
                {(c) => (
                  <div
                    class="col-card"
                    title={`SELECT "${c.name}" FROM "${tVal().name}" LIMIT 1000`}
                    onClick={() => props.onInjectSql(`SELECT "${c.name}" FROM "${tVal().name}" LIMIT 1000;`)}
                  >
                    <span class="col-name">{c.name}</span>
                    <span class="type-badge" data-family={typeFamily(c.type)}>
                      {c.type}
                    </span>
                  </div>
                )}
              </For>
            </div>

            <div class="ri-footer">
              <button
                class="preview-btn"
                disabled={props.busy}
                onClick={() => props.onPreview(tVal())}
              >
                {t("previewRowsBtn")}
              </button>
            </div>
          </>
        )}
      </Show>
    </aside>
  );
}

function formatCount(n: number): string {
  if (n >= 1_000_000_000) return (n / 1_000_000_000).toFixed(1) + "B";
  if (n >= 1_000_000) return (n / 1_000_000).toFixed(1) + "M";
  if (n >= 1_000) return (n / 1_000).toFixed(0) + "K";
  return String(n);
}
