import { For, Show, createSignal } from "solid-js";
import { typeFamily, type SourceTable } from "../lib/types";
import { t } from "../lib/i18n";

/**
 * Right drawer: Schema/column detail inspector for the currently selected
 * SOURCE table. Shows the table's kind, row-count estimate, Hive partition
 * keys, and a clickable list of columns (name + type-colored badge + null).
 *
 * Clicking a column injects `SELECT col FROM "t" LIMIT 1000` into the editor;
 * the Preview button runs `SELECT * FROM "t" LIMIT 50` directly.
 */
export default function RightInspector(props: {
  table: SourceTable | null;
  busy: boolean;
  onInjectSql: (sql: string) => void;
  onPreview: (table: SourceTable) => void;
}) {
  const [copied, setCopied] = createSignal(false);
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

            <div class="ri-path" title={tVal().scanPath}>{tVal().scanPath}</div>

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
